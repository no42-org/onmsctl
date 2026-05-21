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
}

impl Classify for RequisitionCmd {
    fn kind(&self) -> CmdKind {
        match self {
            // Apply is classified Write even with --dry-run: read-only
            // contexts should refuse it consistently so a user who
            // accidentally drops --dry-run on a real-mutation flow
            // can't bypass the safety net.
            RequisitionCmd::Apply { .. } => CmdKind::Write,
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
/// partial state is possible and point at the introspection verb that
/// will help them check (once `requisition status` lands in Group 6).
///
/// Emitted as a separate stderr line BEFORE the error propagates so
/// the underlying error's exit-code class (HTTP / auth / dns / tls)
/// reaches the process exit untouched.
fn eprint_recovery_hint(local: &RequisitionLocal) {
    let name = &local.metadata.name;
    eprintln!(
        "note: partial writes are possible for Requisition/{name} — the foreign-source \
         POST, requisition POST, and import trigger run as separate calls. Re-fetch with \
         `onmsctl requisition status {name}` (when available) or `curl /rest/requisitions/{name}` \
         to verify server state before retrying."
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
