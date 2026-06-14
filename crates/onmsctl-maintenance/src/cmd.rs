/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `onmsctl maintenance` subcommand surface.
//!
//! `list` snapshots the deployed scheduled outages; `status` reports whether
//! given devices are currently in any outage (the `*InOutage` checks); `delete`
//! tears a window down fully (`DELETE /rest/sched-outages/{name}` — definition +
//! every attachment). `list`/`status` are Read; `delete` is Write.

use std::net::IpAddr;

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, Result, TableRow, render_list};
use serde::Serialize;

use crate::api::MaintenanceApi;
use crate::server::Outage;

/// `onmsctl maintenance …` verbs.
#[derive(Subcommand, Debug, Clone)]
pub enum MaintenanceCmd {
    /// List the deployed scheduled-outage maintenance windows.
    List,
    /// Report whether one or more devices (IP or nodeId) are currently in a
    /// maintenance window.
    Status {
        /// IP addresses and/or numeric nodeIds to check.
        #[arg(required = true, value_name = "DEVICE")]
        devices: Vec<String>,
    },
    /// Delete a maintenance window — removes it from every daemon and deletes
    /// the definition (full teardown).
    Delete {
        /// The window name (`metadata.name`).
        #[arg(value_name = "NAME")]
        name: String,
    },
}

impl Classify for MaintenanceCmd {
    fn kind(&self) -> CmdKind {
        match self {
            MaintenanceCmd::List | MaintenanceCmd::Status { .. } => CmdKind::Read,
            MaintenanceCmd::Delete { .. } => CmdKind::Write,
        }
    }
}

impl MaintenanceCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = MaintenanceApi::new(&client);
        match self {
            MaintenanceCmd::List => run_list(&api, ctx).await,
            MaintenanceCmd::Status { devices } => run_status(&api, &devices, ctx).await,
            MaintenanceCmd::Delete { name } => run_delete(&api, &name).await,
        }
    }
}

/// One row of `maintenance list`.
#[derive(Debug, Clone, Serialize)]
struct WindowRow {
    name: String,
    #[serde(rename = "type")]
    schedule_type: String,
    times: usize,
    interfaces: usize,
    nodes: usize,
}

impl From<&Outage> for WindowRow {
    fn from(o: &Outage) -> Self {
        Self {
            name: o.name.clone(),
            schedule_type: o.schedule_type.clone(),
            times: o.time.len(),
            interfaces: o.interface.len(),
            nodes: o.node.len(),
        }
    }
}

impl TableRow for WindowRow {
    fn headers() -> Vec<&'static str> {
        vec!["NAME", "TYPE", "TIMES", "INTERFACES", "NODES"]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.schedule_type.clone(),
            self.times.to_string(),
            self.interfaces.to_string(),
            self.nodes.to_string(),
        ]
    }
}

async fn run_list(api: &MaintenanceApi<'_>, ctx: &Context) -> Result<()> {
    let outages = api.list().await?;
    let rows: Vec<WindowRow> = outages.outage.iter().map(WindowRow::from).collect();
    print!("{}", render_list(&rows, ctx.output_format)?);
    Ok(())
}

/// One `maintenance status` result.
#[derive(Debug, Clone, Serialize)]
struct StatusRow {
    device: String,
    #[serde(rename = "inOutage")]
    in_outage: bool,
}

impl TableRow for StatusRow {
    fn headers() -> Vec<&'static str> {
        vec!["DEVICE", "IN OUTAGE"]
    }
    fn row(&self) -> Vec<String> {
        vec![self.device.clone(), self.in_outage.to_string()]
    }
}

async fn run_status(api: &MaintenanceApi<'_>, devices: &[String], ctx: &Context) -> Result<()> {
    let mut rows = Vec::with_capacity(devices.len());
    for d in devices {
        let in_outage = if d.parse::<IpAddr>().is_ok() {
            api.interface_in_outage(d).await?
        } else if let Ok(id) = d.parse::<i64>() {
            api.node_in_outage(id).await?
        } else {
            return Err(Error::Config(format!(
                "device {d:?} is neither an IP address nor a numeric nodeId"
            )));
        };
        rows.push(StatusRow {
            device: d.clone(),
            in_outage,
        });
    }
    print!("{}", render_list(&rows, ctx.output_format)?);
    Ok(())
}

async fn run_delete(api: &MaintenanceApi<'_>, name: &str) -> Result<()> {
    api.delete(name).await?;
    eprintln!("Deleted maintenance window {name} (removed from all daemons)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_read_and_write() {
        assert_eq!(MaintenanceCmd::List.kind(), CmdKind::Read);
        assert_eq!(
            MaintenanceCmd::Status {
                devices: vec!["1.2.3.4".into()]
            }
            .kind(),
            CmdKind::Read
        );
        assert_eq!(
            MaintenanceCmd::Delete { name: "w".into() }.kind(),
            CmdKind::Write
        );
    }

    #[test]
    fn window_row_summarizes_an_outage() {
        let o = Outage {
            name: "win".into(),
            schedule_type: "daily".into(),
            time: vec![Default::default()],
            interface: vec![Default::default(), Default::default()],
            node: vec![],
        };
        let r = WindowRow::from(&o);
        assert_eq!(r.row(), vec!["win", "daily", "1", "2", "0"]);
    }
}
