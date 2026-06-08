/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The plan → gate → execute scheduler (ADR-003).
//!
//! Flow: peek every document's `kind` and reject any unknown kind before
//! touching the network (FR14); **group documents into per-kind buckets** and
//! order the buckets by precedence rank (FR2); plan every bucket and abort the
//! whole apply if any plan fails (the gate); then execute the buckets in order,
//! halting after the first bucket that reports a failure unless
//! `--continue-on-error`. `--dry-run` stops after the plan phase. Dispatch is
//! per-kind-bucket because several leaf invariants are cross-document within a
//! kind. The router owns scheduling only — all reconciliation lives in the
//! handlers (INV1).

use std::collections::HashMap;

use crate::context::Context;
use crate::error::{Error, Result};

use super::envelope::RawDoc;
use super::outcome::ApplyOutcome;
use super::registry::Registry;

pub use super::handler::ApplyParams;

/// Apply a set of documents through the registry. Returns one
/// [`ApplyOutcome`] per document. Per-document logical failures are reported
/// in the returned vector (status `Failed`/`Skipped`); `Err` is returned only
/// for the plan gate (unknown kind, a planning failure).
pub async fn apply_documents(
    registry: &Registry,
    docs: Vec<RawDoc>,
    params: &ApplyParams,
    ctx: &Context,
) -> Result<Vec<ApplyOutcome>> {
    // -- Gate 1 + grouping: every kind must resolve to a handler, before any
    //    work; group documents into per-kind buckets in first-seen order. --
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<RawDoc>> = HashMap::new();
    for doc in docs {
        let kind = doc.peek_kind()?.to_string();
        if !registry.contains(&kind) {
            return Err(Error::Config(format!(
                "{}: unknown kind {:?} — no handler registered (known kinds: {})",
                doc.label(),
                kind,
                known_kinds_list(registry)
            )));
        }
        if !groups.contains_key(&kind) {
            order.push(kind.clone());
        }
        groups.entry(kind).or_default().push(doc);
    }

    // -- Order buckets by precedence rank (ranks are unique per kind). --
    order.sort_by_key(|k| registry.rank(k).expect("kind validated in the gate above"));

    // -- Plan phase: plan every bucket; any failure aborts before execution. --
    let mut planned = Vec::with_capacity(order.len());
    for kind in &order {
        let handler = registry
            .handler(kind)
            .expect("kind validated against the registry in the gate above");
        let bucket = &groups[kind];
        let plan = handler.plan(bucket, params, ctx).await?;
        planned.push((kind.clone(), plan));
    }

    // -- Dry-run: emit each handler's preview verbatim; issue no mutations. --
    if params.dry_run {
        let mut outcomes = Vec::new();
        for (_, plan) in &planned {
            if params.show_diff
                && let Some(diff) = &plan.diff
                && !diff.is_empty()
            {
                eprintln!("{diff}");
            }
            outcomes.extend(plan.preview.iter().cloned());
        }
        return Ok(outcomes);
    }

    // -- Capture per-bucket previews up front so not-attempted accounting can
    //    name the documents in buckets that never run. --
    let bucket_previews: Vec<Vec<ApplyOutcome>> =
        planned.iter().map(|(_, p)| p.preview.clone()).collect();

    // -- Execute phase: bucket by bucket, stop after the first failing bucket
    //    unless continue-on-error. --
    let mut outcomes = Vec::new();
    let mut stopped_at: Option<usize> = None;
    for (i, (kind, plan)) in planned.into_iter().enumerate() {
        if params.show_diff
            && let Some(diff) = &plan.diff
            && !diff.is_empty()
        {
            eprintln!("{diff}");
        }
        let handler = registry
            .handler(&kind)
            .expect("kind validated against the registry in the gate above");
        let preview = plan.preview.clone();
        match handler.execute(plan, params, ctx).await {
            Ok(bucket_outcomes) => {
                let any_failed = bucket_outcomes.iter().any(|o| o.status.is_failure());
                outcomes.extend(bucket_outcomes);
                if any_failed && !params.continue_on_error {
                    stopped_at = Some(i);
                    break;
                }
            }
            Err(e) => {
                // Bucket-level transport fault: preserve the report (Decision 1
                // → Option 2) by marking each document in the bucket Failed,
                // symmetric with the Ok(Failed) path.
                for p in &preview {
                    outcomes.push(ApplyOutcome::failed(
                        p.kind.clone(),
                        p.name.clone(),
                        p.action,
                        e.to_string(),
                        "re-run `onmsctl apply -f` after resolving the error",
                    ));
                }
                if !params.continue_on_error {
                    stopped_at = Some(i);
                    break;
                }
            }
        }
    }

    // -- Account for documents in buckets not attempted after a halt. --
    if let Some(i) = stopped_at {
        for preview in bucket_previews.iter().skip(i + 1) {
            for p in preview {
                outcomes.push(ApplyOutcome::skipped(
                    p.kind.clone(),
                    p.name.clone(),
                    p.action,
                    "not attempted (stopped on a prior failure)",
                ));
            }
        }
    }

    Ok(outcomes)
}

fn known_kinds_list(registry: &Registry) -> String {
    let mut kinds = registry.known_kinds();
    kinds.sort_unstable();
    kinds.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthCreds;
    use crate::format::OutputFormat;
    use crate::kind::envelope::parse_documents;
    use crate::kind::handler::{KindHandler, Plan};
    use crate::kind::outcome::{Action, OutcomeStatus};
    use crate::kind::precedence::{RANK_EVENT_SOURCE, RANK_REQUISITION, RANK_USER};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    fn test_ctx() -> Context {
        Context {
            name: "test".into(),
            url: reqwest::Url::parse("http://unused/").unwrap(),
            creds: AuthCreds::bearer("t"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        }
    }

    /// A fake handler that records the order in which it executes documents,
    /// and can be configured to fail a named document during plan or execute.
    struct Fake {
        kind: &'static str,
        log: Arc<Mutex<Vec<String>>>,
        fail_plan: Option<String>,
        fail_execute: Option<String>,
        err_execute: Option<String>,
    }

    impl Fake {
        fn new(kind: &'static str, log: Arc<Mutex<Vec<String>>>) -> Box<Self> {
            Box::new(Self {
                kind,
                log,
                fail_plan: None,
                fail_execute: None,
                err_execute: None,
            })
        }
        fn fail_plan_for(mut self: Box<Self>, name: &str) -> Box<Self> {
            self.fail_plan = Some(name.to_string());
            self
        }
        /// Return an `Ok` outcome with `Failed` status for this name.
        fn fail_execute_for(mut self: Box<Self>, name: &str) -> Box<Self> {
            self.fail_execute = Some(name.to_string());
            self
        }
        /// Return `Err(transport fault)` from execute when this name is reached.
        fn err_execute_for(mut self: Box<Self>, name: &str) -> Box<Self> {
            self.err_execute = Some(name.to_string());
            self
        }
    }

    fn doc_name(doc: &RawDoc) -> String {
        doc.value
            .get("metadata")
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string()
    }

    #[async_trait]
    impl KindHandler for Fake {
        fn kind(&self) -> &'static str {
            self.kind
        }
        async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, _ctx: &Context) -> Result<Plan> {
            let mut preview = Vec::new();
            for doc in docs {
                let name = doc_name(doc);
                if self.fail_plan.as_deref() == Some(name.as_str()) {
                    return Err(Error::Config(format!("plan rejected {name}")));
                }
                preview.push(ApplyOutcome::would(self.kind, name, Action::Create));
            }
            Ok(Plan::new(preview, Box::new(())))
        }
        async fn execute(
            &self,
            plan: Plan,
            _params: &ApplyParams,
            _ctx: &Context,
        ) -> Result<Vec<ApplyOutcome>> {
            let mut outcomes = Vec::new();
            for p in &plan.preview {
                self.log
                    .lock()
                    .unwrap()
                    .push(format!("{}:{}", p.kind, p.name));
                if self.err_execute.as_deref() == Some(p.name.as_str()) {
                    return Err(Error::Timeout("simulated transport fault".into()));
                }
                if self.fail_execute.as_deref() == Some(p.name.as_str()) {
                    outcomes.push(ApplyOutcome::failed(
                        p.kind.clone(),
                        p.name.clone(),
                        p.action,
                        "execute boom",
                        "fix it",
                    ));
                } else {
                    outcomes.push(ApplyOutcome::new(
                        p.kind.clone(),
                        p.name.clone(),
                        p.action,
                        OutcomeStatus::Created,
                        "created",
                    ));
                }
            }
            Ok(outcomes)
        }
    }

    fn docs(yaml: &str) -> Vec<RawDoc> {
        parse_documents("test.yaml", yaml).unwrap()
    }

    const THREE_KINDS: &str = "\
kind: Requisition
metadata: {name: r1}
---
kind: User
metadata: {name: u1}
---
kind: EventSource
metadata: {name: e1}
";

    fn full_registry(log: &Arc<Mutex<Vec<String>>>) -> Registry {
        let mut reg = Registry::new();
        reg.register(RANK_USER, Fake::new("User", log.clone()));
        reg.register(RANK_EVENT_SOURCE, Fake::new("EventSource", log.clone()));
        reg.register(RANK_REQUISITION, Fake::new("Requisition", log.clone()));
        reg
    }

    #[tokio::test(flavor = "current_thread")]
    async fn executes_buckets_in_precedence_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let reg = full_registry(&log);
        let outcomes =
            apply_documents(&reg, docs(THREE_KINDS), &ApplyParams::default(), &test_ctx())
                .await
                .unwrap();
        assert_eq!(outcomes.len(), 3);
        // User(100) < EventSource(200) < Requisition(300)
        assert_eq!(
            *log.lock().unwrap(),
            vec!["User:u1", "EventSource:e1", "Requisition:r1"]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn groups_multiple_documents_of_one_kind_into_a_single_bucket() {
        // Two User documents must be handed to ONE plan/execute call so the
        // handler can run cross-document invariants.
        let two_users = "\
kind: User
metadata: {name: alice}
---
kind: User
metadata: {name: bob}
";
        let log = Arc::new(Mutex::new(Vec::new()));
        let plan_calls = Arc::new(Mutex::new(0usize));

        struct Counting {
            log: Arc<Mutex<Vec<String>>>,
            plan_calls: Arc<Mutex<usize>>,
        }
        #[async_trait]
        impl KindHandler for Counting {
            fn kind(&self) -> &'static str {
                "User"
            }
            async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, _ctx: &Context) -> Result<Plan> {
                *self.plan_calls.lock().unwrap() += 1;
                let preview = docs
                    .iter()
                    .map(|d| ApplyOutcome::would("User", doc_name(d), Action::Create))
                    .collect();
                Ok(Plan::new(preview, Box::new(())))
            }
            async fn execute(
                &self,
                plan: Plan,
                _params: &ApplyParams,
                _ctx: &Context,
            ) -> Result<Vec<ApplyOutcome>> {
                let mut outcomes = Vec::new();
                for p in &plan.preview {
                    self.log.lock().unwrap().push(p.name.clone());
                    outcomes.push(ApplyOutcome::new(
                        "User",
                        p.name.clone(),
                        Action::Create,
                        OutcomeStatus::Created,
                        "created",
                    ));
                }
                Ok(outcomes)
            }
        }

        let mut reg = Registry::new();
        reg.register(
            RANK_USER,
            Box::new(Counting {
                log: log.clone(),
                plan_calls: plan_calls.clone(),
            }),
        );
        let outcomes = apply_documents(&reg, docs(two_users), &ApplyParams::default(), &test_ctx())
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(*plan_calls.lock().unwrap(), 1, "one plan call for the bucket");
        assert_eq!(*log.lock().unwrap(), vec!["alice", "bob"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unknown_kind_aborts_before_any_execution() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::new();
        reg.register(RANK_USER, Fake::new("User", log.clone()));
        // EventSource/Requisition NOT registered.
        let err = apply_documents(&reg, docs(THREE_KINDS), &ApplyParams::default(), &test_ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(log.lock().unwrap().is_empty(), "nothing should execute");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn plan_failure_gates_the_whole_apply() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::new();
        reg.register(RANK_USER, Fake::new("User", log.clone()).fail_plan_for("u1"));
        reg.register(RANK_EVENT_SOURCE, Fake::new("EventSource", log.clone()));
        reg.register(RANK_REQUISITION, Fake::new("Requisition", log.clone()));
        let err = apply_documents(&reg, docs(THREE_KINDS), &ApplyParams::default(), &test_ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(log.lock().unwrap().is_empty(), "gate aborts before execute");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dry_run_plans_but_does_not_execute() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let reg = full_registry(&log);
        let params = ApplyParams {
            dry_run: true,
            ..Default::default()
        };
        let outcomes = apply_documents(&reg, docs(THREE_KINDS), &params, &test_ctx())
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes.iter().all(|o| o.status == OutcomeStatus::Skipped));
        assert!(log.lock().unwrap().is_empty(), "dry-run executes nothing");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stop_on_error_reports_applied_failed_and_not_attempted() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::new();
        // Fail the EventSource bucket (rank 200, the middle of the three).
        reg.register(RANK_USER, Fake::new("User", log.clone()));
        reg.register(
            RANK_EVENT_SOURCE,
            Fake::new("EventSource", log.clone()).fail_execute_for("e1"),
        );
        reg.register(RANK_REQUISITION, Fake::new("Requisition", log.clone()));
        let outcomes =
            apply_documents(&reg, docs(THREE_KINDS), &ApplyParams::default(), &test_ctx())
                .await
                .unwrap();
        // u1 applied, e1 failed, r1 not attempted.
        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);
        assert_eq!(outcomes[1].status, OutcomeStatus::Failed);
        assert_eq!(outcomes[2].status, OutcomeStatus::Skipped);
        assert_eq!(outcomes[2].kind, "Requisition");
        assert_eq!(outcomes[2].name, "r1");
        assert_eq!(*log.lock().unwrap(), vec!["User:u1", "EventSource:e1"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn transport_error_under_stop_on_error_preserves_the_report() {
        // Decision 1 → Option 2: an execute() Err (transport fault) in
        // stop-on-error mode must NOT discard the report. It produces a Failed
        // row plus faithful not-attempted rows, and returns Ok (exit 1 via the
        // Failed status), symmetric with the Ok(Failed) path.
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::new();
        reg.register(RANK_USER, Fake::new("User", log.clone()));
        reg.register(
            RANK_EVENT_SOURCE,
            Fake::new("EventSource", log.clone()).err_execute_for("e1"),
        );
        reg.register(RANK_REQUISITION, Fake::new("Requisition", log.clone()));
        let outcomes =
            apply_documents(&reg, docs(THREE_KINDS), &ApplyParams::default(), &test_ctx())
                .await
                .expect("transport fault must not abort with Err in Option 2");
        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);
        assert_eq!(outcomes[1].status, OutcomeStatus::Failed);
        assert_eq!(outcomes[1].name, "e1");
        assert_eq!(outcomes[2].status, OutcomeStatus::Skipped);
        assert_eq!(outcomes[2].kind, "Requisition");
        assert_eq!(outcomes[2].name, "r1");
        assert_eq!(*log.lock().unwrap(), vec!["User:u1", "EventSource:e1"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn continue_on_error_attempts_every_bucket() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::new();
        reg.register(RANK_USER, Fake::new("User", log.clone()));
        reg.register(
            RANK_EVENT_SOURCE,
            Fake::new("EventSource", log.clone()).fail_execute_for("e1"),
        );
        reg.register(RANK_REQUISITION, Fake::new("Requisition", log.clone()));
        let params = ApplyParams {
            continue_on_error: true,
            ..Default::default()
        };
        let outcomes = apply_documents(&reg, docs(THREE_KINDS), &params, &test_ctx())
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes[1].status, OutcomeStatus::Failed);
        assert_eq!(outcomes[2].status, OutcomeStatus::Created);
        assert_eq!(
            *log.lock().unwrap(),
            vec!["User:u1", "EventSource:e1", "Requisition:r1"]
        );
    }
}
