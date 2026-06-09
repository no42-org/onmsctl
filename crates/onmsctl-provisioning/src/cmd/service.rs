/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl requisition service` — read-only inspection verb for the
//! requisition's monitored-service sub-resource.
//!
//! Services are scoped within an interface: the verb takes
//! `<fs> <foreign-id> <ip>`. It issues only `GET` requests. Mutation
//! is declarative: edit the `kind: Requisition` YAML and run `onmsctl
//! apply -f <file>`.

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};
use serde::Serialize;

use crate::api::ProvisioningApi;

/// `onmsctl requisition service ...` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum ServiceCmd {
    /// List every service on a given interface (projected from the
    /// interface's existing GET — no new endpoint hit).
    ///
    /// **Declarative alternative:** read the
    /// `spec.nodes[].interfaces[].services` block from the local YAML,
    /// or `onmsctl requisition export <fs>` for the server's current
    /// state. To change services, edit the YAML and run `onmsctl
    /// apply -f <file>`.
    List {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// IP address of the parent interface (IPv4 or IPv6 literal,
        /// no brackets).
        #[arg(value_parser = super::ip_addr)]
        ip: String,
    },
}

impl Classify for ServiceCmd {
    fn kind(&self) -> CmdKind {
        match self {
            ServiceCmd::List { .. } => CmdKind::Read,
        }
    }
}

impl ServiceCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        match self {
            ServiceCmd::List { fs, foreign_id, ip } => {
                run_list(&api, &fs, &foreign_id, &ip, ctx).await
            }
        }
    }
}

/// Compact list-output row.
#[derive(Debug, Clone, Serialize)]
struct ServiceRow {
    service_name: String,
    /// Server-side category count.
    category_count: usize,
    /// Server-side meta-data count.
    meta_data_count: usize,
}

async fn run_list(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ip: &str,
    ctx: &Context,
) -> Result<()> {
    // Deliberate: project from the interface's existing GET rather
    // than calling a per-service collection endpoint. Services are
    // embedded in the interface payload, so one round-trip is cheaper
    // than N. Tradeoff: a concurrent mutation between this GET and a
    // sibling per-service verb can surface a stale snapshot — fine
    // for a read-only listing.
    let iface = api
        .get_requisition_interface(fs, foreign_id, ip)
        .await?
        .ok_or_else(|| {
            // The interface GET 404s when ANY of {requisition, node,
            // interface} is missing — Horizon doesn't distinguish.
            // Name all three so the operator can spot the typo.
            Error::Config(format!(
                "GET returned 404 — one of requisition '{fs}', node '{foreign_id}', or interface '{ip}' does not exist"
            ))
        })?;
    let rows: Vec<ServiceRow> = iface
        .monitored_service
        .iter()
        .map(|s| ServiceRow {
            service_name: s.service_name.clone(),
            category_count: s.category.len(),
            meta_data_count: s.meta_data.len(),
        })
        .collect();

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&rows)
                .map_err(|e| Error::Config(format!("serializing service list to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&rows)
                .map_err(|e| Error::Config(format!("serializing service list to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            if rows.is_empty() {
                super::write_stdout(b"(no services)\n")?;
            } else {
                for r in &rows {
                    let extras = if r.category_count > 0 || r.meta_data_count > 0 {
                        format!(
                            "  categories={} meta-data={}",
                            r.category_count, r.meta_data_count
                        )
                    } else {
                        String::new()
                    };
                    let line = format!("{}{extras}\n", r.service_name);
                    super::write_stdout(line.as_bytes())?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_list_is_read() {
        let list = ServiceCmd::List {
            fs: "acme".into(),
            foreign_id: "web01".into(),
            ip: "10.0.0.1".into(),
        };
        assert_eq!(list.kind(), CmdKind::Read);
    }
}
