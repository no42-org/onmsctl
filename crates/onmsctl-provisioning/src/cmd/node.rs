/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl requisition node` — read-only inspection verbs for the
//! requisition's node sub-resource.
//!
//! These verbs surface the server's current view of a requisition's
//! nodes; they issue only `GET` requests. Mutation is declarative:
//! edit the `kind: Requisition` YAML and run `onmsctl apply -f
//! <file>`.
//!
//! Sub-verbs:
//!   - `list <fs>` — list every foreign-id + label in the requisition
//!   - `get <fs> <foreign-id>` — full server-shape node payload

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};
use serde::Serialize;

use crate::api::ProvisioningApi;

/// `onmsctl requisition node ...` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum NodeCmd {
    /// List every node (foreign-id + label) in a requisition.
    ///
    /// **Declarative alternative:** read the YAML the requisition was
    /// applied from (or `onmsctl requisition export <fs>`). To change
    /// nodes, edit the YAML and run `onmsctl apply -f <file>`.
    List {
        /// Foreign-source name to enumerate.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
    },
    /// Print a single node's full server-shape payload.
    ///
    /// **Declarative alternative:** inspect the relevant block in
    /// `spec.nodes[]` of the local YAML, or run `onmsctl requisition
    /// export <fs>` to pull the server's current state into YAML. To
    /// change the node, edit the YAML and run `onmsctl apply -f
    /// <file>`.
    Get {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the node to fetch.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
    },
}

impl Classify for NodeCmd {
    fn kind(&self) -> CmdKind {
        match self {
            NodeCmd::List { .. } | NodeCmd::Get { .. } => CmdKind::Read,
        }
    }
}

impl NodeCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        match self {
            NodeCmd::List { fs } => run_list(&api, &fs, ctx).await,
            NodeCmd::Get { fs, foreign_id } => run_get(&api, &fs, &foreign_id, ctx).await,
        }
    }
}

/// Compact list-output row.
#[derive(Debug, Clone, Serialize)]
struct NodeRow {
    foreign_id: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
}

async fn run_list(api: &ProvisioningApi<'_>, fs: &str, ctx: &Context) -> Result<()> {
    let req = api.get_requisition(fs).await?.ok_or_else(|| {
        Error::Config(format!(
            "no requisition '{fs}' on the server (GET /rest/requisitions/{fs} returned 404)"
        ))
    })?;
    let rows: Vec<NodeRow> = req
        .node
        .iter()
        .map(|n| NodeRow {
            foreign_id: n.foreign_id.clone(),
            label: n.node_label.clone(),
            location: n.location.clone(),
        })
        .collect();

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&rows)
                .map_err(|e| Error::Config(format!("serializing node list to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&rows)
                .map_err(|e| Error::Config(format!("serializing node list to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            if rows.is_empty() {
                super::write_stdout(b"(no nodes)\n")?;
            } else {
                for r in &rows {
                    let loc = r
                        .location
                        .as_deref()
                        .map(|l| format!(" location={l}"))
                        .unwrap_or_default();
                    let line = format!("{}  {}{loc}\n", r.foreign_id, r.label);
                    super::write_stdout(line.as_bytes())?;
                }
            }
        }
    }
    Ok(())
}

async fn run_get(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ctx: &Context,
) -> Result<()> {
    let node = api
        .get_requisition_node(fs, foreign_id)
        .await?
        .ok_or_else(|| {
            Error::Config(format!(
                "no node '{foreign_id}' in requisition '{fs}' (GET returned 404)"
            ))
        })?;

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&node)
                .map_err(|e| Error::Config(format!("serializing node to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml | OutputFormat::Table => {
            let yaml = serde_norway::to_string(&node)
                .map_err(|e| Error::Config(format!("serializing node to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
    }
    Ok(())
}
