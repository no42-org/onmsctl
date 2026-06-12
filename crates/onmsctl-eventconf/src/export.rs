/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `event-source export` — snapshot server-side event sources as
//! `kind: EventSource` YAML, the reverse of `onmsctl apply -f`.
//!
//! The server only ever emits eventconf XML, so export routes every source
//! through the same XML→YAML [`crate::convert`] migrator as
//! `event-source download --format yaml`. That makes export inherently lossy
//! in the same modeled-field sense as `convert`: each source can raise
//! `EC###` findings, surfaced to the caller via [`ExportOutcome::result`].
//!
//! Bulk export ([`export_all`]) is **continue-on-error** in both senses: a
//! per-source conversion finding rides along on the [`ExportOutcome`], and a
//! per-source transport failure (e.g. a source deleted between listing and
//! download) is captured as `Err` on its outcome rather than aborting the
//! whole batch. A single-source export ([`export_by_selector`]) treats a
//! transport failure as a hard error — the operator named exactly that source.
//!
//! This module returns structured outcomes and performs **no** stdout/stderr
//! I/O or process-exit — the CLI handler in [`crate::cmd::source`] calls
//! [`render_export`] (also here) which renders findings, writes files, and
//! computes the aggregate exit code. Keeping that out of the handler makes the
//! file-layout / exit-code / `---`-joining logic unit-testable without
//! spawning the binary.

use crate::api::{EventConfApi, SourceLookup};
use crate::convert::{ConversionResult, ConvertOpts, convert};
use onmsctl_core::{Error, Result};
use std::path::Path;

/// `# yaml-language-server` directive prepended to exported documents so
/// editors validate them against the committed schema (parity with
/// `requisition export`). Ends with a newline.
const SCHEMA_DIRECTIVE: &str = "# yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/event-source.schema.json\n";

/// Prepend the schema directive to a converted YAML document.
fn with_directive(yaml: &str) -> String {
    format!("{SCHEMA_DIRECTIVE}{yaml}")
}

/// One source's export result: the server-side source name plus either the
/// conversion result or a transport-failure reason.
pub struct ExportOutcome {
    /// Server-side source name; becomes the YAML `metadata.name` and, under
    /// `--out`, the `<name>.yaml` filename.
    pub name: String,
    /// `Ok` = the source was fetched and converted (the [`ConversionResult`]
    /// may still carry warnings, or be blocking with `yaml: None`).
    /// `Err` = the source could not be fetched/converted at the transport
    /// layer; the string is the human-readable reason.
    pub result: std::result::Result<ConversionResult, String>,
}

/// Resolve an event-source selector to a source id. A value that parses as an
/// integer is treated as an id; any other value is resolved to a source by
/// **exact** name. Returns the id plus the name when it was resolved by name
/// (free — avoids a redundant `get_source`); `None` for the numeric path.
///
/// Shared by `event-source download` and `event-source export` so the two
/// verbs interpret a selector identically.
pub async fn resolve_source_selector(
    api: &EventConfApi<'_>,
    selector: &str,
) -> Result<(i64, Option<String>)> {
    if let Ok(id) = selector.parse::<i64>() {
        crate::cmd::source::ensure_positive_id(id, "source id")?;
        return Ok((id, None));
    }
    match api.find_source_by_name(selector).await? {
        SourceLookup::Found(dto) => Ok((dto.id, Some(dto.name))),
        SourceLookup::Absent => Err(Error::Config(format!(
            "no event source named '{selector}' on the server"
        ))),
        SourceLookup::Ambiguous(ids) => Err(Error::Config(format!(
            "'{selector}' matches multiple event sources (ids: {ids:?}); \
             pass the numeric id to disambiguate"
        ))),
    }
}

/// Export one source by id + known name: download its XML and convert to YAML.
/// Infallible at the type level — a transport failure is captured as `Err` on
/// the returned [`ExportOutcome`] so bulk callers can continue past it.
pub async fn export_one(api: &EventConfApi<'_>, id: i64, name: String) -> ExportOutcome {
    let result = match api.download_source_xml(id).await {
        Ok(bytes) => {
            let opts = ConvertOpts {
                // metadata.name comes from authoritative server state.
                name_override: Some(name.clone()),
                max_findings: None,
            };
            Ok(convert(&bytes, Path::new("-"), &opts))
        }
        Err(e) => Err(format!("download failed: {e}")),
    };
    ExportOutcome { name, result }
}

/// Export the single source named by `selector` (id or exact name). A
/// transport failure is a hard error here — the operator named exactly this
/// source, so there is nothing to "continue past".
pub async fn export_by_selector(api: &EventConfApi<'_>, selector: &str) -> Result<ExportOutcome> {
    let (id, name_opt) = resolve_source_selector(api, selector).await?;
    let name = match name_opt {
        Some(n) => n,
        None => api.get_source(id).await?.name,
    };
    let outcome = export_one(api, id, name).await;
    if let Err(msg) = &outcome.result {
        return Err(Error::Config(format!(
            "failed to export event source '{}': {msg}",
            outcome.name
        )));
    }
    Ok(outcome)
}

/// Export every source the server lists. Skip-and-warn: a per-source transport
/// failure is captured on its [`ExportOutcome`] (rendered + counted by
/// [`render_export`]), not propagated, so the snapshot completes for the
/// sources that are reachable. Only the initial listing call can abort the
/// batch (there is no partial state to preserve at that point).
pub async fn export_all(api: &EventConfApi<'_>) -> Result<Vec<ExportOutcome>> {
    let pairs = api.list_source_names_and_ids().await?;
    let mut out = Vec::with_capacity(pairs.len());
    for p in pairs {
        out.push(export_one(api, p.id, p.name).await);
    }
    Ok(out)
}

/// Aggregate outcome of rendering an export batch.
#[derive(Debug)]
pub struct ExportSummary {
    /// Max severity across all sources used as the process exit code:
    /// 0 clean / 1 warnings or transport failures (partial) / 2 blocking.
    pub exit_code: i32,
    /// Sources that produced YAML and were written (clean + warned).
    pub written: usize,
    /// Of `written`, how many carried warning-level findings.
    pub warned: usize,
    /// Sources skipped because conversion was blocking (no YAML).
    pub skipped: usize,
    /// Sources that failed at the transport layer (download error).
    pub failed: usize,
}

/// Render an export batch: emit per-source findings to `diag` (prefixed by
/// source name), write each convertible source's YAML — one `<name>.yaml`
/// file under `out_dir`, or all `---`-joined to `out` when `out_dir` is
/// `None` — and return the aggregate [`ExportSummary`].
///
/// Continue-on-error: a source that is blocking (no YAML) is skipped, and a
/// source that failed at the transport layer (`Err` result) is counted as
/// `failed` (exit ≥ 1); neither is fatal. For `--out`, every target is
/// validated **up front** (safe filename, no intra-batch `<name>.yaml`
/// collision, `out_dir` is a directory, no clobber without `force`) before any
/// file is written — so `--out` is all-or-nothing rather than partial-write-
/// then-abort. The output directory is not created when there is nothing to
/// write. Pulled out of the CLI handler so this logic is unit-testable without
/// spawning the binary.
pub fn render_export(
    outcomes: &[ExportOutcome],
    out_dir: Option<&Path>,
    force: bool,
    out: &mut dyn std::io::Write,
    diag: &mut dyn std::io::Write,
) -> Result<ExportSummary> {
    let mut exit_code = 0i32;
    let mut warned = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    // (name, yaml) for sources that converted to a YAML document.
    let mut writable: Vec<(&str, &str)> = Vec::new();
    for o in outcomes {
        match &o.result {
            Err(msg) => {
                let _ = writeln!(diag, "# event-source '{}': export failed — {msg}", o.name);
                failed += 1;
                exit_code = exit_code.max(1); // transport/partial failure ⇒ exit 1
            }
            Ok(cr) => {
                if !cr.findings.is_empty() || cr.yaml.is_none() {
                    let _ = writeln!(diag, "# event-source '{}':", o.name);
                    let _ = write!(diag, "{}", crate::convert::render_report_text(cr));
                }
                exit_code = exit_code.max(cr.exit_code());
                match &cr.yaml {
                    None => skipped += 1,
                    Some(y) => {
                        if cr.exit_code() == 1 {
                            warned += 1;
                        }
                        writable.push((o.name.as_str(), y.as_str()));
                    }
                }
            }
        }
    }

    if let Some(dir) = out_dir {
        // Validate every target before writing anything (all-or-nothing).
        if dir.exists() && !dir.is_dir() {
            return Err(Error::Config(format!(
                "--out path {} exists and is not a directory",
                dir.display()
            )));
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (name, _) in &writable {
            if !is_safe_filename(name) {
                return Err(Error::Config(format!(
                    "refusing to write '{}/{}.yaml': source name contains path-unsafe \
                     characters (allowed: alphanumeric, '-', '_', '.')",
                    dir.display(),
                    name,
                )));
            }
            if !seen.insert(name) {
                return Err(Error::Config(format!(
                    "two event sources map to the same file '{name}.yaml'; \
                     refusing to clobber one with the other"
                )));
            }
            let path = dir.join(format!("{name}.yaml"));
            if path.exists() && !force {
                return Err(Error::Config(format!(
                    "refusing to overwrite existing file {}; pass --force to override",
                    path.display()
                )));
            }
        }
        // Don't create an empty directory for a no-op (nothing converted).
        if !writable.is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::Config(format!("creating {}: {e}", dir.display())))?;
            for (name, yaml) in &writable {
                let path = dir.join(format!("{name}.yaml"));
                std::fs::write(&path, with_directive(yaml))
                    .map_err(|e| Error::Config(format!("write {}: {e}", path.display())))?;
            }
        }
    } else {
        let mut first = true;
        for (_, yaml) in &writable {
            if !first && let Err(e) = out.write_all(b"---\n") {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    break;
                }
                return Err(Error::Io(e));
            }
            first = false;
            match out.write_all(with_directive(yaml).as_bytes()) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }

    Ok(ExportSummary {
        exit_code,
        written: writable.len(),
        warned,
        skipped,
        failed,
    })
}

/// Whitelist for `--out` filenames. A source name flows from the server and
/// could carry path-unsafe characters; only allow alphanumerics plus
/// `-` `_` `.`, and reject the directory-traversal names.
pub fn is_safe_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::{AuthCreds, OnmsClient, Url};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const MINIMAL_XML: &[u8] = br#"<events>
  <event>
    <uei>uei.test/foo</uei>
    <event-label>Test Foo</event-label>
    <severity>Warning</severity>
  </event>
</events>"#;

    // An unmodeled child element under <event> trips EC001 (a warning):
    // YAML is still emitted, but exit_code becomes 1.
    const WARN_XML: &[u8] = br#"<events>
  <event>
    <uei>uei.test/warn</uei>
    <event-label>Warn</event-label>
    <severity>Warning</severity>
    <made-up-element>x</made-up-element>
  </event>
</events>"#;

    /// Build an `Ok` outcome by converting `xml` under `name`.
    fn outcome(name: &str, xml: &[u8]) -> ExportOutcome {
        let opts = ConvertOpts {
            name_override: Some(name.to_string()),
            max_findings: None,
        };
        ExportOutcome {
            name: name.to_string(),
            result: Ok(convert(xml, Path::new("-"), &opts)),
        }
    }

    async fn mock_with_client() -> (MockServer, OnmsClient) {
        let mock = MockServer::start().await;
        let url = format!("{}/", mock.uri());
        let client =
            OnmsClient::from_parts(Url::parse(&url).unwrap(), AuthCreds::bearer("t")).unwrap();
        (mock, client)
    }

    #[test]
    fn safe_filename_whitelist() {
        assert!(is_safe_filename("vendor.example.cold-start"));
        assert!(is_safe_filename("cisco_foo-1"));
        assert!(!is_safe_filename("../etc/passwd"));
        assert!(!is_safe_filename("a/b"));
        assert!(!is_safe_filename(""));
        assert!(!is_safe_filename("."));
        assert!(!is_safe_filename(".."));
    }

    #[tokio::test]
    async fn numeric_selector_resolves_without_lookup() {
        // A numeric selector returns the id with no name and issues no HTTP
        // (the mock has no routes mounted, so a stray request would 404).
        let (_mock, client) = mock_with_client().await;
        let api = EventConfApi::new(&client);
        let (id, name) = resolve_source_selector(&api, "42").await.unwrap();
        assert_eq!(id, 42);
        assert!(name.is_none());
    }

    #[tokio::test]
    async fn non_positive_numeric_selector_is_rejected() {
        let (_mock, client) = mock_with_client().await;
        let api = EventConfApi::new(&client);
        assert!(resolve_source_selector(&api, "0").await.is_err());
    }

    #[tokio::test]
    async fn unknown_name_selector_errors() {
        // 204 No Content from the filter endpoint → empty page → Absent.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let err = resolve_source_selector(&api, "ghost").await.unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[tokio::test]
    async fn export_one_converts_downloaded_xml() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/sources/7/events/download"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(MINIMAL_XML))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let outcome = export_one(&api, 7, "vendor.foo".into()).await;
        assert_eq!(outcome.name, "vendor.foo");
        let cr = outcome.result.expect("download ok");
        assert_eq!(cr.exit_code(), 0);
        let yaml = cr.yaml.as_ref().expect("clean XML yields YAML");
        assert!(yaml.contains("kind: EventSource"));
        // metadata.name comes from the server-side name, not a filename.
        assert!(yaml.contains("vendor.foo"));
    }

    #[tokio::test]
    async fn export_one_captures_transport_failure() {
        // A download 404 is captured as Err on the outcome, not propagated.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/sources/9/events/download"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let outcome = export_one(&api, 9, "gone".into()).await;
        assert!(outcome.result.is_err());
    }

    #[tokio::test]
    async fn export_all_iterates_every_source() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/sources/names-and-ids"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": 11, "name": "alpha"},
                {"id": 12, "name": "beta"},
            ])))
            .mount(&mock)
            .await;
        for id in [11, 12] {
            Mock::given(method("GET"))
                .and(path(format!(
                    "/api/v2/eventconf/sources/{id}/events/download"
                )))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(MINIMAL_XML))
                .mount(&mock)
                .await;
        }
        let api = EventConfApi::new(&client);
        let outcomes = export_all(&api).await.unwrap();
        assert_eq!(outcomes.len(), 2);
        let names: Vec<&str> = outcomes.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        // Continue-on-error data path: every source converted.
        assert!(outcomes.iter().all(|o| o.result.is_ok()));
    }

    #[test]
    fn warn_xml_is_a_nonblocking_warning() {
        // Sanity-check the fixture: an unmodeled child yields YAML + exit 1.
        let o = outcome("beta", WARN_XML);
        let cr = o.result.unwrap();
        assert!(cr.yaml.is_some(), "warning must not block YAML");
        assert_eq!(cr.exit_code(), 1);
    }

    #[test]
    fn render_export_stdout_joins_aggregates_and_carries_directive() {
        let outcomes = vec![outcome("alpha", MINIMAL_XML), outcome("beta", WARN_XML)];
        let mut out = Vec::<u8>::new();
        let mut diag = Vec::<u8>::new();
        let summary = render_export(&outcomes, None, false, &mut out, &mut diag).unwrap();

        let stdout = String::from_utf8(out).unwrap();
        let stderr = String::from_utf8(diag).unwrap();
        // Both documents present, joined by a `---` separator, each carrying
        // the schema directive.
        assert_eq!(stdout.matches("kind: EventSource").count(), 2);
        assert_eq!(stdout.matches("# yaml-language-server").count(), 2);
        assert!(stdout.contains("---\n"));
        // Continue-on-error: max severity = warning; both written; one warned.
        assert_eq!(summary.exit_code, 1);
        assert_eq!(summary.written, 2);
        assert_eq!(summary.warned, 1);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.failed, 0);
        // The warning source's findings reached the diagnostics sink, named.
        assert!(stderr.contains("# event-source 'beta':"));
    }

    #[test]
    fn render_export_skips_and_counts_transport_failure() {
        // A mix of one converted source and one transport-failed source:
        // the good one is written, the failure is counted, exit code 1.
        let failed = ExportOutcome {
            name: "gone".to_string(),
            result: Err("download failed: 404".to_string()),
        };
        let outcomes = vec![outcome("alpha", MINIMAL_XML), failed];
        let mut out = Vec::<u8>::new();
        let mut diag = Vec::<u8>::new();
        let summary = render_export(&outcomes, None, false, &mut out, &mut diag).unwrap();
        assert_eq!(summary.written, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.exit_code, 1);
        let stderr = String::from_utf8(diag).unwrap();
        assert!(stderr.contains("# event-source 'gone': export failed"));
    }

    #[test]
    fn render_export_writes_one_file_per_source() {
        let dir = std::env::temp_dir().join(format!("onmsctl-export-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let outcomes = vec![outcome("alpha", MINIMAL_XML), outcome("beta", MINIMAL_XML)];
        let mut out = Vec::<u8>::new();
        let mut diag = Vec::<u8>::new();
        let summary = render_export(&outcomes, Some(&dir), false, &mut out, &mut diag).unwrap();
        assert_eq!(summary.written, 2);
        assert!(dir.join("alpha.yaml").is_file());
        assert!(dir.join("beta.yaml").is_file());
        // Each file carries the schema directive.
        let alpha = std::fs::read_to_string(dir.join("alpha.yaml")).unwrap();
        assert!(alpha.starts_with("# yaml-language-server"));
        // stdout stays empty in --out mode.
        assert!(out.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn render_export_rejects_intra_batch_filename_collision() {
        // Two distinct sources with the same name would clobber one file.
        let dir =
            std::env::temp_dir().join(format!("onmsctl-export-collide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let outcomes = vec![outcome("dup", MINIMAL_XML), outcome("dup", MINIMAL_XML)];
        let mut out = Vec::<u8>::new();
        let mut diag = Vec::<u8>::new();
        let err = render_export(&outcomes, Some(&dir), false, &mut out, &mut diag).unwrap_err();
        assert!(err.to_string().contains("same file"));
        // All-or-nothing: nothing was written despite the first being valid.
        assert!(!dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_export_does_not_create_dir_when_nothing_writable() {
        // All sources blocking (no YAML) → no directory created.
        let dir = std::env::temp_dir().join(format!("onmsctl-export-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let blocking = outcome("a/b", MINIMAL_XML); // invalid name → convert blocks → no yaml
        let mut out = Vec::<u8>::new();
        let mut diag = Vec::<u8>::new();
        let summary = render_export(&[blocking], Some(&dir), false, &mut out, &mut diag).unwrap();
        assert_eq!(summary.written, 0);
        assert_eq!(summary.skipped, 1);
        assert!(!dir.exists(), "no empty --out directory should be created");
    }

    #[test]
    fn render_export_rejects_path_unsafe_name_for_out() {
        let dir =
            std::env::temp_dir().join(format!("onmsctl-export-unsafe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Hand-build a YAML-present outcome with a filesystem-unsafe name to
        // exercise the `is_safe_filename` guard directly. (In practice convert
        // rejects such a `metadata.name` first — see the blocking case above —
        // so this guard is defense-in-depth.)
        let unsafe_outcome = ExportOutcome {
            name: "a/b".to_string(),
            result: Ok(ConversionResult {
                input: None,
                yaml: Some("kind: EventSource\n".to_string()),
                findings: vec![],
                metrics: crate::convert::ConversionMetrics::default(),
            }),
        };
        let mut out = Vec::<u8>::new();
        let mut diag = Vec::<u8>::new();
        let err =
            render_export(&[unsafe_outcome], Some(&dir), false, &mut out, &mut diag).unwrap_err();
        assert!(err.to_string().contains("path-unsafe"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
