/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl requisition node` — imperative escape-hatch verbs for
//! the requisition's node sub-resource.
//!
//! The recommended path is `requisition apply -f <file>` (declarative
//! GitOps); these verbs exist for ad-hoc operator work that doesn't
//! warrant editing YAML and re-applying — quick adds, quick removes,
//! quick label changes. Per task 7.7 every variant's help text
//! cross-references the declarative path.
//!
//! Sub-verbs:
//!   - `list <fs>` — list every foreign-id + label in the requisition
//!   - `get <fs> <foreign-id>` — full server-shape node payload
//!   - `add <fs> <foreign-id> --label <label> [--location <loc>]`
//!   - `set <fs> <foreign-id> [--label <new>] [--location <loc>]`
//!   - `remove <fs> <foreign-id>`
//!
//! `set` reads the existing node, applies the requested mutations to
//! the typed `NodeServer`, and PUTs the full body back. Partial
//! semantics are deliberately rejected: clap requires at least one
//! `--<field>` to be present.

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};
use serde::Serialize;

use crate::api::ProvisioningApi;
use crate::model::server::NodeServer;

/// `onmsctl requisition node ...` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum NodeCmd {
    /// List every node (foreign-id + label) in a requisition.
    ///
    /// **Declarative alternative:** read the YAML the requisition was
    /// applied from (or `onmsctl requisition export <fs>`).
    List {
        /// Foreign-source name to enumerate.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
    },
    /// Print a single node's full server-shape payload.
    ///
    /// **Declarative alternative:** inspect the relevant block in
    /// `spec.nodes[]` of the local YAML, or run `onmsctl requisition
    /// export <fs>` to pull the server's current state into YAML.
    Get {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the node to fetch.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
    },
    /// Add a node to an existing requisition.
    ///
    /// **Warning:** this verb POSTs to `/rest/requisitions/{fs}/nodes`
    /// which Horizon treats as create-or-replace keyed by foreign-id.
    /// Running `add` against an EXISTING foreign-id silently overwrites
    /// the server's interfaces, categories, assets, and meta-data with
    /// the empty bodies this verb constructs. Verify the foreign-id
    /// is new (or use `node get <fs> <foreign-id>` first); a proper
    /// preflight + `--force` flag is deferred.
    ///
    /// **Declarative alternative:** add the entry to `spec.nodes[]`
    /// in the YAML and `requisition apply -f`. Apply will pick up the
    /// new node and trigger an import; this verb skips both the diff
    /// and the import (operator runs `requisition import <fs>` to
    /// take effect).
    Add {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// New node's foreign-id.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// Node label (display name in Horizon).
        #[arg(long, value_parser = super::nonempty_string)]
        label: String,
        /// Optional Minion / geographic location.
        #[arg(long)]
        location: Option<String>,
    },
    /// Mutate fields on an existing node. At least one `--<field>`
    /// must be present.
    ///
    /// **Declarative alternative:** edit the YAML and re-apply.
    /// `apply` will diff the change and re-import; this verb skips
    /// both. Reads the current node, applies the mutations, PUTs the
    /// full body back.
    Set {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the node to mutate.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// New node label.
        #[arg(long)]
        label: Option<String>,
        /// New Minion / geographic location. Pass an empty string to
        /// clear an existing value.
        #[arg(long)]
        location: Option<String>,
    },
    /// Remove a node from the requisition's pending state.
    ///
    /// **Declarative alternative:** delete the entry from
    /// `spec.nodes[]` and `requisition apply -f`.
    Remove {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the node to remove.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
    },
}

impl Classify for NodeCmd {
    fn kind(&self) -> CmdKind {
        match self {
            NodeCmd::List { .. } | NodeCmd::Get { .. } => CmdKind::Read,
            NodeCmd::Add { .. } | NodeCmd::Set { .. } | NodeCmd::Remove { .. } => CmdKind::Write,
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
            NodeCmd::Add {
                fs,
                foreign_id,
                label,
                location,
            } => run_add(&api, &fs, &foreign_id, &label, location.as_deref(), ctx).await,
            NodeCmd::Set {
                fs,
                foreign_id,
                label,
                location,
            } => {
                run_set(
                    &api,
                    &fs,
                    &foreign_id,
                    label.as_deref(),
                    location.as_deref(),
                    ctx,
                )
                .await
            }
            NodeCmd::Remove { fs, foreign_id } => run_remove(&api, &fs, &foreign_id, ctx).await,
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

async fn run_add(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    label: &str,
    location: Option<&str>,
    ctx: &Context,
) -> Result<()> {
    let node = NodeServer {
        foreign_id: foreign_id.to_string(),
        node_label: label.to_string(),
        location: location.map(String::from),
        building: None,
        city: None,
        parent_foreign_source: None,
        parent_foreign_id: None,
        parent_node_label: None,
        interface: vec![],
        category: vec![],
        asset: vec![],
        meta_data: vec![],
    };
    api.post_requisition_node(fs, &node).await?;
    emit_action_outcome(fs, foreign_id, "added", ctx)
}

async fn run_set(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    new_label: Option<&str>,
    new_location: Option<&str>,
    ctx: &Context,
) -> Result<()> {
    if new_label.is_none() && new_location.is_none() {
        return Err(Error::Config(
            "no mutations specified; pass --label and/or --location".into(),
        ));
    }
    let mut node = api
        .get_requisition_node(fs, foreign_id)
        .await?
        .ok_or_else(|| {
            Error::Config(format!(
                "no node '{foreign_id}' in requisition '{fs}' to mutate"
            ))
        })?;
    if let Some(label) = new_label {
        node.node_label = label.to_string();
    }
    if let Some(loc) = new_location {
        // Empty string clears the field — matches the doc-comment
        // contract on the `--location` flag.
        node.location = if loc.is_empty() {
            None
        } else {
            Some(loc.to_string())
        };
    }
    api.put_requisition_node(fs, foreign_id, &node).await?;
    emit_action_outcome(fs, foreign_id, "updated", ctx)
}

async fn run_remove(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ctx: &Context,
) -> Result<()> {
    api.delete_requisition_node(fs, foreign_id).await?;
    emit_action_outcome(fs, foreign_id, "removed", ctx)
}

fn emit_action_outcome(fs: &str, foreign_id: &str, action: &str, ctx: &Context) -> Result<()> {
    let payload = serde_json::json!({
        "foreign_source": fs,
        "foreign_id": foreign_id,
        "action": action,
    });
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&payload)
                .map_err(|e| Error::Config(format!("serializing node action to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&payload)
                .map_err(|e| Error::Config(format!("serializing node action to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let line = format!("Requisition/{fs} node/{foreign_id}: {action}\n");
            super::write_stdout(line.as_bytes())?;
        }
    }
    Ok(())
}
