/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! CLI subcommand surface for the Provisioning capability.
//!
//! Exposed to the binary crate as [`RequisitionCmd`]; the binary
//! composes it into the top-level command tree at `onmsctl requisition`
//! (with `req` as the visible alias).

pub mod asset;
pub mod category;
pub mod interface;
pub mod node;
pub mod service;

use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use clap::Subcommand;
use onmsctl_core::apply_input::ApplyDispatch;
use onmsctl_core::{
    AsyncFlags, Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result,
};

use crate::api::ProvisioningApi;
use crate::apply::multi::{CollisionCode, MultiApplyPlan, execute_multi, plan_directory};
use crate::apply::{
    ApplyOptions, MultiApplyOptions, MultiApplyOutcome, MultiApplyState, PlanState, RescanChoice,
    apply_requisition,
};
use crate::convert::{ConversionResult, FindingCode, Severity, convert_directory, explain};
use crate::export::{export_all_requisitions, export_requisition};
use crate::model::RequisitionLocal;
use crate::render::render_apply_diff;
use crate::wait::wait_for_import_completion;

/// Hard cap on the size of a single `-f <file>` input. Matches the
/// `onmsctl source apply` convention (16 MiB) — requisition documents
/// are normally orders of magnitude smaller than this, the cap exists
/// to prevent a malformed path from streaming GB of data into memory.
const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;

/// `onmsctl requisition ...` subcommands.
///
/// Variants are ordered to reflect three grouped families (per
/// design.md §D8). clap 4 doesn't surface `help_heading` on Subcommand
/// enum variants, so the grouping appears in declaration order rather
/// than as named sections in `--help`:
///
/// - **GitOps**: `apply`, `export` — declarative reconcile loop
/// - **Lifecycle**: `import`, `status` — async operations + introspection
/// - **Migration**: `convert` — one-shot XML→YAML migrator
/// - **Sub-resources**: `node` (and future `interface`, `service`,
///   `category`, `asset`) — imperative escape-hatch verbs
#[derive(Subcommand, Debug, Clone)]
pub enum RequisitionCmd {
    // ---- GitOps verbs (declarative workflow) ----
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
        /// Path to a requisition YAML document, a directory of
        /// documents, or a glob pattern. With a directory, every
        /// `*.yaml` / `*.yml` file is applied in alphabetical order
        /// (non-recursive). With a glob pattern (contains `*`, `?`,
        /// or `[`), the matching files are applied in alphabetical
        /// order — quote the pattern (`-f 'requisitions/*.yaml'`) so
        /// the shell doesn't expand it first. `**` enables recursion;
        /// a bare `*` matches a single path segment.
        #[arg(short = 'f', long)]
        file: PathBuf,
        /// Compute the diff + decisions but issue no mutating HTTP.
        #[arg(long)]
        dry_run: bool,
        /// Print the structured diff (text form) to stderr in addition
        /// to the outcome summary. Stderr (not stdout) so the diff text
        /// doesn't corrupt `-o json` / `-o yaml` output downstream of a
        /// pipe (matching the eventconf precedent in `source apply`).
        /// Single-file only — multi-file mode (directory or glob with
        /// 2+ matches) emits per-file summaries without the full diff
        /// body (use `--dry-run` per-file for review). A glob that
        /// matches exactly one file collapses to single-file mode so
        /// `--diff` still works.
        #[arg(long)]
        diff: bool,
        /// Override the `rescanExisting` query parameter on import.
        /// Accepts `true`, `false`, or `auto`. `auto` (the default)
        /// runs the diff's scan-relevance classification per design
        /// §D3.
        #[arg(long, value_parser = rescan_flag, default_value = "auto")]
        rescan_existing: RescanFlag,
        /// Print to stderr the leaf path(s) that drove the
        /// `rescanExisting=true` auto-decision per design §D3. Useful
        /// for understanding why a small change triggered (or didn't
        /// trigger) a full rescan. Combine with `--dry-run` to see the
        /// classification without mutating server state. Single-file
        /// only — multi-file mode (directory or glob with 2+ matches)
        /// emits no per-file rationale (use it per-file with
        /// `--dry-run`). A glob that matches exactly one file
        /// collapses to single-file mode so `--explain-rescan` still
        /// works.
        #[arg(long)]
        explain_rescan: bool,
        /// Directory mode: halt phase 2 after the first per-file
        /// error instead of continuing. kubectl-style fail-fast.
        /// Has no effect in single-file mode.
        #[arg(long)]
        stop_on_error: bool,
        /// Block until the triggered import completes (--wait), with
        /// timeout (--timeout) and polling cadence (--poll-interval).
        /// Without --wait, the verb returns as soon as the server
        /// accepts the trigger. Exit code 10 on timeout, 11 on
        /// observed async failure (mid-poll requisition deletion).
        /// In directory mode, `--wait` polls AFTER each per-file
        /// apply (not after the whole batch).
        #[command(flatten)]
        wait_flags: AsyncFlags,
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
        /// Block until the triggered import completes (--wait), with
        /// timeout (--timeout) and polling cadence (--poll-interval).
        /// Exit code 10 on timeout, 11 on observed async failure
        /// (mid-poll requisition deletion).
        #[command(flatten)]
        wait_flags: AsyncFlags,
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
    /// List every requisition name deployed on the server.
    ///
    /// Wraps `GET /rest/requisitionNames`. Output respects `-o`:
    /// table prints one foreign-source name per line; json / yaml
    /// emit the array. Classified `Read`.
    List,
    /// Fully purge a requisition from the server.
    ///
    /// Horizon stores pending and deployed requisition snapshots
    /// separately. This verb issues both `DELETE
    /// /rest/requisitions/{fs}` (pending) AND `DELETE
    /// /rest/requisitions/deployed/{fs}` (deployed) so the requisition
    /// is fully removed in one call. The local YAML is NOT touched.
    /// Classified `Write`.
    ///
    /// **Idempotent on both snapshots:** a 404 from either DELETE is
    /// treated as success (the snapshot was already absent). If both
    /// 404, the requisition didn't exist on the server and a stderr
    /// note records that fact. If the pending DELETE succeeds but
    /// the deployed DELETE fails with a non-404 error, the operator
    /// is warned about the orphaned deployed snapshot before the
    /// error propagates.
    ///
    /// **Confirmation guard (BREAKING since v0.1.1):** because the
    /// verb purges both pending and deployed snapshots in one call,
    /// it refuses to run without explicit operator acknowledgement.
    /// With `--yes` / `-y`, the verb proceeds without prompting.
    /// Without `--yes`: TTY → interactive prompt showing the
    /// requisition name + node count; non-TTY (CI) → refuse with a
    /// clear error pointing at `--yes`. Pre-delete 404 (requisition
    /// already absent) skips confirmation entirely.
    Delete {
        /// Foreign-source name to purge.
        #[arg(value_parser = nonempty_fs)]
        fs: String,
        /// Skip the interactive confirmation prompt and proceed with
        /// the dual DELETE. In non-TTY (CI / scripting) contexts
        /// this flag is REQUIRED — the verb refuses without it.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Export server requisitions to declarative `kind: Requisition`
    /// YAML — the reverse of `apply`.
    ///
    /// With a `<foreign-source>` argument, exports that single
    /// requisition. With no argument, exports every requisition
    /// listed by `GET /rest/requisitionNames` (sorted alphabetically).
    ///
    /// Default output:
    ///   - YAML stream to stdout (one doc per requisition, separated
    ///     by `---`).
    ///   - With `--out <dir>`, per-requisition `<fs>.yaml` files
    ///     into the directory.
    ///
    /// `--include-defaults` opts in to inlining Horizon's default
    /// foreign-source when the requisition has no custom FS. Without
    /// the flag, the YAML stays in portable style (omits
    /// `spec.foreignSource`); with the flag, the YAML inlines the
    /// default-FS with a snapshot-timestamp comment so the operator
    /// sees what the requisition would inherit at apply time.
    ///
    /// Classified `Read` — only `GET` endpoints are issued.
    Export {
        /// Foreign-source name to export. Omit to export every
        /// requisition on the server.
        #[arg(value_parser = nonempty_fs)]
        fs: Option<String>,
        /// Write per-requisition YAML files to this directory
        /// instead of stdout. Filenames are `<fs>.yaml` (rejected if
        /// the foreign-source name contains path-unsafe characters).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Inline Horizon's default foreign-source into the exported
        /// YAML when the requisition has no custom FS. Adds a
        /// snapshot-timestamp comment naming the inlining.
        #[arg(long, action = clap::ArgAction::SetTrue)]
        include_defaults: bool,
    },
    /// Convert provision.pl-shape XML to declarative `kind: Requisition`
    /// YAML for git-managed apply workflows.
    ///
    /// Reads every `*.xml` under `--from <xml-dir>` as a requisition.
    /// Each is paired with a matching foreign-source XML
    /// (`<basename>.xml` under `--foreign-sources-dir`, when supplied)
    /// to produce the composite YAML form. Findings are reported to
    /// stderr with stable `PR###` codes — `--explain <code>` prints
    /// the rationale for any single code.
    ///
    /// Pure local transform: no HTTP, no context required. Classified
    /// `Read` so even a `--read-only` context permits the verb.
    Convert {
        /// Directory of requisition `*.xml` files (the
        /// `provision.pl`-shape inputs).
        #[arg(long)]
        from: Option<PathBuf>,
        /// Optional directory of foreign-source `*.xml` files. Matched
        /// to requisitions by basename (`acme-prod.xml` pairs with
        /// `acme-prod.xml`). Orphans (no matching requisition) raise
        /// PR002 findings.
        #[arg(long)]
        foreign_sources_dir: Option<PathBuf>,
        /// Write per-requisition YAML files to this directory instead
        /// of stdout. Output filenames mirror the input basenames with
        /// `.yaml` extension.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Print the rationale for the given finding code (e.g.
        /// `--explain PR001`) and exit without converting. `--from`
        /// may be omitted when this flag is used.
        #[arg(long)]
        explain: Option<String>,
    },
    /// Imperative sub-resource verbs for the requisition's nodes.
    ///
    /// Use these for ad-hoc operator work (quick add / remove / set
    /// of a single node) instead of editing YAML and re-applying.
    /// Each sub-verb's help text cross-references the declarative
    /// alternative. The GitOps path (`apply -f`) remains the
    /// recommended workflow — these are escape-hatches.
    #[command(subcommand)]
    Node(node::NodeCmd),
    /// Imperative sub-resource verbs for a node's interfaces.
    ///
    /// Interfaces are scoped within a node (`<fs> <foreign-id> <ip>`).
    /// `set` here can mutate three wire-only fields (`--snmp-primary`,
    /// `--status`, `--descr`) — the latter two are NOT modeled in the
    /// local YAML, so this is the one place imperative verbs offer
    /// functionality the declarative path doesn't.
    #[command(subcommand)]
    Interface(interface::InterfaceCmd),
    /// Imperative sub-resource verbs for an interface's monitored
    /// services.
    ///
    /// Services are scoped within an interface
    /// (`<fs> <foreign-id> <ip> <service>`). Coverage is `list / add /
    /// remove` only — services on the wire carry just a name and
    /// category / meta-data arrays; `set` adds no value beyond
    /// delete-and-re-add, and `get` adds no information `list`
    /// doesn't.
    #[command(subcommand)]
    Service(service::ServiceCmd),
    /// Imperative sub-resource verbs for a node's categories.
    ///
    /// Categories are scoped within a node
    /// (`<fs> <foreign-id> <category>`). Coverage is `list / add /
    /// remove` only — categories are tag-like (just a name), so
    /// `set` and `get` add no value.
    #[command(subcommand)]
    Category(category::CategoryCmd),
    /// Imperative sub-resource verbs for a post-import node's asset
    /// record. The misfit of the sub-resource family.
    ///
    /// Asset records live under `/rest/nodes/{db-id}/assetRecord`
    /// (NOT `/rest/requisitions/...`) and operate on IMPORTED nodes
    /// keyed by database node ID, not requisition entries keyed by
    /// foreign-id. Mutations take effect immediately; there is no
    /// `requisition import` follow-up needed. Coverage is `list /
    /// get / set` only — the asset record has a fixed schema, so
    /// `add` and `remove` don't apply.
    #[command(subcommand)]
    Asset(asset::AssetCmd),
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
            // List is read-only.
            RequisitionCmd::List => CmdKind::Read,
            // Delete issues DELETE calls.
            RequisitionCmd::Delete { .. } => CmdKind::Write,
            // Export only issues GETs.
            RequisitionCmd::Export { .. } => CmdKind::Read,
            // Convert is pure local file transform — no HTTP at all.
            // Classified Read so --read-only contexts still allow it.
            RequisitionCmd::Convert { .. } => CmdKind::Read,
            // Delegate sub-resource classification to the nested
            // command (list/get = Read, add/set/remove = Write).
            RequisitionCmd::Node(cmd) => cmd.kind(),
            RequisitionCmd::Interface(cmd) => cmd.kind(),
            RequisitionCmd::Service(cmd) => cmd.kind(),
            RequisitionCmd::Category(cmd) => cmd.kind(),
            RequisitionCmd::Asset(cmd) => cmd.kind(),
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
                explain_rescan,
                stop_on_error,
                wait_flags,
            } => {
                run_apply(
                    file,
                    dry_run,
                    diff,
                    rescan_existing,
                    explain_rescan,
                    stop_on_error,
                    wait_flags,
                    ctx,
                )
                .await
            }
            RequisitionCmd::Import {
                fs,
                rescan_existing,
                wait_flags,
            } => run_import(fs, rescan_existing, wait_flags, ctx).await,
            RequisitionCmd::Status { fs } => run_status(fs, ctx).await,
            RequisitionCmd::List => run_list_requisitions(ctx).await,
            RequisitionCmd::Delete { fs, yes } => run_delete_requisition(fs, yes, ctx).await,
            RequisitionCmd::Export {
                fs,
                out,
                include_defaults,
            } => run_export(fs, out, include_defaults, ctx).await,
            RequisitionCmd::Convert {
                from,
                foreign_sources_dir,
                out,
                explain,
            } => run_convert(from, foreign_sources_dir, out, explain).await,
            RequisitionCmd::Node(cmd) => cmd.run(ctx).await,
            RequisitionCmd::Interface(cmd) => cmd.run(ctx).await,
            RequisitionCmd::Service(cmd) => cmd.run(ctx).await,
            RequisitionCmd::Category(cmd) => cmd.run(ctx).await,
            RequisitionCmd::Asset(cmd) => cmd.run(ctx).await,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_apply(
    file: PathBuf,
    dry_run: bool,
    diff: bool,
    rescan_existing: RescanFlag,
    explain_rescan: bool,
    stop_on_error: bool,
    wait_flags: AsyncFlags,
    ctx: &Context,
) -> Result<()> {
    // ---- 1. Validate + dispatch on file / directory / glob ----
    //
    // The `-f` argument accepts three shapes per the spec:
    //   - a single file: dispatch to the single-file path below
    //   - a directory: expand to its `*.yaml` / `*.yml` children
    //     (non-recursive) and dispatch to multi-file
    //   - a glob pattern (contains `*`, `?`, or `[`): expand the
    //     pattern (the `glob` crate honors `**` for recursion;
    //     unprefixed `*` matches one path segment) and dispatch to
    //     multi-file. A glob that happens to match exactly one file
    //     collapses to the single-file path so `--diff` and
    //     `--explain-rescan` still apply.
    let resolved = match resolve_apply_input(&file)? {
        ApplyDispatch::Multi(files) => {
            if diff {
                eprintln!(
                    "note: --diff has no effect in multi-file mode (use --dry-run + -o yaml for a per-file preview)"
                );
            }
            if explain_rescan {
                eprintln!(
                    "note: --explain-rescan has no effect in multi-file mode (use it per-file with --dry-run)"
                );
            }
            return run_apply_files(
                files,
                dry_run,
                rescan_existing,
                stop_on_error,
                wait_flags,
                ctx,
            )
            .await;
        }
        ApplyDispatch::Single(path) => path,
        // `ApplyDispatch` is `#[non_exhaustive]` — future variants
        // (e.g. `Stdin`) require explicit per-capability routing.
        // Refuse loudly today rather than dispatch silently.
        other => {
            return Err(Error::Config(format!(
                "unsupported apply input shape: {other:?} (this CLI build does not \
                 recognise this dispatch variant)"
            )));
        }
    };
    let meta = std::fs::metadata(&resolved)
        .map_err(|e| Error::Config(format!("failed to stat {}: {e}", resolved.display())))?;
    if !meta.is_file() {
        return Err(Error::Config(format!(
            "{} is not a regular file (got {:?})",
            resolved.display(),
            meta.file_type()
        )));
    }
    if stop_on_error {
        eprintln!(
            "note: --stop-on-error has no effect in single-file mode (use it with --file <dir>)"
        );
    }
    if meta.len() > MAX_INPUT_BYTES {
        return Err(Error::Config(format!(
            "{} is {} bytes, exceeds apply input cap of {} bytes",
            resolved.display(),
            meta.len(),
            MAX_INPUT_BYTES
        )));
    }
    let bytes = std::fs::read(&resolved)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", resolved.display())))?;
    let local: RequisitionLocal = serde_norway::from_slice(&bytes)
        .map_err(|e| Error::Config(format!("{}: {e}", resolved.display())))?;

    // ---- 2. Build client + API ----
    let client = OnmsClient::from_context(ctx)?;
    let api = ProvisioningApi::new(&client);

    // ---- 3. Run the apply orchestrator ----
    let opts = ApplyOptions {
        dry_run,
        rescan_existing: rescan_existing.into(),
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
            //
            // Only emit the hint when the error indicates a request
            // actually reached the server — `HttpStatus` is the only
            // class that could leave partial state behind. Parse
            // errors, auth failures, DNS lookup failures, TLS
            // handshake failures, etc. all happen pre-mutation, so
            // the "partial writes are possible" warning would be
            // misleading.
            if matches!(e, Error::HttpStatus { .. }) {
                eprint_recovery_hint(&local);
            }
            return Err(e);
        }
    };

    // ---- 4. Optional --wait phase ----
    // Only meaningful when apply actually triggered an import. Unchanged
    // and DryRun paths skip the trigger and therefore skip the wait.
    if wait_flags.wait
        && matches!(
            outcome.state,
            crate::apply::ApplyState::Created | crate::apply::ApplyState::Updated
        )
    {
        let new_ts = wait_for_import_completion(
            &api,
            &local.metadata.name,
            outcome.pre_trigger_last_import_ms,
            &wait_flags,
        )
        .await?;
        eprintln!(
            "Requisition/{}: import completed (last-import-ms={new_ts})",
            local.metadata.name
        );
    }

    // ---- 4. Format + print ----
    if diff {
        // Stderr so the text diff doesn't pollute stdout when the
        // caller also requested `-o json` / `-o yaml`. Matches the
        // eventconf `source apply --diff` precedent.
        eprint!("{}", render_apply_diff(&local, &outcome));
    }
    if explain_rescan {
        eprint_rescan_explanation(&outcome);
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

async fn run_import(
    fs: String,
    rescan_existing: bool,
    wait_flags: AsyncFlags,
    ctx: &Context,
) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = ProvisioningApi::new(&client);

    // Capture the pre-trigger last-import snapshot ONLY when the
    // operator asked to wait. Without --wait, the extra GET is dead
    // work; with --wait, this is the baseline the poller watches
    // advance past.
    let pre_trigger_last_import_ms = if wait_flags.wait {
        api.get_requisition(&fs).await?.and_then(|r| r.last_import)
    } else {
        None
    };

    api.trigger_import(&fs, rescan_existing).await?;

    if wait_flags.wait {
        let new_ts =
            wait_for_import_completion(&api, &fs, pre_trigger_last_import_ms, &wait_flags).await?;
        eprintln!("Requisition/{fs}: import completed (last-import-ms={new_ts})");
    }

    // Single-line confirmation. Scan-report id surfacing is task 6.3
    // / 5.7 territory — for now the trigger is fire-and-forget at the
    // HTTP layer too. Build the structured payload once; the JSON
    // and YAML arms serialize it via different encoders so the wire
    // shape stays in lockstep.
    let payload = serde_json::json!({
        "foreign_source": fs,
        "rescan_existing": rescan_existing,
        "triggered": true,
        "waited": wait_flags.wait,
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
            let line =
                format!("Requisition/{fs}: import triggered (rescanExisting={rescan_existing})\n");
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

async fn run_export(
    fs: Option<String>,
    out: Option<PathBuf>,
    include_defaults: bool,
    ctx: &Context,
) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = ProvisioningApi::new(&client);

    let results = match fs {
        Some(name) => vec![export_requisition(&api, &name, include_defaults).await?],
        None => export_all_requisitions(&api, include_defaults).await?,
    };

    if let Some(out_dir) = &out {
        std::fs::create_dir_all(out_dir)
            .map_err(|e| Error::Config(format!("creating {}: {e}", out_dir.display())))?;
        for r in &results {
            // Same safe-filename whitelist as convert's --out (P1
            // from the eleventh-pass review) — `r.foreign_source`
            // flows from the server's response body and could carry
            // path-unsafe characters if Horizon ever allowed them.
            if !is_safe_filename(&r.foreign_source) {
                return Err(Error::Config(format!(
                    "refusing to write to '{out_dir}/{name}.yaml': foreign-source name \
                     contains path-unsafe characters (allowed: alphanumeric, '-', '_', '.')",
                    out_dir = out_dir.display(),
                    name = r.foreign_source,
                )));
            }
            let path = out_dir.join(format!("{}.yaml", r.foreign_source));
            std::fs::write(&path, &r.yaml)
                .map_err(|e| Error::Config(format!("write {}: {e}", path.display())))?;
        }
        eprintln!(
            "Exported {} requisition(s) to {}",
            results.len(),
            out_dir.display()
        );
        return Ok(());
    }

    // No --out: stream to stdout per global -o flag.
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&results)
                .map_err(|e| Error::Config(format!("serializing export to JSON: {e}")))?;
            write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml | OutputFormat::Table => {
            // Both yaml and table modes stream raw YAML documents
            // separated by `---`. The yaml output is itself yaml so
            // table mode just shares it; a true tabular summary of
            // an export doesn't carry the actual payload, which is
            // the whole point of the verb.
            let mut first = true;
            for r in &results {
                if !first {
                    write_stdout(b"---\n")?;
                }
                first = false;
                write_stdout(r.yaml.as_bytes())?;
            }
        }
    }

    Ok(())
}

async fn run_convert(
    from: Option<PathBuf>,
    foreign_sources_dir: Option<PathBuf>,
    out: Option<PathBuf>,
    explain_code: Option<String>,
) -> Result<()> {
    // --explain short-circuits everything else (no XML inputs needed).
    if let Some(code_str) = explain_code {
        match FindingCode::parse(&code_str) {
            Some(code) => {
                write_stdout_line(explain(code).as_bytes())?;
                if from.is_some() {
                    eprintln!(
                        "note: --explain was set; --from was ignored. Drop --explain to run the conversion."
                    );
                }
                return Ok(());
            }
            None => {
                let known: Vec<&str> = FindingCode::all().iter().map(|c| c.as_str()).collect();
                return Err(Error::Config(format!(
                    "unknown finding code '{code_str}'; known: {}",
                    known.join(", ")
                )));
            }
        }
    }

    let xml_dir = from.ok_or_else(|| {
        Error::Config(
            "missing --from <xml-dir>; pass --explain <code> if you wanted to print rationale instead"
                .into(),
        )
    })?;
    let meta = std::fs::metadata(&xml_dir)
        .map_err(|e| Error::Config(format!("failed to stat {}: {e}", xml_dir.display())))?;
    if !meta.is_dir() {
        return Err(Error::Config(format!(
            "{} is not a directory",
            xml_dir.display()
        )));
    }

    let results =
        convert_directory(&xml_dir, foreign_sources_dir.as_deref()).map_err(Error::Config)?;

    // Emit YAML — stdout when --out is None (one document per
    // requisition, separated by `---` so the user can pipe through
    // `yq` or split downstream), or per-file when --out is set.
    if let Some(out_dir) = &out {
        std::fs::create_dir_all(out_dir)
            .map_err(|e| Error::Config(format!("creating {}: {e}", out_dir.display())))?;
        for r in &results {
            if let Some(yaml) = &r.yaml {
                // Validate the foreign-source name against a safe
                // filename whitelist before joining into --out. The
                // value flows from the XML root @foreign-source
                // attribute and is operator-controllable (or
                // attacker-controllable if the XML came from an
                // untrusted source) — a malicious name like
                // `../../etc/passwd` would otherwise escape --out.
                if !is_safe_filename(&r.foreign_source) {
                    return Err(Error::Config(format!(
                        "refusing to write to '{out_dir}/{name}.yaml': foreign-source name \
                         contains path-unsafe characters (allowed: alphanumeric, '-', '_', '.')",
                        out_dir = out_dir.display(),
                        name = r.foreign_source,
                    )));
                }
                let path = out_dir.join(format!("{}.yaml", r.foreign_source));
                std::fs::write(&path, yaml)
                    .map_err(|e| Error::Config(format!("write {}: {e}", path.display())))?;
            }
        }
    } else {
        let mut first = true;
        for r in &results {
            if let Some(yaml) = &r.yaml {
                if !first {
                    write_stdout(b"---\n")?;
                }
                first = false;
                write_stdout(yaml.as_bytes())?;
            }
        }
    }

    // Findings always go to stderr, regardless of --out. Format:
    // `<PRxxx> <severity> [source-path] message`.
    print_findings(&results);

    // Exit code per design D4 mirrored on convert: aggregate the
    // worst severity across all results.
    let exit = worst_exit_code(&results);
    if exit != 0 {
        std::process::exit(exit);
    }
    Ok(())
}

fn print_findings(results: &[ConversionResult]) {
    for r in results {
        for f in &r.findings {
            let sev = match f.severity {
                Severity::Info => "info",
                Severity::Warning => "warning",
                Severity::Error => "error",
            };
            let src = f
                .source_path
                .as_ref()
                .map(|p| format!(" [{}]", p.display()))
                .unwrap_or_default();
            eprintln!("{} {sev}{src}: {}", f.code.as_str(), f.message);
        }
    }
}

fn worst_exit_code(results: &[ConversionResult]) -> i32 {
    let mut worst = 0;
    for r in results {
        let c = r.exit_code();
        if c > worst {
            worst = c;
        }
    }
    worst
}

/// Whitelist check for foreign-source names that flow into `--out`
/// paths. Allowed characters: ASCII alphanumeric plus `-`, `_`, `.`.
/// Empty string and leading `.` (hidden files) are also rejected.
/// Anything else — `/`, `..`, null, control chars, Unicode — is
/// refused before `path.join` constructs an escape vector.
fn is_safe_filename(s: &str) -> bool {
    if s.is_empty() || s.starts_with('.') {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// clap value parser for foreign-source CLI arguments. Rejects empty
/// and whitespace-only inputs at parse time so the user sees a clean
/// usage error instead of a confusing 404 against a URL like
/// `/rest/requisitions//import`.
pub(super) fn nonempty_fs(s: &str) -> std::result::Result<String, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("foreign-source name must not be empty or whitespace-only".into());
    }
    // Preserve the original (un-trimmed) form — operators who deliberately
    // pass leading/trailing spaces deserve the round-trip — but we've at
    // least caught the all-whitespace case.
    Ok(s.to_string())
}

/// clap value parser for generic "non-empty after trim" arguments
/// (used by foreign-id and other string positionals in the sub-resource
/// verb files). Mirrors `nonempty_fs` but without the FS-specific error
/// wording. Lives here so `cmd/node.rs` and `cmd/interface.rs` share a
/// single definition.
pub(super) fn nonempty_string(s: &str) -> std::result::Result<String, String> {
    if s.trim().is_empty() {
        Err("value must not be empty or whitespace-only".into())
    } else {
        Ok(s.to_string())
    }
}

/// clap value parser for IP-address positionals. Accepts IPv4 and IPv6
/// literals (no brackets). Rejects typos and surrounding whitespace at
/// parse time so the user sees a clean usage error instead of a 400/404
/// against a malformed URL. Shared by `cmd/interface.rs` and
/// `cmd/service.rs`.
pub(super) fn ip_addr(s: &str) -> std::result::Result<String, String> {
    use std::net::IpAddr;
    use std::str::FromStr;
    IpAddr::from_str(s)
        .map(|ip| ip.to_string())
        .map_err(|_| format!("invalid IP address {s:?} (expected IPv4 or IPv6 literal)"))
}

/// CLI surface for the tri-state `--rescan-existing=<true|false|auto>`
/// flag. `Auto` (the default) hands the decision off to the diff
/// engine's scan-relevance classification per design §D3; `Force(b)`
/// overrides regardless of the diff content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RescanFlag {
    #[default]
    Auto,
    Force(bool),
}

impl From<RescanFlag> for RescanChoice {
    fn from(f: RescanFlag) -> Self {
        match f {
            RescanFlag::Auto => RescanChoice::Auto,
            RescanFlag::Force(b) => RescanChoice::Force(b),
        }
    }
}

/// clap value parser for `--rescan-existing`. Accepts `true`,
/// `false`, or `auto` (case-insensitive). Rejects other inputs at
/// parse time.
fn rescan_flag(s: &str) -> std::result::Result<RescanFlag, String> {
    match s.to_ascii_lowercase().as_str() {
        "true" => Ok(RescanFlag::Force(true)),
        "false" => Ok(RescanFlag::Force(false)),
        "auto" => Ok(RescanFlag::Auto),
        other => Err(format!(
            "rescan-existing must be one of 'true', 'false', or 'auto' (got {other:?})"
        )),
    }
}

/// Render the `--explain-rescan` rationale to stderr. Shows the
/// scan-relevant leaf paths from the diff so the operator can see
/// why the auto-decision landed on `rescanExisting=true`, or that no
/// leaf was relevant and the auto-decision is `false`. When the
/// operator forced the value via `--rescan-existing=true|false`, the
/// rendering distinguishes the *would-have* auto-decision from the
/// effective outcome.
fn eprint_rescan_explanation(outcome: &crate::apply::ApplyOutcome) {
    eprint!("{}", explain_rescan_text(outcome));
}

/// Pure-function form of [`eprint_rescan_explanation`] used by unit
/// tests so the rendering can be asserted without capturing stderr.
fn explain_rescan_text(outcome: &crate::apply::ApplyOutcome) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let auto_would_be = !outcome.scan_relevant_leaves.is_empty();
    let overridden = auto_would_be != outcome.rescan_existing;

    if outcome.scan_relevant_leaves.is_empty() {
        writeln!(
            s,
            "explain-rescan: no scan-relevant leaves in the diff — auto-decision would be \
             rescanExisting=false."
        )
        .ok();
    } else if overridden {
        writeln!(
            s,
            "explain-rescan: {} scan-relevant leaf path(s) would have driven \
             rescanExisting=true per design §D3 (overridden by \
             --rescan-existing flag):",
            outcome.scan_relevant_leaves.len()
        )
        .ok();
        for p in &outcome.scan_relevant_leaves {
            writeln!(s, "  - {p}").ok();
        }
    } else {
        writeln!(
            s,
            "explain-rescan: {} scan-relevant leaf path(s) drive rescanExisting=true \
             per design §D3:",
            outcome.scan_relevant_leaves.len()
        )
        .ok();
        for p in &outcome.scan_relevant_leaves {
            writeln!(s, "  - {p}").ok();
        }
    }
    writeln!(s, "Effective rescanExisting={}", outcome.rescan_existing).ok();
    s
}

async fn run_list_requisitions(ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = ProvisioningApi::new(&client);
    let names = api.list_requisition_names().await?;

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&names)
                .map_err(|e| Error::Config(format!("serializing requisition list to JSON: {e}")))?;
            write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&names)
                .map_err(|e| Error::Config(format!("serializing requisition list to YAML: {e}")))?;
            write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            if names.is_empty() {
                write_stdout(b"(no requisitions)\n")?;
            } else {
                for n in &names {
                    let line = format!("{n}\n");
                    write_stdout(line.as_bytes())?;
                }
            }
        }
    }
    Ok(())
}

/// Format the interactive confirmation prompt for `requisition
/// delete`. Pure function so the formatting can be unit-tested
/// without driving stdin/stdout. Includes the requisition name, the
/// node count, and the last-import timestamp (when present, rendered
/// as ISO-8601 UTC via the same helper export uses) so the operator
/// sees the blast radius before typing `yes`.
fn format_delete_confirmation_prompt(
    fs: &str,
    node_count: usize,
    last_import_ms: Option<i64>,
) -> String {
    let import_clause = match last_import_ms {
        Some(ms) if ms > 0 => {
            // Render as `YYYY-MM-DDTHH:MM:SSZ` UTC. Negative or
            // zero ms is unusable (pre-epoch / unset); fall through
            // to "never imported" in that case.
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
            format!(", last imported {}", crate::export::format_unix_ts(t))
        }
        _ => String::from(", never imported"),
    };
    format!(
        "About to purge Requisition/{fs} ({node_count} node(s){import_clause}). \
         This deletes BOTH pending and deployed snapshots and cannot be undone.\n\
         Type 'yes' or 'y' to confirm (case-insensitive): "
    )
}

/// Whether operator input at the delete prompt counts as
/// confirmation. Accepts `yes`/`y` case-insensitively after
/// whitespace trimming. Anything else (including an empty line or
/// EOF) is a cancellation.
fn is_delete_confirmation(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "yes" | "y")
}

async fn run_delete_requisition(fs: String, yes: bool, ctx: &Context) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};

    let client = OnmsClient::from_context(ctx)?;
    let api = ProvisioningApi::new(&client);

    // ---- Confirmation guard ----
    //
    // `--yes` skips the prompt entirely. Without `--yes`:
    //   - TTY (both stdin AND stderr): GET the requisition to show
    //     node count + last-import, then prompt. Pre-delete 404 →
    //     emit a stderr note and fall through to the idempotent
    //     DELETE path.
    //   - non-TTY (CI / scripted / stderr redirected): refuse with
    //     a clear pointer at --yes.
    //
    // `confirmed_interactively` tracks whether the operator just
    // approved a non-empty requisition. Used at the end to mirror
    // the success outcome to stderr (in case stdout is redirected).
    //
    // `preflight_404_already_noted` tracks whether the pre-confirm
    // GET 404'd and emitted its own "not present" stderr note. The
    // dual-DELETE absent-on-both path checks this to avoid emitting
    // a near-duplicate second note.
    let mut confirmed_interactively = false;
    let mut preflight_404_already_noted = false;
    if !yes {
        let stdin_is_tty = std::io::stdin().is_terminal();
        let stderr_is_tty = std::io::stderr().is_terminal();
        if !(stdin_is_tty && stderr_is_tty) {
            let which = match (stdin_is_tty, stderr_is_tty) {
                (false, false) => "stdin and stderr are not terminals",
                (false, true) => "stdin is not a terminal",
                (true, false) => "stderr is not a terminal (redirected?)",
                (true, true) => unreachable!(),
            };
            return Err(Error::Config(format!(
                "error: `requisition delete {fs}` requires --yes in non-interactive \
                 contexts ({which}). Re-run with: onmsctl requisition delete {fs} --yes \
                 — this guards a destructive operation that purges both pending and \
                 deployed snapshots in a single call."
            )));
        }
        // TTY path: probe the requisition first so the prompt names
        // the blast radius. 404 → emit a "skipping confirmation"
        // note and fall through to the idempotent DELETE path.
        match api.get_requisition(&fs).await? {
            Some(req) => {
                let prompt =
                    format_delete_confirmation_prompt(&fs, req.node.len(), req.last_import);
                eprint!("{prompt}");
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                let read = std::io::stdin()
                    .lock()
                    .read_line(&mut line)
                    .map_err(|e| Error::Config(format!("reading confirmation from stdin: {e}")))?;
                // EOF (Ctrl-D, closed pipe) → treat as cancellation,
                // not an I/O fault. Operator intent is "no".
                if read == 0 {
                    eprintln!();
                    return Err(Error::Config(format!(
                        "delete cancelled by operator (EOF on stdin): {fs}"
                    )));
                }
                if !is_delete_confirmation(&line) {
                    return Err(Error::Config(format!("delete cancelled by operator: {fs}")));
                }
                confirmed_interactively = true;
            }
            None => {
                eprintln!(
                    "note: requisition '{fs}' is not present on the server (pre-confirm GET returned 404); \
                     skipping confirmation."
                );
                preflight_404_already_noted = true;
            }
        }
    }

    // Issue the pending DELETE first. 404 here means the pending
    // snapshot was already absent — fine, the deployed call will
    // still run.
    let pending_absent = match api.delete_pending_requisition(&fs).await {
        Ok(()) => false,
        Err(Error::HttpStatus { status: 404, .. }) => true,
        Err(e) => return Err(e),
    };

    // Now the deployed DELETE. 404 means the snapshot was already
    // absent (e.g. requisition was never imported). A non-404 error
    // here, after a successful pending DELETE, leaves an orphan on
    // the server — warn loudly so the operator knows to investigate.
    let deployed_absent = match api.delete_deployed_requisition(&fs).await {
        Ok(()) => false,
        Err(Error::HttpStatus { status: 404, .. }) => true,
        Err(e) => {
            if !pending_absent {
                eprintln!(
                    "warning: Requisition/{fs} — pending snapshot was deleted, but the \
                     deployed snapshot DELETE failed; server is now in a half-purged \
                     state. Run `onmsctl requisition status {fs}` to check, and re-run \
                     `requisition delete {fs}` to retry the deployed purge."
                );
            }
            return Err(e);
        }
    };

    if pending_absent && deployed_absent && !preflight_404_already_noted {
        eprintln!(
            "note: Requisition/{fs} was not present on the server (both pending and \
             deployed snapshots returned 404); delete was a no-op."
        );
    }

    let payload = serde_json::json!({
        "foreign_source": fs,
        "action": "deleted",
        "pending_absent": pending_absent,
        "deployed_absent": deployed_absent,
    });
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&payload)
                .map_err(|e| Error::Config(format!("serializing delete outcome to JSON: {e}")))?;
            write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&payload)
                .map_err(|e| Error::Config(format!("serializing delete outcome to YAML: {e}")))?;
            write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let line = format!("Requisition/{fs}: deleted (pending + deployed snapshots purged)\n");
            write_stdout(line.as_bytes())?;
        }
    }
    // Mirror the outcome to stderr when the operator confirmed
    // interactively — if stdout is redirected (`delete X >log`),
    // they'd otherwise see no feedback after typing `yes`.
    // Skipped when both snapshots were absent (no-op delete) since
    // the stderr note above already covers the operator-visible
    // case.
    if confirmed_interactively && !(pending_absent && deployed_absent) {
        eprintln!("Requisition/{fs} deleted (pending + deployed snapshots purged).");
    }
    Ok(())
}

/// Write `bytes` to stdout, treating `BrokenPipe` as a clean exit
/// (e.g. when the user pipes our output into `head -c N`). Other I/O
/// errors propagate as `Error::Io` so the exit-code mapping picks them
/// up. Mirrors the binary's `write_stdout` helper.
pub(super) fn write_stdout(bytes: &[u8]) -> Result<()> {
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
pub(super) fn write_stdout_line(bytes: &[u8]) -> Result<()> {
    write_stdout(bytes)?;
    write_stdout(b"\n")
}

/// Run `apply` over a pre-resolved list of requisition YAML files
/// (from either directory expansion or glob expansion). Two-phase
/// orchestration: `plan_directory` reads deployed state per file and
/// builds a [`MultiApplyPlan`]; the combined plan is rendered to
/// stderr; `--dry-run` exits here; otherwise `execute_multi` consumes
/// the pre-computed plans. The caller is responsible for producing a
/// non-empty file list.
async fn run_apply_files(
    files: Vec<PathBuf>,
    dry_run: bool,
    rescan_existing: RescanFlag,
    stop_on_error: bool,
    wait_flags: AsyncFlags,
    ctx: &Context,
) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = ProvisioningApi::new(&client);

    let opts = MultiApplyOptions {
        rescan_existing: rescan_existing.into(),
        stop_on_error,
    };

    // ---- Phase 1: plan ----
    let plan = plan_directory(&files, &api, &opts).await?;
    render_combined_plan(&plan, dry_run)?;

    if plan.is_aborted() {
        // Map Phase-1 abort cause to a structured error. The
        // diagnostics (collision messages / parse errors) already
        // landed on stderr via render_combined_plan.
        let hard_collisions: Vec<&str> = plan
            .collision_findings
            .iter()
            .filter(|f| matches!(f.code, CollisionCode::DuplicateMetadataName))
            .map(|f| f.key.as_str())
            .collect();
        if !hard_collisions.is_empty() {
            let first = hard_collisions.first().copied().unwrap_or("?");
            return Err(Error::Config(format!(
                "phase-1 abort: {} hard collision(s) detected (first: '{first}'); \
                 see stderr for the full list",
                hard_collisions.len()
            )));
        }
        let parse_count = plan.parse_errors.len();
        return Err(Error::Config(format!(
            "phase-1 abort: {parse_count} parse error(s); see stderr"
        )));
    }

    if dry_run {
        // Spec: --dry-run exits 0 after Phase 1, after the combined
        // plan is rendered. No structured stdout — JSON / YAML
        // consumers that need the plan in a parseable shape should
        // open an issue; the text plan on stderr is the contract.
        return Ok(());
    }

    // ---- Phase 2: execute ----
    let outcome = execute_multi(plan, &api, &opts).await?;
    render_multi_outcome(&outcome, ctx)?;

    // ---- --wait phase ----
    // Honor --wait by polling per-file in the order results were
    // produced (alphabetical path). Only Created/Updated successes
    // are waited on; apply errors skip naturally. Wait errors abort
    // the whole sequence (the Horizon-side import is already done;
    // what failed is the polling, which is a real problem the
    // operator wants to surface).
    if wait_flags.wait {
        for r in &outcome.results {
            if let (Some(fs), Ok(apply)) = (&r.foreign_source, &r.outcome) {
                use crate::apply::ApplyState;
                if matches!(apply.state, ApplyState::Created | ApplyState::Updated) {
                    let new_ts = wait_for_import_completion(
                        &api,
                        fs,
                        apply.pre_trigger_last_import_ms,
                        &wait_flags,
                    )
                    .await?;
                    eprintln!("Requisition/{fs}: import completed (last-import-ms={new_ts})");
                }
            }
        }
    }

    // Exit-code policy: any per-file Err → PartialSuccess (exit 1).
    // Phase-1 aborts already returned above.
    let failed = count_failures(&outcome);
    if failed > 0 {
        return Err(Error::PartialSuccess { failed });
    }
    Ok(())
}

/// Render the Phase-1 combined plan to stderr.
///
/// Format (multi-line so the output scales to N files):
///
/// ```text
/// Phase 1 plan (3 files):
///   ✓ a.yaml -> Requisition/acme-prod: would-create (rescanExisting=true, foreignSource=created)
///   ✓ b.yaml -> Requisition/site-b: unchanged
///   ✓ c.yaml -> Requisition/lab-east: would-update (rescanExisting=false, foreignSource=no-change)
/// ```
///
/// Aborted Phase 1 (parse error or hard collision) renders the
/// header as `ABORTED` and lists the parse-error rows / collision
/// findings below.
fn render_combined_plan(plan: &MultiApplyPlan, dry_run: bool) -> Result<()> {
    if plan.is_aborted() {
        eprintln!("Phase 1 plan: ABORTED");
    } else {
        let n = plan.entries.len();
        let suffix = if dry_run { " (dry-run)" } else { "" };
        eprintln!(
            "Phase 1 plan ({n} file{}):{suffix}",
            if n == 1 { "" } else { "s" }
        );
    }

    for entry in &plan.entries {
        eprintln!(
            "  ✓ {} -> Requisition/{}: {} (rescanExisting={}, foreignSource={})",
            entry.path.display(),
            entry.plan.local.metadata.name,
            plan_state_word(entry.plan.state),
            entry.plan.rescan_existing,
            fs_word(entry.plan.foreign_source_action),
        );
    }

    for err in &plan.parse_errors {
        let msg = err
            .outcome
            .as_ref()
            .err()
            .map(String::as_str)
            .unwrap_or("?");
        eprintln!("  ✗ {} -> (parse error): {msg}", err.path.display());
    }

    // Collision findings — hard errors as "error", soft duplicates
    // as "warn". These are emitted here (Phase 1) rather than after
    // Phase 2 so the spec scenario "warning ... continue to Phase 2"
    // observes the warning BEFORE any non-GET HTTP request.
    for f in &plan.collision_findings {
        let sev = if matches!(f.code, CollisionCode::DuplicateMetadataName) {
            "error"
        } else {
            "warn"
        };
        eprintln!("collision {sev}: {}", f.message);
    }

    Ok(())
}

fn plan_state_word(s: PlanState) -> &'static str {
    match s {
        PlanState::Unchanged => "unchanged",
        PlanState::WouldCreate => "would-create",
        PlanState::WouldUpdate => "would-update",
    }
}

/// Resolve the provisioning `-f` argument by delegating to the
/// shared `onmsctl_core::apply_input::resolve_apply_input` helper
/// with `&["yaml", "yml"]` as the extension filter. Eventconf calls
/// the same helper with the same filter (see the parity Requirement
/// in `harden-provisioning-and-eventconf-parity`'s spec deltas).
fn resolve_apply_input(file: &std::path::Path) -> Result<onmsctl_core::apply_input::ApplyDispatch> {
    onmsctl_core::apply_input::resolve_apply_input(file, &["yaml", "yml"])
}

fn count_failures(outcome: &MultiApplyOutcome) -> usize {
    outcome
        .results
        .iter()
        .filter(|r| r.outcome.is_err())
        .count()
}

/// Render the multi-apply outcome by the global output-format flag.
///
/// Collision findings are NOT rendered here — they land on stderr
/// via `render_combined_plan` during Phase 1 so that the spec's
/// "warning ... continue to Phase 2" scenario observes the warning
/// before any non-GET HTTP request.
fn render_multi_outcome(outcome: &MultiApplyOutcome, ctx: &Context) -> Result<()> {
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(outcome)
                .map_err(|e| Error::Config(format!("serializing multi outcome to JSON: {e}")))?;
            write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(outcome)
                .map_err(|e| Error::Config(format!("serializing multi outcome to YAML: {e}")))?;
            write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            for r in &outcome.results {
                let line = match &r.outcome {
                    Ok(o) => format!(
                        "  ✓ {} -> Requisition/{}: {} (rescanExisting={}, foreignSource={})\n",
                        r.path.display(),
                        r.foreign_source.as_deref().unwrap_or("?"),
                        state_word(o.state),
                        o.rescan_existing,
                        fs_word(o.foreign_source_action),
                    ),
                    Err(e) => format!(
                        "  ✗ {} -> {}: {e}\n",
                        r.path.display(),
                        r.foreign_source.as_deref().unwrap_or("(parse error)"),
                    ),
                };
                write_stdout(line.as_bytes())?;
            }
            let summary = format!(
                "Multi-apply {}: {} ok, {} failed, {} collision finding(s)\n",
                multi_state_word(outcome.state),
                outcome.results.iter().filter(|r| r.outcome.is_ok()).count(),
                count_failures(outcome),
                outcome.collision_findings.len(),
            );
            write_stdout(summary.as_bytes())?;
        }
    }
    Ok(())
}

fn multi_state_word(s: MultiApplyState) -> &'static str {
    match s {
        MultiApplyState::AbortedPhase1 => "aborted (phase 1)",
        MultiApplyState::Completed => "completed",
        MultiApplyState::StoppedEarly => "stopped early",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_addr_accepts_ipv4_and_ipv6() {
        assert_eq!(ip_addr("10.0.0.1").unwrap(), "10.0.0.1");
        assert_eq!(ip_addr("2001:db8::1").unwrap(), "2001:db8::1");
    }

    #[test]
    fn ip_addr_rejects_garbage() {
        assert!(ip_addr("not-an-ip").is_err());
        assert!(ip_addr("10.0.0.1.2").is_err());
        assert!(ip_addr(" 10.0.0.1 ").is_err());
    }

    #[test]
    fn nonempty_string_rejects_whitespace_only() {
        assert!(nonempty_string("").is_err());
        assert!(nonempty_string("   ").is_err());
        assert_eq!(nonempty_string("foo").unwrap(), "foo");
    }

    // ---- requisition delete confirmation prompt ----

    #[test]
    fn delete_prompt_includes_fs_node_count_and_last_import_as_iso8601() {
        // 1_700_000_000_000 ms = 2023-11-14T22:13:20Z
        let s = format_delete_confirmation_prompt("acme-prod", 42, Some(1_700_000_000_000));
        assert!(s.contains("Requisition/acme-prod"));
        assert!(s.contains("42 node(s)"));
        // ISO-8601 UTC rendering via the shared `format_unix_ts`
        // helper — operators can read this at a glance.
        assert!(s.contains("2023-11-14T22:13:20Z"), "prompt was: {s}");
        assert!(!s.contains("epoch-ms"));
        assert!(s.contains("Type 'yes' or 'y' to confirm"));
    }

    #[test]
    fn delete_prompt_handles_never_imported_requisition() {
        let s = format_delete_confirmation_prompt("acme-prod", 0, None);
        assert!(s.contains("0 node(s)"));
        assert!(s.contains("never imported"));
        assert!(!s.contains("epoch-ms"));
    }

    #[test]
    fn delete_prompt_treats_non_positive_epoch_ms_as_never_imported() {
        // Negative or zero ms is unusable; fall through to "never".
        let s = format_delete_confirmation_prompt("acme-prod", 0, Some(0));
        assert!(s.contains("never imported"));
        let s = format_delete_confirmation_prompt("acme-prod", 0, Some(-1));
        assert!(s.contains("never imported"));
    }

    #[test]
    fn delete_prompt_warns_about_dual_snapshot_purge() {
        let s = format_delete_confirmation_prompt("acme-prod", 1, None);
        // The blast-radius wording is load-bearing — operators need
        // to know this deletes BOTH snapshots.
        assert!(s.contains("BOTH pending and deployed"));
        assert!(s.contains("cannot be undone"));
    }

    #[test]
    fn is_delete_confirmation_accepts_yes_and_y_case_insensitive() {
        assert!(is_delete_confirmation("yes"));
        assert!(is_delete_confirmation("YES"));
        assert!(is_delete_confirmation("Yes"));
        assert!(is_delete_confirmation("y"));
        assert!(is_delete_confirmation("Y"));
        // Trailing newline from read_line.
        assert!(is_delete_confirmation("yes\n"));
        // Surrounding whitespace tolerated.
        assert!(is_delete_confirmation("  yes  \n"));
    }

    #[test]
    fn is_delete_confirmation_rejects_anything_else() {
        assert!(!is_delete_confirmation(""));
        assert!(!is_delete_confirmation("\n"));
        assert!(!is_delete_confirmation("no"));
        assert!(!is_delete_confirmation("yeah"));
        assert!(!is_delete_confirmation("ya"));
        assert!(!is_delete_confirmation("yes please"));
        assert!(!is_delete_confirmation("0"));
    }

    #[test]
    fn rescan_flag_accepts_canonical_values_case_insensitive() {
        assert_eq!(rescan_flag("true").unwrap(), RescanFlag::Force(true));
        assert_eq!(rescan_flag("TRUE").unwrap(), RescanFlag::Force(true));
        assert_eq!(rescan_flag("false").unwrap(), RescanFlag::Force(false));
        assert_eq!(rescan_flag("FALSE").unwrap(), RescanFlag::Force(false));
        assert_eq!(rescan_flag("auto").unwrap(), RescanFlag::Auto);
        assert_eq!(rescan_flag("Auto").unwrap(), RescanFlag::Auto);
    }

    #[test]
    fn rescan_flag_rejects_unknown_values() {
        assert!(rescan_flag("maybe").is_err());
        assert!(rescan_flag("").is_err());
        assert!(rescan_flag("1").is_err());
    }

    #[test]
    fn rescan_flag_converts_to_rescan_choice() {
        assert_eq!(RescanChoice::from(RescanFlag::Auto), RescanChoice::Auto);
        assert_eq!(
            RescanChoice::from(RescanFlag::Force(true)),
            RescanChoice::Force(true)
        );
        assert_eq!(
            RescanChoice::from(RescanFlag::Force(false)),
            RescanChoice::Force(false)
        );
    }

    // ---- explain_rescan_text rendering ----

    fn outcome_with(rescan_existing: bool, leaves: Vec<&str>) -> crate::apply::ApplyOutcome {
        crate::apply::ApplyOutcome {
            state: crate::apply::ApplyState::DryRun,
            delta: Default::default(),
            rescan_existing,
            foreign_source_action: crate::apply::ForeignSourceAction::NoChange,
            original_remote_fs: None,
            pre_trigger_last_import_ms: None,
            scan_relevant_leaves: leaves.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn explain_rescan_empty_leaves_reports_no_relevant_paths() {
        let outcome = outcome_with(false, vec![]);
        let s = explain_rescan_text(&outcome);
        assert!(s.contains("no scan-relevant leaves"));
        assert!(s.contains("Effective rescanExisting=false"));
    }

    #[test]
    fn explain_rescan_auto_with_leaves_drives_true() {
        let outcome = outcome_with(true, vec!["spec.nodes[0].interfaces[0].services"]);
        let s = explain_rescan_text(&outcome);
        // Plain "drive" wording — auto-decision was honored.
        assert!(s.contains("drive rescanExisting=true"));
        assert!(!s.contains("would have driven"));
        assert!(s.contains("spec.nodes[0].interfaces[0].services"));
        assert!(s.contains("Effective rescanExisting=true"));
    }

    #[test]
    fn explain_rescan_override_distinguishes_would_have_from_effective() {
        // Operator forced --rescan-existing=false against a diff that
        // auto would have driven to true.
        let outcome = outcome_with(false, vec!["spec.nodes[0].interfaces[0].services"]);
        let s = explain_rescan_text(&outcome);
        assert!(s.contains("would have driven"));
        assert!(s.contains("overridden by --rescan-existing"));
        assert!(s.contains("Effective rescanExisting=false"));
    }

    // ---- Glob dispatch ----
    //
    // The bulk of glob-dispatch tests live in
    // `onmsctl_core::apply_input::tests`. Provisioning keeps a single
    // sanity test here verifying that the wrapper actually delegates
    // to the shared helper with the yaml/yml extension filter — if
    // the wrapper is renamed or removed, this test breaks.

    #[test]
    fn resolve_apply_input_wrapper_dispatches_to_core_with_yaml_filter() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.yaml"), "x").unwrap();
        std::fs::write(tmp.path().join("b.yml"), "x").unwrap();
        std::fs::write(tmp.path().join("README.md"), "x").unwrap();
        match resolve_apply_input(tmp.path()).unwrap() {
            ApplyDispatch::Multi(files) => {
                assert_eq!(files.len(), 2, "yaml + yml kept, README.md filtered");
            }
            ApplyDispatch::Single(_) => {
                panic!("directory with two yaml files should resolve to Multi")
            }
            other => panic!("unexpected dispatch variant: {other:?}"),
        }
    }
}
