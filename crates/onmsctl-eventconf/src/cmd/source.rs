/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl source ...` subcommands.
//!
//! Each variant corresponds to one or more REST endpoints in
//! `EventConfApi`. The handlers wire flag inputs to API calls and stream
//! output through `onmsctl_core::render_*`.

use std::path::PathBuf;

use clap::Subcommand;
use onmsctl_core::client::MultipartPart;
use onmsctl_core::{Context, Error, OnmsClient, Result, render_list, render_one};

use crate::api::{EventConfApi, SourceFilter};
use crate::dto::{AddEventConfSourceRequest, EventConfSourceDto, SourceNameAndId};
use crate::render::SourceName;

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
    Download {
        /// Source id.
        id: i64,
        /// Write to this file (default: stdout).
        #[arg(short = 'O', long)]
        output_file: Option<PathBuf>,
    },

    /// List the names of all eventconf sources.
    Names,

    /// List `{id, name}` pairs for all eventconf sources.
    NamesAndIds,
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
                let n = ids.len();
                api.delete_sources(&ids).await?;
                eprintln!("deleted {n} source(s)");
            }
            SourceCmd::Enable { ids, cascade } => {
                let n = ids.len();
                api.set_sources_enabled(&ids, true, cascade).await?;
                eprintln!(
                    "enabled {n} source(s){}",
                    if cascade { " (cascade)" } else { "" }
                );
            }
            SourceCmd::Disable { ids, cascade } => {
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
                if !result.success.is_empty() {
                    let out = render_list(&result.success, ctx.output_format)?;
                    println!("{out}");
                }
                if !result.errors.is_empty() {
                    let out = render_list(&result.errors, ctx.output_format)?;
                    eprintln!("{out}");
                }
                eprintln!("upload: {success_n} succeeded, {error_n} failed");
                if error_n > 0 {
                    return Err(Error::Config(format!(
                        "upload completed with {error_n} file failure(s)"
                    )));
                }
            }
            SourceCmd::Download { id, output_file } => {
                let bytes = api.download_source_xml(id).await?;
                if let Some(path) = output_file {
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
                    stdout.write_all(&bytes).map_err(Error::Io)?;
                }
            }
            SourceCmd::Names => {
                let names: Vec<SourceName> = api
                    .list_source_names()
                    .await?
                    .into_iter()
                    .map(SourceName)
                    .collect();
                let out = render_list(&names, ctx.output_format)?;
                println!("{out}");
            }
            SourceCmd::NamesAndIds => {
                let pairs: Vec<SourceNameAndId> = api.list_source_names_and_ids().await?;
                let out = render_list(&pairs, ctx.output_format)?;
                println!("{out}");
            }
        }
        Ok(())
    }
}

/// Build a `MultipartPart` per file, deriving the multipart `filename`
/// parameter from the basename. If a file is literally named
/// `eventconf.xml`, the upload handler will treat it as the master.
fn build_upload_parts(paths: &[PathBuf]) -> Result<Vec<MultipartPart>> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let body = std::fs::read(p)
            .map_err(|e| Error::Config(format!("failed to read {}: {e}", p.display())))?;
        let filename = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                Error::Config(format!("path {} has no filename component", p.display()))
            })?
            .to_string();
        out.push(MultipartPart::xml(filename, body));
    }
    Ok(out)
}

// `clippy::all` is fine; this single use is intended.
#[allow(dead_code)]
fn _ensure_dto_renderable() -> &'static str {
    // Compile-time check that the DTO has a TableRow impl wired in.
    // Removed warnings about EventConfSourceDto being unused if the
    // render impl ever gets accidentally deleted.
    use onmsctl_core::TableRow;
    let _: Vec<&'static str> = EventConfSourceDto::headers();
    "ok"
}
