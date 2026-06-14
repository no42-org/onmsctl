/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `MaintenanceHandler` — the maintenance capability's adapter into the core
//! kind-router.
//!
//! `kind: Maintenance` is **named and multi-instance** (one document per
//! window), so `plan()` parses the whole bucket, gates on duplicate
//! `metadata.name`, validates each document (parse-time, before any HTTP), then
//! per window resolves node foreign references, GETs the deployed definition, and
//! diffs it (normalized). A node reference that does not resolve fails *that*
//! window (not the batch).
//!
//! `execute()` is the **composite reconcile** (design D4): write the definition
//! first (create/update/unchanged), then — only if that succeeded — `attach` each
//! desired `suppress` target (ensure-present). Each gets its own outcome: the
//! definition is `Created`/`Updated`/`Unchanged`, attachments are `Ensured`
//! (the attachment set is not readable, so we can only guarantee presence). A
//! failed attach is a `Failed` outcome; if the definition write fails, attaches
//! are `Skipped` (not attempted). Reducing suppression is `maintenance delete` +
//! re-apply — apply never detaches (it cannot read the current attachment set).

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OnmsClient, OutcomeStatus,
    Plan, RawDoc, Result,
};

use crate::api::MaintenanceApi;
use crate::convert;
use crate::diff::{self, AttachTarget};
use crate::model::{KIND, MaintenanceLocal};

/// Handler for `kind: Maintenance` documents.
#[derive(Default)]
pub struct MaintenanceHandler;

/// A window whose read-only plan succeeded.
struct ReadyWindow {
    local: MaintenanceLocal,
    node_ids: Vec<i64>,
    deployed_exists: bool,
    def_unchanged: bool,
    targets: Vec<AttachTarget>,
}

/// The per-window plan result.
enum Planned {
    Ready(Box<ReadyWindow>),
    Failed { name: String, message: String },
}

struct ExecPayload {
    windows: Vec<Planned>,
}

#[async_trait]
impl KindHandler for MaintenanceHandler {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        // Parse the whole bucket (gate on parse failure).
        let mut locals: Vec<MaintenanceLocal> = Vec::with_capacity(docs.len());
        for d in docs {
            let local: MaintenanceLocal =
                serde_norway::from_value(d.value.clone()).map_err(|e| {
                    Error::Config(format!(
                        "{}: invalid `kind: Maintenance` document: {e}",
                        d.label()
                    ))
                })?;
            locals.push(local);
        }

        // Gate: duplicate metadata.name within the bucket.
        if let Some(dup) = first_duplicate(locals.iter().map(|l| l.metadata.name.as_str())) {
            return Err(Error::Config(format!(
                "kind: Maintenance names must be unique within an apply — duplicate metadata.name {dup:?}"
            )));
        }

        // Gate: validate each document before any HTTP.
        for local in &locals {
            local.validate()?;
        }

        // Advisory warnings (e.g. past `specific` windows) — non-fatal.
        let now = now_civil();
        for local in &locals {
            for w in local.warnings(now) {
                eprintln!("warning: {}: {w}", local.metadata.name);
            }
        }

        let client = OnmsClient::from_context(ctx)?;
        let api = MaintenanceApi::new(&client);

        let mut previews = Vec::new();
        let mut windows = Vec::with_capacity(locals.len());
        for local in locals {
            let planned = plan_window(local, &api).await?;
            previews.extend(preview_for(&planned));
            windows.push(planned);
        }

        Ok(
            Plan::new(previews, Box::new(ExecPayload { windows })).with_diff(Some(
                "maintenance: definition is reconciled (create/update); daemon attachments are \
             ensure-present — apply never detaches, the current attachment set is not readable. \
             To reduce suppression use `onmsctl maintenance delete <name>` + re-apply."
                    .to_string(),
            )),
        )
    }

    async fn execute(
        &self,
        plan: Plan,
        _params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let payload = plan.payload.downcast::<ExecPayload>().map_err(|_| {
            Error::Config("internal: MaintenanceHandler payload type mismatch".into())
        })?;
        let client = OnmsClient::from_context(ctx)?;
        let api = MaintenanceApi::new(&client);

        let mut outcomes = Vec::new();
        for w in payload.windows {
            match w {
                Planned::Failed { name, message } => {
                    outcomes.push(ApplyOutcome::failed(
                        KIND,
                        name,
                        Action::None,
                        message,
                        "resolve the node reference (import the node) and re-apply",
                    ));
                }
                Planned::Ready(rw) => execute_window(*rw, &api, &mut outcomes).await,
            }
        }
        Ok(outcomes)
    }
}

/// Read-only plan for one window. Returns `Err` only on a transport failure
/// (which aborts the batch); a node that does not resolve yields a per-window
/// `Planned::Failed`.
async fn plan_window(local: MaintenanceLocal, api: &MaintenanceApi<'_>) -> Result<Planned> {
    let name = local.metadata.name.clone();
    let mut node_ids = Vec::with_capacity(local.spec.devices.nodes.len());
    for n in &local.spec.devices.nodes {
        match api.resolve_node(&n.foreign_source, &n.foreign_id).await? {
            Some(id) => node_ids.push(id),
            None => {
                return Ok(Planned::Failed {
                    name,
                    message: format!(
                        "node {}:{} not found / not yet imported (resolve to a nodeId failed)",
                        n.foreign_source, n.foreign_id
                    ),
                });
            }
        }
    }
    let deployed = api.get(&name).await?;
    let desired = convert::to_wire(&local, &node_ids);
    let def_unchanged = deployed
        .as_ref()
        .is_some_and(|d| diff::definition_unchanged(&desired, d));
    let targets = diff::attachment_targets(&local.spec.suppress);
    Ok(Planned::Ready(Box::new(ReadyWindow {
        local,
        node_ids,
        deployed_exists: deployed.is_some(),
        def_unchanged,
        targets,
    })))
}

/// The definition action a ready window would take.
fn def_action(rw: &ReadyWindow) -> Action {
    if !rw.deployed_exists {
        Action::Create
    } else if rw.def_unchanged {
        Action::None
    } else {
        Action::Update
    }
}

/// Plan-phase previews for one window: the definition, then each attach target.
fn preview_for(planned: &Planned) -> Vec<ApplyOutcome> {
    match planned {
        Planned::Failed { name, message } => vec![ApplyOutcome::failed(
            KIND,
            name.clone(),
            Action::None,
            message.clone(),
            "resolve the node reference (import the node) and re-apply",
        )],
        Planned::Ready(rw) => {
            let name = &rw.local.metadata.name;
            let mut out = vec![match def_action(rw) {
                Action::None => ApplyOutcome::new(
                    KIND,
                    name.clone(),
                    Action::None,
                    OutcomeStatus::Unchanged,
                    "in sync",
                ),
                action => ApplyOutcome::would(KIND, name.clone(), action),
            }];
            for t in &rw.targets {
                out.push(ApplyOutcome::would(
                    KIND,
                    target_name(name, t),
                    Action::Update,
                ));
            }
            out
        }
    }
}

/// Execute one ready window: definition first, then attaches (ensure-present).
async fn execute_window(rw: ReadyWindow, api: &MaintenanceApi<'_>, out: &mut Vec<ApplyOutcome>) {
    let name = rw.local.metadata.name.clone();

    // -- Definition --
    let def_ok = if rw.def_unchanged {
        out.push(ApplyOutcome::new(
            KIND,
            name.clone(),
            Action::None,
            OutcomeStatus::Unchanged,
            "in sync",
        ));
        true
    } else {
        let desired = convert::to_wire(&rw.local, &rw.node_ids);
        let (action, status, msg) = if rw.deployed_exists {
            (Action::Update, OutcomeStatus::Updated, "definition updated")
        } else {
            (Action::Create, OutcomeStatus::Created, "definition created")
        };
        match api.upsert(&desired).await {
            Ok(()) => {
                out.push(ApplyOutcome::new(KIND, name.clone(), action, status, msg));
                true
            }
            Err(e) => {
                out.push(ApplyOutcome::failed(
                    KIND,
                    name.clone(),
                    action,
                    e.to_string(),
                    "verify connectivity and re-apply",
                ));
                false
            }
        }
    };

    // -- Attachments (ensure-present), only if the definition is in place --
    for t in &rw.targets {
        let tname = target_name(&name, t);
        if !def_ok {
            out.push(ApplyOutcome::new(
                KIND,
                tname,
                Action::Update,
                OutcomeStatus::Skipped,
                "not attempted (definition write failed)",
            ));
            continue;
        }
        match api.attach(&name, t).await {
            Ok(()) => out.push(ApplyOutcome::new(
                KIND,
                tname,
                Action::Update,
                OutcomeStatus::Ensured,
                "attachment ensured",
            )),
            Err(e) => out.push(ApplyOutcome::failed(
                KIND,
                tname,
                Action::Update,
                e.to_string(),
                "verify the daemon package exists and re-apply",
            )),
        }
    }
}

/// Display name for a per-target outcome, e.g. `weekend [pollerd/prod]`.
fn target_name(window: &str, t: &AttachTarget) -> String {
    match &t.package {
        Some(pkg) => format!("{window} [{}/{}]", t.daemon.segment(), pkg),
        None => format!("{window} [{}]", t.daemon.segment()),
    }
}

/// First duplicate in an iterator of names, if any.
fn first_duplicate<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    for n in names {
        if !seen.insert(n) {
            return Some(n.to_string());
        }
    }
    None
}

/// Current UTC `(year, month 1–12, day, hour, min, sec)` for advisory warnings.
/// Uses Howard Hinnant's civil-from-days; no chrono dependency.
fn now_civil() -> (i32, u8, u8, u8, u8, u8) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, s) = (
        (rem / 3600) as u8,
        ((rem % 3600) / 60) as u8,
        (rem % 60) as u8,
    );
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mon = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mon <= 2 { y + 1 } else { y };
    (year as i32, mon as u8, d as u8, h, m, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::kind::parse_documents;
    use onmsctl_core::{AuthCreds, OutputFormat, Url};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx_for(server: &MockServer) -> Context {
        Context {
            name: "test".into(),
            url: Url::parse(&format!("{}/", server.uri())).unwrap(),
            creds: AuthCreds::basic("admin", "secret"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        }
    }

    fn docs(specs: &[&str]) -> Vec<RawDoc> {
        let yaml = specs.join("---\n");
        parse_documents("maint.yaml", &yaml).unwrap()
    }

    fn daily(name: &str) -> String {
        format!(
            "apiVersion: maintenance.opennms.org/v1\nkind: Maintenance\nmetadata:\n  name: {name}\nspec:\n  schedule:\n    type: daily\n    times:\n      - {{ begins: \"22:00:00\", ends: \"23:00:00\" }}\n  devices:\n    interfaces: [match-any]\n  suppress:\n    polling: {{ packages: [prod] }}\n    notifications: true\n"
        )
    }

    #[tokio::test]
    async fn create_then_attach_both_daemons() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/sched-outages/win"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/sched-outages"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/sched-outages/win/pollerd/prod"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/sched-outages/win/notifd"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let plan = MaintenanceHandler
            .plan(&docs(&[&daily("win")]), &params, &ctx)
            .await
            .unwrap();
        // 1 definition preview + 2 attach previews.
        assert_eq!(plan.preview.len(), 3);
        assert_eq!(plan.preview[0].action, Action::Create);

        let outcomes = MaintenanceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);
        assert!(
            outcomes[1..]
                .iter()
                .all(|o| o.status == OutcomeStatus::Ensured)
        );
    }

    #[tokio::test]
    async fn unchanged_definition_still_ensures_attachments() {
        let server = MockServer::start().await;
        // Deployed definition equals desired (daily 22-23, match-any).
        Mock::given(method("GET"))
            .and(path("/rest/sched-outages/win"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "win", "type": "daily",
                "time": [{ "begins": "22:00:00", "ends": "23:00:00" }],
                "interface": [{ "address": "match-any" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/sched-outages/win/pollerd/prod"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/sched-outages/win/notifd"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let plan = MaintenanceHandler
            .plan(&docs(&[&daily("win")]), &params, &ctx)
            .await
            .unwrap();
        let outcomes = MaintenanceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(
            outcomes[0].status,
            OutcomeStatus::Unchanged,
            "definition in sync"
        );
        assert!(
            outcomes[1..]
                .iter()
                .all(|o| o.status == OutcomeStatus::Ensured)
        );
        // No POST issued for an unchanged definition.
        let reqs = server.received_requests().await.unwrap();
        assert!(
            reqs.iter().all(|r| r.method.as_str() != "POST"),
            "unchanged definition must not POST"
        );
    }

    #[tokio::test]
    async fn duplicate_name_gates() {
        let server = MockServer::start().await;
        let err = match MaintenanceHandler
            .plan(
                &docs(&[&daily("win"), &daily("win")]),
                &ApplyParams::default(),
                &ctx_for(&server),
            )
            .await
        {
            Ok(_) => panic!("expected a duplicate-name gate refusal"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("unique"), "got: {err}");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "gate before any HTTP"
        );
    }

    #[tokio::test]
    async fn unresolved_node_fails_that_window() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/nodes/lab:ghost"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let doc = "apiVersion: maintenance.opennms.org/v1\nkind: Maintenance\nmetadata:\n  name: win\nspec:\n  schedule:\n    type: daily\n    times:\n      - { begins: \"22:00:00\", ends: \"23:00:00\" }\n  devices:\n    nodes:\n      - { foreignSource: lab, foreignId: ghost }\n  suppress:\n    notifications: true\n";
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let plan = MaintenanceHandler
            .plan(&docs(&[doc]), &params, &ctx)
            .await
            .unwrap();
        let outcomes = MaintenanceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes[0].status, OutcomeStatus::Failed);
        assert!(outcomes[0].message.contains("not yet imported"));
        // No definition POST for a window that failed node resolution.
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| r.method.as_str() == "GET")
        );
    }

    #[tokio::test]
    async fn failed_attach_is_reported_and_definition_still_created() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/sched-outages/win"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/sched-outages"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/sched-outages/win/pollerd/prod"))
            .respond_with(ResponseTemplate::new(404).set_body_string("no such package"))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/sched-outages/win/notifd"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let plan = MaintenanceHandler
            .plan(&docs(&[&daily("win")]), &params, &ctx)
            .await
            .unwrap();
        let outcomes = MaintenanceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(
            outcomes[0].status,
            OutcomeStatus::Created,
            "definition still created"
        );
        let pollerd = outcomes
            .iter()
            .find(|o| o.name.contains("pollerd"))
            .unwrap();
        assert_eq!(pollerd.status, OutcomeStatus::Failed);
        let notifd = outcomes.iter().find(|o| o.name.contains("notifd")).unwrap();
        assert_eq!(notifd.status, OutcomeStatus::Ensured);
    }
}
