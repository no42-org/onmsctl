/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Declarative `apply -f` orchestration for `kind: Requisition`.
//!
//! The library-side driver: takes a parsed [`RequisitionLocal`] and a
//! [`ProvisioningApi`] handle, pulls server state, computes the diff,
//! decides `rescanExisting`, and executes the write sequence (FS
//! upsert/delete → requisition POST → import trigger) per design D7.
//!
//! Out of scope for this module:
//!
//! - CLI flag parsing (lives in [`crate::cmd`])
//! - `--diff` / `-o json` rendering (task 4.5, future)
//! - The `--wait` polling loop (task 6.3, future) — `trigger_import`
//!   is fire-and-forget at this layer
//! - Partial-write recovery message formatting (task 5.5, CLI layer)
//! - Multi-file `apply -f <dir>` orchestration (task 5.10, future)
//!
//! Test strategy: wiremock against a synthetic Horizon, exercising
//! every (remote-state, local-state, decision) tuple in the design
//! D1 + D7 + D9 matrix.
//!
//! Multi-file orchestration (`apply -f <dir>`) lives in [`multi`]:
//! two-phase semantics (parse + collision-check, then apply
//! alphabetically) with continue-on-error default and an opt-in
//! `--stop-on-error` flip.

pub mod multi;

pub use multi::{
    CollisionCode, CollisionFinding, MultiApplyFileResult, MultiApplyOptions, MultiApplyOutcome,
    MultiApplyState, apply_directory,
};

use crate::api::ProvisioningApi;
use crate::diff::{RequisitionDelta, aggregate_rescan_decision, diff_requisition};
use crate::model::{
    RequisitionLocal, requisition_from_wire, requisition_to_wire,
    server::{ForeignSourceServer, RequisitionServer},
};
use onmsctl_core::Result;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Options and outcomes
// ---------------------------------------------------------------------------

/// Caller-facing knobs for [`apply_requisition`]. The library default
/// is fully automatic — `dry_run = false`, `rescan_existing = Auto`.
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// When `true`, computes the diff and the decisions but issues
    /// no mutating HTTP requests. `Outcome::state` reports
    /// [`ApplyState::DryRun`].
    pub dry_run: bool,

    /// Override the automatic `rescanExisting` selection. `Auto`
    /// (the default) feeds the diff's leaf paths through
    /// [`crate::diff::aggregate_rescan_decision`].
    pub rescan_existing: RescanChoice,
}

/// Force / let-auto-decide the `rescanExisting` query parameter on
/// import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RescanChoice {
    /// Decide from diff scan-relevance per design D3.
    #[default]
    Auto,
    /// Force the named value regardless of diff content.
    Force(bool),
}

/// What the apply driver did. Returned even on success so CLI output
/// (and tests) can describe the result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyOutcome {
    pub state: ApplyState,
    /// The computed L2/L3 delta between local and the composite
    /// remote state. Always populated, even when `state` is
    /// [`ApplyState::Unchanged`] (it's empty in that case).
    pub delta: RequisitionDelta,
    /// The `rescanExisting` value the driver decided on (or that
    /// would have been used in dry-run / unchanged paths).
    pub rescan_existing: bool,
    /// What the driver did to `/foreignSources/{fs}` — created,
    /// updated, deleted, or untouched.
    pub foreign_source_action: ForeignSourceAction,
    /// The server's custom foreign-source content at the moment the
    /// apply started, before any writes. `None` when the server had
    /// no custom FS (default-FS in effect). Carried through so the
    /// `--diff` renderer (task 5.9) can show what was deleted or
    /// replaced.
    pub original_remote_fs: Option<ForeignSourceServer>,
    /// The server's `last-import` timestamp on the requisition at the
    /// moment the apply read deployed state, before the import was
    /// triggered. `None` when the requisition didn't exist remotely
    /// (Created path) or when the field was absent. Used by the
    /// `--wait` poller (task 6.3) as the snapshot to watch advance.
    pub pre_trigger_last_import_ms: Option<i64>,
}

/// Top-level apply outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApplyState {
    /// L1 short-circuit: local and remote canonicalize identically.
    /// No HTTP writes issued.
    Unchanged,
    /// `--dry-run` was set; diff was computed but no writes issued.
    DryRun,
    /// Server had no requisition by this foreign-source name; we
    /// POSTed it and triggered import.
    Created,
    /// Server already had a requisition; we POSTed the new state and
    /// triggered import.
    Updated,
}

/// What the driver did to the `/foreignSources/{fs}` endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForeignSourceAction {
    /// No POST or DELETE issued — neither local nor remote has a
    /// custom FS, or `state == Unchanged`.
    NoChange,
    /// Server had no custom FS, local has one; we POSTed it.
    Created,
    /// Both sides had custom FS; we POSTed the new content (upsert).
    Updated,
    /// Server had a custom FS, local omits it; we DELETEd so future
    /// imports use Horizon's default-FS (per design D1).
    Deleted,
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Apply a parsed `kind: Requisition` document against a Horizon
/// instance. Returns an [`ApplyOutcome`] describing what happened.
///
/// Sequence:
///   1. Pull deployed requisition (`GET /requisitions/{fs}`) + FS
///      (`GET /foreignSources/{fs}`) concurrently — both may return
///      404.
///   2. Build the diff baseline. If the local YAML omits
///      `spec.foreignSource` OR the server has no custom FS, GET
///      `/foreignSources/default` and substitute it symmetrically
///      into either side's composite so the diff is apples-to-apples
///      (per design D1).
///   3. Convert remote state → local form via
///      [`requisition_from_wire`], canonicalize both sides, and
///      compute the [`RequisitionDelta`]. Early-exit Unchanged only
///      when the diff is empty, the server already has the
///      requisition, AND no FS-side action is required.
///   4. Choose `rescanExisting` from
///      [`aggregate_rescan_decision`] over the delta's leaves — any
///      single scan-relevant leaf (OR semantics) flips the decision
///      to `true`. `opts.rescan_existing` overrides.
///   5. With `--dry-run`, stop here and return the decisions.
///   6. Execute writes in order (per design D7): foreign-source
///      first (upsert OR delete OR no-op), then requisition POST,
///      then import trigger.
pub async fn apply_requisition(
    local: &RequisitionLocal,
    api: &ProvisioningApi<'_>,
    opts: &ApplyOptions,
) -> Result<ApplyOutcome> {
    let fs_name = local.metadata.name.as_str();

    // ---- 1. Pull deployed state (concurrent — independent reads) ----
    let (remote_req, remote_fs) = tokio::try_join!(
        api.get_requisition(fs_name),
        api.get_foreign_source(fs_name),
    )?;
    // Snapshot the pre-trigger last-import timestamp for the `--wait`
    // poller (task 6.3). Captured before any writes so a CLI wait
    // observes only the import we triggered, not any prior import.
    let pre_trigger_last_import_ms = remote_req.as_ref().and_then(|r| r.last_import);

    // ---- 2. Resolve default-FS once if either side needs it ----
    // Per design D1, when EITHER local omits foreignSource OR remote
    // has no custom FS, the diff baseline for the missing side is
    // Horizon's default. Fetching once and substituting on both
    // sides keeps the diff symmetric — a portable-style YAML against
    // a default-using server produces an empty FS-side diff because
    // both composites land on the same default values.
    let needs_default = remote_fs.is_none() || local.spec.foreign_source.is_none();
    let default_fs = if needs_default {
        Some(api.get_default_foreign_source().await?)
    } else {
        None
    };

    let local_composite = build_local_composite(local, default_fs.as_ref());
    let remote_composite = build_remote_composite(
        fs_name,
        remote_req.as_ref(),
        remote_fs.as_ref(),
        default_fs.as_ref(),
    );

    // ---- 3. Diff + FS-action classification + L1 short-circuit ----
    // The short-circuit only fires when the server already has the
    // requisition, the composite diff is empty, AND no FS-side action
    // is required. Without the fs_action check, a custom FS that
    // canonicalizes equal to default-FS substitution would short-
    // circuit "Unchanged" while still owing a DELETE to bring the
    // server back to default — leaving the server in the wrong state.
    let delta = diff_requisition(&local_composite, &remote_composite);
    let foreign_source_action =
        classify_fs_action(remote_fs.is_some(), local.spec.foreign_source.is_some());

    if delta.is_empty()
        && remote_req.is_some()
        && matches!(foreign_source_action, ForeignSourceAction::NoChange)
    {
        return Ok(ApplyOutcome {
            state: ApplyState::Unchanged,
            delta,
            rescan_existing: false,
            foreign_source_action,
            original_remote_fs: remote_fs,
            pre_trigger_last_import_ms,
        });
    }

    // ---- 4. rescanExisting decision ----
    let rescan_existing = match opts.rescan_existing {
        RescanChoice::Force(b) => b,
        RescanChoice::Auto => aggregate_rescan_decision(delta.iter_paths()),
    };

    // ---- 5. Dry-run: stop here, return decisions ----
    if opts.dry_run {
        return Ok(ApplyOutcome {
            state: ApplyState::DryRun,
            delta,
            rescan_existing,
            foreign_source_action,
            original_remote_fs: remote_fs,
            pre_trigger_last_import_ms,
        });
    }

    // ---- 6. Execute writes per design D7 ----
    let was_created = remote_req.is_none();
    let (wire_req, wire_fs) = requisition_to_wire(local);

    match foreign_source_action {
        ForeignSourceAction::Created | ForeignSourceAction::Updated => {
            if let Some(fs) = &wire_fs {
                api.post_foreign_source(fs).await?;
            }
        }
        ForeignSourceAction::Deleted => {
            api.delete_foreign_source(fs_name).await?;
        }
        ForeignSourceAction::NoChange => {}
    }

    api.post_requisition(&wire_req).await?;
    api.trigger_import(fs_name, rescan_existing).await?;

    Ok(ApplyOutcome {
        state: if was_created {
            ApplyState::Created
        } else {
            ApplyState::Updated
        },
        delta,
        rescan_existing,
        foreign_source_action,
        original_remote_fs: remote_fs,
        pre_trigger_last_import_ms,
    })
}

/// Map (remote-has-custom-FS, local-has-FS) onto the apply action
/// for `/foreignSources/{fs}` per the table in design D1.
fn classify_fs_action(remote_has_fs: bool, local_has_fs: bool) -> ForeignSourceAction {
    match (remote_has_fs, local_has_fs) {
        (false, false) => ForeignSourceAction::NoChange,
        (false, true) => ForeignSourceAction::Created,
        (true, true) => ForeignSourceAction::Updated,
        (true, false) => ForeignSourceAction::Deleted,
    }
}

/// Project the local YAML doc onto a diff-side composite. When the
/// local omits `spec.foreignSource`, substitute Horizon's default-FS
/// (already converted via the wire DTO bridge) so the diff is
/// symmetric with the remote composite's substitution.
fn build_local_composite(
    local: &RequisitionLocal,
    default_fs: Option<&ForeignSourceServer>,
) -> RequisitionLocal {
    if local.spec.foreign_source.is_some() {
        return local.clone();
    }
    // Local omits foreignSource. Substitute the default by routing
    // through requisition_from_wire so the conversion rules are
    // applied consistently with the remote side.
    let mut composite = local.clone();
    composite.spec.foreign_source = default_fs.map(|fs| {
        requisition_from_wire(
            &RequisitionServer {
                foreign_source: local.metadata.name.clone(),
                date_stamp: None,
                last_import: None,
                node: vec![],
            },
            Some(fs),
        )
        .spec
        .foreign_source
        .expect("from_wire with Some(fs) yields Some(foreign_source)")
    });
    composite
}

/// Build the remote-side composite for the diff. Substitutes
/// default-FS if the server has no custom FS for this foreign-source
/// name. When the server has no requisition at all, the composite is
/// "empty nodes + (default-FS or none)".
fn build_remote_composite(
    fs_name: &str,
    remote_req: Option<&RequisitionServer>,
    remote_fs: Option<&ForeignSourceServer>,
    default_fs: Option<&ForeignSourceServer>,
) -> RequisitionLocal {
    let fs_for_composite = remote_fs.or(default_fs);
    match remote_req {
        Some(r) => requisition_from_wire(r, fs_for_composite),
        None => {
            // No server requisition yet — synthesize an empty one
            // and convert with the resolved FS baseline.
            let empty = RequisitionServer {
                foreign_source: fs_name.to_string(),
                date_stamp: None,
                last_import: None,
                node: vec![],
            };
            requisition_from_wire(&empty, fs_for_composite)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — wiremock against synthetic Horizon
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ProvisioningApi;
    use onmsctl_core::{AuthCreds, Context, OnmsClient, OutputFormat, Url};
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_with_client() -> (MockServer, OnmsClient) {
        let server = MockServer::start().await;
        let url = Url::parse(&format!("{}/", server.uri())).unwrap();
        let ctx = Context {
            name: "test".into(),
            url,
            creds: AuthCreds::bearer("t"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
        };
        let client = OnmsClient::from_context(&ctx).unwrap();
        (server, client)
    }

    fn parse_local(yaml: &str) -> RequisitionLocal {
        serde_norway::from_str(yaml).expect("YAML parses")
    }

    fn minimal_local(fs_name: &str) -> RequisitionLocal {
        parse_local(&format!(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: {fs_name}\n\
             spec:\n  nodes: []\n"
        ))
    }

    fn empty_default_fs_response() -> serde_json::Value {
        json!({"name": "default", "scan-interval": "1d", "detectors": [], "policies": []})
    }

    // -- Path 1: server is empty, local has minimal content -> CREATE -----

    #[tokio::test]
    async fn create_when_server_has_no_requisition() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        // Minimal local doc with one node so there IS a diff
        let local = parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  nodes:\n    - foreignId: web01\n      label: web01.acme\n",
        );
        let outcome = apply_requisition(&local, &api, &ApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, ApplyState::Created);
        assert_eq!(outcome.foreign_source_action, ForeignSourceAction::NoChange);
        // Node added is scan-relevant per the classifier.
        assert!(outcome.rescan_existing);
    }

    // -- Path 2: server matches local exactly -> UNCHANGED short-circuit --

    #[tokio::test]
    async fn unchanged_short_circuits_without_writes() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        // Server has an empty requisition; local also empty → identical.
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "node": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;

        let local = minimal_local("acme-prod");
        let outcome = apply_requisition(&local, &api, &ApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, ApplyState::Unchanged);
        assert!(outcome.delta.is_empty());
        // No mutating mocks were defined — if writes had been issued
        // the test would fail with an unmatched-request panic.
    }

    // -- Path 3: dry-run never writes ------------------------------------

    #[tokio::test]
    async fn dry_run_decides_but_does_not_write() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;
        // Explicit .expect(0) on every mutating endpoint — wiremock
        // panics on drop if any of these saw a request. Stronger
        // than "no mock defined" because it asserts intent rather
        // than relying on the default-404 path. Mocks return 200
        // (not 500) so a regression that issues a write during dry-
        // run fails the `outcome.state == DryRun` assertion first
        // rather than masking it behind an HTTP error in `unwrap()`.
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let local = parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  nodes:\n    - foreignId: web01\n      label: w\n",
        );
        let outcome = apply_requisition(
            &local,
            &api,
            &ApplyOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(outcome.state, ApplyState::DryRun);
        assert!(!outcome.delta.is_empty());
    }

    // -- Path 4: FS creation -------------------------------------------

    #[tokio::test]
    async fn fs_creation_posts_fs_then_requisition() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // Default-FS IS fetched: remote has no custom FS, so the
        // diff baseline for the remote composite comes from default.
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let local = parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  foreignSource:\n    scanInterval: 1d\n  nodes: []\n",
        );
        let outcome = apply_requisition(&local, &api, &ApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, ApplyState::Created);
        assert_eq!(outcome.foreign_source_action, ForeignSourceAction::Created);
    }

    // -- Path 5: FS deletion (portable YAML, server has custom FS) -------

    #[tokio::test]
    async fn fs_deletion_when_local_omits_and_server_has_custom() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "node": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "acme-prod",
                "scan-interval": "30m",
                "detectors": [],
                "policies": []
            })))
            .mount(&server)
            .await;
        // Default-FS IS fetched because local omits foreignSource —
        // the diff compares local-as-default vs remote-custom.
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let local = minimal_local("acme-prod");
        let outcome = apply_requisition(&local, &api, &ApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, ApplyState::Updated);
        assert_eq!(outcome.foreign_source_action, ForeignSourceAction::Deleted);
    }

    // -- Path 6: rescan-existing override ----------------------------

    #[tokio::test]
    async fn rescan_existing_force_overrides_classifier() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            // Forced false even though node addition is auto-relevant.
            .and(query_param("rescanExisting", "false"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let local = parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  nodes:\n    - foreignId: web01\n      label: w\n",
        );
        let outcome = apply_requisition(
            &local,
            &api,
            &ApplyOptions {
                dry_run: false,
                rescan_existing: RescanChoice::Force(false),
            },
        )
        .await
        .unwrap();
        assert!(!outcome.rescan_existing);
    }

    // -- Path 7: auto rescan when only an irrelevant leaf changed -----

    #[tokio::test]
    async fn rescan_existing_auto_false_for_irrelevant_change() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "node": [{
                    "foreign-id": "web01",
                    "node-label": "original",
                    "interface": [],
                    "category": [],
                    "asset": [],
                    "meta-data": []
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .and(query_param("rescanExisting", "false"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        // Same foreignId, different label only → label is scan-IRRELEVANT.
        let local = parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  nodes:\n    - foreignId: web01\n      label: changed\n",
        );
        let outcome = apply_requisition(&local, &api, &ApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, ApplyState::Updated);
        assert!(!outcome.rescan_existing, "label-only change is irrelevant");
    }

    // -- Path 8: FS-POST failure aborts before requisition write --------

    #[tokio::test]
    async fn fs_post_failure_aborts_before_requisition_or_import() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;
        // FS POST returns 500 — design D7 requires we abort here.
        Mock::given(method("POST"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        // Requisition POST and import must NOT be issued — assert
        // via .expect(0) so wiremock panics on drop if they fire.
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let local = parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  foreignSource:\n    scanInterval: 1d\n  nodes: []\n",
        );
        let result = apply_requisition(&local, &api, &ApplyOptions::default()).await;
        assert!(result.is_err(), "FS POST 500 must propagate as Err");
    }

    // -- Path 9: category-only change picks rescanExisting=false --------

    #[tokio::test]
    async fn category_only_change_picks_no_rescan() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "node": [{
                    "foreign-id": "web01",
                    "node-label": "web01",
                    "interface": [],
                    "category": [{"name": "Production"}],
                    "asset": [],
                    "meta-data": []
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        // Categories are scan-IRRELEVANT — classifier must pick false.
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .and(query_param("rescanExisting", "false"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let local = parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  nodes:\n\
             \x20   - foreignId: web01\n\
             \x20     label: web01\n\
             \x20     categories: [Production, Critical]\n",
        );
        let outcome = apply_requisition(&local, &api, &ApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, ApplyState::Updated);
        assert!(
            !outcome.rescan_existing,
            "category-only diff must not trigger rescan"
        );
    }

    // -- Path 10: snmpPrimary change picks rescanExisting=true ----------

    #[tokio::test]
    async fn snmp_primary_change_picks_rescan_true() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "node": [{
                    "foreign-id": "web01",
                    "node-label": "web01",
                    "interface": [{
                        "ip-addr": "10.0.0.1",
                        "snmp-primary": "S",
                        "status": 1,
                        "monitored-service": [],
                        "meta-data": []
                    }],
                    "category": [],
                    "asset": [],
                    "meta-data": []
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs_response()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        // snmpPrimary is scan-RELEVANT — classifier must pick true.
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .and(query_param("rescanExisting", "true"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        // Promote 10.0.0.1 from secondary (S) to primary (P).
        let local = parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  nodes:\n\
             \x20   - foreignId: web01\n\
             \x20     label: web01\n\
             \x20     interfaces:\n\
             \x20       - ip: 10.0.0.1\n\
             \x20         snmpPrimary: P\n",
        );
        let outcome = apply_requisition(&local, &api, &ApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, ApplyState::Updated);
        assert!(
            outcome.rescan_existing,
            "snmpPrimary change must trigger rescan"
        );
    }

    // -- Path 11: classify_fs_action matrix ------------------------------

    #[test]
    fn fs_action_classification_matches_design_d1() {
        // (remote_has_fs, local_has_fs) -> action
        assert_eq!(
            classify_fs_action(false, false),
            ForeignSourceAction::NoChange
        );
        assert_eq!(
            classify_fs_action(false, true),
            ForeignSourceAction::Created
        );
        assert_eq!(classify_fs_action(true, true), ForeignSourceAction::Updated);
        assert_eq!(
            classify_fs_action(true, false),
            ForeignSourceAction::Deleted
        );
    }
}
