/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl requisition service` — imperative escape-hatch verbs for
//! the requisition's monitored-service sub-resource (Group 7 phase 3,
//! task 7.3).
//!
//! Services are scoped within an interface: every verb takes
//! `<fs> <foreign-id> <ip>` before the service name.
//!
//! Verb coverage per design.md §D8: `list / add / remove` only
//! (no `set`, no `get`). Services on the wire carry just a
//! `service-name`, `category[]`, and `meta-data[]` — there's
//! nothing meaningfully mutable beyond delete-and-re-add, and `get`
//! adds no information `list` doesn't.

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};
use serde::Serialize;

use crate::api::ProvisioningApi;
use crate::model::server::MonitoredServiceServer;

/// `onmsctl requisition service ...` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum ServiceCmd {
    /// List every service on a given interface (projected from the
    /// interface's existing GET — no new endpoint hit).
    ///
    /// **Declarative alternative:** read the
    /// `spec.nodes[].interfaces[].services` block from the local YAML,
    /// or `onmsctl requisition export <fs>` for the server's current
    /// state.
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
    /// Add a service to an existing interface.
    ///
    /// **Warning:** this verb POSTs to the services collection
    /// endpoint which Horizon treats as create-or-replace keyed by
    /// `service-name`. Two cases:
    ///
    /// - **New service-name**: created with empty `category` and
    ///   `meta-data` arrays (populate later via `apply -f`).
    /// - **Existing service-name**: those two arrays are silently
    ///   replaced with the empty body this verb sends, wiping any
    ///   server-side curation.
    ///
    /// A proper preflight-GET + `--force` flag is deferred. The hazard
    /// scope is small (services carry only those two collections), but
    /// real.
    ///
    /// **Declarative alternative:** add the service to
    /// `spec.nodes[].interfaces[].services` in the YAML and
    /// `requisition apply -f`. Apply will diff the change and
    /// re-import; this verb skips both (operator runs `requisition
    /// import <fs>` to take effect).
    Add {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// IP address of the parent interface.
        #[arg(value_parser = super::ip_addr)]
        ip: String,
        /// Service name to add (e.g. `HTTP`, `SNMP`, `ICMP`). Allowed
        /// characters: ASCII alphanumeric, `.`, `_`, `-`.
        #[arg(value_parser = service_name)]
        service: String,
    },
    /// Remove a service from the interface's pending state.
    ///
    /// **Declarative alternative:** delete the entry from
    /// `spec.nodes[].interfaces[].services` and `requisition apply -f`.
    Remove {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// IP address of the parent interface.
        #[arg(value_parser = super::ip_addr)]
        ip: String,
        /// Service name to remove. Allowed characters: ASCII
        /// alphanumeric, `.`, `_`, `-`.
        #[arg(value_parser = service_name)]
        service: String,
    },
}

impl Classify for ServiceCmd {
    fn kind(&self) -> CmdKind {
        match self {
            ServiceCmd::List { .. } => CmdKind::Read,
            ServiceCmd::Add { .. } | ServiceCmd::Remove { .. } => CmdKind::Write,
        }
    }
}

impl ServiceCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        match self {
            ServiceCmd::List {
                fs,
                foreign_id,
                ip,
            } => run_list(&api, &fs, &foreign_id, &ip, ctx).await,
            ServiceCmd::Add {
                fs,
                foreign_id,
                ip,
                service,
            } => run_add(&api, &fs, &foreign_id, &ip, &service, ctx).await,
            ServiceCmd::Remove {
                fs,
                foreign_id,
                ip,
                service,
            } => run_remove(&api, &fs, &foreign_id, &ip, &service, ctx).await,
        }
    }
}

/// Compact list-output row.
#[derive(Debug, Clone, Serialize)]
struct ServiceRow {
    service_name: String,
    /// Server-side category count. Surfaced so JSON / YAML consumers
    /// can detect that the service carries non-empty curation that
    /// `add` (sent as empty arrays) would clobber on overwrite.
    category_count: usize,
    /// Server-side meta-data count. Same rationale as `category_count`.
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

async fn run_add(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ip: &str,
    service: &str,
    ctx: &Context,
) -> Result<()> {
    let svc = MonitoredServiceServer {
        service_name: service.to_string(),
        category: vec![],
        meta_data: vec![],
    };
    api.post_requisition_service(fs, foreign_id, ip, &svc).await?;
    emit_action_outcome(fs, foreign_id, ip, service, "added", ctx)
}

async fn run_remove(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ip: &str,
    service: &str,
    ctx: &Context,
) -> Result<()> {
    api.delete_requisition_service(fs, foreign_id, ip, service)
        .await?;
    emit_action_outcome(fs, foreign_id, ip, service, "removed", ctx)
}

fn emit_action_outcome(
    fs: &str,
    foreign_id: &str,
    ip: &str,
    service: &str,
    action: &str,
    ctx: &Context,
) -> Result<()> {
    let payload = serde_json::json!({
        "foreign_source": fs,
        "foreign_id": foreign_id,
        "ip": ip,
        "service": service,
        "action": action,
    });
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&payload).map_err(|e| {
                Error::Config(format!("serializing service action to JSON: {e}"))
            })?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&payload).map_err(|e| {
                Error::Config(format!("serializing service action to YAML: {e}"))
            })?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let line = format!(
                "Requisition/{fs} node/{foreign_id} interface/{ip} service/{service}: {action}\n"
            );
            super::write_stdout(line.as_bytes())?;
        }
    }
    Ok(())
}

/// clap value parser for service-name positionals. Whitelists ASCII
/// alphanumeric plus `.`, `_`, `-` — the documented character set for
/// monitored-service names. Rejects path-traversal (`/`, `..`),
/// shell-metacharacters, and embedded whitespace at parse time so the
/// path-segment encoder doesn't have to canonicalize away surprises.
fn service_name(s: &str) -> std::result::Result<String, String> {
    if s.is_empty() {
        return Err("service-name must not be empty".into());
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(format!(
            "service-name {s:?} contains disallowed characters \
             (allowed: ASCII alphanumeric, '.', '_', '-')"
        ));
    }
    // Require at least one alphanumeric so values like `.`, `..`, or
    // `--` (which pass the whitelist) can't sneak through as path
    // segments.
    if !s.bytes().any(|b| b.is_ascii_alphanumeric()) {
        return Err(format!(
            "service-name {s:?} must contain at least one alphanumeric character"
        ));
    }
    Ok(s.to_string())
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

    #[test]
    fn classify_add_and_remove_are_write() {
        let add = ServiceCmd::Add {
            fs: "acme".into(),
            foreign_id: "web01".into(),
            ip: "10.0.0.1".into(),
            service: "HTTP".into(),
        };
        let remove = ServiceCmd::Remove {
            fs: "acme".into(),
            foreign_id: "web01".into(),
            ip: "10.0.0.1".into(),
            service: "HTTP".into(),
        };
        assert_eq!(add.kind(), CmdKind::Write);
        assert_eq!(remove.kind(), CmdKind::Write);
    }

    #[test]
    fn service_name_accepts_canonical_horizon_services() {
        assert_eq!(service_name("HTTP").unwrap(), "HTTP");
        assert_eq!(service_name("SNMP").unwrap(), "SNMP");
        assert_eq!(service_name("ICMP").unwrap(), "ICMP");
        assert_eq!(service_name("Postgres-9").unwrap(), "Postgres-9");
        assert_eq!(service_name("HTTP_v2").unwrap(), "HTTP_v2");
        assert_eq!(service_name("svc.local").unwrap(), "svc.local");
    }

    #[test]
    fn service_name_rejects_path_traversal_and_specials() {
        assert!(service_name("").is_err());
        assert!(service_name("..").is_err());
        assert!(service_name("HTTP/foo").is_err());
        assert!(service_name("HTTP foo").is_err());
        assert!(service_name("HTTP\x00").is_err());
        assert!(service_name("HTTP;rm").is_err());
    }
}
