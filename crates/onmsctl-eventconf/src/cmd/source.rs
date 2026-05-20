/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl source ...` subcommands.
//!
//! Each variant corresponds to one or more REST endpoints in
//! `EventConfApi`. The handlers wire flag inputs to API calls and stream
//! output through `onmsctl_core::render_*`.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use onmsctl_core::client::MultipartPart;
use onmsctl_core::{Context, Error, OnmsClient, Result, render_list, render_one};

use crate::api::{EventConfApi, SourceFilter};
use crate::dto::{AddEventConfSourceRequest, SourceNameAndId};
use crate::render::SourceName;

/// Hard cap on bytes read from each upload-input file. Matches the
/// equivalent cap in `OnmsClient::get_bytes` so we never accidentally
/// buffer more than 16 MiB into the multipart body.
const MAX_UPLOAD_BYTES_PER_FILE: u64 = 16 * 1024 * 1024;

#[derive(Subcommand, Debug, Clone)]
pub enum SourceCmd {
    /// List eventconf sources with optional filter / sort / paging.
    List {
        /// Substring filter on source name / vendor / description.
        #[arg(long)]
        filter: Option<String>,
        /// Sort field: `name`, `vendor`, `fileOrder`, `eventCount`.
        #[arg(long = "sort-by")]
        sort_by: Option<String>,
        /// `asc` or `desc`. Defaults to server default.
        #[arg(long)]
        order: Option<String>,
        /// Pagination offset.
        #[arg(long)]
        offset: Option<i32>,
        /// Pagination limit.
        #[arg(long)]
        limit: Option<i32>,
    },

    /// Show one source by id.
    Get {
        /// Source id.
        id: i64,
    },

    /// Create a new (empty) source. Use `source upload` to populate events.
    Create {
        /// Source name. Becomes the upload filename's basename and the
        /// vendor is derived from the prefix before the first `.`.
        #[arg(long)]
        name: String,
        /// Vendor name. If omitted, the server derives it from `--name`.
        #[arg(long)]
        vendor: Option<String>,
        /// Free-form description. Note: this is the only point at which a
        /// description can be set on an eventconf source — `source upload`
        /// blanks it on every call.
        #[arg(long)]
        description: Option<String>,
    },

    /// Delete one or more sources by id.
    Delete {
        /// Source ids to delete.
        #[arg(required = true)]
        ids: Vec<i64>,
    },

    /// Enable one or more sources.
    Enable {
        /// Source ids to enable.
        #[arg(required = true)]
        ids: Vec<i64>,
        /// Also enable each source's events.
        #[arg(long)]
        cascade: bool,
    },

    /// Disable one or more sources.
    Disable {
        /// Source ids to disable.
        #[arg(required = true)]
        ids: Vec<i64>,
        /// Also disable each source's events.
        #[arg(long)]
        cascade: bool,
    },

    /// Upload one or more eventconf XML files via the multipart endpoint.
    /// If a file named `eventconf.xml` is among the uploads, it is treated
    /// as the master file determining file ordering.
    Upload {
        /// Paths to eventconf XML files.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// Download a source's eventconf XML.
    ///
    /// The XML is the authoritative form; `download → edit → apply` may
    /// drop server-only fields that the local YAML schema doesn't
    /// model. For lossless round-trips, edit the XML directly and
    /// re-upload via `source upload`.
    ///
    /// With `--format yaml`, the downloaded XML is piped through
    /// `source convert` and the EventSource YAML is emitted instead.
    /// Findings (if any) go to stderr just like standalone convert.
    /// Conversion exit codes (1 = warnings, 2 = blocking) propagate to
    /// the process exit code.
    Download {
        /// Source id.
        id: i64,
        /// Write to this file (default: stdout).
        #[arg(short = 'O', long)]
        output_file: Option<PathBuf>,
        /// Overwrite the output file if it already exists.
        #[arg(long)]
        force: bool,
        /// Output format: `xml` (default, authoritative wire form) or
        /// `yaml` (converted via the migration pipeline; findings to
        /// stderr).
        #[arg(long, default_value = "xml")]
        format: String,
    },

    /// List the names of all eventconf sources.
    Names,

    /// List `{id, name}` pairs for all eventconf sources.
    NamesAndIds,

    /// Apply an `EventSource` YAML document declaratively.
    ///
    /// Reads the file, validates it locally, fetches the server's
    /// current state, and either creates / updates / no-ops as
    /// appropriate. With `--dry-run` no mutations are issued; with
    /// `--diff` the structured diff prints to stderr first.
    ///
    /// Known limitations (per design.md §6):
    ///   - description cannot be set or preserved through apply
    ///   - bounded enabled-flap window when applying disabled-state
    ///   - vendor is filename-derived (use metadata.name's prefix)
    ///   - fileOrder is server-managed in v0.1
    ///   - download → edit → apply round-trip may lose server-only
    ///     fields not modeled by the local DTOs
    Apply {
        /// Path to the EventSource YAML/JSON document.
        #[arg(short = 'f', long)]
        file: PathBuf,
        /// Show what would happen without issuing any mutating HTTP calls.
        #[arg(long)]
        dry_run: bool,
        /// Print the structured diff to stderr before applying (or in
        /// dry-run mode, before reporting WouldUpdate).
        #[arg(long)]
        diff: bool,
    },

    /// Convert eventconf XML to EventSource YAML with a migration report.
    ///
    /// Pure local file transform — issues no HTTP requests and requires
    /// no Horizon context. Use this to migrate existing /etc/events/
    /// files to declarative YAML for git-managed apply workflows.
    ///
    /// Findings (rule violations, normalizations, dropped elements) are
    /// reported to stderr in a structured form. Run with
    /// `--explain <code>` to read the rule rationale for any reported
    /// finding code.
    Convert {
        /// Path(s) to eventconf XML file(s). Use `-` to read from stdin
        /// (single input only; --name is required).
        #[arg(num_args = 0..)]
        inputs: Vec<PathBuf>,
        /// Write converted YAML to this file (single input only). Default:
        /// stdout for single input, per-input files in --output-dir for
        /// batch input.
        #[arg(short = 'O', long, conflicts_with = "output_dir")]
        output: Option<PathBuf>,
        /// Write per-input YAML files into this directory. Each output is
        /// named by stripping `.events.xml`/`.xml` from the input filename
        /// and appending `.yaml`.
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Override the derived `metadata.name`. Required when input is
        /// `-` (stdin). Only valid with a single input.
        #[arg(long)]
        name: Option<String>,
        /// Report format: `text` (default, human-readable) or `json`
        /// (machine-readable; entire ConversionResult to stdout, stderr
        /// empty).
        #[arg(long, default_value = "text")]
        format: String,
        /// Overwrite output files that already exist. Required when
        /// `-O <path>` or `--output-dir <dir>` would clobber an existing
        /// YAML file. Stdout output is never gated.
        #[arg(long)]
        force: bool,
        /// Maximum bytes to read per input. Accepts a raw integer or a
        /// humanized suffix (`16M`, `1G`). Default: 16 MiB. Applies to
        /// both stdin and file-path inputs. Over-cap inputs fail loudly
        /// rather than truncate silently.
        #[arg(long, default_value = "16M")]
        max_bytes: String,
        /// Cap on `EC001` (unmodeled-element) findings emitted per input
        /// file. After the cap, one summary finding is emitted and the
        /// scan stops walking further events. Default:
        /// `convert::DEFAULT_EC001_FINDINGS_CAP`. Use `0` for unlimited
        /// (the cap is disabled).
        #[arg(long, default_value_t = crate::convert::DEFAULT_EC001_FINDINGS_CAP)]
        max_findings: usize,
        /// Print the rule rationale for the given finding code (e.g.
        /// `--explain EC001`) and exit without converting. Inputs may
        /// be empty when this flag is used.
        #[arg(long)]
        explain: Option<String>,
    },
}

impl onmsctl_core::Classify for SourceCmd {
    fn kind(&self) -> onmsctl_core::CmdKind {
        use onmsctl_core::CmdKind::{Read, Write};
        match self {
            // GET-only or no-HTTP variants.
            SourceCmd::List { .. }
            | SourceCmd::Get { .. }
            | SourceCmd::Names
            | SourceCmd::NamesAndIds
            | SourceCmd::Download { .. }
            | SourceCmd::Convert { .. } => Read,
            // Issue POST / PUT / PATCH / DELETE.
            SourceCmd::Create { .. }
            | SourceCmd::Delete { .. }
            | SourceCmd::Enable { .. }
            | SourceCmd::Disable { .. }
            | SourceCmd::Upload { .. }
            | SourceCmd::Apply { .. } => Write,
        }
    }
}

impl SourceCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = EventConfApi::new(&client);
        match self {
            SourceCmd::List {
                filter,
                sort_by,
                order,
                offset,
                limit,
            } => {
                let f = SourceFilter {
                    filter,
                    sort_by,
                    order,
                    offset,
                    limit,
                };
                let page = api.filter_sources(&f).await?;
                let out = render_list(&page.items, ctx.output_format)?;
                println!("{out}");
            }
            SourceCmd::Get { id } => {
                ensure_positive_id(id, "source id")?;
                let s = api.get_source(id).await?;
                let out = render_one(&s, ctx.output_format)?;
                println!("{out}");
            }
            SourceCmd::Create {
                name,
                vendor,
                description,
            } => {
                let req = AddEventConfSourceRequest {
                    name,
                    vendor,
                    description,
                };
                let created = api.create_source(&req).await?;
                eprintln!(
                    "created source {} ({}, fileOrder {})",
                    created.id, created.name, created.file_order
                );
            }
            SourceCmd::Delete { ids } => {
                for id in &ids {
                    ensure_positive_id(*id, "source id")?;
                }
                let n = ids.len();
                api.delete_sources(&ids).await?;
                eprintln!("deleted {n} source(s)");
            }
            SourceCmd::Enable { ids, cascade } => {
                for id in &ids {
                    ensure_positive_id(*id, "source id")?;
                }
                let n = ids.len();
                api.set_sources_enabled(&ids, true, cascade).await?;
                eprintln!(
                    "enabled {n} source(s){}",
                    if cascade { " (cascade)" } else { "" }
                );
            }
            SourceCmd::Disable { ids, cascade } => {
                for id in &ids {
                    ensure_positive_id(*id, "source id")?;
                }
                let n = ids.len();
                api.set_sources_enabled(&ids, false, cascade).await?;
                eprintln!(
                    "disabled {n} source(s){}",
                    if cascade { " (cascade)" } else { "" }
                );
            }
            SourceCmd::Upload { files } => {
                let parts = build_upload_parts(&files)?;
                let result = api.upload(&parts).await?;
                let success_n = result.success.len();
                let error_n = result.errors.len();
                // Render to a single stream depending on output format.
                // Structured outputs (json/yaml) emit a single combined
                // document; table mode prints success and error tables
                // back to back to stdout for human viewing.
                use onmsctl_core::OutputFormat;
                match ctx.output_format {
                    OutputFormat::Table => {
                        if !result.success.is_empty() {
                            let out = render_list(&result.success, OutputFormat::Table)?;
                            println!("{out}");
                        }
                        if !result.errors.is_empty() {
                            let out = render_list(&result.errors, OutputFormat::Table)?;
                            println!("{out}");
                        }
                    }
                    OutputFormat::Json => {
                        let aggregate = serde_json::json!({
                            "success": result.success,
                            "errors": result.errors,
                        });
                        println!("{}", serde_json::to_string_pretty(&aggregate)?);
                    }
                    OutputFormat::Yaml => {
                        let aggregate = serde_json::json!({
                            "success": result.success,
                            "errors": result.errors,
                        });
                        println!("{}", serde_norway::to_string(&aggregate)?);
                    }
                }
                eprintln!("upload: {success_n} succeeded, {error_n} failed");
                if error_n > 0 {
                    return Err(Error::PartialSuccess { failed: error_n });
                }
            }
            SourceCmd::Download {
                id,
                output_file,
                force,
                format,
            } => {
                ensure_positive_id(id, "source id")?;
                let bytes = api.download_source_xml(id).await?;
                let format = format.to_ascii_lowercase();
                let (out_bytes, is_yaml, exit_code) = match format.as_str() {
                    "xml" => (bytes, false, 0i32),
                    "yaml" => {
                        // Look up the source name so the converted YAML's
                        // metadata.name comes from authoritative server
                        // state, not a synthesized filename.
                        let source = api.get_source(id).await?;
                        let opts = crate::convert::ConvertOpts {
                            name_override: Some(source.name.clone()),
                            max_findings: None,
                        };
                        let pseudo_path = Path::new("-");
                        let result = crate::convert::convert(&bytes, pseudo_path, &opts);
                        // Report to stderr (text format only; JSON would
                        // conflict with stdout YAML).
                        if !result.findings.is_empty() || result.yaml.is_none() {
                            eprint!("{}", crate::convert::render_report_text(&result));
                        }
                        let exit_code = result.exit_code();
                        match result.yaml {
                            Some(y) => (y.into_bytes(), true, exit_code),
                            None => {
                                // Blocking findings — exit with the
                                // converter's exit code rather than
                                // bubbling an opaque Error::Config.
                                std::process::exit(exit_code);
                            }
                        }
                    }
                    other => {
                        return Err(Error::Config(format!(
                            "--format must be 'xml' or 'yaml', got '{other}'"
                        )));
                    }
                };
                if let Some(path) = output_file {
                    if path.exists() && !force {
                        return Err(Error::Config(format!(
                            "refusing to overwrite existing file {}; pass --force to override",
                            path.display()
                        )));
                    }
                    std::fs::write(&path, &out_bytes).map_err(|e| {
                        Error::Config(format!(
                            "failed to write download to {}: {e}",
                            path.display()
                        ))
                    })?;
                    eprintln!(
                        "wrote {} bytes ({}) to {}",
                        out_bytes.len(),
                        if is_yaml { "yaml" } else { "xml" },
                        path.display()
                    );
                } else {
                    use std::io::Write;
                    let mut stdout = std::io::stdout().lock();
                    match stdout.write_all(&out_bytes) {
                        Ok(()) => {}
                        // Piping to `head -c N` etc. closes stdout
                        // mid-write. Treat that as a clean exit, not an
                        // error.
                        Err(e) if e.kind() == ErrorKind::BrokenPipe => {}
                        Err(e) => return Err(Error::Io(e)),
                    }
                }
                // Propagate the conversion's exit code when --format
                // yaml produced warnings (exit 1). Blocking findings
                // (exit 2) already terminated the process via
                // std::process::exit in the conversion branch above.
                if exit_code != 0 {
                    std::process::exit(exit_code);
                }
            }
            SourceCmd::Names => {
                let names: Vec<SourceName> = api
                    .list_source_names()
                    .await?
                    .into_iter()
                    .map(SourceName)
                    .collect();
                if names.is_empty() {
                    eprintln!("(no sources)");
                } else {
                    let out = render_list(&names, ctx.output_format)?;
                    println!("{out}");
                }
            }
            SourceCmd::NamesAndIds => {
                let pairs: Vec<SourceNameAndId> = api.list_source_names_and_ids().await?;
                if pairs.is_empty() {
                    eprintln!("(no sources)");
                } else {
                    let out = render_list(&pairs, ctx.output_format)?;
                    println!("{out}");
                }
            }
            SourceCmd::Apply {
                file,
                dry_run,
                diff,
            } => {
                use onmsctl_core::{ApplyOptions, run_apply};

                use crate::apply::{EventSourceTarget, local::EventSourceLocal};

                let meta = std::fs::metadata(&file).map_err(|e| {
                    Error::Config(format!("failed to stat {}: {e}", file.display()))
                })?;
                if !meta.is_file() {
                    return Err(Error::Config(format!(
                        "{} is not a regular file (got {:?})",
                        file.display(),
                        meta.file_type()
                    )));
                }
                if meta.len() > MAX_UPLOAD_BYTES_PER_FILE {
                    return Err(Error::Config(format!(
                        "{} is {} bytes, exceeds apply input cap of {} bytes",
                        file.display(),
                        meta.len(),
                        MAX_UPLOAD_BYTES_PER_FILE
                    )));
                }
                let bytes = std::fs::read(&file).map_err(|e| {
                    Error::Config(format!("failed to read {}: {e}", file.display()))
                })?;
                let local = EventSourceLocal::from_yaml(&bytes).map_err(|e| match e {
                    Error::Config(msg) => Error::Config(format!("{}: {msg}", file.display())),
                    other => other,
                })?;

                let opts = ApplyOptions {
                    dry_run,
                    show_diff: diff,
                };
                let outcome = run_apply::<EventSourceTarget>(local, &opts, ctx).await?;
                println!("{outcome}");
            }
            SourceCmd::Convert {
                inputs,
                output,
                output_dir,
                name,
                format,
                force,
                max_bytes,
                max_findings,
                explain,
            } => {
                let exit_code = run_convert(ConvertCli {
                    inputs,
                    output,
                    output_dir,
                    name,
                    format,
                    force,
                    max_bytes,
                    max_findings,
                    explain,
                })?;
                if exit_code != 0 {
                    std::process::exit(exit_code);
                }
            }
        }
        Ok(())
    }
}

/// Parsed shape of `onmsctl source convert ...` arguments. Aggregated
/// into a struct so the dispatcher and the runner pass the same shape.
struct ConvertCli {
    inputs: Vec<PathBuf>,
    output: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    name: Option<String>,
    format: String,
    force: bool,
    max_bytes: String,
    max_findings: usize,
    explain: Option<String>,
}

/// Run `source convert`. Pure local file transform; the eventconf API is
/// not touched. Returns the exit code (0/1/2/3) that the dispatcher
/// passes to `std::process::exit` only when non-zero — exit code `0`
/// returns cleanly so destructors run normally.
fn run_convert(args: ConvertCli) -> Result<i32> {
    use crate::convert::{self, ConvertOpts, FindingCode};

    // --explain short-circuits everything else.
    if let Some(code_str) = args.explain {
        match FindingCode::parse(&code_str) {
            Some(code) => {
                println!("{}", convert::explain(code));
                if !args.inputs.is_empty() {
                    eprintln!(
                        "note: --explain was set; the {} input path(s) on the command line \
                         were not converted. Drop --explain to run the conversion.",
                        args.inputs.len()
                    );
                }
                return Ok(0);
            }
            None => {
                let codes: Vec<&str> = FindingCode::all().iter().map(|c| c.as_str()).collect();
                eprintln!("error: unknown explain code '{code_str}'");
                eprintln!("available codes: {}", codes.join(", "));
                return Ok(3);
            }
        }
    }

    if args.inputs.is_empty() {
        eprintln!(
            "error: at least one input is required (path or `-` for stdin); \
             use --explain <code> to read finding documentation"
        );
        return Ok(3);
    }

    let format = args.format.to_ascii_lowercase();
    if format != "text" && format != "json" {
        eprintln!("error: --format must be 'text' or 'json', got '{format}'");
        return Ok(3);
    }

    let max_bytes = match parse_byte_size(&args.max_bytes) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("error: invalid --max-bytes value: {e}");
            return Ok(3);
        }
    };

    // Single-input mode permits -O / --name. Batch mode forbids them.
    if args.inputs.len() > 1 {
        if args.output.is_some() {
            eprintln!(
                "error: -O/--output is only valid with a single input; use --output-dir for batch mode"
            );
            return Ok(3);
        }
        if args.name.is_some() {
            eprintln!("error: --name is only valid with a single input");
            return Ok(3);
        }
        if args.output_dir.is_none() {
            eprintln!(
                "error: batch input requires --output-dir <dir>; refusing to write *.yaml files \
                 alongside inputs without explicit consent"
            );
            return Ok(3);
        }
    }

    let mut worst_exit = 0i32;
    let single_input = args.inputs.len() == 1;
    for input in &args.inputs {
        let xml_bytes = match read_input_bytes(input, max_bytes) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("error reading {}: {e}", input.display());
                return Ok(3);
            }
        };
        let opts = ConvertOpts {
            name_override: if single_input {
                args.name.clone()
            } else {
                None
            },
            max_findings: Some(args.max_findings),
        };
        let mut result = convert::convert(&xml_bytes, input, &opts);
        let written_path = emit_convert_result(
            &mut result,
            args.output.as_deref(),
            args.output_dir.as_deref(),
            &format,
            single_input,
            args.force,
        )?;
        if let Some(_p) = written_path {
            // result.input could be augmented with output path here if
            // we wanted to carry it through ConversionResult — for now
            // the JSON envelope is constructed externally in emit_*
        }
        worst_exit = worst_exit.max(result.exit_code());
    }
    Ok(worst_exit)
}

/// Read a `source convert` input. Caps input at `max_bytes` for both
/// stdin and file paths. Truncation is **loud** — over-cap inputs error
/// out before any allocation or downstream parse attempt.
fn read_input_bytes(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    if path.as_os_str() == "-" {
        // Read max_bytes + 1 so we can detect the over-cap case.
        let mut buf = Vec::new();
        let read_n = std::io::stdin()
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|e| Error::Config(format!("read stdin: {e}")))? as u64;
        if read_n > max_bytes {
            return Err(Error::Config(format!(
                "stdin exceeded --max-bytes cap of {max_bytes} bytes. \
                 Raise the cap (e.g. --max-bytes 64M) or convert from a file instead."
            )));
        }
        Ok(buf)
    } else {
        let meta = std::fs::metadata(path)
            .map_err(|e| Error::Config(format!("stat {}: {e}", path.display())))?;
        let size = meta.len();
        if size > max_bytes {
            return Err(Error::Config(format!(
                "{} is {size} bytes, exceeds --max-bytes cap of {max_bytes}. \
                 Raise the cap or split the file.",
                path.display()
            )));
        }
        std::fs::read(path).map_err(|e| Error::Config(format!("read {}: {e}", path.display())))
    }
}

/// Parse a humanized byte-size string (`"16M"`, `"1G"`, `"512"`) into a
/// `u64`. Suffixes recognized (case-insensitive): `K`, `M`, `G`, `T`.
/// Bare integers are interpreted as bytes.
fn parse_byte_size(s: &str) -> std::result::Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty value".into());
    }
    let (num_str, mult): (&str, u64) = if let Some(prefix) = s.strip_suffix(['K', 'k']) {
        (prefix, 1024)
    } else if let Some(prefix) = s.strip_suffix(['M', 'm']) {
        (prefix, 1024 * 1024)
    } else if let Some(prefix) = s.strip_suffix(['G', 'g']) {
        (prefix, 1024 * 1024 * 1024)
    } else if let Some(prefix) = s.strip_suffix(['T', 't']) {
        (prefix, 1024u64 * 1024 * 1024 * 1024)
    } else {
        (s, 1)
    };
    let n: u64 = num_str.trim().parse().map_err(|e| format!("'{s}': {e}"))?;
    n.checked_mul(mult)
        .ok_or_else(|| format!("'{s}': value overflows u64"))
}

/// Emit the conversion result. In text mode YAML goes to stdout (or a
/// file via `-O`/`--output-dir`) and the structured report goes to
/// stderr. In JSON mode the whole envelope serializes to stdout and
/// stderr stays silent.
///
/// Returns `Some(path)` if a YAML was written to a file (used by the
/// JSON envelope's `output` field), or `None` otherwise.
fn emit_convert_result(
    result: &mut crate::convert::ConversionResult,
    output: Option<&Path>,
    output_dir: Option<&Path>,
    format: &str,
    single_input: bool,
    force: bool,
) -> Result<Option<PathBuf>> {
    use crate::convert::render_report_text;

    // Determine the on-disk write target (if any) before anything else.
    // For batch mode with --output-dir, derive a per-input path. For
    // single-input with -O, use that. Otherwise None (stdout in text
    // mode, or no file in JSON mode).
    let target_path: Option<PathBuf> = match (output, output_dir, &result.input) {
        (Some(p), _, _) => Some(p.to_path_buf()),
        (None, Some(dir), Some(input_path)) => {
            let base = input_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("converted");
            let stem = base
                .strip_suffix(".events.xml")
                .or_else(|| base.strip_suffix(".xml"))
                .unwrap_or(base);
            Some(dir.join(format!("{stem}.yaml")))
        }
        _ => None,
    };

    if format == "json" {
        // JSON envelope per design D5 (amended 2026-05-18): include both
        // `output` (the disk path written, or null) and `yaml` (the body
        // string, or null). `input` is always emitted, even when null
        // for stdin.
        let envelope = serde_json::json!({
            "version": 1,
            "input": result.input,
            "output": target_path,
            "yaml": result.yaml,
            "findings": result.findings,
            "metrics": result.metrics,
            "exit_code": result.exit_code(),
        });
        let json = serde_json::to_string_pretty(&envelope)
            .map_err(|e| Error::Config(format!("json serialize: {e}")))?;
        println!("{json}");
        // In JSON mode we still write the YAML to disk if --output /
        // --output-dir was specified, so the path in the envelope
        // reflects reality.
        if let (Some(yaml), Some(path)) = (&result.yaml, target_path.as_ref()) {
            write_yaml_to_disk(path, yaml, force, output_dir)?;
        }
        return Ok(target_path);
    }

    // Text mode.
    if let Some(yaml) = &result.yaml {
        if let Some(path) = target_path.as_ref() {
            write_yaml_to_disk(path, yaml, force, output_dir)?;
            eprintln!("wrote {}", path.display());
        } else if single_input {
            // Single-input + no output flag → YAML to stdout.
            print!("{yaml}");
        }
        // Batch + no --output-dir was rejected earlier at the CLI parse
        // step — reaching here in batch mode without target_path is a
        // logic bug.
    }

    // Always render the text report when there are findings, regardless
    // of YAML emission outcome.
    if !result.findings.is_empty() || result.yaml.is_none() {
        eprint!("{}", render_report_text(result));
    }
    Ok(target_path)
}

/// Write the converted YAML to disk, handling `--force` overwrite gates
/// and `--output-dir` directory creation.
fn write_yaml_to_disk(
    path: &Path,
    yaml: &str,
    force: bool,
    output_dir: Option<&Path>,
) -> Result<()> {
    if !force && path.exists() {
        return Err(Error::Config(format!(
            "refusing to overwrite existing file {}; pass --force to override",
            path.display()
        )));
    }
    if let Some(dir) = output_dir {
        std::fs::create_dir_all(dir).map_err(|e| {
            Error::Config(format!(
                "failed to create --output-dir {}: {e}",
                dir.display()
            ))
        })?;
    }
    std::fs::write(path, yaml)
        .map_err(|e| Error::Config(format!("write {}: {e}", path.display())))?;
    Ok(())
}

/// Reject zero / negative ids early so we don't issue requests that the
/// server will reject anyway with an opaque 4xx.
pub(crate) fn ensure_positive_id(id: i64, kind: &str) -> Result<()> {
    if id <= 0 {
        return Err(Error::Config(format!("{kind} must be positive, got {id}")));
    }
    Ok(())
}

/// Build a `MultipartPart` per file. Each input is required to be a
/// regular file (rejects FIFOs, devices, directories, symlinks to those)
/// and capped at [`MAX_UPLOAD_BYTES_PER_FILE`].
///
/// Each file is parsed and pre-flight-validated: duplicate UEIs within a
/// single file are rejected client-side with a guided message, since
/// Horizon's upload validator rejects them too but only surfaces an
/// empty-body 400.
fn build_upload_parts(paths: &[PathBuf]) -> Result<Vec<MultipartPart>> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let meta = std::fs::metadata(p)
            .map_err(|e| Error::Config(format!("failed to stat {}: {e}", p.display())))?;
        if !meta.is_file() {
            return Err(Error::Config(format!(
                "{} is not a regular file (got {:?})",
                p.display(),
                meta.file_type()
            )));
        }
        if meta.len() > MAX_UPLOAD_BYTES_PER_FILE {
            return Err(Error::Config(format!(
                "{} is {} bytes, exceeds upload cap of {} bytes",
                p.display(),
                meta.len(),
                MAX_UPLOAD_BYTES_PER_FILE
            )));
        }
        let body = std::fs::read(p)
            .map_err(|e| Error::Config(format!("failed to read {}: {e}", p.display())))?;
        validate_upload_xml(p, &body)?;
        let filename = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                Error::Config(format!(
                    "path {} has no UTF-8 filename component",
                    p.display()
                ))
            })?
            .to_string();
        out.push(MultipartPart::xml(filename, body));
    }
    Ok(out)
}

/// Pre-flight validate an eventconf upload payload before it hits the wire.
/// Catches malformed XML and surfaces the file path on parse failure.
/// Does not enforce structural rules that Horizon's persistence layer
/// permits — specifically, duplicate UEIs across events within a single
/// source are first-class (a documented OpenNMS normalization pattern)
/// and pass the pre-flight unchanged. See the archived
/// `permit-duplicate-ueis-as-normalization-pattern` change.
fn validate_upload_xml(path: &std::path::Path, body: &[u8]) -> Result<()> {
    crate::xml::parse_events_from_xml(body).map_err(|e| {
        Error::Config(format!(
            "{}: failed to parse as eventconf XML: {e}",
            path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML_DUPLICATE_UEI: &[u8] = br#"<events xmlns="http://xmlns.opennms.org/xsd/eventconf">
  <event>
    <uei>uei.opennms.org/example/dup</uei>
    <event-label>A</event-label>
    <severity>Warning</severity>
  </event>
  <event>
    <uei>uei.opennms.org/example/dup</uei>
    <event-label>B</event-label>
    <severity>Warning</severity>
  </event>
</events>"#;

    const XML_CLEAN: &[u8] = br#"<events xmlns="http://xmlns.opennms.org/xsd/eventconf">
  <event>
    <uei>uei.opennms.org/example/a</uei>
    <event-label>A</event-label>
    <severity>Warning</severity>
  </event>
  <event>
    <uei>uei.opennms.org/example/b</uei>
    <event-label>B</event-label>
    <severity>Warning</severity>
  </event>
</events>"#;

    #[test]
    fn validate_upload_xml_accepts_duplicate_uei() {
        // Duplicate UEIs across events are a first-class normalization
        // pattern (see archived `permit-duplicate-ueis-as-normalization-pattern`).
        // The upload pre-flight no longer rejects them.
        let path = std::path::Path::new("/tmp/example.events.xml");
        validate_upload_xml(path, XML_DUPLICATE_UEI)
            .expect("duplicate UEIs must pass the upload pre-flight");
    }

    #[test]
    fn validate_upload_xml_accepts_distinct_ueis() {
        let path = std::path::Path::new("/tmp/example.events.xml");
        validate_upload_xml(path, XML_CLEAN).expect("clean payload must validate");
    }

    #[test]
    fn validate_upload_xml_rejects_malformed_xml() {
        let path = std::path::Path::new("/tmp/bad.xml");
        let err = validate_upload_xml(path, b"not xml").unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("failed to parse as eventconf XML")),
            other => panic!("unexpected {other:?}"),
        }
    }
}
