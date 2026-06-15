/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The multi-instance `kind: DataCollectionSource` handler.
//!
//! Per-source composite reconcile (OpenSpec DC4/DC5/DC8), folded into **one
//! outcome per document**:
//! 1. **Preflight** the endpoint once (DC7) — a 404/405 aborts the whole apply.
//! 2. Gate duplicate `metadata.name` (hard `Err` before any write).
//! 3. For each source: diff the group tree (whole-source upload prunes), reconcile
//!    the optional inline `profileSpec`, and **true-reconcile** the `profiles`
//!    associations (attach missing AND detach dropped — membership is readable).
//!
//! `plan` is read-only (it may fetch live state); `execute` performs the writes.

use async_trait::async_trait;
use std::collections::BTreeSet;

use onmsctl_core::kind::envelope::RawDoc;
use onmsctl_core::kind::handler::{ApplyParams, KindHandler, Plan};
use onmsctl_core::kind::outcome::{Action, ApplyOutcome, OutcomeStatus};
use onmsctl_core::{Context, Error, OnmsClient, Result};

use crate::api::{DataCollectionApi, ProfileWrite};
use crate::convert::{source_unchanged, to_group_xml};
use crate::model::{DataCollectionSourceLocal, KIND, ProfileSpec};
use crate::server::ProfileDto;

/// The handler the binary registers at `RANK_DATACOLLECTION`.
pub struct DataCollectionSourceHandler;

/// What to do with the inline `profileSpec`, resolved in `plan`.
#[derive(Clone, Debug)]
enum ProfilePlan {
    /// No `profileSpec` in the document.
    Absent,
    /// `profileSpec` present and equal to the deployed profile.
    Unchanged,
    /// Create a new profile from `profileSpec`.
    Create,
    /// Update the existing profile (by id) from `profileSpec`.
    Update(i64),
}

/// The resolved per-document plan, replayed in `execute`.
struct DocPlan {
    local: DataCollectionSourceLocal,
    xml: String,
    /// `Some(id)` when the source already exists; `None` ⇒ create.
    source_id: Option<i64>,
    /// Whether the deployed source tree equals the desired one (existing only).
    tree_unchanged: bool,
    /// Desired profile associations (`spec.profiles`).
    desired_profiles: Vec<String>,
    /// Profile names to attach (existing-source path; create attaches via upload).
    attach: Vec<String>,
    /// `(profile_id, profile_name)` to detach.
    detach: Vec<(i64, String)>,
    profile_plan: ProfilePlan,
    /// `Some(target)` when the source's `enabled` state must change.
    enabled_target: Option<bool>,
    /// Overall action for the preview row.
    action: Action,
    /// A plan-time failure (e.g. bare create, profile not found) — execute skips.
    plan_error: Option<String>,
}

impl DocPlan {
    fn name(&self) -> &str {
        &self.local.metadata.name
    }
}

/// Compare a deployed profile against the inline `profileSpec`.
fn profile_differs(dto: &ProfileDto, ps: &ProfileSpec) -> bool {
    dto.rrd_step != ps.rrd_step
        || dto.rrd_rras != ps.rras
        || !dto.storage_flag.eq_ignore_ascii_case(&ps.storage_flag)
}

fn profile_write(ps: &ProfileSpec) -> ProfileWrite {
    ProfileWrite {
        name: ps.name.clone(),
        rrd_step: ps.rrd_step,
        rrd_rras: ps.rras.clone(),
        storage_flag: ps.storage_flag.clone(),
        enabled: true,
    }
}

#[async_trait]
impl KindHandler for DataCollectionSourceHandler {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        // 1. Parse + validate every document; gate duplicate metadata.name.
        let mut locals = Vec::with_capacity(docs.len());
        let mut seen = BTreeSet::new();
        for d in docs {
            let local: DataCollectionSourceLocal = serde_norway::from_value(d.value.clone())
                .map_err(|e| {
                    Error::Config(format!(
                        "{}: invalid `kind: DataCollectionSource` document: {e}",
                        d.label()
                    ))
                })?;
            local.validate()?;
            if !seen.insert(local.metadata.name.clone()) {
                return Err(Error::Config(format!(
                    "duplicate kind: DataCollectionSource metadata.name {:?} — names must be unique \
                     within an apply",
                    local.metadata.name
                )));
            }
            locals.push(local);
        }

        let client = OnmsClient::from_context(ctx)?;
        let api = DataCollectionApi::new(&client);

        // 2. Preflight once (DC7): 404/405 ⇒ "server too old", aborting the apply.
        let summaries = api.preflight().await?;
        let profiles = api.list_profiles().await?;

        // 3. Per-document plan.
        let mut plans = Vec::with_capacity(locals.len());
        let mut previews = Vec::with_capacity(locals.len());
        let mut diff_lines = Vec::new();

        for local in locals {
            let name = local.metadata.name.clone();
            let xml = to_group_xml(&local);
            let source_id = summaries.iter().find(|s| s.name == name).map(|s| s.id);

            // -- group-tree diff (existing) --
            let tree_unchanged = if let Some(id) = source_id {
                match api.download_source(id).await? {
                    Some(dl) => source_unchanged(&local, &dl),
                    None => false, // listed but no download ⇒ treat as changed
                }
            } else {
                false
            };

            // -- profileSpec plan --
            let profile_plan = match &local.spec.profile_spec {
                None => ProfilePlan::Absent,
                Some(ps) => match profiles.iter().find(|p| p.name == ps.name) {
                    None => ProfilePlan::Create,
                    Some(p) if profile_differs(p, ps) => ProfilePlan::Update(p.id),
                    Some(_) => ProfilePlan::Unchanged,
                },
            };

            // -- association reconcile (true reconcile) --
            let desired_profiles: Vec<String> = local.spec.profiles.clone();
            let desired: BTreeSet<&str> = desired_profiles.iter().map(|s| s.as_str()).collect();
            let current: BTreeSet<&str> = profiles
                .iter()
                .filter(|p| p.source_names.iter().any(|s| s == &name))
                .map(|p| p.name.as_str())
                .collect();
            let attach: Vec<String> = desired
                .difference(&current)
                .map(|s| s.to_string())
                .collect();
            let detach: Vec<(i64, String)> = profiles
                .iter()
                .filter(|p| current.contains(p.name.as_str()) && !desired.contains(p.name.as_str()))
                .map(|p| (p.id, p.name.clone()))
                .collect();

            // -- enabled reconcile (existing only; create handles it post-upload) --
            let enabled_target = if let Some(id) = source_id {
                let cur = api.get_source(id).await?.enabled;
                (cur != local.spec.enabled).then_some(local.spec.enabled)
            } else {
                (!local.spec.enabled).then_some(false)
            };

            // -- plan-time validity --
            let mut plan_error = None;
            if source_id.is_none() && local.spec.profiles.is_empty() {
                plan_error = Some(
                    "a new source must be attached to at least one profile — set spec.profiles \
                     (the server rejects a source with no profile)"
                        .into(),
                );
            }
            // pure-C: a desired profile that neither exists nor is created by profileSpec.
            if plan_error.is_none() {
                for want in &local.spec.profiles {
                    let exists = profiles.iter().any(|p| &p.name == want);
                    let is_spec = local
                        .spec
                        .profile_spec
                        .as_ref()
                        .is_some_and(|ps| &ps.name == want);
                    if !exists && !is_spec {
                        plan_error = Some(format!(
                            "spec.profiles references profile {want:?}, which does not exist on the \
                             server (add a profileSpec to create it, or reference an existing profile)"
                        ));
                        break;
                    }
                }
            }

            // -- overall action --
            let profile_changes =
                !matches!(profile_plan, ProfilePlan::Absent | ProfilePlan::Unchanged);
            let action = if plan_error.is_some() {
                Action::Update // a would-be change that fails; status comes from preview
            } else if source_id.is_none() {
                Action::Create
            } else if !tree_unchanged
                || !attach.is_empty()
                || !detach.is_empty()
                || profile_changes
                || enabled_target.is_some()
            {
                Action::Update
            } else {
                Action::None
            };

            // -- preview row + diff line --
            let preview = if let Some(err) = &plan_error {
                ApplyOutcome::failed(
                    KIND,
                    &name,
                    action,
                    err.clone(),
                    "fix the document and re-apply",
                )
            } else {
                ApplyOutcome::would(KIND, &name, action)
            };
            previews.push(preview);
            diff_lines.push(render_diff_line(
                &name,
                source_id.is_some(),
                tree_unchanged,
                &attach,
                &detach,
                &profile_plan,
                enabled_target,
                plan_error.as_deref(),
            ));

            plans.push(DocPlan {
                local,
                xml,
                source_id,
                tree_unchanged,
                attach,
                detach,
                profile_plan,
                enabled_target,
                action,
                plan_error,
                desired_profiles,
            });
        }

        Ok(Plan::new(previews, Box::new(plans)).with_diff(Some(diff_lines.join("\n"))))
    }

    async fn execute(
        &self,
        plan: Plan,
        params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let plans = plan
            .payload
            .downcast::<Vec<DocPlan>>()
            .map_err(|_| Error::Config("internal: DataCollection payload type mismatch".into()))?;
        let client = OnmsClient::from_context(ctx)?;
        let api = DataCollectionApi::new(&client);

        // Fresh profile name→id map for association/profileSpec resolution.
        let mut profile_ids: std::collections::HashMap<String, i64> = api
            .list_profiles()
            .await?
            .into_iter()
            .map(|p| (p.name, p.id))
            .collect();

        let mut outcomes = Vec::with_capacity(plans.len());
        let mut halted = false;

        for dp in plans.into_iter() {
            let name = dp.name().to_string();

            if halted {
                outcomes.push(ApplyOutcome::skipped(
                    KIND,
                    &name,
                    dp.action,
                    "not attempted (a prior source failed and --continue-on-error is off)",
                ));
                continue;
            }

            if let Some(err) = dp.plan_error {
                outcomes.push(ApplyOutcome::failed(
                    KIND,
                    &name,
                    dp.action,
                    err,
                    "fix the document and re-apply",
                ));
                if !params.continue_on_error {
                    halted = true;
                }
                continue;
            }

            // `reconcile_one` folds per-source write errors into a `Failed`
            // outcome (preserving partial progress); `Err` is reserved for a
            // fault not attributable to this document. Either path halts a
            // stop-on-error run.
            let outcome = match reconcile_one(&api, &mut profile_ids, &dp).await {
                Ok(outcome) => outcome,
                Err(e) => ApplyOutcome::failed(
                    KIND,
                    &name,
                    dp.action,
                    e.to_string(),
                    "resolve the reported error and re-apply",
                ),
            };
            if outcome.status.is_failure() && !params.continue_on_error {
                halted = true;
            }
            outcomes.push(outcome);
        }

        Ok(outcomes)
    }
}

/// Run one source's composite reconcile, folded into ONE outcome. On a
/// mid-reconcile write error the already-completed parts are preserved in the
/// outcome (so a failed attach never masks "source replaced") and the
/// structured part list is carried in `details`. Returns `Err` only for a fault
/// that cannot be attributed to this document.
async fn reconcile_one(
    api: &DataCollectionApi<'_>,
    profile_ids: &mut std::collections::HashMap<String, i64>,
    dp: &DocPlan,
) -> Result<ApplyOutcome> {
    let name = dp.name();
    let mut parts: Vec<String> = Vec::new();

    // Each fallible write folds its error into the outcome WITHOUT discarding the
    // parts already done (F3: a failed attach must not mask the source result).
    macro_rules! step {
        ($e:expr) => {
            match $e {
                Ok(v) => v,
                Err(e) => return Ok(folded_failure(dp, &parts, &e.to_string())),
            }
        };
    }

    // 1. profileSpec first, so a newly-created profile can be attached.
    if let Some(ps) = &dp.local.spec.profile_spec {
        match &dp.profile_plan {
            ProfilePlan::Create => {
                let id = step!(api.create_profile(&profile_write(ps)).await);
                profile_ids.insert(ps.name.clone(), id);
                parts.push(format!("profile {:?} created", ps.name));
            }
            ProfilePlan::Update(id) => {
                step!(api.update_profile(*id, &profile_write(ps)).await);
                parts.push(format!("profile {:?} updated", ps.name));
            }
            ProfilePlan::Unchanged => parts.push(format!("profile {:?} in sync", ps.name)),
            ProfilePlan::Absent => {}
        }
    }

    // 2. Source tree.
    match dp.source_id {
        None => {
            // Create: the upload attaches to all desired profiles at once.
            step!(
                api.upload_source(name, dp.xml.clone(), &dp.desired_profiles)
                    .await
            );
            parts.push(format!(
                "source created (attached to {})",
                join_names(&dp.desired_profiles)
            ));
            // enabled: a new source is enabled by default; disable if asked.
            if dp.enabled_target == Some(false) {
                // The source was just uploaded, so it MUST resolve; a miss is an
                // internal inconsistency, not a silent no-op (C1).
                let id = match step!(api.source_id(name).await) {
                    Some(id) => id,
                    None => {
                        return Ok(folded_failure(
                            dp,
                            &parts,
                            &format!(
                                "source {name:?} was created but did not appear in names-and-ids, \
                                 so it could not be disabled — re-apply to reconcile enabled state"
                            ),
                        ));
                    }
                };
                step!(api.set_source_enabled(&[id], false).await);
                parts.push("disabled".into());
            }
        }
        Some(id) => {
            if !dp.tree_unchanged {
                step!(api.upload_source(name, dp.xml.clone(), &[]).await);
                parts.push("source replaced".into());
            } else {
                parts.push("source in sync".into());
            }
            // associations: attach missing, detach dropped (true reconcile).
            for pname in &dp.attach {
                let pid = step!(profile_ids.get(pname).copied().ok_or_else(|| {
                    Error::Config(format!("profile {pname:?} not found for attach"))
                }));
                step!(api.attach_source(pid, name).await);
            }
            if !dp.attach.is_empty() {
                parts.push(format!("attached to {}", join_names(&dp.attach)));
            }
            for (pid, pname) in &dp.detach {
                step!(api.detach_source(*pid, name).await);
                let _ = pname;
            }
            if !dp.detach.is_empty() {
                let names: Vec<String> = dp.detach.iter().map(|(_, n)| n.clone()).collect();
                parts.push(format!("detached from {}", join_names(&names)));
            }
            if let Some(target) = dp.enabled_target {
                step!(api.set_source_enabled(&[id], target).await);
                parts.push(if target { "enabled" } else { "disabled" }.into());
            }
        }
    }

    let (action, status) = match dp.action {
        Action::Create => (Action::Create, OutcomeStatus::Created),
        Action::None => (Action::None, OutcomeStatus::Unchanged),
        _ => (Action::Update, OutcomeStatus::Updated),
    };
    let mut outcome = ApplyOutcome::new(KIND, name, action, status, parts.join("; "));
    outcome.details = part_details(&parts); // F2: per-part results in `details`.
    Ok(outcome)
}

/// Build a `Failed` outcome that preserves the parts already completed (so a
/// late failure does not mask earlier success) plus the structured part list.
fn folded_failure(dp: &DocPlan, parts: &[String], err: &str) -> ApplyOutcome {
    let prefix = if parts.is_empty() {
        String::new()
    } else {
        format!("{}; ", parts.join("; "))
    };
    let mut outcome = ApplyOutcome::failed(
        KIND,
        dp.name(),
        dp.action,
        format!("{prefix}FAILED: {err}"),
        "resolve the reported error and re-apply",
    );
    outcome.details = part_details(parts);
    outcome
}

/// The structured per-part result list for the outcome's `details` field.
fn part_details(parts: &[String]) -> Option<serde_json::Value> {
    Some(serde_json::json!({ "parts": parts }))
}

fn join_names(names: &[String]) -> String {
    if names.is_empty() {
        "none".into()
    } else {
        format!("[{}]", names.join(", "))
    }
}

#[allow(clippy::too_many_arguments)]
fn render_diff_line(
    name: &str,
    exists: bool,
    tree_unchanged: bool,
    attach: &[String],
    detach: &[(i64, String)],
    profile_plan: &ProfilePlan,
    enabled_target: Option<bool>,
    plan_error: Option<&str>,
) -> String {
    if let Some(err) = plan_error {
        return format!("{name}: ERROR — {err}");
    }
    let mut bits = Vec::new();
    bits.push(if !exists {
        "create source".to_string()
    } else if tree_unchanged {
        "tree unchanged".to_string()
    } else {
        "replace tree".to_string()
    });
    match profile_plan {
        ProfilePlan::Create => bits.push("create profile".into()),
        ProfilePlan::Update(_) => bits.push("update profile".into()),
        _ => {}
    }
    if !attach.is_empty() {
        bits.push(format!("attach {}", join_names(attach)));
    }
    if !detach.is_empty() {
        let d: Vec<String> = detach.iter().map(|(_, n)| n.clone()).collect();
        bits.push(format!("detach {}", join_names(&d)));
    }
    if let Some(t) = enabled_target {
        bits.push(if t { "enable".into() } else { "disable".into() });
    }
    format!("{name}: {}", bits.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::format::OutputFormat;
    use onmsctl_core::{AuthCreds, Url};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const NAI: &str = "/api/v2/datacollectionconf/collectsources/names-and-ids";
    const PROFILES: &str = "/api/v2/datacollectionconf/profiles";

    /// `Plan` isn't `Debug`, so `unwrap_err` won't compile — extract the error.
    fn plan_err(r: Result<Plan>) -> Error {
        match r {
            Ok(_) => panic!("expected the plan to fail"),
            Err(e) => e,
        }
    }

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

    fn doc(yaml: &str) -> Vec<RawDoc> {
        onmsctl_core::kind::envelope::parse_documents("t.yaml", yaml).unwrap()
    }

    const NEW_SRC: &str = r#"
apiVersion: datacollection.opennms.org/v1
kind: DataCollectionSource
metadata: { name: acme }
spec:
  profiles: [default]
  groups:
    - name: g1
      ifType: all
      mibObjects: [ { oid: .1.3.6.1, instance: "0", alias: a1, type: counter } ]
"#;

    #[tokio::test]
    async fn preflight_404_aborts_plan() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(NAI))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let err = plan_err(
            DataCollectionSourceHandler
                .plan(&doc(NEW_SRC), &ApplyParams::default(), &ctx_for(&server))
                .await,
        );
        assert!(err.to_string().contains("data-collection REST"), "{err}");
    }

    #[tokio::test]
    async fn duplicate_name_gated() {
        let server = MockServer::start().await;
        // No HTTP mocks needed — the gate fires before preflight.
        let two = format!("{}\n---\n{}", NEW_SRC.trim(), NEW_SRC.trim());
        let err = plan_err(
            DataCollectionSourceHandler
                .plan(&doc(&two), &ApplyParams::default(), &ctx_for(&server))
                .await,
        );
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[tokio::test]
    async fn new_source_creates_and_attaches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(NAI))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(PROFILES))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id":1,"name":"default","rrdStep":300,"rrdRras":["RRA:X"],"storageFlag":"select","sourceNames":[],"enabled":true}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/datacollectionconf/upload"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"errors":[],"success":[{"file":"acme"}]})),
            )
            .mount(&server)
            .await;
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let plan = DataCollectionSourceHandler
            .plan(&doc(NEW_SRC), &params, &ctx)
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Create);
        let out = DataCollectionSourceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, OutcomeStatus::Created);
        assert!(
            out[0].message.contains("attached to [default]"),
            "{}",
            out[0].message
        );
    }

    #[tokio::test]
    async fn new_source_without_profile_is_planned_failure() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(NAI))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(PROFILES))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        let yaml = "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: bare }\nspec:\n  groups:\n    - name: g\n      ifType: all\n      mibObjects: [ { oid: .1.3, instance: '0', alias: a, type: counter } ]\n";
        let ctx = ctx_for(&server);
        let plan = DataCollectionSourceHandler
            .plan(&doc(yaml), &ApplyParams::default(), &ctx)
            .await
            .unwrap();
        assert_eq!(plan.preview[0].status, OutcomeStatus::Failed);
        assert!(plan.preview[0].message.contains("at least one profile"));
    }

    #[tokio::test]
    async fn existing_unchanged_source_with_assoc_changes_updates() {
        let server = MockServer::start().await;
        // Source "acme" exists (id 5), already attached to "old", desired "default".
        Mock::given(method("GET"))
            .and(path(NAI))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id":5,"name":"acme"}])))
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path(PROFILES))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id":1,"name":"default","rrdStep":300,"rrdRras":["RRA:X"],"storageFlag":"select","sourceNames":[],"enabled":true},
                {"id":2,"name":"old","rrdStep":300,"rrdRras":["RRA:X"],"storageFlag":"select","sourceNames":["acme"],"enabled":true}
            ])))
            .mount(&server).await;
        // Tree download equals desired (single group g1 / a1 / counter) → unchanged.
        Mock::given(method("GET")).and(path("/api/v2/datacollectionconf/collectsources/5/download"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name":"acme","resourceTypes":[],"systemDefs":[],
                "groups":[{"name":"g1","ifType":"all","includeGroups":[],"mibObjs":[{"oid":".1.3.6.1","instance":"0","alias":"a1","type":"counter"}]}]
            })))
            .mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/v2/datacollectionconf/collectsources/5"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id":5,"name":"acme","enabled":true})),
            )
            .mount(&server)
            .await;
        // Expect: attach to default (id 1), detach from old (id 2). No upload (tree unchanged).
        Mock::given(method("POST"))
            .and(path("/api/v2/datacollectionconf/profiles/1/sources"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/v2/datacollectionconf/profiles/2/sources/acme"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let plan = DataCollectionSourceHandler
            .plan(&doc(NEW_SRC), &params, &ctx)
            .await
            .unwrap();
        assert_eq!(
            plan.preview[0].action,
            Action::Update,
            "assoc change ⇒ Update even with unchanged tree"
        );
        let out = DataCollectionSourceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Updated);
        assert!(
            out[0].message.contains("source in sync"),
            "{}",
            out[0].message
        );
        assert!(
            out[0].message.contains("attached to [default]"),
            "{}",
            out[0].message
        );
        assert!(
            out[0].message.contains("detached from [old]"),
            "{}",
            out[0].message
        );
        // Mocks with .expect(1) verify on drop that attach + detach each fired once.
        // F2: the per-part results are also carried in `details`.
        assert!(out[0].details.is_some(), "details payload populated");
    }

    /// F3: a source write that succeeds followed by a failing attach SHALL
    /// produce a `Failed` outcome that still records the source result (the
    /// failure must not mask "source replaced").
    #[tokio::test]
    async fn failed_attach_does_not_mask_source_result() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(NAI))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id":5,"name":"acme"}])))
            .mount(&server)
            .await;
        // default (id 1) not yet attached → an attach is planned.
        Mock::given(method("GET")).and(path(PROFILES))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id":1,"name":"default","rrdStep":300,"rrdRras":["RRA:X"],"storageFlag":"select","sourceNames":[],"enabled":true}
            ])))
            .mount(&server).await;
        // Deployed tree DIFFERS (alias a1_old) → tree changed → upload.
        Mock::given(method("GET")).and(path("/api/v2/datacollectionconf/collectsources/5/download"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name":"acme","resourceTypes":[],"systemDefs":[],
                "groups":[{"name":"g1","ifType":"all","includeGroups":[],"mibObjs":[{"oid":".1.3.6.1","instance":"0","alias":"a1_old","type":"counter"}]}]
            })))
            .mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/v2/datacollectionconf/collectsources/5"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id":5,"name":"acme","enabled":true})),
            )
            .mount(&server)
            .await;
        // Source upload SUCCEEDS …
        Mock::given(method("POST"))
            .and(path("/api/v2/datacollectionconf/upload"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"errors":[],"success":[{"file":"acme"}]})),
            )
            .mount(&server)
            .await;
        // … but the attach FAILS.
        Mock::given(method("POST"))
            .and(path("/api/v2/datacollectionconf/profiles/1/sources"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let plan = DataCollectionSourceHandler
            .plan(&doc(NEW_SRC), &params, &ctx)
            .await
            .unwrap();
        let out = DataCollectionSourceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Failed);
        assert!(
            out[0].message.contains("source replaced"),
            "source result preserved: {}",
            out[0].message
        );
        assert!(
            out[0].message.contains("FAILED"),
            "failure recorded: {}",
            out[0].message
        );
        // The preserved parts are in `details` too.
        let parts = out[0].details.as_ref().unwrap()["parts"]
            .as_array()
            .unwrap();
        assert!(parts.iter().any(|p| p.as_str() == Some("source replaced")));
    }
}
