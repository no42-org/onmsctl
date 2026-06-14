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

        // One preview row per document (the kind-router contract): the window's
        // definition action carries an attachment summary; the per-attachment
        // results are folded into the single execute outcome's message/details.
        let mut previews = Vec::with_capacity(locals.len());
        let mut windows = Vec::with_capacity(locals.len());
        for local in locals {
            let planned = plan_window(local, &api).await?;
            previews.push(preview_for(&planned));
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
                Planned::Ready(rw) => outcomes.push(execute_window(*rw, &api).await),
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

/// The plan-phase preview for one window — exactly ONE row per document (the
/// kind-router contract). The window's definition action is the row's action; an
/// attachment summary rides in the message.
fn preview_for(planned: &Planned) -> ApplyOutcome {
    match planned {
        Planned::Failed { name, message } => ApplyOutcome::failed(
            KIND,
            name.clone(),
            Action::None,
            message.clone(),
            "resolve the node reference (import the node) and re-apply",
        ),
        Planned::Ready(rw) => {
            let name = rw.local.metadata.name.clone();
            let ensure = ensure_suffix(&rw.targets);
            match def_action(rw) {
                Action::None => ApplyOutcome::new(
                    KIND,
                    name,
                    Action::None,
                    OutcomeStatus::Unchanged,
                    format!("in sync{ensure}"),
                ),
                action => {
                    let mut o = ApplyOutcome::would(KIND, name, action);
                    o.message = format!("{}{ensure}", o.message);
                    o
                }
            }
        }
    }
}

/// Execute one ready window — the composite reconcile folded into ONE outcome:
/// write the definition, then (only if that succeeded) ensure each attachment.
/// The single outcome's status is the definition's, downgraded to `Failed` if
/// the definition write or any attachment failed; the message summarizes the
/// attachment results and `details` carries the structured per-target list.
async fn execute_window(rw: ReadyWindow, api: &MaintenanceApi<'_>) -> ApplyOutcome {
    let name = rw.local.metadata.name.clone();

    // -- Definition --
    let (def_action_taken, def_status, def_msg) = if rw.def_unchanged {
        (Action::None, OutcomeStatus::Unchanged, "definition in sync")
    } else if rw.deployed_exists {
        (Action::Update, OutcomeStatus::Updated, "definition updated")
    } else {
        (Action::Create, OutcomeStatus::Created, "definition created")
    };

    if !rw.def_unchanged {
        let desired = convert::to_wire(&rw.local, &rw.node_ids);
        if let Err(e) = api.upsert(&desired).await {
            return ApplyOutcome::failed(
                KIND,
                name,
                def_action_taken,
                format!("definition write failed: {e}; attachments not attempted"),
                "verify connectivity and re-apply",
            );
        }
    }

    // -- Attachments (ensure-present) — only reached when the definition is in place. --
    let mut ensured: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for t in &rw.targets {
        match api.attach(&name, t).await {
            Ok(()) => ensured.push(target_label(t)),
            Err(e) => failed.push(format!("{}: {e}", target_label(t))),
        }
    }

    let details = serde_json::json!({
        "definition": def_status.to_string(),
        "ensured": ensured,
        "failed": failed,
    });

    if failed.is_empty() {
        let msg = if ensured.is_empty() {
            def_msg.to_string()
        } else {
            format!("{def_msg}; ensured {}", ensured.join(", "))
        };
        let mut o = ApplyOutcome::new(KIND, name, def_action_taken, def_status, msg);
        o.details = Some(details);
        o
    } else {
        let ensured_part = if ensured.is_empty() {
            String::new()
        } else {
            format!("; ensured {}", ensured.join(", "))
        };
        let mut o = ApplyOutcome::failed(
            KIND,
            name,
            def_action_taken,
            format!(
                "{def_msg}{ensured_part}; FAILED to attach {}",
                failed.join("; ")
            ),
            "verify the daemon package(s) exist and re-apply",
        );
        o.details = Some(details);
        o
    }
}

/// `; ensure pollerd/prod, notifd` — the attachment summary appended to a
/// preview message (empty when there are no targets).
fn ensure_suffix(targets: &[AttachTarget]) -> String {
    if targets.is_empty() {
        String::new()
    } else {
        let list = targets
            .iter()
            .map(target_label)
            .collect::<Vec<_>>()
            .join(", ");
        format!("; ensure {list}")
    }
}

/// A compact target label, e.g. `pollerd/prod` or `notifd`.
fn target_label(t: &AttachTarget) -> String {
    match &t.package {
        Some(pkg) => format!("{}/{}", t.daemon.segment(), pkg),
        None => t.daemon.segment().to_string(),
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
        // Exactly ONE preview row per document (the kind-router contract).
        assert_eq!(plan.preview.len(), 1);
        assert_eq!(plan.preview[0].action, Action::Create);
        assert!(
            plan.preview[0].message.contains("ensure"),
            "preview summarizes attachments"
        );

        let outcomes = MaintenanceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1, "one outcome per document");
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);
        assert!(
            outcomes[0].message.contains("pollerd/prod") && outcomes[0].message.contains("notifd"),
            "outcome summarizes both ensured attachments: {}",
            outcomes[0].message
        );
        // Both attach PUTs were issued.
        let reqs = server.received_requests().await.unwrap();
        assert_eq!(
            reqs.iter().filter(|r| r.method.as_str() == "PUT").count(),
            2
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
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0].status,
            OutcomeStatus::Unchanged,
            "definition in sync"
        );
        assert!(
            outcomes[0].message.contains("ensured"),
            "attachments still ensured: {}",
            outcomes[0].message
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
        assert_eq!(plan.preview.len(), 1);
        let outcomes = MaintenanceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1);
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
        // One outcome per document; a failed attach makes the document Failed,
        // but the message records that the definition was created and notifd
        // ensured — so the partial result is visible, not masked.
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, OutcomeStatus::Failed);
        let m = &outcomes[0].message;
        assert!(m.contains("definition created"), "got: {m}");
        assert!(m.contains("ensured notifd"), "notifd ensured recorded: {m}");
        assert!(
            m.contains("FAILED to attach pollerd/prod"),
            "pollerd failure recorded: {m}"
        );
    }

    /// Regression guard: drive the handler THROUGH the kind-router (not directly),
    /// which enforces exactly one preview + one outcome per document. A composite
    /// handler that emits a row per sub-resource (definition + each attachment)
    /// is rejected by the router — the bug a direct handler test cannot catch.
    #[tokio::test]
    async fn router_path_accepts_one_row_per_document() {
        use onmsctl_core::Registry;
        use onmsctl_core::kind::apply_documents;

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

        let mut reg = Registry::new();
        reg.register(900, Box::new(MaintenanceHandler));
        let outcomes = apply_documents(
            &reg,
            docs(&[&daily("win")]),
            &ApplyParams::default(),
            &ctx_for(&server),
        )
        .await
        .expect("router accepts one row per document");
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);
    }
}
