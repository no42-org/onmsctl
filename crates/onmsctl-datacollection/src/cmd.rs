/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `onmsctl datacollection` subcommand surface.
//!
//! `list` snapshots the deployed sources (and, with `--profiles`, the
//! snmp-collection profiles); `export` dumps one source's `datacollection-group`
//! document (`xml`/`json`); `delete` removes a source and its children.
//! `list`/`export` are Read; `delete` is Write. All three surface the friendly
//! "endpoint not available" message on a server without the subsystem.

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, Result, TableRow, render_list};
use serde::Serialize;

use crate::api::DataCollectionApi;

/// `onmsctl datacollection …` verbs.
#[derive(Subcommand, Debug, Clone)]
pub enum DatacollectionCmd {
    /// List the deployed data-collection sources (datacollection-groups).
    List {
        /// List the snmp-collection profiles instead of the sources.
        #[arg(long)]
        profiles: bool,
    },
    /// Export one source's datacollection-group document.
    Export {
        /// The source (datacollection-group) name.
        #[arg(value_name = "NAME")]
        name: String,
        /// Output format: `xml` (canonical) or `json`.
        #[arg(long, default_value = "xml", value_parser = ["xml", "json"])]
        format: String,
    },
    /// Delete a source and its children (mib groups / resource types / system
    /// defs). Profiles are left in place.
    Delete {
        /// The source (datacollection-group) name.
        #[arg(value_name = "NAME")]
        name: String,
    },
}

impl Classify for DatacollectionCmd {
    fn kind(&self) -> CmdKind {
        match self {
            DatacollectionCmd::List { .. } | DatacollectionCmd::Export { .. } => CmdKind::Read,
            DatacollectionCmd::Delete { .. } => CmdKind::Write,
        }
    }
}

impl DatacollectionCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = DataCollectionApi::new(&client);
        match self {
            DatacollectionCmd::List { profiles } => run_list(&api, profiles, ctx).await,
            DatacollectionCmd::Export { name, format } => run_export(&api, &name, &format).await,
            DatacollectionCmd::Delete { name } => run_delete(&api, &name).await,
        }
    }
}

/// One row of `datacollection list`.
#[derive(Debug, Clone, Serialize)]
struct SourceRow {
    id: i64,
    name: String,
}

impl TableRow for SourceRow {
    fn headers() -> Vec<&'static str> {
        vec!["ID", "NAME"]
    }
    fn row(&self) -> Vec<String> {
        vec![self.id.to_string(), self.name.clone()]
    }
}

/// One row of `datacollection list --profiles`.
#[derive(Debug, Clone, Serialize)]
struct ProfileRow {
    name: String,
    #[serde(rename = "rrdStep")]
    rrd_step: u32,
    #[serde(rename = "storageFlag")]
    storage_flag: String,
    sources: usize,
}

impl TableRow for ProfileRow {
    fn headers() -> Vec<&'static str> {
        vec!["NAME", "RRD STEP", "STORAGE", "SOURCES"]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.rrd_step.to_string(),
            self.storage_flag.clone(),
            self.sources.to_string(),
        ]
    }
}

async fn run_list(api: &DataCollectionApi<'_>, profiles: bool, ctx: &Context) -> Result<()> {
    if profiles {
        let rows: Vec<ProfileRow> = api
            .list_profiles()
            .await?
            .into_iter()
            .map(|p| ProfileRow {
                name: p.name,
                rrd_step: p.rrd_step,
                storage_flag: p.storage_flag,
                sources: p.source_names.len(),
            })
            .collect();
        print!("{}", render_list(&rows, ctx.output_format)?);
    } else {
        // `preflight` doubles as the source listing and maps an absent endpoint
        // to the friendly "server too old" message.
        let mut rows: Vec<SourceRow> = api
            .preflight()
            .await?
            .into_iter()
            .map(|s| SourceRow {
                id: s.id,
                name: s.name,
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        print!("{}", render_list(&rows, ctx.output_format)?);
    }
    Ok(())
}

async fn run_export(api: &DataCollectionApi<'_>, name: &str, format: &str) -> Result<()> {
    let id = resolve_id(api, name).await?;
    let doc = api.download_raw(id, format).await?;
    println!("{}", doc.trim_end());
    Ok(())
}

async fn run_delete(api: &DataCollectionApi<'_>, name: &str) -> Result<()> {
    let id = resolve_id(api, name).await?;
    api.delete_source(id).await?;
    eprintln!("Deleted data-collection source {name:?} (id {id}) and its children");
    Ok(())
}

/// Resolve a source name to its id, surfacing the friendly endpoint-absent
/// message (via `preflight`) and a clear not-found error.
async fn resolve_id(api: &DataCollectionApi<'_>, name: &str) -> Result<i64> {
    api.preflight()
        .await?
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.id)
        .ok_or_else(|| Error::Config(format!("no data-collection source named {name:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_read_and_write() {
        assert_eq!(
            DatacollectionCmd::List { profiles: false }.kind(),
            CmdKind::Read
        );
        assert_eq!(
            DatacollectionCmd::Export {
                name: "Cisco".into(),
                format: "xml".into()
            }
            .kind(),
            CmdKind::Read
        );
        assert_eq!(
            DatacollectionCmd::Delete {
                name: "Cisco".into()
            }
            .kind(),
            CmdKind::Write
        );
    }
}
