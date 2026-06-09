/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Live-Horizon integration tests for the top-level `onmsctl apply -f`
//! declarative entrypoint (the kind-router): a directory of mixed-kind YAML
//! documents is peeked, ordered by precedence, planned, gated, and executed
//! through the real capability `KindHandler`s.
//!
//! These exercise the SAME path the binary takes (`resolve_apply_input` →
//! `load_documents` → `apply_documents(&registry, …)`), but assemble the
//! registry inline because the binary's `registry::build()` lives in the bin
//! crate and isn't reachable here. The wiring is kept in lockstep with
//! `crates/onmsctl/src/registry.rs`.
//!
//! `#[ignore]`d like the rest of the IT suite; run via `make integration`.
//! The fine-grained stop-on-error / not-attempted reporting matrix is also
//! covered deterministically by the core router unit tests
//! (`onmsctl-core/src/kind/router.rs`) with fake handlers; here we exercise
//! the real handlers end-to-end against a live server.

use std::path::Path;

use onmsctl_core::apply_input::resolve_apply_input;
use onmsctl_core::kind::load_documents;
use onmsctl_core::kind::precedence::{RANK_EVENT_SOURCE, RANK_REQUISITION, RANK_USER};
use onmsctl_core::{ApplyParams, OutcomeStatus, Registry, apply_documents};

use onmsctl_eventconf::EventConfApi;
use onmsctl_eventconf::apply::EventSourceHandler;
use onmsctl_iam::api::IamApi;
use onmsctl_iam::apply::UserHandler;
use onmsctl_provisioning::api::ProvisioningApi;
use onmsctl_provisioning::apply::ProvisioningHandler;

use onmsctl_it::{ENV_PASSWORD, Harness, harness_or_skip};

/// Assemble the kind registry from the real capability handlers — a mirror of
/// the binary's `registry::build()`, which can't be imported from a bin crate.
fn test_registry() -> Registry {
    let mut reg = Registry::new();
    reg.register(RANK_USER, Box::new(UserHandler));
    reg.register(RANK_EVENT_SOURCE, Box::new(EventSourceHandler));
    reg.register(RANK_REQUISITION, Box::new(ProvisioningHandler));
    reg
}

/// A `kind: User` document whose password is sourced from the harness's own
/// `ONMSCTL_TEST_PASSWORD` env var (guaranteed set when the harness resolves).
fn user_doc(name: &str) -> String {
    format!(
        "apiVersion: onmsctl.no42.org/v1alpha1\n\
         kind: User\n\
         metadata:\n  name: {name}\n\
         spec:\n  fullName: Integration Test\n  roles:\n    - ROLE_USER\n  \
         passwordRef:\n    fromEnv: {ENV_PASSWORD}\n"
    )
}

/// A `kind: EventSource` document. Built from the real local struct and
/// serialized so the YAML field names can't drift from what the handler parses.
fn source_doc(name: &str) -> String {
    use onmsctl_eventconf::apply::{EventDef, EventSourceLocal, EventSourceSpec, Metadata};
    let local = EventSourceLocal {
        api_version: "eventconf.opennms.org/v1".into(),
        kind: "EventSource".into(),
        metadata: Metadata { name: name.into() },
        spec: EventSourceSpec {
            enabled: true,
            events: vec![EventDef {
                uei: format!("uei.opennms.org/it/{name}/0"),
                label: "First".into(),
                severity: "Warning".into(),
                ..EventDef::default()
            }],
        },
    };
    local.validate().expect("EventSource fixture must validate");
    serde_norway::to_string(&local).expect("serialize EventSourceLocal")
}

/// A minimal `kind: Requisition` document with a single node.
fn requisition_doc(name: &str) -> String {
    format!(
        "apiVersion: provisioning.opennms.org/v1\n\
         kind: Requisition\n\
         metadata:\n  name: {name}\n\
         spec:\n  nodes:\n    - foreignId: web01\n      label: web01.acme\n"
    )
}

fn write_doc(dir: &Path, file: &str, body: &str) {
    std::fs::write(dir.join(file), body)
        .unwrap_or_else(|e| panic!("writing fixture {file}: {e}"));
}

/// Resolve a directory of YAML into the router's `RawDoc` list, exactly as the
/// binary's `apply -f <dir>` does.
fn load_dir(dir: &Path) -> Vec<onmsctl_core::RawDoc> {
    let dispatch = resolve_apply_input(dir, &["yaml", "yml"]).expect("resolve dir");
    load_documents(&dispatch).expect("load documents")
}

async fn cleanup(h: &Harness, when: &str) {
    let u = h.cleanup_users().await.unwrap_or(0);
    let s = h.cleanup_event_sources().await.unwrap_or(0);
    let r = h.cleanup_requisitions().await.unwrap_or(0);
    if u + s + r > 0 {
        eprintln!("{when} cleanup: {u} users, {s} sources, {r} requisitions");
    }
}

// ---------------------------------------------------------------------------
// 5.1 — multi-kind happy path
// ---------------------------------------------------------------------------

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn apply_multi_kind_dir_creates_all_three() {
    let h = harness_or_skip!();
    cleanup(&h, "pre").await;

    let user = h.unique_name("u");
    let source = h.unique_name("src");
    let req = h.unique_name("req");

    let dir = tempfile::tempdir().expect("tempdir");
    write_doc(dir.path(), "10-user.yaml", &user_doc(&user));
    write_doc(dir.path(), "20-source.yaml", &source_doc(&source));
    write_doc(dir.path(), "30-requisition.yaml", &requisition_doc(&req));

    let docs = load_dir(dir.path());
    assert_eq!(docs.len(), 3, "three documents resolved from the directory");

    let outcomes = apply_documents(&test_registry(), docs, &ApplyParams::default(), &h.context(false))
        .await
        .expect("multi-kind apply must not hit the plan gate");

    assert_eq!(outcomes.len(), 3, "one outcome per document");
    assert!(
        outcomes.iter().all(|o| !o.status.is_failure()),
        "no document should fail: {outcomes:?}"
    );

    // Verify each resource really landed on the server.
    assert!(
        IamApi::new(h.client())
            .get_user(&user)
            .await
            .expect("get_user")
            .is_some(),
        "user '{user}' must exist after apply"
    );
    assert!(
        EventConfApi::new(h.client())
            .list_source_names()
            .await
            .expect("list_source_names")
            .iter()
            .any(|n| n == &source),
        "source '{source}' must exist after apply"
    );
    assert!(
        ProvisioningApi::new(h.client())
            .get_requisition(&req)
            .await
            .expect("get_requisition")
            .is_some(),
        "requisition '{req}' must exist after apply"
    );

    cleanup(&h, "post").await;
}

// ---------------------------------------------------------------------------
// 5.2 — dry-run issues no mutation; unknown kind aborts before any mutation
// ---------------------------------------------------------------------------

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn apply_dry_run_issues_no_mutation() {
    let h = harness_or_skip!();
    cleanup(&h, "pre").await;

    let user = h.unique_name("u");
    let source = h.unique_name("src");

    let dir = tempfile::tempdir().expect("tempdir");
    write_doc(dir.path(), "10-user.yaml", &user_doc(&user));
    write_doc(dir.path(), "20-source.yaml", &source_doc(&source));

    let params = ApplyParams {
        dry_run: true,
        ..Default::default()
    };
    let outcomes = apply_documents(&test_registry(), load_dir(dir.path()), &params, &h.context(false))
        .await
        .expect("dry-run must not error");

    assert_eq!(outcomes.len(), 2);
    // Dry-run never reports a real mutation status.
    assert!(
        outcomes
            .iter()
            .all(|o| !matches!(o.status, OutcomeStatus::Created | OutcomeStatus::Updated | OutcomeStatus::Deleted)),
        "dry-run must preview only (Skipped/Unchanged), got: {outcomes:?}"
    );

    // And nothing was actually created.
    assert!(
        IamApi::new(h.client()).get_user(&user).await.expect("get_user").is_none(),
        "dry-run must not create the user"
    );
    assert!(
        !EventConfApi::new(h.client())
            .list_source_names()
            .await
            .expect("list_source_names")
            .iter()
            .any(|n| n == &source),
        "dry-run must not create the source"
    );

    cleanup(&h, "post").await;
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn apply_unknown_kind_aborts_before_any_mutation() {
    let h = harness_or_skip!();
    cleanup(&h, "pre").await;

    let user = h.unique_name("u");
    let bogus = h.unique_name("bogus");

    let dir = tempfile::tempdir().expect("tempdir");
    write_doc(dir.path(), "10-user.yaml", &user_doc(&user));
    write_doc(
        dir.path(),
        "20-bogus.yaml",
        &format!("apiVersion: v1\nkind: Nope\nmetadata:\n  name: {bogus}\n"),
    );

    let err = apply_documents(
        &test_registry(),
        load_dir(dir.path()),
        &ApplyParams::default(),
        &h.context(false),
    )
    .await
    .expect_err("an unknown kind must abort the apply at the plan gate");
    assert!(
        err.to_string().contains("unknown kind"),
        "gate error should name the unknown kind: {err}"
    );

    // The whole apply is gated: the valid User doc must NOT have been created.
    assert!(
        IamApi::new(h.client()).get_user(&user).await.expect("get_user").is_none(),
        "unknown-kind gate must abort before any document is executed"
    );

    cleanup(&h, "post").await;
}

// ---------------------------------------------------------------------------
// 5.3 — continue-on-error attempts every bucket
// ---------------------------------------------------------------------------
//
// The applied / failed / not-attempted reporting matrix (and the default
// stop-on-error halt) is covered deterministically by the core router unit
// tests with fake handlers. Here we confirm, against the real handlers, that
// `continue_on_error` lets a clean multi-kind set apply fully end-to-end (every
// bucket attempted, every document reconciled).

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn apply_continue_on_error_attempts_every_bucket() {
    let h = harness_or_skip!();
    cleanup(&h, "pre").await;

    let user = h.unique_name("u");
    let source = h.unique_name("src");
    let req = h.unique_name("req");

    let dir = tempfile::tempdir().expect("tempdir");
    write_doc(dir.path(), "10-user.yaml", &user_doc(&user));
    write_doc(dir.path(), "20-source.yaml", &source_doc(&source));
    write_doc(dir.path(), "30-requisition.yaml", &requisition_doc(&req));

    let params = ApplyParams {
        continue_on_error: true,
        ..Default::default()
    };
    let outcomes = apply_documents(&test_registry(), load_dir(dir.path()), &params, &h.context(false))
        .await
        .expect("continue-on-error apply must not hit the plan gate");

    // Every bucket attempted → one outcome per document, none Skipped
    // (Skipped is the not-attempted marker the router emits only after a halt).
    assert_eq!(outcomes.len(), 3);
    assert!(
        outcomes.iter().all(|o| o.status != OutcomeStatus::Skipped),
        "continue-on-error must attempt every bucket (no not-attempted rows): {outcomes:?}"
    );

    cleanup(&h, "post").await;
}
