/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `EventSourceOrderHandler` — the router adapter for `kind: EventSourceOrder`.
//!
//! `plan()` validates the singleton, reads the live order, and gates on
//! `spec.first` names that are neither live nor an `EventSource` in the same
//! apply (the router's manifest). It predicts the order after the
//! `EventSource` bucket and decides `Unchanged` or `Update`.
//!
//! `execute()` re-reads the live order (the `EventSource` bucket has run),
//! recomputes the target, uploads the **full** target list as a synthesized
//! `eventconf.xml`, and reads the order back. It never relies on how the
//! server handles a partial list, and a read-back that differs from the
//! target is a `Failed` outcome.

use std::collections::HashSet;

use async_trait::async_trait;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OnmsClient, OutcomeStatus,
    Plan, RawDoc, Result,
};

use super::{
    EventSourceOrderLocal, KIND, SINGLETON_NAME, predict_order, render_diff, target_order,
};

const EVENT_SOURCE_KIND: &str = crate::apply::local::KIND;
const RETRY_HINT: &str = "re-run `onmsctl apply -f` after resolving the error";

/// Handler for `kind: EventSourceOrder` documents.
#[derive(Default)]
pub struct EventSourceOrderHandler;

/// Opaque execute payload: the validated `spec.first` list.
struct OrderExecPayload {
    first: Vec<String>,
}

#[async_trait]
impl KindHandler for EventSourceOrderHandler {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn plan(&self, docs: &[RawDoc], params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        if docs.len() != 1 {
            return Err(Error::Config(format!(
                "kind: {KIND} is a singleton. Expected one document, got {}.",
                docs.len()
            )));
        }
        let local = EventSourceOrderLocal::from_raw(&docs[0])?;
        let first = local.spec.first;

        let client = OnmsClient::from_context(ctx)?;
        let live = crate::api::EventConfApi::new(&client)
            .list_source_order()
            .await?;

        // Sources the same apply creates, in manifest order.
        let live_set: HashSet<&str> = live.iter().map(String::as_str).collect();
        let mut created: Vec<String> = Vec::new();
        for d in &params.manifest {
            if d.kind == EVENT_SOURCE_KIND
                && let Some(n) = &d.name
                && !live_set.contains(n.as_str())
                && !created.contains(n)
            {
                created.push(n.clone());
            }
        }

        // Name gate: a typo aborts the whole apply before any write.
        let missing: Vec<&str> = first
            .iter()
            .filter(|n| !live_set.contains(n.as_str()) && !created.contains(n))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            return Err(Error::Config(format!(
                "{}: spec.first names {} not found on the server or as an EventSource \
                 in this apply: {}",
                docs[0].label(),
                if missing.len() == 1 {
                    "a source"
                } else {
                    "sources"
                },
                missing.join(", ")
            )));
        }

        let predicted = predict_order(&live, &created);
        let target = target_order(&predicted, &first);
        // Where new sources land among themselves is only predicted, so a plan
        // that lists one is an update: execute recomputes from the live order.
        let places_created = first.iter().any(|n| created.contains(n));
        let (action, diff) = if target == predicted && !places_created {
            (Action::None, None)
        } else {
            (
                Action::Update,
                Some(render_diff(&predicted, &target, &first, &created)),
            )
        };
        Ok(Plan::new(
            vec![ApplyOutcome::would(KIND, SINGLETON_NAME, action)],
            Box::new(OrderExecPayload { first }),
        )
        .with_diff(diff))
    }

    async fn execute(
        &self,
        plan: Plan,
        _params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let payload = plan.payload.downcast::<OrderExecPayload>().map_err(|_| {
            Error::Config("internal: EventSourceOrderHandler payload type mismatch".into())
        })?;
        let first = payload.first;
        let client = OnmsClient::from_context(ctx)?;
        let api = crate::api::EventConfApi::new(&client);

        let live = api.list_source_order().await?;
        let missing: Vec<&str> = first
            .iter()
            .filter(|n| !live.contains(n))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            return Ok(vec![ApplyOutcome::failed(
                KIND,
                SINGLETON_NAME,
                Action::Update,
                format!(
                    "spec.first names missing on the server: {}",
                    missing.join(", ")
                ),
                RETRY_HINT,
            )]);
        }

        let target = target_order(&live, &first);
        if target == live {
            return Ok(vec![ApplyOutcome::new(
                KIND,
                SINGLETON_NAME,
                Action::None,
                OutcomeStatus::Unchanged,
                "in sync",
            )]);
        }

        let upload = api.upload_source_order(&target).await?;
        let after = api.list_source_order().await?;
        let outcome = if after == target {
            ApplyOutcome::new(
                KIND,
                SINGLETON_NAME,
                Action::Update,
                OutcomeStatus::Updated,
                "reordered",
            )
        } else {
            ApplyOutcome::failed(
                KIND,
                SINGLETON_NAME,
                Action::Update,
                if upload.errors.is_empty() {
                    "server did not apply the requested order".to_string()
                } else {
                    let errs: Vec<String> = upload
                        .errors
                        .iter()
                        .map(|e| format!("{}: {}", e.file, e.error))
                        .collect();
                    format!(
                        "server did not apply the requested order ({})",
                        errs.join("; ")
                    )
                },
                "check that the server supports reordering through an `eventconf.xml` upload part",
            )
        };
        Ok(vec![outcome])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::CATCH_ALL;
    use onmsctl_core::kind::{DocRef, parse_documents};
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

    fn order_doc(first: &str) -> Vec<RawDoc> {
        let yaml = format!(
            "apiVersion: eventconf.opennms.org/v1\nkind: EventSourceOrder\nmetadata:\n  name: default\nspec:\n  first: {first}\n"
        );
        parse_documents("order.yaml", &yaml).unwrap()
    }

    fn page(names: &[&str]) -> serde_json::Value {
        let n = names.len() as i32;
        let items: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                serde_json::json!({
                    "id": i + 1, "name": name, "fileOrder": n - i as i32,
                    "eventCount": 1, "enabled": true
                })
            })
            .collect();
        serde_json::json!({"totalRecords": names.len(), "items": items})
    }

    /// Serve `first` for the first `times` source reads, then `rest`.
    async fn mount_orders(server: &MockServer, first: &[&str], times: u64, rest: &[&str]) {
        if times > 0 {
            Mock::given(method("GET"))
                .and(path("/api/v2/eventconf/filter/sources"))
                .respond_with(ResponseTemplate::new(200).set_body_json(page(first)))
                .up_to_n_times(times)
                .with_priority(1)
                .mount(server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(rest)))
            .with_priority(2)
            .mount(server)
            .await;
    }

    async fn mount_upload(server: &MockServer, times: u64) {
        Mock::given(method("POST"))
            .and(path("/api/v2/eventconf/upload"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"success": [], "errors": []})),
            )
            .expect(times)
            .mount(server)
            .await;
    }

    const LIVE: &[&str] = &[
        "cisco.syslog",
        "a",
        "b",
        "c",
        "d",
        "e",
        "cisco.custom",
        CATCH_ALL,
    ];
    const ORDERED: &[&str] = &[
        "cisco.custom",
        "cisco.syslog",
        "a",
        "b",
        "c",
        "d",
        "e",
        CATCH_ALL,
    ];

    fn plan_err(r: Result<Plan>) -> String {
        match r {
            Err(Error::Config(m)) => m,
            Err(e) => panic!("expected Config, got {e:?}"),
            Ok(_) => panic!("expected a plan error"),
        }
    }

    #[tokio::test]
    async fn listed_source_moves_to_the_top_and_is_verified() {
        let server = MockServer::start().await;
        // plan read + execute read see LIVE; the read-back sees ORDERED.
        mount_orders(&server, LIVE, 2, ORDERED).await;
        mount_upload(&server, 1).await;
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let h = EventSourceOrderHandler;
        let plan = h
            .plan(&order_doc("[cisco.custom]"), &params, &ctx)
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Update);
        let out = h.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Updated, "{:?}", out[0]);

        let reqs = server.received_requests().await.unwrap();
        let upload = reqs.iter().find(|r| r.method.as_str() == "POST").unwrap();
        let body = String::from_utf8_lossy(&upload.body);
        let mut last = 0;
        for name in ORDERED {
            let at = body
                .find(&format!("<event-file>{name}.xml</event-file>"))
                .unwrap_or_else(|| panic!("{name} missing from full-list master: {body}"));
            assert!(at >= last, "full list must be in target order: {body}");
            last = at;
        }
    }

    #[tokio::test]
    async fn ordered_state_is_unchanged_and_uploads_nothing() {
        let server = MockServer::start().await;
        mount_orders(&server, &[], 0, ORDERED).await;
        mount_upload(&server, 0).await;
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let h = EventSourceOrderHandler;
        let plan = h
            .plan(&order_doc("[cisco.custom]"), &params, &ctx)
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::None);
        assert!(plan.diff.is_none());
        let out = h.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Unchanged);
    }

    #[tokio::test]
    async fn server_ignoring_the_order_is_a_failure() {
        let server = MockServer::start().await;
        mount_orders(&server, &[], 0, LIVE).await;
        mount_upload(&server, 1).await;
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let h = EventSourceOrderHandler;
        let plan = h
            .plan(&order_doc("[cisco.custom]"), &params, &ctx)
            .await
            .unwrap();
        let out = h.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Failed);
        assert!(
            out[0]
                .message
                .contains("server did not apply the requested order"),
            "{:?}",
            out[0]
        );
    }

    #[tokio::test]
    async fn upload_errors_are_reported_on_a_failed_verify() {
        let server = MockServer::start().await;
        mount_orders(&server, &[], 0, LIVE).await;
        Mock::given(method("POST"))
            .and(path("/api/v2/eventconf/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": [],
                "errors": [{"file": "eventconf.xml", "error": "ParseException: bad master"}]
            })))
            .mount(&server)
            .await;
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let h = EventSourceOrderHandler;
        let plan = h
            .plan(&order_doc("[cisco.custom]"), &params, &ctx)
            .await
            .unwrap();
        let out = h.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Failed);
        assert!(
            out[0]
                .message
                .contains("eventconf.xml: ParseException: bad master"),
            "{:?}",
            out[0]
        );
    }

    #[tokio::test]
    async fn listing_a_created_source_plans_an_update_even_when_predicted_in_place() {
        let server = MockServer::start().await;
        mount_orders(&server, &[], 0, LIVE).await;
        let params = ApplyParams {
            manifest: vec![DocRef {
                kind: "EventSource".into(),
                name: Some("acme.traps".into()),
            }],
            ..Default::default()
        };
        let plan = EventSourceOrderHandler
            .plan(&order_doc("[acme.traps]"), &params, &ctx_for(&server))
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Update);
    }

    #[tokio::test]
    async fn typo_fails_the_plan_naming_the_entry() {
        let server = MockServer::start().await;
        mount_orders(&server, &[], 0, LIVE).await;
        let m = plan_err(
            EventSourceOrderHandler
                .plan(
                    &order_doc("[cisco.custon]"),
                    &ApplyParams::default(),
                    &ctx_for(&server),
                )
                .await,
        );
        assert!(m.contains("cisco.custon") && m.contains("not found"), "{m}");
    }

    #[tokio::test]
    async fn co_applied_source_passes_the_gate_and_is_marked_in_the_diff() {
        let server = MockServer::start().await;
        mount_orders(&server, &[], 0, LIVE).await;
        let params = ApplyParams {
            manifest: vec![
                DocRef {
                    kind: "EventSource".into(),
                    name: Some("acme.traps".into()),
                },
                DocRef {
                    kind: KIND.into(),
                    name: Some("default".into()),
                },
            ],
            ..Default::default()
        };
        let plan = EventSourceOrderHandler
            .plan(
                &order_doc("[cisco.custom, acme.traps]"),
                &params,
                &ctx_for(&server),
            )
            .await
            .unwrap();
        let diff = plan.diff.unwrap();
        assert!(diff.contains("cisco.custom  8 -> 1"), "{diff}");
        assert!(
            diff.contains("acme.traps    1 -> 2  (created by this apply)"),
            "{diff}"
        );
    }

    #[tokio::test]
    async fn a_name_from_another_kind_does_not_pass_the_gate() {
        let server = MockServer::start().await;
        mount_orders(&server, &[], 0, LIVE).await;
        let params = ApplyParams {
            manifest: vec![DocRef {
                kind: "Requisition".into(),
                name: Some("acme.traps".into()),
            }],
            ..Default::default()
        };
        let m = plan_err(
            EventSourceOrderHandler
                .plan(&order_doc("[acme.traps]"), &params, &ctx_for(&server))
                .await,
        );
        assert!(m.contains("acme.traps"), "{m}");
    }

    #[tokio::test]
    async fn two_documents_are_a_singleton_error() {
        let mut docs = order_doc("[a]");
        docs.extend(order_doc("[b]"));
        let server = MockServer::start().await;
        let m = plan_err(
            EventSourceOrderHandler
                .plan(&docs, &ApplyParams::default(), &ctx_for(&server))
                .await,
        );
        assert!(m.contains("singleton"), "{m}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    fn registry() -> onmsctl_core::Registry {
        use onmsctl_core::kind::precedence::{RANK_EVENT_SOURCE, RANK_EVENT_SOURCE_ORDER};
        let mut reg = onmsctl_core::Registry::new();
        reg.register(
            RANK_EVENT_SOURCE,
            Box::new(crate::apply::EventSourceHandler),
        );
        reg.register(RANK_EVENT_SOURCE_ORDER, Box::new(EventSourceOrderHandler));
        reg
    }

    /// An order document listed before a new `EventSource`, as in a directory
    /// apply where `00-order.yaml` sorts first.
    fn source_and_order(first: &str) -> Vec<RawDoc> {
        let yaml = format!(
            "apiVersion: eventconf.opennms.org/v1\nkind: EventSourceOrder\nmetadata:\n  name: default\nspec:\n  first: {first}\n---\napiVersion: eventconf.opennms.org/v1\nkind: EventSource\nmetadata:\n  name: acme.traps\nspec:\n  events:\n    - uei: uei.opennms.org/acme/trap\n      label: Acme trap\n      severity: Warning\n"
        );
        parse_documents("eventconf/", &yaml).unwrap()
    }

    async fn mutating_requests(server: &MockServer) -> usize {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() != "GET")
            .count()
    }

    #[tokio::test]
    async fn typo_aborts_the_whole_apply_before_any_write() {
        let server = MockServer::start().await;
        mount_orders(&server, &[], 0, LIVE).await;
        let err = onmsctl_core::apply_documents(
            &registry(),
            source_and_order("[cisco.custon]"),
            &ApplyParams::default(),
            &ctx_for(&server),
        )
        .await
        .expect_err("typo must abort at the plan gate");
        assert!(err.to_string().contains("cisco.custon"), "{err}");
        assert_eq!(
            mutating_requests(&server).await,
            0,
            "no upload for the co-applied EventSource either"
        );
    }

    #[tokio::test]
    async fn dry_run_diff_plans_the_move_and_writes_nothing() {
        let server = MockServer::start().await;
        mount_orders(&server, &[], 0, LIVE).await;
        let params = ApplyParams {
            dry_run: true,
            show_diff: true,
            ..Default::default()
        };
        let outcomes = onmsctl_core::apply_documents(
            &registry(),
            source_and_order("[cisco.custom, acme.traps]"),
            &params,
            &ctx_for(&server),
        )
        .await
        .unwrap();
        let order = outcomes.iter().find(|o| o.kind == KIND).unwrap();
        assert_eq!(order.action, Action::Update);
        assert_eq!(order.status, OutcomeStatus::Skipped);
        assert_eq!(mutating_requests(&server).await, 0);
    }

    #[tokio::test]
    async fn source_missing_at_execute_is_a_failure_without_upload() {
        let server = MockServer::start().await;
        let without: Vec<&str> = LIVE
            .iter()
            .copied()
            .filter(|n| *n != "cisco.custom")
            .collect();
        mount_orders(&server, LIVE, 1, &without).await;
        mount_upload(&server, 0).await;
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let h = EventSourceOrderHandler;
        let plan = h
            .plan(&order_doc("[cisco.custom]"), &params, &ctx)
            .await
            .unwrap();
        let out = h.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Failed);
        assert!(out[0].message.contains("cisco.custom"), "{:?}", out[0]);
    }
}
