/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl requisition interface` — imperative escape-hatch verbs
//! for the requisition's interface sub-resource (Group 7 phase 2,
//! task 7.2).
//!
//! Interfaces are scoped within a node: every verb takes both an
//! `<fs>` (foreign-source) and a `<foreign-id>` argument before the
//! IP address.
//!
//! The recommended path is `requisition apply -f <file>` (declarative
//! GitOps); these verbs exist for ad-hoc operator work that doesn't
//! warrant editing YAML and re-applying. Per task 7.7 every variant's
//! help text cross-references the declarative path. Compared to
//! `NodeCmd`, `set` here can mutate three wire-only fields (`snmp-
//! primary`, `status`, `descr`) which are NOT modeled in the local
//! YAML — so this is the one place imperative verbs offer functionality
//! the declarative path doesn't.

use std::net::IpAddr;
use std::str::FromStr;

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};
use serde::Serialize;

use crate::api::ProvisioningApi;
use crate::model::server::InterfaceServer;

/// `onmsctl requisition interface ...` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum InterfaceCmd {
    /// List every interface (IP + snmp-primary) on a given node.
    ///
    /// **Declarative alternative:** read the `spec.nodes[].interfaces`
    /// block from the local YAML, or `onmsctl requisition export
    /// <fs>` for the server's current state.
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
    /// into YAML.
    Get {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// IP address to fetch (IPv4 or IPv6 literal, no brackets).
        #[arg(value_parser = ip_addr)]
        ip: String,
    },
    /// Add an interface to an existing node.
    ///
    /// **Warning:** this verb POSTs to the interfaces collection
    /// endpoint which Horizon treats as create-or-replace keyed by
    /// `ip-addr`. Running `add` against an EXISTING IP silently
    /// overwrites every wire field on that interface — including
    /// `snmp-primary`, `status`, `descr`, monitored-services,
    /// categories, and meta-data — with the (mostly empty) body this
    /// verb constructs. Verify the IP is new (or use `interface get`
    /// first); a proper preflight + `--force` flag is deferred.
    ///
    /// **Declarative alternative:** add the entry to
    /// `spec.nodes[].interfaces` in the YAML and `requisition apply
    /// -f`. Apply will diff the change and re-import; this verb skips
    /// both (operator runs `requisition import <fs>` to take effect).
    Add {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// IP address (IPv4 or IPv6 literal, no brackets).
        #[arg(value_parser = ip_addr)]
        ip: String,
        /// SNMP primary marker — required. `P` (primary), `S`
        /// (secondary), or `N` (not eligible). Case-insensitive.
        #[arg(long, value_parser = snmp_primary)]
        snmp_primary: String,
        /// Status code: `1` = managed, `3` = unmanaged.
        #[arg(long, value_parser = status_code)]
        status: Option<i32>,
        /// Human-readable description.
        #[arg(long)]
        descr: Option<String>,
    },
    /// Mutate fields on an existing interface. At least one
    /// `--<field>` must be present.
    ///
    /// Three fields are mutable here that the LOCAL YAML doesn't
    /// model: `--status`, `--descr`, plus `--snmp-primary` which IS
    /// modeled. Operators editing those wire-only fields hit a
    /// genuine declarative-side gap; for the modeled fields the
    /// declarative path is preferred.
    ///
    /// **Declarative alternative for modeled fields:** edit
    /// `spec.nodes[].interfaces[].snmpPrimary` in the YAML and
    /// re-apply.
    Set {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// IP address (IPv4 or IPv6 literal, no brackets).
        #[arg(value_parser = ip_addr)]
        ip: String,
        /// New SNMP primary marker (`P`/`S`/`N`, case-insensitive).
        #[arg(long, value_parser = snmp_primary)]
        snmp_primary: Option<String>,
        /// New status code: `1` = managed, `3` = unmanaged.
        #[arg(long, value_parser = status_code)]
        status: Option<i32>,
        /// New description. Pass an empty string to clear an existing
        /// value.
        #[arg(long)]
        descr: Option<String>,
    },
    /// Remove an interface from the node's pending state.
    ///
    /// **Declarative alternative:** delete the entry from
    /// `spec.nodes[].interfaces` and `requisition apply -f`.
    Remove {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// IP address to remove (IPv4 or IPv6 literal, no brackets).
        #[arg(value_parser = ip_addr)]
        ip: String,
    },
}

impl Classify for InterfaceCmd {
    fn kind(&self) -> CmdKind {
        match self {
            InterfaceCmd::List { .. } | InterfaceCmd::Get { .. } => CmdKind::Read,
            InterfaceCmd::Add { .. } | InterfaceCmd::Set { .. } | InterfaceCmd::Remove { .. } => {
                CmdKind::Write
            }
        }
    }
}

impl InterfaceCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        match self {
            InterfaceCmd::List { fs, foreign_id } => run_list(&api, &fs, &foreign_id, ctx).await,
            InterfaceCmd::Get {
                fs,
                foreign_id,
                ip,
            } => run_get(&api, &fs, &foreign_id, &ip, ctx).await,
            InterfaceCmd::Add {
                fs,
                foreign_id,
                ip,
                snmp_primary,
                status,
                descr,
            } => {
                run_add(
                    &api,
                    &fs,
                    &foreign_id,
                    &ip,
                    &snmp_primary,
                    status,
                    descr.as_deref(),
                    ctx,
                )
                .await
            }
            InterfaceCmd::Set {
                fs,
                foreign_id,
                ip,
                snmp_primary,
                status,
                descr,
            } => {
                run_set(
                    &api,
                    &fs,
                    &foreign_id,
                    &ip,
                    snmp_primary.as_deref(),
                    status,
                    descr.as_deref(),
                    ctx,
                )
                .await
            }
            InterfaceCmd::Remove {
                fs,
                foreign_id,
                ip,
            } => run_remove(&api, &fs, &foreign_id, &ip, ctx).await,
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
                    let status = r
                        .status
                        .map(|s| format!(" status={s}"))
                        .unwrap_or_default();
                    let line =
                        format!("{}  snmp-primary={}{status}\n", r.ip, r.snmp_primary);
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

#[allow(clippy::too_many_arguments)]
async fn run_add(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ip: &str,
    snmp_primary: &str,
    status: Option<i32>,
    descr: Option<&str>,
    ctx: &Context,
) -> Result<()> {
    let iface = InterfaceServer {
        ip_addr: ip.to_string(),
        snmp_primary: snmp_primary.to_string(),
        status,
        managed: None,
        descr: descr.map(String::from),
        monitored_service: vec![],
        category: vec![],
        meta_data: vec![],
    };
    api.post_requisition_interface(fs, foreign_id, &iface).await?;
    emit_action_outcome(fs, foreign_id, ip, "added", ctx)
}

#[allow(clippy::too_many_arguments)]
async fn run_set(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ip: &str,
    new_snmp_primary: Option<&str>,
    new_status: Option<i32>,
    new_descr: Option<&str>,
    ctx: &Context,
) -> Result<()> {
    if new_snmp_primary.is_none() && new_status.is_none() && new_descr.is_none() {
        return Err(Error::Config(
            "no mutations specified; pass --snmp-primary, --status, and/or --descr".into(),
        ));
    }
    let mut iface = api
        .get_requisition_interface(fs, foreign_id, ip)
        .await?
        .ok_or_else(|| {
            Error::Config(format!(
                "no interface '{ip}' on node '{foreign_id}' in requisition '{fs}' to mutate"
            ))
        })?;
    if let Some(s) = new_snmp_primary {
        iface.snmp_primary = s.to_string();
    }
    if let Some(s) = new_status {
        iface.status = Some(s);
    }
    if let Some(d) = new_descr {
        iface.descr = if d.is_empty() {
            None
        } else {
            Some(d.to_string())
        };
    }
    api.put_requisition_interface(fs, foreign_id, ip, &iface).await?;
    emit_action_outcome(fs, foreign_id, ip, "updated", ctx)
}

async fn run_remove(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ip: &str,
    ctx: &Context,
) -> Result<()> {
    api.delete_requisition_interface(fs, foreign_id, ip).await?;
    emit_action_outcome(fs, foreign_id, ip, "removed", ctx)
}

fn emit_action_outcome(
    fs: &str,
    foreign_id: &str,
    ip: &str,
    action: &str,
    ctx: &Context,
) -> Result<()> {
    let payload = serde_json::json!({
        "foreign_source": fs,
        "foreign_id": foreign_id,
        "ip": ip,
        "action": action,
    });
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&payload).map_err(|e| {
                Error::Config(format!("serializing interface action to JSON: {e}"))
            })?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&payload).map_err(|e| {
                Error::Config(format!("serializing interface action to YAML: {e}"))
            })?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let line =
                format!("Requisition/{fs} node/{foreign_id} interface/{ip}: {action}\n");
            super::write_stdout(line.as_bytes())?;
        }
    }
    Ok(())
}

/// clap value parser for `--snmp-primary`. Accepts `P` / `S` / `N`
/// case-insensitively and normalizes to upper-case so the wire payload
/// matches the local model's canonical form.
fn snmp_primary(s: &str) -> std::result::Result<String, String> {
    match s.to_ascii_uppercase().as_str() {
        v @ ("P" | "S" | "N") => Ok(v.to_string()),
        _ => Err(format!(
            "snmp-primary must be one of P, S, N (got {s:?})"
        )),
    }
}

/// clap value parser for `--status`. Horizon documents `1` (managed)
/// and `3` (unmanaged); other values are rejected at parse time rather
/// than reaching the server as garbage.
fn status_code(s: &str) -> std::result::Result<i32, String> {
    match s.parse::<i32>() {
        Ok(v @ (1 | 3)) => Ok(v),
        Ok(other) => Err(format!(
            "status must be 1 (managed) or 3 (unmanaged); got {other}"
        )),
        Err(_) => Err(format!("status must be an integer; got {s:?}")),
    }
}

/// clap value parser for IP-address positionals. Accepts IPv4 and
/// IPv6 literals (no brackets). Rejects typos and surrounding
/// whitespace at parse time so the user sees a clean usage error
/// instead of a 400/404 against a malformed URL.
fn ip_addr(s: &str) -> std::result::Result<String, String> {
    IpAddr::from_str(s)
        .map(|ip| ip.to_string())
        .map_err(|_| format!("invalid IP address {s:?} (expected IPv4 or IPv6 literal)"))
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

    #[test]
    fn classify_add_set_remove_are_write() {
        let add = InterfaceCmd::Add {
            fs: "acme".into(),
            foreign_id: "web01".into(),
            ip: "10.0.0.1".into(),
            snmp_primary: "P".into(),
            status: None,
            descr: None,
        };
        let set = InterfaceCmd::Set {
            fs: "acme".into(),
            foreign_id: "web01".into(),
            ip: "10.0.0.1".into(),
            snmp_primary: None,
            status: None,
            descr: None,
        };
        let remove = InterfaceCmd::Remove {
            fs: "acme".into(),
            foreign_id: "web01".into(),
            ip: "10.0.0.1".into(),
        };
        assert_eq!(add.kind(), CmdKind::Write);
        assert_eq!(set.kind(), CmdKind::Write);
        assert_eq!(remove.kind(), CmdKind::Write);
    }

    #[test]
    fn snmp_primary_accepts_canonical_uppercase() {
        assert_eq!(snmp_primary("P").unwrap(), "P");
        assert_eq!(snmp_primary("S").unwrap(), "S");
        assert_eq!(snmp_primary("N").unwrap(), "N");
    }

    #[test]
    fn snmp_primary_normalizes_lowercase_to_uppercase() {
        assert_eq!(snmp_primary("p").unwrap(), "P");
        assert_eq!(snmp_primary("s").unwrap(), "S");
        assert_eq!(snmp_primary("n").unwrap(), "N");
    }

    #[test]
    fn snmp_primary_rejects_unknown_values() {
        assert!(snmp_primary("X").is_err());
        assert!(snmp_primary("").is_err());
        assert!(snmp_primary("PS").is_err());
    }

    #[test]
    fn status_code_accepts_documented_values() {
        assert_eq!(status_code("1").unwrap(), 1);
        assert_eq!(status_code("3").unwrap(), 3);
    }

    #[test]
    fn status_code_rejects_unknown_and_non_integer() {
        assert!(status_code("2").is_err());
        assert!(status_code("-1").is_err());
        assert!(status_code("abc").is_err());
    }

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
}
