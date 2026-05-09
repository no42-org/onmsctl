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
use std::path::PathBuf;

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
    Download {
        /// Source id.
        id: i64,
        /// Write to this file (default: stdout).
        #[arg(short = 'O', long)]
        output_file: Option<PathBuf>,
        /// Overwrite the output file if it already exists.
        #[arg(long)]
        force: bool,
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
            } => {
                ensure_positive_id(id, "source id")?;
                let bytes = api.download_source_xml(id).await?;
                if let Some(path) = output_file {
                    if path.exists() && !force {
                        return Err(Error::Config(format!(
                            "refusing to overwrite existing file {}; pass --force to override",
                            path.display()
                        )));
                    }
                    std::fs::write(&path, &bytes).map_err(|e| {
                        Error::Config(format!(
                            "failed to write download to {}: {e}",
                            path.display()
                        ))
                    })?;
                    eprintln!("wrote {} bytes to {}", bytes.len(), path.display());
                } else {
                    use std::io::Write;
                    let mut stdout = std::io::stdout().lock();
                    match stdout.write_all(&bytes) {
                        Ok(()) => {}
                        // Piping to `head -c N` etc. closes stdout
                        // mid-write. Treat that as a clean exit, not an
                        // error.
                        Err(e) if e.kind() == ErrorKind::BrokenPipe => {}
                        Err(e) => return Err(Error::Io(e)),
                    }
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
        }
        Ok(())
    }
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
