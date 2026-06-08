/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `ProvisioningHandler` — the provisioning capability's adapter into the core
//! kind-router.
//!
//! A thin [`KindHandler`] over the existing multi-file orchestrator. `plan()`
//! parses the bucket of `kind: Requisition` documents, then runs the
//! cross-document collision checks + per-document `plan_requisition` (GET only)
//! via [`plan_parsed`] — the value-based seam carved out of `plan_directory`.
//! A hard collision (duplicate `metadata.name`) is raised as a gate `Err` so
//! the router aborts before any write; soft collisions (duplicate `foreignId`)
//! ride along as per-document warnings. `execute()` runs the pre-computed plans
//! via [`execute_multi`] (no second GET) and maps each file result to an
//! [`ApplyOutcome`].
//!
//! Like the iam handler, `execute_multi` is always run with
//! `stop_on_error = false` so every document in the bucket is attempted and
//! reported accurately; the router's bucket-level stop-on-error governs whether
//! *later kinds* run.

use std::path::PathBuf;

use async_trait::async_trait;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OnmsClient, OutcomeStatus,
    Plan, RawDoc, Result,
};

use crate::api::ProvisioningApi;
use crate::apply::multi::{
    CollisionCode, CollisionFinding, MultiApplyFileResult, MultiApplyOptions, MultiApplyPlan,
    MultiApplyPlanEntry, execute_multi, plan_parsed,
};
use crate::apply::{ApplyState, PlanState, RescanChoice};
use crate::model::{KIND, RequisitionLocal};

/// Handler for `kind: Requisition` documents.
#[derive(Default)]
pub struct ProvisioningHandler;

/// Opaque execute payload: the pre-computed Phase-1 plan.
struct ProvExecPayload {
    plan: MultiApplyPlan,
}

/// Multi-apply options for the kind-router path. Rescan auto-selects (the
/// generic `apply` has no per-kind flag); `stop_on_error` is false so the
/// router governs cross-kind halting (see module docs).
fn multi_opts() -> MultiApplyOptions {
    MultiApplyOptions {
        rescan_existing: RescanChoice::default(),
        stop_on_error: false,
    }
}

#[async_trait]
impl KindHandler for ProvisioningHandler {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        // -- Parse each document into the strict local DTO (gate on failure).
        //    Use the `source#index` label so a same-file multi-doc collision is
        //    distinguishable in findings. --
        let mut parsed: Vec<(PathBuf, RequisitionLocal)> = Vec::with_capacity(docs.len());
        for d in docs {
            let local: RequisitionLocal = serde_norway::from_value(d.value.clone()).map_err(|e| {
                Error::Config(format!(
                    "{}: invalid `kind: Requisition` document: {e}",
                    d.label()
                ))
            })?;
            parsed.push((PathBuf::from(d.label()), local));
        }

        let opts = multi_opts();
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        let plan = plan_parsed(parsed, &api, &opts).await?;

        // -- Hard collision (duplicate metadata.name) → gate Err. Parse errors
        //    can't appear here (we parsed above), so an aborted plan is a hard
        //    collision. --
        if plan.is_aborted() {
            let msg = plan
                .collision_findings
                .iter()
                .filter(|f| f.code == CollisionCode::DuplicateMetadataName)
                .map(|f| f.message.clone())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(Error::Config(msg));
        }

        // -- Build the per-document preview; attach soft-collision warnings
        //    (duplicate foreignId) to the documents they implicate. --
        let soft: Vec<&CollisionFinding> = plan
            .collision_findings
            .iter()
            .filter(|f| f.code != CollisionCode::DuplicateMetadataName)
            .collect();
        let preview = plan
            .entries
            .iter()
            .map(|e| preview_for(e, &soft))
            .collect();

        Ok(Plan::new(preview, Box::new(ProvExecPayload { plan })))
    }

    async fn execute(
        &self,
        plan: Plan,
        _params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let payload = plan
            .payload
            .downcast::<ProvExecPayload>()
            .map_err(|_| Error::Config("internal: ProvisioningHandler payload type mismatch".into()))?;
        let opts = multi_opts();
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        let out = execute_multi(payload.plan, &api, &opts).await?;
        Ok(out.results.into_iter().map(outcome_of).collect())
    }
}

/// The representative action for a requisition plan's state.
fn action_of(state: PlanState) -> Action {
    match state {
        PlanState::Unchanged => Action::None,
        PlanState::WouldCreate => Action::Create,
        PlanState::WouldUpdate => Action::Update,
    }
}

/// Plan-phase preview for one document, with any soft-collision warnings
/// attached (message hint for `-o table`, full text in `details`).
fn preview_for(entry: &MultiApplyPlanEntry, soft: &[&CollisionFinding]) -> ApplyOutcome {
    let name = entry.plan.local.metadata.name.clone();
    let mut o = ApplyOutcome::would(KIND, name, action_of(entry.plan.state));
    let warns: Vec<String> = soft
        .iter()
        .filter(|f| f.files.contains(&entry.path))
        .map(|f| f.message.clone())
        .collect();
    if !warns.is_empty() {
        o.message = format!("{} ({} warning(s))", o.message, warns.len());
        o.details = Some(serde_json::json!({ "warnings": warns }));
    }
    o
}

/// Map an execute-phase per-file result to an `ApplyOutcome`.
fn outcome_of(r: MultiApplyFileResult) -> ApplyOutcome {
    let name = r.foreign_source.clone().unwrap_or_default();
    match r.outcome {
        Ok(po) => match po.state {
            ApplyState::Unchanged => {
                ApplyOutcome::new(KIND, name, Action::None, OutcomeStatus::Unchanged, "in sync")
            }
            ApplyState::Created => {
                ApplyOutcome::new(KIND, name, Action::Create, OutcomeStatus::Created, "created")
            }
            ApplyState::Updated => {
                ApplyOutcome::new(KIND, name, Action::Update, OutcomeStatus::Updated, "updated")
            }
            // execute_multi never runs dry-run; defensive.
            ApplyState::DryRun => {
                ApplyOutcome::new(KIND, name, Action::None, OutcomeStatus::Skipped, "dry-run")
            }
        },
        Err(msg) => ApplyOutcome::failed(
            KIND,
            name,
            Action::None,
            msg,
            "re-run `onmsctl apply -f` after resolving the error",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::kind::parse_documents;
    use onmsctl_core::{AuthCreds, OutputFormat, Url};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx_for(server: &MockServer) -> Context {
        Context {
            name: "test".into(),
            url: Url::parse(&format!("{}/", server.uri())).unwrap(),
            creds: AuthCreds::bearer("t"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        }
    }

    fn req_docs(specs: &[(&str, &str)]) -> Vec<RawDoc> {
        let yaml = specs
            .iter()
            .map(|(name, fid)| {
                format!(
                    "apiVersion: provisioning.opennms.org/v1\nkind: Requisition\nmetadata:\n  name: {name}\nspec:\n  nodes:\n    - foreignId: {fid}\n      label: {fid}.lab\n"
                )
            })
            .collect::<Vec<_>>()
            .join("---\n");
        parse_documents("reqs.yaml", &yaml).unwrap()
    }

    fn empty_default_fs() -> serde_json::Value {
        json!({"name": "default", "scan-interval": "1d", "detectors": [], "policies": []})
    }

    async fn mount_create(server: &MockServer, name: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/rest/requisitions/{name}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/rest/foreignSources/{name}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(server)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("/rest/requisitions/{name}/import")))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn plan_then_execute_creates_absent_requisition() {
        let server = MockServer::start().await;
        mount_create(&server, "acme").await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs()))
            .mount(&server)
            .await;

        let docs = req_docs(&[("acme", "web01")]);
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let handler = ProvisioningHandler;

        let plan = handler.plan(&docs, &params, &ctx).await.unwrap();
        assert_eq!(plan.preview.len(), 1);
        assert_eq!(plan.preview[0].action, Action::Create);
        assert_eq!(plan.preview[0].kind, "Requisition");

        let outcomes = handler.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].name, "acme");
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);
    }

    #[tokio::test]
    async fn duplicate_metadata_name_across_docs_gates_with_err() {
        let server = MockServer::start().await;
        // No HTTP mocks: a hard collision must refuse in plan() before any GET.
        let docs = req_docs(&[("acme", "web01"), ("acme", "web02")]);
        let ctx = ctx_for(&server);
        let err = match ProvisioningHandler
            .plan(&docs, &ApplyParams::default(), &ctx)
            .await
        {
            Ok(_) => panic!("expected a duplicate-metadata.name gate refusal"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        assert!(err.to_string().contains("acme"));
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "hard collision must issue no HTTP"
        );
    }

    #[tokio::test]
    async fn duplicate_foreign_id_warns_in_preview_but_plans() {
        let server = MockServer::start().await;
        for name in ["site-a", "site-b"] {
            mount_create(&server, name).await;
        }
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs()))
            .mount(&server)
            .await;

        // Different names, SAME foreignId → soft collision (warning).
        let docs = req_docs(&[("site-a", "web01"), ("site-b", "web01")]);
        let ctx = ctx_for(&server);
        let plan = ProvisioningHandler
            .plan(&docs, &ApplyParams::default(), &ctx)
            .await
            .unwrap();
        assert_eq!(plan.preview.len(), 2);
        // Both entries implicate the same foreignId → both carry the warning.
        assert!(
            plan.preview.iter().all(|o| o.details.is_some()),
            "soft collision should attach a warning to each implicated document"
        );
        assert!(plan.preview.iter().all(|o| o.message.contains("warning")));
    }
}
