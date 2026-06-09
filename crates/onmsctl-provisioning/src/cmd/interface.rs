/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl requisition interface` — read-only inspection verbs for
//! the requisition's interface sub-resource.
//!
//! Interfaces are scoped within a node: every verb takes both an
//! `<fs>` (foreign-source) and a `<foreign-id>` argument before the
//! IP address. These verbs issue only `GET` requests. Mutation is
//! declarative: edit the `kind: Requisition` YAML and run `onmsctl
//! apply -f <file>`.

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};
use serde::Serialize;

use crate::api::ProvisioningApi;

/// `onmsctl requisition interface ...` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum InterfaceCmd {
    /// List every interface (IP + snmp-primary) on a given node.
    ///
    /// **Declarative alternative:** read the `spec.nodes[].interfaces`
    /// block from the local YAML, or `onmsctl requisition export
    /// <fs>` for the server's current state. To change interfaces,
    /// edit the YAML and run `onmsctl apply -f <file>`.
    List {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
    },
    /// Print a single interface's full server-shape payload.
    ///
    /// **Declarative alternative:** inspect the relevant entry in
    /// `spec.nodes[].interfaces` of the local YAML, or run `onmsctl
    /// requisition export <fs>` to pull the server's current state
    /// into YAML. To change the interface, edit the YAML and run
    /// `onmsctl apply -f <file>`.
    Get {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// IP address to fetch (IPv4 or IPv6 literal, no brackets).
        #[arg(value_parser = super::ip_addr)]
        ip: String,
    },
}

impl Classify for InterfaceCmd {
    fn kind(&self) -> CmdKind {
        match self {
            InterfaceCmd::List { .. } | InterfaceCmd::Get { .. } => CmdKind::Read,
        }
    }
}

impl InterfaceCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        match self {
            InterfaceCmd::List { fs, foreign_id } => run_list(&api, &fs, &foreign_id, ctx).await,
            InterfaceCmd::Get { fs, foreign_id, ip } => {
                run_get(&api, &fs, &foreign_id, &ip, ctx).await
            }
        }
    }
}

/// Compact list-output row.
#[derive(Debug, Clone, Serialize)]
struct InterfaceRow {
    ip: String,
    snmp_primary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<i32>,
}

async fn run_list(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ctx: &Context,
) -> Result<()> {
    // Deliberate: project from the node's existing GET rather than
    // hitting a per-interface collection endpoint. The interfaces are
    // embedded in the node payload, so one round-trip is cheaper than
    // N. Tradeoff: a concurrent mutation between this GET and a
    // sibling per-interface verb can surface a stale snapshot — fine
    // for a read-only listing.
    let node = api
        .get_requisition_node(fs, foreign_id)
        .await?
        .ok_or_else(|| {
            Error::Config(format!(
                "no node '{foreign_id}' in requisition '{fs}' (GET returned 404)"
            ))
        })?;
    let rows: Vec<InterfaceRow> = node
        .interface
        .iter()
        .map(|i| InterfaceRow {
            ip: i.ip_addr.clone(),
            snmp_primary: i.snmp_primary.clone(),
            status: i.status,
        })
        .collect();

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&rows)
                .map_err(|e| Error::Config(format!("serializing interface list to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&rows)
                .map_err(|e| Error::Config(format!("serializing interface list to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            if rows.is_empty() {
                super::write_stdout(b"(no interfaces)\n")?;
            } else {
                for r in &rows {
                    let status = r.status.map(|s| format!(" status={s}")).unwrap_or_default();
                    let line = format!("{}  snmp-primary={}{status}\n", r.ip, r.snmp_primary);
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
    ip: &str,
    ctx: &Context,
) -> Result<()> {
    let iface = api
        .get_requisition_interface(fs, foreign_id, ip)
        .await?
        .ok_or_else(|| {
            Error::Config(format!(
                "no interface '{ip}' on node '{foreign_id}' in requisition '{fs}' (GET returned 404)"
            ))
        })?;

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&iface)
                .map_err(|e| Error::Config(format!("serializing interface to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml | OutputFormat::Table => {
            let yaml = serde_norway::to_string(&iface)
                .map_err(|e| Error::Config(format!("serializing interface to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_list_and_get_are_read() {
        let list = InterfaceCmd::List {
            fs: "acme".into(),
            foreign_id: "web01".into(),
        };
        let get = InterfaceCmd::Get {
            fs: "acme".into(),
            foreign_id: "web01".into(),
            ip: "10.0.0.1".into(),
        };
        assert_eq!(list.kind(), CmdKind::Read);
        assert_eq!(get.kind(), CmdKind::Read);
    }
}
