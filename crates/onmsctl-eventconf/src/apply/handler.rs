/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `EventSourceHandler` — the eventconf capability's adapter into the core
//! kind-router.
//!
//! `EventSource` documents are mostly independent, with **one** cross-document
//! invariant: `metadata.name` must be unique across the bucket, because the
//! upload endpoint upserts by derived source name (two same-named docs would
//! clobber each other). `plan()` parses the bucket, gates on duplicate names,
//! then fetches each source's server state and computes its action via the
//! shared [`fetch_remote`] / [`diff_source`] reconcile seams in the `target`
//! module; `execute()` uploads each changed source via
//! [`upload_then_optionally_disable`] (the canonical replace path) and follows
//! up with the `spec.enabled` PATCH. The gate-class refusals — duplicate name
//! and ambiguous source name — are raised from `plan()` as `Err` so the router
//! aborts before any upload.

use async_trait::async_trait;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OutcomeStatus, Plan, RawDoc,
    Result,
};

use crate::apply::local::{EventSourceLocal, KIND};
use crate::apply::target::{diff_source, fetch_remote, upload_then_optionally_disable};

/// Handler for `kind: EventSource` documents.
#[derive(Default)]
pub struct EventSourceHandler;

/// What `execute()` will do for one source, decided during `plan()`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExecAction {
    Create,
    Update,
    Unchanged,
}

impl ExecAction {
    fn as_core(self) -> Action {
        match self {
            ExecAction::Create => Action::Create,
            ExecAction::Update => Action::Update,
            ExecAction::Unchanged => Action::None,
        }
    }
}

/// Opaque execute payload: the parsed sources and their planned actions.
struct EventExecPayload {
    sources: Vec<(EventSourceLocal, ExecAction)>,
}

#[async_trait]
impl KindHandler for EventSourceHandler {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        // -- Parse every document first (gate on parse failure). --
        let mut parsed: Vec<(String, EventSourceLocal)> = Vec::with_capacity(docs.len());
        for d in docs {
            // Re-serialize the single already-split document and route it
            // through `from_yaml` for the same validation + guided field-key
            // errors as the legacy path. (Parse-error line numbers refer to
            // this normalized form, not the user's original source bytes.)
            let yaml = serde_norway::to_string(&d.value).map_err(|e| {
                Error::Config(format!(
                    "{}: could not re-serialize document: {e}",
                    d.label()
                ))
            })?;
            let local = EventSourceLocal::from_yaml(yaml.as_bytes()).map_err(|e| match e {
                Error::Config(msg) => Error::Config(format!("{}: {msg}", d.label())),
                other => other,
            })?;
            parsed.push((d.label(), local));
        }

        // -- Cross-document uniqueness gate (the upload upserts by name). --
        check_unique_names(&parsed)?;

        // -- Fetch + diff per source (read-only). Ambiguous name → Err → gate. --
        let mut sources: Vec<(EventSourceLocal, ExecAction)> = Vec::with_capacity(parsed.len());
        let mut preview: Vec<ApplyOutcome> = Vec::with_capacity(parsed.len());
        for (_, local) in parsed {
            let action = match fetch_remote(&local.metadata.name, ctx).await? {
                None => ExecAction::Create,
                Some(remote) => {
                    if diff_source(&local, &remote).is_empty() {
                        ExecAction::Unchanged
                    } else {
                        ExecAction::Update
                    }
                }
            };
            preview.push(ApplyOutcome::would(
                KIND,
                local.metadata.name.clone(),
                action.as_core(),
            ));
            sources.push((local, action));
        }

        Ok(Plan::new(preview, Box::new(EventExecPayload { sources })))
    }

    async fn execute(
        &self,
        plan: Plan,
        _params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let payload = plan.payload.downcast::<EventExecPayload>().map_err(|_| {
            Error::Config("internal: EventSourceHandler payload type mismatch".into())
        })?;

        let mut outcomes = Vec::with_capacity(payload.sources.len());
        for (local, action) in payload.sources {
            let name = local.metadata.name.clone();
            let outcome = match action {
                ExecAction::Unchanged => ApplyOutcome::new(
                    KIND,
                    name,
                    Action::None,
                    OutcomeStatus::Unchanged,
                    "in sync",
                ),
                ExecAction::Create | ExecAction::Update => {
                    let is_update = action == ExecAction::Update;
                    match upload_then_optionally_disable(&local, ctx, is_update).await {
                        Ok(()) => {
                            let (act, status, msg) = if is_update {
                                (Action::Update, OutcomeStatus::Updated, "updated")
                            } else {
                                (Action::Create, OutcomeStatus::Created, "created")
                            };
                            ApplyOutcome::new(KIND, name, act, status, msg)
                        }
                        Err(e) => ApplyOutcome::failed(
                            KIND,
                            name,
                            action.as_core(),
                            e.to_string(),
                            "re-run `onmsctl apply -f` after resolving the error",
                        ),
                    }
                }
            };
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }
}

/// Refuse a bucket that declares the same `metadata.name` in more than one
/// document — the upload endpoint upserts by name, so duplicates would clobber
/// each other silently. `labels` are `source#index` for diagnostics. Mirrors
/// the iam/provisioning cross-document name gates.
fn check_unique_names(parsed: &[(String, EventSourceLocal)]) -> Result<()> {
    use std::collections::BTreeMap;
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (label, local) in parsed {
        by_name
            .entry(local.metadata.name.as_str())
            .or_default()
            .push(label.as_str());
    }
    let dups: Vec<String> = by_name
        .iter()
        .filter(|(_, labels)| labels.len() > 1)
        .map(|(name, labels)| format!("'{name}' ({})", labels.join(", ")))
        .collect();
    if dups.is_empty() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "duplicate EventSource metadata.name across the apply input — the upload \
             endpoint upserts by name, so duplicates would clobber each other: {}",
            dups.join("; ")
        )))
    }
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
            creds: AuthCreds::bearer("t"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        }
    }

    fn source_doc(name: &str) -> Vec<RawDoc> {
        let yaml = format!(
            "apiVersion: eventconf.opennms.org/v1\nkind: EventSource\nmetadata:\n  name: {name}\nspec:\n  enabled: true\n  events:\n    - uei: uei.opennms.org/test/{name}\n      label: Test\n      severity: Warning\n"
        );
        parse_documents("src.yaml", &yaml).unwrap()
    }

    /// Mount the source-list lookup used by `find_source_by_name`.
    async fn mount_source_list(server: &MockServer, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn plan_then_execute_creates_absent_source() {
        let server = MockServer::start().await;
        // Source absent → empty list → plan Create.
        mount_source_list(&server, serde_json::json!({"totalRecords": 0, "items": []})).await;
        Mock::given(method("POST"))
            .and(path("/api/v2/eventconf/upload"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"success": [], "errors": []})),
            )
            // Verify (on server drop) the upload actually happened exactly once,
            // rather than trusting the outcome status alone.
            .expect(1)
            .mount(&server)
            .await;

        let docs = source_doc("cisco.foo");
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let handler = EventSourceHandler;

        let plan = handler.plan(&docs, &params, &ctx).await.unwrap();
        assert_eq!(plan.preview.len(), 1);
        assert_eq!(plan.preview[0].kind, "EventSource");
        assert_eq!(plan.preview[0].action, Action::Create);

        let outcomes = handler.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].name, "cisco.foo");
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);
    }

    #[tokio::test]
    async fn duplicate_metadata_name_gates_with_err() {
        let server = MockServer::start().await;
        // Two EventSource docs share metadata.name → upsert-by-name would
        // clobber. Must refuse before any fetch (no mocks mounted).
        let yaml = "apiVersion: eventconf.opennms.org/v1\nkind: EventSource\nmetadata:\n  name: dup\nspec:\n  enabled: true\n  events:\n    - uei: uei.x/a\n      label: A\n      severity: Warning\n---\napiVersion: eventconf.opennms.org/v1\nkind: EventSource\nmetadata:\n  name: dup\nspec:\n  enabled: true\n  events:\n    - uei: uei.x/b\n      label: B\n      severity: Major\n";
        let docs = parse_documents("dup.yaml", yaml).unwrap();
        assert_eq!(docs.len(), 2);
        let ctx = ctx_for(&server);
        let err = match EventSourceHandler
            .plan(&docs, &ApplyParams::default(), &ctx)
            .await
        {
            Ok(_) => panic!("expected a duplicate-name refusal"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("duplicate"), "{msg}");
        assert!(
            msg.contains("dup.yaml#0") && msg.contains("dup.yaml#1"),
            "message should name both colliding docs: {msg}"
        );
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "duplicate-name gate must issue no HTTP"
        );
    }

    #[tokio::test]
    async fn ambiguous_name_gates_with_err() {
        let server = MockServer::start().await;
        // Two sources share the name → ambiguous → plan() refuses.
        mount_source_list(
            &server,
            serde_json::json!({
                "totalRecords": 2,
                "items": [
                    {"id": 1, "name": "cisco.foo", "fileOrder": 50, "eventCount": 0, "enabled": true},
                    {"id": 2, "name": "cisco.foo", "fileOrder": 51, "eventCount": 0, "enabled": true}
                ]
            }),
        )
        .await;
        // No upload mock: must refuse in plan() before any write.
        let docs = source_doc("cisco.foo");
        let ctx = ctx_for(&server);
        let err = match EventSourceHandler
            .plan(&docs, &ApplyParams::default(), &ctx)
            .await
        {
            Ok(_) => panic!("expected an ambiguous-name refusal"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        assert!(err.to_string().contains("ambiguous"));
        assert!(
            !server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.method.as_str() != "GET"),
            "ambiguous-name refusal must issue no writes"
        );
    }
}
