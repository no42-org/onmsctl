/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local model ([`crate::model`]) → wire DTO ([`crate::server`]).
//!
//! [`to_wire`] projects a validated `Maintenance` document onto a server
//! `Outage`, given the node references already resolved to server nodeIds (the
//! resolution is I/O and happens in [`crate::api`]). The reverse (`from_wire`)
//! is not needed: `list` renders the wire `Outage`/`Outages` directly.

use crate::model::{MaintenanceLocal, ScheduleType};
use crate::server;

/// The wire `type` string for a schedule type.
pub fn type_str(t: ScheduleType) -> &'static str {
    match t {
        ScheduleType::Specific => "specific",
        ScheduleType::Daily => "daily",
        ScheduleType::Weekly => "weekly",
        ScheduleType::Monthly => "monthly",
    }
}

/// Project the local document onto a wire `Outage`. `node_ids` are the resolved
/// server nodeIds for `spec.devices.nodes`, in declaration order.
pub fn to_wire(local: &MaintenanceLocal, node_ids: &[i64]) -> server::Outage {
    server::Outage {
        name: local.metadata.name.clone(),
        schedule_type: type_str(local.spec.schedule.schedule_type).to_string(),
        time: local
            .spec
            .schedule
            .times
            .iter()
            .map(|t| server::Time {
                id: None,
                day: t.day.clone(),
                begins: t.begins.clone(),
                ends: t.ends.clone(),
            })
            .collect(),
        interface: local
            .spec
            .devices
            .interfaces
            .iter()
            .map(|a| server::Interface { address: a.clone() })
            .collect(),
        node: node_ids.iter().map(|&id| server::Node { id }).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> MaintenanceLocal {
        serde_norway::from_str(
            r#"
apiVersion: maintenance.opennms.org/v1
kind: Maintenance
metadata: { name: weekend }
spec:
  schedule:
    type: weekly
    times:
      - { day: Monday, begins: "22:00:00", ends: "23:00:00" }
  devices:
    interfaces: [192.168.8.8]
    nodes:
      - { foreignSource: lab, foreignId: web01 }
  suppress:
    polling: { packages: [prod-poller] }
"#,
        )
        .unwrap()
    }

    #[test]
    fn to_wire_maps_schedule_devices_and_resolved_nodes() {
        let w = to_wire(&local(), &[42]);
        assert_eq!(w.name, "weekend");
        assert_eq!(w.schedule_type, "weekly");
        assert_eq!(w.time.len(), 1);
        assert_eq!(w.time[0].day.as_deref(), Some("Monday"));
        assert_eq!(w.interface[0].address, "192.168.8.8");
        assert_eq!(w.node, vec![server::Node { id: 42 }]);
    }
}
