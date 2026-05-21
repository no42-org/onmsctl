/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! CLI subcommand surface for the Provisioning capability.
//!
//! Exposed to the binary crate as [`RequisitionCmd`]; the binary
//! composes it into the top-level command tree at `onmsctl requisition`
//! (with `req` as the visible alias).

use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};

use crate::api::ProvisioningApi;
use crate::apply::{ApplyOptions, RescanChoice, apply_requisition};
use crate::model::RequisitionLocal;
use crate::render::render_apply_diff;

/// Hard cap on the size of a single `-f <file>` input. Matches the
/// `onmsctl source apply` convention (16 MiB) — requisition documents
/// are normally orders of magnitude smaller than this, the cap exists
/// to prevent a malformed path from streaming GB of data into memory.
const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;

/// `onmsctl requisition ...` subcommands.
///
/// Three grouped families (per design.md §D8) will eventually live here:
///
/// - **GitOps**: `apply`, `convert`, `export`
/// - **Lifecycle**: `list`, `get`, `delete`, `import`, `status`
/// - **Sub-resources**: `node`, `interface`, `service`, `category`, `asset`
///
/// Today only `apply` is implemented; the others land in subsequent
/// tasks of the `add-provisioning-capability` change.
#[derive(Subcommand, Debug, Clone)]
pub enum RequisitionCmd {
    /// Apply a `kind: Requisition` YAML document declaratively.
    ///
    /// Reads the file, validates it locally, fetches the server's
    /// current state, computes the L1/L2/L3 diff, and either creates
    /// or updates the requisition + (optional) custom foreign-source.
    /// With `--dry-run` no mutations are issued. With `--diff` the
    /// structured diff prints to stdout.
    ///
    /// Output format follows the global `-o` flag:
    ///   - `table` (default): one-line summary of the outcome
    ///   - `json` / `yaml`: full structured [`ApplyOutcome`] including
    ///     the L2/L3 delta tree
    ///
    /// `rescanExisting` is auto-decided from the diff's scan-relevance
    /// (per design D3); override with `--rescan-existing true|false`.
    Apply {
        /// Path to the requisition YAML document.
        #[arg(short = 'f', long)]
        file: PathBuf,
        /// Compute the diff + decisions but issue no mutating HTTP.
        #[arg(long)]
        dry_run: bool,
        /// Print the structured diff (text form) to stderr in addition
        /// to the outcome summary. Stderr (not stdout) so the diff text
        /// doesn't corrupt `-o json` / `-o yaml` output downstream of a
        /// pipe (matching the eventconf precedent in `source apply`).
        #[arg(long)]
        diff: bool,
        /// Force the `rescanExisting` query parameter on import.
        /// Default: auto-decided from the diff's scan-relevance.
        #[arg(long)]
        rescan_existing: Option<bool>,
    },
    /// Trigger an import for an already-deployed requisition without
    /// re-POSTing its content.
    ///
    /// Equivalent to `PUT /rest/requisitions/{fs}/import?rescanExisting=...`
    /// on Horizon. Useful when the server's requisition is up to date
    /// (e.g. just edited via the UI) but a fresh scan is required —
    /// `apply` would re-POST the same content and only then trigger
    /// import; `import` skips the POST.
    ///
    /// `--wait` and scan-report identifier surface land with task 6.3.
    Import {
        /// Foreign-source name to import.
        #[arg(value_parser = nonempty_fs)]
        fs: String,
        /// Pass `rescanExisting=true` so the import re-evaluates
        /// already-imported nodes (services, asset fields). Absence
        /// matches Horizon's `provision.pl` convention (no rescan).
        #[arg(long, action = clap::ArgAction::SetTrue)]
        rescan_existing: bool,
    },
    /// Report the deployed state of a requisition.
    ///
    /// Issues `GET /rest/requisitions/{fs}` and surfaces the
    /// server-managed fields: requisition `date-stamp` (last edit),
    /// `last-import` timestamp, and node count.
    ///
    /// Per-import outcome (success/failure) lives in scan-reports and
    /// is wired by task 6.3. Today this verb tells the operator "what
    /// state does the server believe the requisition is in?" without
    /// editorializing on the most recent import's result.
    Status {
        /// Foreign-source name to inspect.
        #[arg(value_parser = nonempty_fs)]
        fs: String,
    },
}

impl Classify for RequisitionCmd {
    fn kind(&self) -> CmdKind {
        match self {
            // Apply is classified Write even with --dry-run: read-only
            // contexts should refuse it consistently so a user who
            // accidentally drops --dry-run on a real-mutation flow
            // can't bypass the safety net.
            RequisitionCmd::Apply { .. } => CmdKind::Write,
            // Import issues PUT /import — Write.
            RequisitionCmd::Import { .. } => CmdKind::Write,
            // Status is read-only.
            RequisitionCmd::Status { .. } => CmdKind::Read,
        }
    }
}

impl RequisitionCmd {
    /// Dispatch the parsed verb against a resolved [`Context`].
    pub async fn run(self, ctx: &Context) -> Result<()> {
        match self {
            RequisitionCmd::Apply {
                file,
                dry_run,
                diff,
                rescan_existing,
            } => run_apply(file, dry_run, diff, rescan_existing, ctx).await,
            RequisitionCmd::Import {
                fs,
                rescan_existing,
            } => run_import(fs, rescan_existing, ctx).await,
            RequisitionCmd::Status { fs } => run_status(fs, ctx).await,
        }
    }
}

async fn run_apply(
    file: PathBuf,
    dry_run: bool,
    diff: bool,
    rescan_existing: Option<bool>,
    ctx: &Context,
) -> Result<()> {
    // ---- 1. Validate + read input file ----
    let meta = std::fs::metadata(&file)
        .map_err(|e| Error::Config(format!("failed to stat {}: {e}", file.display())))?;
    if !meta.is_file() {
        return Err(Error::Config(format!(
            "{} is not a regular file (got {:?})",
            file.display(),
            meta.file_type()
        )));
    }
    if meta.len() > MAX_INPUT_BYTES {
        return Err(Error::Config(format!(
            "{} is {} bytes, exceeds apply input cap of {} bytes",
            file.display(),
            meta.len(),
            MAX_INPUT_BYTES
        )));
    }
    let bytes = std::fs::read(&file)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", file.display())))?;
    let local: RequisitionLocal = serde_norway::from_slice(&bytes)
        .map_err(|e| Error::Config(format!("{}: {e}", file.display())))?;

    // ---- 2. Build client + API ----
    let client = OnmsClient::from_context(ctx)?;
    let api = ProvisioningApi::new(&client);

    // ---- 3. Run the apply orchestrator ----
    let opts = ApplyOptions {
        dry_run,
        rescan_existing: match rescan_existing {
            Some(b) => RescanChoice::Force(b),
            None => RescanChoice::Auto,
        },
    };
    let outcome = match apply_requisition(&local, &api, &opts).await {
        Ok(o) => o,
        Err(e) => {
            // Surface the recovery hint to stderr BEFORE propagating
            // the error so the operator sees both the failure cause
            // (with its proper exit-code class — HTTP / auth / dns /
            // tls — preserved by returning `e` unchanged) and the
            // recovery guidance. Wrapping into `Error::Config` would
            // collapse every failure to exit code 2 and double-prefix
            // the message as "config error: apply failed for...".
            eprint_recovery_hint(&local);
            return Err(e);
        }
    };

    // ---- 4. Format + print ----
    if diff {
        // Stderr so the text diff doesn't pollute stdout when the
        // caller also requested `-o json` / `-o yaml`. Matches the
        // eventconf `source apply --diff` precedent.
        eprint!("{}", render_apply_diff(&local, &outcome));
    }

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&outcome)
                .map_err(|e| Error::Config(format!("serializing outcome to JSON: {e}")))?;
            write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&outcome)
                .map_err(|e| Error::Config(format!("serializing outcome to YAML: {e}")))?;
            write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            // One-line summary when --diff wasn't requested (the diff
            // body itself already names the requisition + state).
            if !diff {
                let line = format!(
                    "Requisition/{}: {} (rescanExisting={}, foreignSource={})\n",
                    local.metadata.name,
                    state_word(outcome.state),
                    outcome.rescan_existing,
                    fs_word(outcome.foreign_source_action),
                );
                write_stdout(line.as_bytes())?;
            }
        }
    }

    Ok(())
}

async fn run_import(fs: String, rescan_existing: bool, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = ProvisioningApi::new(&client);
    api.trigger_import(&fs, rescan_existing).await?;

    // Single-line confirmation. Scan-report id surfacing is task 6.3
    // / 5.7 territory — for now the trigger is fire-and-forget at the
    // HTTP layer too. Build the structured payload once; the JSON
    // and YAML arms serialize it via different encoders so the wire
    // shape stays in lockstep.
    let payload = serde_json::json!({
        "foreign_source": fs,
        "rescan_existing": rescan_existing,
        "triggered": true,
    });
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&payload)
                .map_err(|e| Error::Config(format!("serializing import outcome to JSON: {e}")))?;
            write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&payload)
                .map_err(|e| Error::Config(format!("serializing import outcome to YAML: {e}")))?;
            write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let line = format!(
                "Requisition/{fs}: import triggered (rescanExisting={rescan_existing})\n"
            );
            write_stdout(line.as_bytes())?;
        }
    }

    Ok(())
}

/// `requisition status` outcome. Timestamps are surfaced as raw epoch
/// milliseconds (matching the wire shape) plus the node count.
/// Human-friendly formatting (`-d @<epoch>` / `jq | fromdateiso8601`)
/// is left to the caller — adding a calendar dep for one verb's table
/// view doesn't earn its weight.
#[derive(Debug, Clone, serde::Serialize)]
struct StatusOutcome {
    foreign_source: String,
    /// Server-side last-modified epoch ms. `None` when Horizon has the
    /// requisition cached but hasn't stamped it (rare; preserves the
    /// wire field's optionality).
    date_stamp_ms: Option<i64>,
    /// Last successful import epoch ms. `None` if the requisition has
    /// never been imported.
    last_import_ms: Option<i64>,
    /// Number of nodes in the deployed requisition.
    node_count: usize,
    /// Per-import outcome (success / failure / partial-success). Today
    /// always `None` — surfaces with task 6.3 once the scan-reports
    /// endpoint is wired. Reserved as a struct field now so the JSON
    /// / YAML wire shape stays stable across the 6.2 → 6.3 transition
    /// (no consumer breakage when the field starts populating).
    #[serde(skip_serializing_if = "Option::is_none")]
    last_import_outcome: Option<ImportOutcome>,
}

/// Result of the most recent import as reported by Horizon's scan-
/// reports endpoint. Wired by task 6.3; today always synthesized as
/// `None` on `StatusOutcome`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)] // Variants are reserved for task 6.3; the type is in
                   // the public-ish API surface today so consumers can
                   // discover the enum shape before 6.3 populates it.
enum ImportOutcome {
    Success,
    Failure,
    PartialSuccess,
}

async fn run_status(fs: String, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = ProvisioningApi::new(&client);

    let req = api.get_requisition(&fs).await?.ok_or_else(|| {
        Error::Config(format!(
            "no requisition '{fs}' on the server (GET /rest/requisitions/{fs} returned 404)"
        ))
    })?;

    let outcome = StatusOutcome {
        foreign_source: req.foreign_source.clone(),
        date_stamp_ms: req.date_stamp,
        last_import_ms: req.last_import,
        node_count: req.node.len(),
        last_import_outcome: None, // Populated by task 6.3
    };

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&outcome)
                .map_err(|e| Error::Config(format!("serializing status to JSON: {e}")))?;
            write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&outcome)
                .map_err(|e| Error::Config(format!("serializing status to YAML: {e}")))?;
            write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let line = format!(
                "Requisition/{}: nodes={}, date-stamp-ms={}, last-import-ms={}\n",
                outcome.foreign_source,
                outcome.node_count,
                opt_ms(outcome.date_stamp_ms),
                opt_ms(outcome.last_import_ms),
            );
            write_stdout(line.as_bytes())?;
        }
    }

    Ok(())
}

fn opt_ms(ms: Option<i64>) -> String {
    match ms {
        Some(v) => v.to_string(),
        None => "<absent>".into(),
    }
}

/// clap value parser for foreign-source CLI arguments. Rejects empty
/// and whitespace-only inputs at parse time so the user sees a clean
/// usage error instead of a confusing 404 against a URL like
/// `/rest/requisitions//import`.
fn nonempty_fs(s: &str) -> std::result::Result<String, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("foreign-source name must not be empty or whitespace-only".into());
    }
    // Preserve the original (un-trimmed) form — operators who deliberately
    // pass leading/trailing spaces deserve the round-trip — but we've at
    // least caught the all-whitespace case.
    Ok(s.to_string())
}

/// Write `bytes` to stdout, treating `BrokenPipe` as a clean exit
/// (e.g. when the user pipes our output into `head -c N`). Other I/O
/// errors propagate as `Error::Io` so the exit-code mapping picks them
/// up. Mirrors the binary's `write_stdout` helper.
fn write_stdout(bytes: &[u8]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(bytes) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Convenience wrapper that appends a trailing newline. JSON output
/// uses this (the serializer doesn't add one); YAML / table paths
/// already include the newline in their byte slices.
fn write_stdout_line(bytes: &[u8]) -> Result<()> {
    write_stdout(bytes)?;
    write_stdout(b"\n")
}

/// Emit the partial-write recovery hint to stderr. The library returns
/// a single error without telling us how far the write sequence got
/// (FS write → requisition POST → import). We can't tell which step
/// failed from the error alone, but we can warn the operator that
/// partial state is possible and point at the introspection verb to
/// check.
///
/// Emitted as a separate stderr line BEFORE the error propagates so
/// the underlying error's exit-code class (HTTP / auth / dns / tls)
/// reaches the process exit untouched.
fn eprint_recovery_hint(local: &RequisitionLocal) {
    let name = &local.metadata.name;
    eprintln!(
        "note: partial writes are possible for Requisition/{name} — the foreign-source \
         POST, requisition POST, and import trigger run as separate calls. Run \
         `onmsctl requisition status {name}` to verify server state before retrying."
    );
}

fn state_word(s: crate::apply::ApplyState) -> &'static str {
    use crate::apply::ApplyState::*;
    match s {
        Unchanged => "unchanged",
        DryRun => "dry-run",
        Created => "created",
        Updated => "updated",
    }
}

fn fs_word(a: crate::apply::ForeignSourceAction) -> &'static str {
    use crate::apply::ForeignSourceAction::*;
    match a {
        NoChange => "no-change",
        Created => "created",
        Updated => "updated",
        Deleted => "deleted",
    }
}
