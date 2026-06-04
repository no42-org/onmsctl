/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Multi-file `apply -f <dir>` orchestration.
//!
//! Two-phase semantics (Group 4 of `harden-provisioning-and-eventconf-parity`):
//!
//! - **Phase 1** (`plan_directory`) parses every input file, runs
//!   cross-file collision checks, AND issues per-file read-only GETs
//!   to compute a [`RequisitionPlan`] for each file. Phase 1 is
//!   all-or-nothing: a parse failure, schema error, or hard collision
//!   aborts before Phase 2 begins, **and** any plan-time HTTP error
//!   propagates as `Err` from `plan_directory`. No mutating call is
//!   ever issued in Phase 1.
//! - **Phase 2** (`execute_multi`) consumes the pre-computed plans
//!   and issues mutating calls in alphabetical path order. The
//!   Phase-1 diff is reused — Phase 2 does no second GET. Default is
//!   continue-on-error; `--stop-on-error` switches to kubectl-style
//!   fail-fast.
//!
//! Dry-run lives **above** this layer: the CLI calls `plan_directory`,
//! renders the combined plan to stderr, and either returns (dry-run)
//! or proceeds to `execute_multi`. The plan-vs-execute split itself
//! does not honor `dry_run` internally.
//!
//! Collision rules (unchanged from the original implementation):
//!
//! - **Duplicate `metadata.name`** across files is a HARD ERROR.
//!   Two files trying to manage the same requisition is almost
//!   always an operator mistake; let them resolve it before any
//!   mutation happens.
//! - **Duplicate `foreignId`** across files (different
//!   requisitions, but a node id collides) is a WARNING. Horizon
//!   allows it — foreign-id is scoped per requisition — but it's
//!   almost always a copy-paste mistake. Surface it loudly without
//!   blocking the apply.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::api::ProvisioningApi;
use crate::apply::{
    ApplyOptions, ApplyOutcome, ApplyState, PlanState, RequisitionPlan, RescanChoice, execute_plan,
    plan_requisition,
};
use crate::model::RequisitionLocal;
use onmsctl_core::{Error, Result};

/// Caller-facing knobs for [`plan_directory`] / [`execute_multi`].
///
/// `dry_run` is intentionally absent — the multi-file pipeline
/// honors dry-run at the CLI boundary by calling `plan_directory` +
/// rendering + returning. See module docs.
#[derive(Debug, Clone, Default)]
pub struct MultiApplyOptions {
    /// Forwarded to every per-file `plan_requisition` call.
    pub rescan_existing: RescanChoice,
    /// When `true`, the first per-file error in phase 2 halts the
    /// loop and remaining files are skipped. Default `false`
    /// (continue-on-error).
    pub stop_on_error: bool,
}

/// Aggregate outcome across all input files.
#[derive(Debug, Clone, Serialize)]
pub struct MultiApplyOutcome {
    /// Per-file outcomes in the order they were processed
    /// (alphabetical by path, parse failures interleaved). Files
    /// that hit a phase-1 parse error get an `Err` entry; files
    /// that successfully parsed but failed in phase 2 get an
    /// `Err` from `apply_requisition`'s call.
    pub results: Vec<MultiApplyFileResult>,
    /// Cross-file findings raised in phase 1.
    pub collision_findings: Vec<CollisionFinding>,
    /// Top-level state of the multi-apply run.
    pub state: MultiApplyState,
}

/// Top-level outcome of a multi-apply run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MultiApplyState {
    /// Phase 1 detected a hard collision (duplicate
    /// `metadata.name`). No writes were issued.
    AbortedPhase1,
    /// Phase 2 ran to completion. Individual files may still have
    /// `Err` outcomes — inspect `results`.
    Completed,
    /// `--stop-on-error` was set and a per-file error halted phase
    /// 2. `results` contains the per-file outcomes up to and
    /// including the failing file; later files were not attempted.
    StoppedEarly,
}

/// Per-file outcome. Successful applies carry the structured
/// `ApplyOutcome`; failures carry a human-readable error string
/// (the original `Error::*` is stringified to keep the wire shape
/// `Serialize`-friendly for `-o json` consumers).
#[derive(Debug, Clone, Serialize)]
pub struct MultiApplyFileResult {
    pub path: PathBuf,
    /// The requisition's `metadata.name` from the parsed YAML.
    /// `None` for files that failed parse (no metadata to extract).
    /// Surfaced separately from `ApplyOutcome` because parse-error
    /// rows don't have an outcome but the CLI still wants to
    /// attribute the failure to a foreign-source for the operator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreign_source: Option<String>,
    /// `Ok` when both parse and apply succeeded; `Err` with the
    /// error message otherwise.
    pub outcome: std::result::Result<ApplyOutcome, String>,
}

/// Stable code identifying a cross-file collision class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CollisionCode {
    /// Same `metadata.name` declared in 2+ files. Hard error;
    /// phase 1 aborts before any writes.
    DuplicateMetadataName,
    /// Same `foreignId` declared in 2+ files (under different
    /// `metadata.name`s). Warning; phase 2 continues.
    DuplicateForeignId,
}

#[derive(Debug, Clone, Serialize)]
pub struct CollisionFinding {
    pub code: CollisionCode,
    /// The colliding key (`metadata.name` or `foreignId`).
    pub key: String,
    /// All files declaring the same key.
    pub files: Vec<PathBuf>,
    /// Human-readable message ready for stderr / `-o json`.
    pub message: String,
}

impl CollisionFinding {
    fn is_hard_error(&self) -> bool {
        self.code == CollisionCode::DuplicateMetadataName
    }
}

/// What Phase 1 would do for a single file: a `RequisitionPlan`
/// pinned to its source path. Carries enough context for the CLI
/// layer to render the combined Phase-1 plan and for
/// [`execute_multi`] to run Phase 2 without re-GETting.
#[derive(Debug, Clone)]
pub struct MultiApplyPlanEntry {
    pub path: PathBuf,
    pub plan: RequisitionPlan,
}

/// Phase-1 output across all input files.
///
/// Either `entries` is populated (Phase 1 succeeded across every
/// file; Phase 2 can proceed via [`execute_multi`]) **or** one of
/// `parse_errors` / `collision_findings` carries an abort reason.
/// `is_aborted()` is the load-bearing predicate.
///
/// # Abort-cause invariant
///
/// On the abort path, `parse_errors` and **hard** `collision_findings`
/// are mutually exclusive: a parse error short-circuits Phase 1
/// before the collision check runs, so a plan with non-empty
/// `parse_errors` always has empty `collision_findings`, and vice
/// versa. Soft `DuplicateForeignId` findings ride along with a
/// successful `entries` set, not as an abort cause. The struct shape
/// does not enforce this — callers that must distinguish abort
/// causes should check `parse_errors` first.
///
/// TODO: A future refactor should replace these two `Vec` fields
/// with `enum PlanAbort { ParseErrors(Vec<_>), HardCollisions(Vec<_>) }`
/// and reshape `MultiApplyPlan` to `enum { Ready { entries,
/// warnings, input_count }, Aborted(PlanAbort) }` so the invariant
/// is type-level. Deferred from the Group-4 post-merge review.
#[derive(Debug, Clone)]
pub struct MultiApplyPlan {
    /// Per-file plan entries in alphabetical path order. Populated
    /// only when no parse error and no hard collision aborted Phase 1.
    pub entries: Vec<MultiApplyPlanEntry>,
    /// Parse failures encountered in Phase 1a. Non-empty implies
    /// Phase 1 aborted before Phase 1b's collision check ran.
    pub parse_errors: Vec<MultiApplyFileResult>,
    /// Cross-file findings raised in Phase 1b. May include warnings
    /// (`DuplicateForeignId`) even on the success path.
    pub collision_findings: Vec<CollisionFinding>,
    /// Total deduped input file count. Set by [`plan_directory`]
    /// regardless of abort path so the combined-plan renderer can
    /// display a consistent "Phase 1 plan (N files):" header on both
    /// success and abort.
    pub input_count: usize,
}

impl MultiApplyPlan {
    /// True when Phase 1 surfaced a parse error or a hard collision.
    /// Callers (CLI layer, [`apply_directory`]) MUST NOT call
    /// [`execute_multi`] on an aborted plan.
    pub fn is_aborted(&self) -> bool {
        !self.parse_errors.is_empty()
            || self
                .collision_findings
                .iter()
                .any(CollisionFinding::is_hard_error)
    }
}

/// Phase 1: parse + collision-check + per-file `plan_requisition`.
///
/// All-or-nothing semantics: returns a [`MultiApplyPlan`] populated
/// either with full `entries` (success) or with the abort reason in
/// `parse_errors` / `collision_findings`. Plan-time HTTP failures
/// (e.g. a `GET /requisitions/{fs}` returning 500) propagate as
/// `Err` — the caller can't safely act on partial plan information.
///
/// Files are processed in alphabetical path order, deterministically,
/// regardless of caller-provided ordering.
pub async fn plan_directory(
    files: &[PathBuf],
    api: &ProvisioningApi<'_>,
    opts: &MultiApplyOptions,
) -> Result<MultiApplyPlan> {
    // ---- Phase 1a: read + parse every file ----
    let mut sorted: Vec<PathBuf> = files.to_vec();
    sorted.sort();
    // Same path appearing twice in the caller-provided list would
    // otherwise trigger a misleading DuplicateMetadataName collision
    // listing the same path on both sides ("declared in 2 files: x,
    // x"). Dedup before parsing so the same-path-twice case is a
    // no-op, while genuine cross-file collisions still surface.
    sorted.dedup();
    let input_count = sorted.len();

    let mut parsed: Vec<(PathBuf, RequisitionLocal)> = Vec::new();
    let mut parse_errors: Vec<MultiApplyFileResult> = Vec::new();

    for path in &sorted {
        match read_and_parse(path) {
            Ok(local) => parsed.push((path.clone(), local)),
            Err(e) => parse_errors.push(MultiApplyFileResult {
                path: path.clone(),
                foreign_source: None,
                outcome: Err(e.to_string()),
            }),
        }
    }

    // Parse failure aborts Phase 1 (spec: "all-or-nothing"). Skip
    // collision check and per-file planning — we already know we
    // won't reach Phase 2.
    if !parse_errors.is_empty() {
        return Ok(MultiApplyPlan {
            entries: vec![],
            parse_errors,
            collision_findings: vec![],
            input_count,
        });
    }

    // ---- Phase 1b: cross-file collision checks ----
    let collisions = check_collisions(&parsed);

    if collisions.iter().any(CollisionFinding::is_hard_error) {
        // Hard collision: skip per-file planning. Soft collisions
        // (foreignId warnings) don't reach here — they fall through
        // to per-file planning below and ride along on the plan.
        return Ok(MultiApplyPlan {
            entries: vec![],
            parse_errors: vec![],
            collision_findings: collisions,
            input_count,
        });
    }

    // ---- Phase 1c: per-file plan_requisition (GET only) ----
    let per_file_opts = ApplyOptions {
        dry_run: false,
        rescan_existing: opts.rescan_existing,
    };

    let mut entries: Vec<MultiApplyPlanEntry> = Vec::with_capacity(parsed.len());
    for (path, local) in parsed {
        // Surface which file's plan-time GET failed before returning
        // the bare HTTP error — the operator otherwise sees only the
        // server's URL and can't attribute the failure to a file. We
        // log to stderr (rather than wrapping the Error) so the
        // original variant's exit-code mapping is preserved.
        let plan = match plan_requisition(&local, api, &per_file_opts).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("phase-1 plan failed for {}: {e}", path.display());
                return Err(e);
            }
        };
        entries.push(MultiApplyPlanEntry { path, plan });
    }

    Ok(MultiApplyPlan {
        entries,
        parse_errors: vec![],
        collision_findings: collisions,
        input_count,
    })
}

/// Phase 2: execute the pre-computed plans in alphabetical path
/// order. Reuses each entry's `RequisitionPlan` — no second GET.
///
/// `Unchanged` entries short-circuit into an `Unchanged` outcome
/// without any write. Other entries flow through the single-file
/// `execute_plan`. Errors collect per-file when `stop_on_error` is
/// `false` (default) and halt the loop when it's `true`.
///
/// # Panics
///
/// Panics in `debug_assert!` (and returns `Err` in release) if the
/// plan is aborted — callers must check [`MultiApplyPlan::is_aborted`]
/// first.
pub async fn execute_multi(
    plan: MultiApplyPlan,
    api: &ProvisioningApi<'_>,
    opts: &MultiApplyOptions,
) -> Result<MultiApplyOutcome> {
    debug_assert!(
        !plan.is_aborted(),
        "execute_multi called on an aborted MultiApplyPlan — caller bug"
    );
    if plan.is_aborted() {
        let mut bits: Vec<String> = Vec::new();
        if !plan.parse_errors.is_empty() {
            let paths: Vec<String> = plan
                .parse_errors
                .iter()
                .map(|p| p.path.display().to_string())
                .collect();
            bits.push(format!(
                "{} parse error(s) [{}]",
                plan.parse_errors.len(),
                paths.join(", ")
            ));
        }
        let hard_keys: Vec<&str> = plan
            .collision_findings
            .iter()
            .filter(|f| f.is_hard_error())
            .map(|f| f.key.as_str())
            .collect();
        if !hard_keys.is_empty() {
            bits.push(format!(
                "{} hard collision(s) [{}]",
                hard_keys.len(),
                hard_keys.join(", ")
            ));
        }
        return Err(Error::Config(format!(
            "execute_multi called on aborted MultiApplyPlan (caller bug): {}",
            bits.join("; ")
        )));
    }

    let mut results: Vec<MultiApplyFileResult> = Vec::with_capacity(plan.entries.len());
    let mut stopped_early = false;

    for entry in plan.entries {
        let MultiApplyPlanEntry { path, plan: rp } = entry;
        let fs_name = rp.local.metadata.name.clone();

        // Per-entry short-circuit: Unchanged plans produce an
        // `Unchanged` ApplyOutcome without any HTTP. Other plans
        // hand off to single-file execute_plan, which reuses the
        // pre-computed delta + decisions.
        let outcome = if rp.state == PlanState::Unchanged {
            Ok(rp.into_short_circuit(ApplyState::Unchanged))
        } else {
            execute_plan(rp, api).await
        };

        let is_err = outcome.is_err();
        results.push(MultiApplyFileResult {
            path,
            foreign_source: Some(fs_name),
            outcome: outcome.map_err(|e| e.to_string()),
        });
        if is_err && opts.stop_on_error {
            stopped_early = true;
            break;
        }
    }

    debug_assert!(
        results.windows(2).all(|w| w[0].path <= w[1].path),
        "MultiApplyOutcome.results must remain in alphabetical path order"
    );

    Ok(MultiApplyOutcome {
        results,
        collision_findings: plan.collision_findings,
        state: if stopped_early {
            MultiApplyState::StoppedEarly
        } else {
            MultiApplyState::Completed
        },
    })
}

/// Read a file and parse it as `RequisitionLocal`. Errors carry the
/// path so the CLI can attribute parse failures to their file.
fn read_and_parse(path: &Path) -> Result<RequisitionLocal> {
    let bytes = std::fs::read(path)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
    serde_norway::from_slice::<RequisitionLocal>(&bytes)
        .map_err(|e| Error::Config(format!("{}: {e}", path.display())))
}

/// Walk the parsed file list and emit collision findings.
fn check_collisions(parsed: &[(PathBuf, RequisitionLocal)]) -> Vec<CollisionFinding> {
    let mut findings = Vec::new();

    // metadata.name → files using it
    let mut by_name: BTreeMap<&str, Vec<&Path>> = BTreeMap::new();
    for (path, local) in parsed {
        by_name
            .entry(local.metadata.name.as_str())
            .or_default()
            .push(path);
    }
    for (name, files) in &by_name {
        if files.len() > 1 {
            findings.push(CollisionFinding {
                code: CollisionCode::DuplicateMetadataName,
                key: (*name).to_string(),
                files: files.iter().map(|p| p.to_path_buf()).collect(),
                message: format!(
                    "metadata.name '{name}' declared in {} files: {}",
                    files.len(),
                    files
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    }

    // foreignId → files (the path of the requisition the node belongs to)
    let mut by_fid: BTreeMap<&str, Vec<&Path>> = BTreeMap::new();
    for (path, local) in parsed {
        for node in &local.spec.nodes {
            by_fid
                .entry(node.foreign_id.as_str())
                .or_default()
                .push(path);
        }
    }
    for (fid, files) in &by_fid {
        // Dedup within a single file — a per-file collision is a
        // model-level violation already caught by the parse-time
        // duplicate check; here we only flag CROSS-file.
        let mut unique_files: Vec<&Path> = files.to_vec();
        unique_files.sort();
        unique_files.dedup();
        if unique_files.len() > 1 {
            findings.push(CollisionFinding {
                code: CollisionCode::DuplicateForeignId,
                key: (*fid).to_string(),
                files: unique_files.iter().map(|p| p.to_path_buf()).collect(),
                message: format!(
                    "foreignId '{fid}' appears in {} files: {} (Horizon scopes \
                     foreign-id per requisition — likely a copy-paste mistake)",
                    unique_files.len(),
                    unique_files
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ProvisioningApi;
    use onmsctl_core::{AuthCreds, Context, OnmsClient, OutputFormat, Url};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Test-only thin orchestrator over `plan_directory` + `execute_multi`.
    /// The production path is `cmd::run_apply_files` which composes the
    /// two phases with combined-plan rendering between them; tests use
    /// this helper to assert end-to-end behavior without re-implementing
    /// the wiring in every case.
    async fn apply_directory(
        files: &[PathBuf],
        api: &ProvisioningApi<'_>,
        opts: &MultiApplyOptions,
    ) -> Result<MultiApplyOutcome> {
        let plan = plan_directory(files, api, opts).await?;
        if plan.is_aborted() {
            return Ok(MultiApplyOutcome {
                results: plan.parse_errors,
                collision_findings: plan.collision_findings,
                state: MultiApplyState::AbortedPhase1,
            });
        }
        execute_multi(plan, api, opts).await
    }

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
            iam: Default::default(),
        };
        let client = OnmsClient::from_context(&ctx).unwrap();
        (server, client)
    }

    fn yaml_with_name(name: &str, fid: &str) -> String {
        format!(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: {name}\n\
             spec:\n  nodes:\n    - foreignId: {fid}\n      label: {fid}.lab\n",
        )
    }

    fn empty_default_fs() -> serde_json::Value {
        json!({"name": "default", "scan-interval": "1d", "detectors": [], "policies": []})
    }

    #[tokio::test]
    async fn empty_input_returns_completed_with_zero_results() {
        let (_server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);
        let outcome = apply_directory(&[], &api, &MultiApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, MultiApplyState::Completed);
        assert!(outcome.results.is_empty());
        assert!(outcome.collision_findings.is_empty());
    }

    #[tokio::test]
    async fn duplicate_metadata_name_aborts_phase_1() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.yaml");
        let f2 = dir.path().join("b.yaml");
        std::fs::write(&f1, yaml_with_name("acme-prod", "web01")).unwrap();
        std::fs::write(&f2, yaml_with_name("acme-prod", "web02")).unwrap();

        // No HTTP mocks defined — Phase 1 must abort on hard collision
        // before Phase 1c's GETs run.
        let outcome = apply_directory(
            &[f1.clone(), f2.clone()],
            &api,
            &MultiApplyOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.state, MultiApplyState::AbortedPhase1);
        assert_eq!(outcome.collision_findings.len(), 1);
        let f = &outcome.collision_findings[0];
        assert_eq!(f.code, CollisionCode::DuplicateMetadataName);
        assert_eq!(f.key, "acme-prod");
        assert_eq!(f.files.len(), 2);
        // Positive assertion: zero HTTP requests reached the server,
        // independent of wiremock's panic-on-unmatched behavior.
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "Phase 1 must issue no HTTP requests on hard-collision abort"
        );
    }

    #[tokio::test]
    async fn duplicate_foreign_id_warns_but_continues() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.yaml");
        let f2 = dir.path().join("b.yaml");
        // Different metadata.name, SAME foreignId — warning, not abort.
        std::fs::write(&f1, yaml_with_name("site-a", "web01")).unwrap();
        std::fs::write(&f2, yaml_with_name("site-b", "web01")).unwrap();

        // Phase 2 runs against both — wiremock the GETs + writes.
        for name in ["site-a", "site-b"] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/requisitions/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/rest/foreignSources/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("PUT"))
                .and(path(format!("/rest/requisitions/{name}/import")))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
        }
        // Shared collection create POST (SMOKE-001: POST → /rest/requisitions).
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

        let outcome = apply_directory(&[f1, f2], &api, &MultiApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, MultiApplyState::Completed);
        assert_eq!(outcome.collision_findings.len(), 1);
        assert_eq!(
            outcome.collision_findings[0].code,
            CollisionCode::DuplicateForeignId
        );
        // Both files applied successfully.
        assert_eq!(outcome.results.len(), 2);
        assert!(outcome.results.iter().all(|r| r.outcome.is_ok()));
    }

    #[tokio::test]
    async fn results_are_sorted_alphabetically_by_path() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        let dir = tempfile::tempdir().unwrap();
        // Write in non-alphabetical order, expect alphabetical output.
        let zeta = dir.path().join("zeta.yaml");
        let alpha = dir.path().join("alpha.yaml");
        std::fs::write(&zeta, yaml_with_name("zeta-req", "z1")).unwrap();
        std::fs::write(&alpha, yaml_with_name("alpha-req", "a1")).unwrap();

        for name in ["alpha-req", "zeta-req"] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/requisitions/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/rest/foreignSources/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path(format!("/rest/requisitions/{name}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
                .mount(&server)
                .await;
            Mock::given(method("PUT"))
                .and(path(format!("/rest/requisitions/{name}/import")))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs()))
            .mount(&server)
            .await;

        let outcome = apply_directory(
            // Pass in non-sorted order — function must sort.
            &[zeta.clone(), alpha.clone()],
            &api,
            &MultiApplyOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.state, MultiApplyState::Completed);
        assert_eq!(outcome.results.len(), 2);
        assert_eq!(outcome.results[0].path, alpha);
        assert_eq!(outcome.results[1].path, zeta);
    }

    #[tokio::test]
    async fn parse_error_aborts_phase_1_before_any_http() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.yaml");
        let bad = dir.path().join("bad.yaml");
        std::fs::write(&good, yaml_with_name("good-req", "g1")).unwrap();
        std::fs::write(&bad, "this is not valid yaml: [[[").unwrap();

        // No HTTP mocks defined — Phase 1 must abort before any GET
        // or non-GET is issued.
        let outcome = apply_directory(
            &[good.clone(), bad.clone()],
            &api,
            &MultiApplyOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.state, MultiApplyState::AbortedPhase1);
        // Only the parse error surfaces in results; the good file is
        // not planned because Phase 1 is all-or-nothing.
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].path, bad);
        assert!(outcome.results[0].outcome.is_err());
        // No collision findings — Phase 1b never ran.
        assert!(outcome.collision_findings.is_empty());
        // Positive assertion: zero HTTP requests reached the server,
        // independent of wiremock's panic-on-unmatched behavior.
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "Phase 1 must issue no HTTP requests on parse-error abort"
        );
    }

    #[tokio::test]
    async fn stop_on_error_halts_phase_2_after_first_failure() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.yaml");
        let b = dir.path().join("b.yaml");
        let c = dir.path().join("c.yaml");
        // All three plan successfully (404 → WouldCreate). In Phase 2:
        // a's POST fails (500); --stop-on-error halts before b / c.
        std::fs::write(&a, yaml_with_name("a-req", "a1")).unwrap();
        std::fs::write(&b, yaml_with_name("b-req", "b1")).unwrap();
        std::fs::write(&c, yaml_with_name("c-req", "c1")).unwrap();

        for name in ["a-req", "b-req", "c-req"] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/requisitions/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/rest/foreignSources/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs()))
            .mount(&server)
            .await;
        // Phase 2: a fails (500), no mocks for b / c — wiremock
        // unmatched-request panic if Phase 2 reaches them.
        Mock::given(method("POST"))
            .and(path("/rest/requisitions"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let outcome = apply_directory(
            &[a.clone(), b.clone(), c.clone()],
            &api,
            &MultiApplyOptions {
                stop_on_error: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(outcome.state, MultiApplyState::StoppedEarly);
        // Only `a` was attempted in Phase 2.
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].path, a);
        assert!(outcome.results[0].outcome.is_err());
    }

    #[tokio::test]
    async fn plan_directory_dry_run_path_issues_only_gets() {
        // Task 4.7: when the CLI dry-run flow calls plan_directory
        // and skips execute_multi, only GET requests touch the wire
        // — no POST, no PUT, no DELETE.
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.yaml");
        let b = dir.path().join("b.yaml");
        std::fs::write(&a, yaml_with_name("a-req", "a1")).unwrap();
        std::fs::write(&b, yaml_with_name("b-req", "b1")).unwrap();

        for name in ["a-req", "b-req"] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/requisitions/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/rest/foreignSources/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs()))
            .mount(&server)
            .await;
        // NO POST / PUT / DELETE mocks. If plan_directory issues any,
        // wiremock panics on the unmatched request.

        let plan = plan_directory(&[a.clone(), b.clone()], &api, &MultiApplyOptions::default())
            .await
            .unwrap();

        assert!(!plan.is_aborted());
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].path, a);
        assert_eq!(plan.entries[1].path, b);
        // Both plans are WouldCreate (404 + 404).
        assert_eq!(plan.entries[0].plan.state, PlanState::WouldCreate);
        assert_eq!(plan.entries[1].plan.state, PlanState::WouldCreate);
    }

    #[tokio::test]
    async fn execute_multi_outcomes_match_phase1_plan() {
        // Task 4.6: Phase 2's per-file outcomes correspond 1-1 to
        // Phase 1's plan entries — alphabetical order, same files,
        // ApplyOutcome.state derives from RequisitionPlan.state.
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.yaml");
        let b = dir.path().join("b.yaml");
        std::fs::write(&a, yaml_with_name("a-req", "a1")).unwrap();
        std::fs::write(&b, yaml_with_name("b-req", "b1")).unwrap();

        for name in ["a-req", "b-req"] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/requisitions/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/rest/foreignSources/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            // Per-name import mock; the create POST is a single shared
            // collection mock mounted below (SMOKE-001: POST → /rest/requisitions).
            Mock::given(method("PUT"))
                .and(path(format!("/rest/requisitions/{name}/import")))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
        }
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

        let plan = plan_directory(&[a.clone(), b.clone()], &api, &MultiApplyOptions::default())
            .await
            .unwrap();
        let phase1_states: Vec<_> = plan.entries.iter().map(|e| e.plan.state).collect();

        let outcome = execute_multi(plan, &api, &MultiApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, MultiApplyState::Completed);
        assert_eq!(outcome.results.len(), 2);
        assert_eq!(outcome.results[0].path, a);
        assert_eq!(outcome.results[1].path, b);
        // Phase 1 WouldCreate → Phase 2 Created, one-for-one.
        for (i, ph1) in phase1_states.iter().enumerate() {
            assert_eq!(*ph1, PlanState::WouldCreate);
            let ph2 = outcome.results[i].outcome.as_ref().unwrap();
            assert_eq!(ph2.state, ApplyState::Created);
        }
    }
}
