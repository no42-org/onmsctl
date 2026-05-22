/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Multi-file `apply -f <dir>` orchestration.
//!
//! Two-phase semantics (task 5.10–5.12):
//!
//! - **Phase 1** parses every input file and runs cross-file
//!   collision checks (per task 5.12). No HTTP is issued during
//!   phase 1, so an abort here costs only file-system reads.
//! - **Phase 2** applies parseable files in alphabetical path order.
//!   Default is continue-on-error (later files still attempt their
//!   apply when earlier files failed); `--stop-on-error` switches
//!   to kubectl-style fail-fast (task 5.11).
//!
//! Collision rules (task 5.12):
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

use crate::apply::{ApplyOptions, ApplyOutcome, RescanChoice, apply_requisition};
use crate::api::ProvisioningApi;
use crate::model::RequisitionLocal;
use onmsctl_core::{Error, Result};

/// Caller-facing knobs for [`apply_directory`].
#[derive(Debug, Clone, Default)]
pub struct MultiApplyOptions {
    /// Forwarded to every per-file `apply_requisition` call.
    pub dry_run: bool,
    /// Forwarded to every per-file `apply_requisition` call.
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

/// Apply every requisition YAML in `files` against the provided
/// [`ProvisioningApi`]. Returns once every file has been processed
/// (or earlier on `--stop-on-error` / phase-1 abort).
///
/// `files` is consumed by reference — the caller (CLI layer) is
/// responsible for glob expansion + filtering. Phase 2 processes
/// files in alphabetical path order regardless of the input list's
/// order.
pub async fn apply_directory(
    files: &[PathBuf],
    api: &ProvisioningApi<'_>,
    opts: &MultiApplyOptions,
) -> Result<MultiApplyOutcome> {
    // ---- Phase 1a: read + parse every file ----
    // Sort up-front so per-file results land in alphabetical path
    // order regardless of caller-provided order. This also pins
    // the order collision findings reference (deterministic).
    let mut sorted: Vec<PathBuf> = files.to_vec();
    sorted.sort();

    // Two parallel collections: successfully-parsed (PathBuf,
    // RequisitionLocal) pairs and per-file parse errors that need
    // to surface as Err results.
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

    // ---- Phase 1b: cross-file collision checks ----
    let collisions = check_collisions(&parsed);

    if collisions.iter().any(CollisionFinding::is_hard_error) {
        // Hard error — return parse results plus collision findings
        // without issuing any writes. The CLI layer renders the
        // findings + exits non-zero.
        return Ok(MultiApplyOutcome {
            results: parse_errors,
            collision_findings: collisions,
            state: MultiApplyState::AbortedPhase1,
        });
    }

    // ---- Phase 2: per-file apply in alphabetical order ----
    let per_file_opts = ApplyOptions {
        dry_run: opts.dry_run,
        rescan_existing: opts.rescan_existing,
    };

    let mut results = parse_errors;
    let mut stopped_early = false;
    for (path, local) in parsed {
        let outcome = apply_requisition(&local, api, &per_file_opts).await;
        let is_err = outcome.is_err();
        results.push(MultiApplyFileResult {
            path,
            foreign_source: Some(local.metadata.name.clone()),
            outcome: outcome.map_err(|e| e.to_string()),
        });
        if is_err && opts.stop_on_error {
            stopped_early = true;
            break;
        }
    }

    // `results` is already in alphabetical path order: phase 1a
    // pushed parse errors in `sorted` order, and the phase-2 loop
    // iterates `parsed` (also in `sorted` order) for the
    // successful + failed-apply rows. No re-sort needed; pinned
    // here so a future refactor that breaks insertion order also
    // updates this contract or restores an explicit sort.
    debug_assert!(
        results
            .windows(2)
            .all(|w| w[0].path <= w[1].path),
        "MultiApplyOutcome.results must remain in alphabetical path order"
    );

    Ok(MultiApplyOutcome {
        results,
        collision_findings: collisions,
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

        // No HTTP mocks defined — if phase 2 runs the test panics on
        // unmatched requests.
        let _ = server;
        let outcome = apply_directory(&[f1.clone(), f2.clone()], &api, &MultiApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, MultiApplyState::AbortedPhase1);
        assert_eq!(outcome.collision_findings.len(), 1);
        let f = &outcome.collision_findings[0];
        assert_eq!(f.code, CollisionCode::DuplicateMetadataName);
        assert_eq!(f.key, "acme-prod");
        assert_eq!(f.files.len(), 2);
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
    async fn parse_error_surfaces_as_err_result_continue_on_error() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.yaml");
        let bad = dir.path().join("bad.yaml");
        std::fs::write(&good, yaml_with_name("good-req", "g1")).unwrap();
        std::fs::write(&bad, "this is not valid yaml: [[[").unwrap();

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/good-req"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/good-req"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/good-req"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/good-req/import"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        // Continue-on-error default — both files in results, bad is Err.
        let outcome = apply_directory(&[good.clone(), bad.clone()], &api, &MultiApplyOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.state, MultiApplyState::Completed);
        assert_eq!(outcome.results.len(), 2);
        // Alphabetical: bad before good.
        assert_eq!(outcome.results[0].path, bad);
        assert!(outcome.results[0].outcome.is_err());
        assert_eq!(outcome.results[1].path, good);
        assert!(outcome.results[1].outcome.is_ok());
    }

    #[tokio::test]
    async fn stop_on_error_halts_phase_2_after_first_failure() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.yaml");
        let b = dir.path().join("b.yaml");
        let c = dir.path().join("c.yaml");
        // a parses but its apply fails (500); b would succeed; c never tried.
        std::fs::write(&a, yaml_with_name("a-req", "a1")).unwrap();
        std::fs::write(&b, yaml_with_name("b-req", "b1")).unwrap();
        std::fs::write(&c, yaml_with_name("c-req", "c1")).unwrap();

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/a-req"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        // No mocks for b-req or c-req — wiremock unmatched-request
        // panic if Phase 2 reaches them.
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_default_fs()))
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
        // Only `a` was attempted.
        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].path, a);
        assert!(outcome.results[0].outcome.is_err());
    }
}
