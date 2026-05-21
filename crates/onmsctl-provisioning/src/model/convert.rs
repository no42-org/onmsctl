/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Conversions between local YAML DTOs ([`super::*`]) and server
//! wire-format DTOs ([`super::server::*`]).
//!
//! The conversions are intentionally **lossy** in both directions:
//!
//! - **Server → Local** drops wire fields the local YAML doesn't
//!   expose (`meta-data`, `parent-*`, per-interface `category`,
//!   `descr`, `managed`, `status`, `date-stamp`, `last-import`,
//!   service-level `category` / `meta-data`, the legacy `building` /
//!   `city` shortcut fields). The diff path only ever compares local
//!   YAML against a server response that's been converted into the
//!   local form, so the dropped fields can't surface false positives.
//!
//! - **Local → Server** reconstructs wire shape with empty / `null`
//!   defaults for the unmodeled fields. POSTing this is acceptable
//!   for a CLI-authored requisition (foreign data the operator
//!   didn't write); Horizon fills in server-derived fields on its
//!   own.
//!
//! Where the wire and local types use different shapes (categories as
//! `[{name}]` vs `[String]`, assets as `[{name,value}]` vs
//! `BTreeMap<String,String>`, services as `[{service-name, …}]` vs
//! `[String]`), the conversion flattens or wraps accordingly.

use std::collections::BTreeMap;

use crate::model::{
    ApiVersion, Detector, ForeignSourceSpec, Interface, Kind, Metadata, Node, Parameter, Policy,
    RequisitionLocal, SnmpPrimary, Spec,
    server::{
        AssetEntry, CategoryRef, DetectorServer, ForeignSourceServer, InterfaceServer,
        MonitoredServiceServer, NodeServer, ParameterEntry, PolicyServer, RequisitionServer,
    },
};

// ---------------------------------------------------------------------------
// Server → Local — used as the diff baseline (canonicalize remote state)
// ---------------------------------------------------------------------------

/// Build a local [`RequisitionLocal`] from a server requisition body
/// and an optional foreign-source definition. When `fs` is `None`,
/// the resulting local document omits `spec.foreignSource` — meaning
/// "uses Horizon's default-FS" per design D1.
///
/// The composite metadata.name is taken from the requisition's
/// `foreign-source` field (the URL path segment); the optional `fs`
/// must agree on the name when present.
pub fn requisition_from_wire(
    req: &RequisitionServer,
    fs: Option<&ForeignSourceServer>,
) -> RequisitionLocal {
    RequisitionLocal {
        api_version: ApiVersion,
        kind: Kind,
        metadata: Metadata {
            name: req.foreign_source.clone(),
        },
        spec: Spec {
            foreign_source: fs.map(foreign_source_from_wire),
            nodes: req.node.iter().map(node_from_wire).collect(),
        },
    }
}

fn foreign_source_from_wire(fs: &ForeignSourceServer) -> ForeignSourceSpec {
    ForeignSourceSpec {
        scan_interval: fs.scan_interval.clone(),
        detectors: fs.detectors.iter().map(detector_from_wire).collect(),
        policies: fs.policies.iter().map(policy_from_wire).collect(),
    }
}

fn detector_from_wire(d: &DetectorServer) -> Detector {
    Detector {
        name: d.name.clone(),
        class: Some(d.class.clone()),
        parameters: d.parameter.iter().map(parameter_from_wire).collect(),
    }
}

fn policy_from_wire(p: &PolicyServer) -> Policy {
    Policy {
        name: p.name.clone(),
        class: p.class.clone(),
        parameters: p.parameter.iter().map(parameter_from_wire).collect(),
    }
}

fn parameter_from_wire(p: &ParameterEntry) -> Parameter {
    Parameter {
        key: p.key.clone(),
        value: p.value.clone(),
    }
}

fn node_from_wire(n: &NodeServer) -> Node {
    let mut assets = BTreeMap::new();
    for a in &n.asset {
        // Last-write wins on duplicate asset names — defensive against
        // a malformed wire response. Horizon enforces uniqueness server-side.
        assets.insert(a.name.clone(), a.value.clone());
    }
    Node {
        foreign_id: n.foreign_id.clone(),
        label: n.node_label.clone(),
        interfaces: n.interface.iter().map(interface_from_wire).collect(),
        categories: n.category.iter().map(|c| c.name.clone()).collect(),
        assets,
    }
}

fn interface_from_wire(i: &InterfaceServer) -> Interface {
    Interface {
        ip: i.ip_addr.clone(),
        services: i
            .monitored_service
            .iter()
            .map(|s| s.service_name.clone())
            .collect(),
        snmp_primary: match i.snmp_primary.as_str() {
            "P" => Some(SnmpPrimary::P),
            "S" => Some(SnmpPrimary::S),
            "N" => Some(SnmpPrimary::N),
            // Defensive: an unrecognized wire value drops to None.
            // Horizon should never emit anything else; if it does,
            // the diff treats this interface as "snmpPrimary omitted"
            // which is the safest interpretation.
            _ => None,
        },
    }
}

// ---------------------------------------------------------------------------
// Local → Server — used as the apply payload (POSTs to Horizon)
// ---------------------------------------------------------------------------

/// Project a [`RequisitionLocal`] onto two server payloads suitable
/// for POSTing to `/rest/requisitions/{fs}` and (when
/// `spec.foreignSource` is present) `/rest/foreignSources/{fs}`. The
/// foreign-source side is `None` when the local YAML omits
/// `spec.foreignSource` — apply path then DELETEs any existing custom
/// FS on the server to revert to the Horizon default (per design D1).
pub fn requisition_to_wire(
    local: &RequisitionLocal,
) -> (RequisitionServer, Option<ForeignSourceServer>) {
    let req = RequisitionServer {
        foreign_source: local.metadata.name.clone(),
        // Server-managed — omit on POST.
        date_stamp: None,
        last_import: None,
        node: local.spec.nodes.iter().map(node_to_wire).collect(),
    };
    let fs = local
        .spec
        .foreign_source
        .as_ref()
        .map(|fs_local| foreign_source_to_wire(&local.metadata.name, fs_local));
    (req, fs)
}

fn foreign_source_to_wire(name: &str, fs: &ForeignSourceSpec) -> ForeignSourceServer {
    ForeignSourceServer {
        name: name.to_string(),
        date_stamp: None,
        scan_interval: fs.scan_interval.clone(),
        detectors: fs.detectors.iter().map(detector_to_wire).collect(),
        policies: fs.policies.iter().map(policy_to_wire).collect(),
    }
}

fn detector_to_wire(d: &Detector) -> DetectorServer {
    DetectorServer {
        name: d.name.clone(),
        // Local Detector.class is Option<String>; the wire requires
        // a string. Empty-string fallback lets Horizon respond with
        // a clear error rather than silently accepting nothing — the
        // apply-time spec already says class is needed in practice.
        class: d.class.clone().unwrap_or_default(),
        parameter: d.parameters.iter().map(parameter_to_wire).collect(),
    }
}

fn policy_to_wire(p: &Policy) -> PolicyServer {
    PolicyServer {
        name: p.name.clone(),
        class: p.class.clone(),
        parameter: p.parameters.iter().map(parameter_to_wire).collect(),
    }
}

fn parameter_to_wire(p: &Parameter) -> ParameterEntry {
    ParameterEntry {
        key: p.key.clone(),
        value: p.value.clone(),
    }
}

fn node_to_wire(n: &Node) -> NodeServer {
    NodeServer {
        foreign_id: n.foreign_id.clone(),
        node_label: n.label.clone(),
        // Unmodeled-locally fields default to null/empty on POST.
        // Horizon fills server-managed values; foreign data the
        // operator didn't author isn't ours to invent.
        location: None,
        building: None,
        city: None,
        parent_foreign_source: None,
        parent_foreign_id: None,
        parent_node_label: None,
        interface: n.interfaces.iter().map(interface_to_wire).collect(),
        category: n
            .categories
            .iter()
            .map(|name| CategoryRef { name: name.clone() })
            .collect(),
        asset: n
            .assets
            .iter()
            .map(|(k, v)| AssetEntry {
                name: k.clone(),
                value: v.clone(),
            })
            .collect(),
        meta_data: Vec::new(),
    }
}

fn interface_to_wire(i: &Interface) -> InterfaceServer {
    InterfaceServer {
        ip_addr: i.ip.clone(),
        // SnmpPrimary defaults to "N" (not eligible) when the local
        // YAML omits the field — safest interpretation for a
        // service-discovery-driven CLI.
        snmp_primary: match i.snmp_primary {
            Some(SnmpPrimary::P) => "P".to_string(),
            Some(SnmpPrimary::S) => "S".to_string(),
            Some(SnmpPrimary::N) | None => "N".to_string(),
        },
        status: None,
        managed: None,
        descr: None,
        monitored_service: i
            .services
            .iter()
            .map(|name| MonitoredServiceServer {
                service_name: name.clone(),
                category: Vec::new(),
                meta_data: Vec::new(),
            })
            .collect(),
        category: Vec::new(),
        meta_data: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests — round-trip the captured fixtures and verify field mapping
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const REQUISITION_FIXTURE: &str = include_str!("../../tests/fixtures/requisition.json");
    const FOREIGN_SOURCE_FIXTURE: &str = include_str!("../../tests/fixtures/foreign_source.json");

    fn parse_req() -> RequisitionServer {
        serde_json::from_str(REQUISITION_FIXTURE).expect("requisition fixture parses")
    }

    fn parse_fs() -> ForeignSourceServer {
        serde_json::from_str(FOREIGN_SOURCE_FIXTURE).expect("FS fixture parses")
    }

    // -- Server → Local --------------------------------------------------

    #[test]
    fn fixture_server_to_local_preserves_modeled_fields() {
        let r = parse_req();
        let fs = parse_fs();
        let local = requisition_from_wire(&r, Some(&fs));

        assert_eq!(local.metadata.name, "acme-prod");
        assert_eq!(local.spec.nodes.len(), 1);

        let n = &local.spec.nodes[0];
        assert_eq!(n.foreign_id, "1779349515116");
        assert_eq!(n.label, "my-blinky-node");
        assert_eq!(n.categories, vec!["Routers"]);
        assert_eq!(n.assets.get("city"), Some(&"Heilbronn".to_string()));
        assert_eq!(n.interfaces.len(), 3);

        let primary = n
            .interfaces
            .iter()
            .find(|i| i.snmp_primary == Some(SnmpPrimary::P))
            .expect("primary interface present");
        assert_eq!(primary.ip, "127.0.23.23");
        assert_eq!(primary.services, vec!["ICMP", "SNMP"]);

        let secondary = n
            .interfaces
            .iter()
            .find(|i| i.snmp_primary == Some(SnmpPrimary::S))
            .expect("secondary interface present");
        assert_eq!(secondary.ip, "127.0.23.24");
        assert!(secondary.services.is_empty());

        let not_eligible = n
            .interfaces
            .iter()
            .find(|i| i.snmp_primary == Some(SnmpPrimary::N))
            .expect("not-eligible interface present");
        assert_eq!(not_eligible.ip, "172.0.23.25");

        let fs_local = local.spec.foreign_source.as_ref().expect("FS present");
        assert_eq!(fs_local.scan_interval.as_deref(), Some("1d"));
        assert!(!fs_local.detectors.is_empty());
        assert_eq!(fs_local.policies.len(), 1);

        let pol = &fs_local.policies[0];
        assert_eq!(pol.name, "enable-interface-collection");
        assert!(pol.parameters.iter().any(|p| p.key == "matchBehavior"));
    }

    #[test]
    fn server_to_local_with_no_fs_omits_foreign_source() {
        let r = parse_req();
        let local = requisition_from_wire(&r, None);
        assert!(local.spec.foreign_source.is_none());
        // The nodes side still converts.
        assert_eq!(local.spec.nodes.len(), 1);
    }

    // -- Local → Server --------------------------------------------------

    #[test]
    fn local_to_server_reconstructs_wire_shape() {
        // Build a local doc, then convert to wire, then re-deserialize
        // through `RequisitionServer` to verify the wire field names
        // match what Horizon expects.
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  foreignSource:
    scanInterval: 1d
    detectors:
      - name: ICMP
        class: org.opennms.netmgt.provision.detector.icmp.IcmpDetector
    policies: []
  nodes:
    - foreignId: web01
      label: web01.acme
      categories: [Production, Web]
      assets:
        city: Heilbronn
        rack: A4
      interfaces:
        - ip: 10.0.0.5
          snmpPrimary: P
          services: [ICMP, SNMP, HTTP]
"#;
        let local: RequisitionLocal = serde_norway::from_str(yaml).expect("YAML parses");
        let (req, fs) = requisition_to_wire(&local);

        assert_eq!(req.foreign_source, "acme-prod");
        assert!(req.date_stamp.is_none());
        assert!(req.last_import.is_none());
        assert_eq!(req.node.len(), 1);

        let n = &req.node[0];
        assert_eq!(n.foreign_id, "web01");
        assert_eq!(n.node_label, "web01.acme");
        let cats: Vec<_> = n.category.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(cats, vec!["Production", "Web"]);
        let assets: Vec<_> = n
            .asset
            .iter()
            .map(|a| (a.name.as_str(), a.value.as_str()))
            .collect();
        // BTreeMap iterates in sorted key order — `city` < `rack`.
        assert_eq!(assets, vec![("city", "Heilbronn"), ("rack", "A4")]);

        assert_eq!(n.interface.len(), 1);
        let i = &n.interface[0];
        assert_eq!(i.ip_addr, "10.0.0.5");
        assert_eq!(i.snmp_primary, "P");
        let svc: Vec<_> = i
            .monitored_service
            .iter()
            .map(|s| s.service_name.as_str())
            .collect();
        assert_eq!(svc, vec!["ICMP", "SNMP", "HTTP"]);

        let fs = fs.expect("FS reconstructed");
        assert_eq!(fs.name, "acme-prod");
        assert_eq!(fs.scan_interval.as_deref(), Some("1d"));
        assert_eq!(fs.detectors.len(), 1);
        assert_eq!(
            fs.detectors[0].class,
            "org.opennms.netmgt.provision.detector.icmp.IcmpDetector"
        );
    }

    #[test]
    fn omitted_snmp_primary_defaults_to_not_eligible() {
        // When local YAML omits snmpPrimary, the wire defaults to "N".
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata: { name: acme-prod }
spec:
  nodes:
    - foreignId: web01
      label: w
      interfaces:
        - ip: 10.0.0.5
"#;
        let local: RequisitionLocal = serde_norway::from_str(yaml).expect("YAML parses");
        let (req, _fs) = requisition_to_wire(&local);
        assert_eq!(req.node[0].interface[0].snmp_primary, "N");
    }

    #[test]
    fn omitted_foreign_source_produces_no_fs_payload() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata: { name: acme-prod }
spec:
  nodes: []
"#;
        let local: RequisitionLocal = serde_norway::from_str(yaml).expect("YAML parses");
        let (_req, fs) = requisition_to_wire(&local);
        assert!(
            fs.is_none(),
            "omitting foreignSource ⇒ no FS payload to POST"
        );
    }

    // -- Round-trip: server → local → server (lossy, modeled fields preserved) --

    #[test]
    fn round_trip_preserves_modeled_fields() {
        let r = parse_req();
        let fs = parse_fs();
        let local = requisition_from_wire(&r, Some(&fs));
        let (round_req, round_fs) = requisition_to_wire(&local);
        let round_fs = round_fs.expect("FS round-tripped");

        // Modeled fields round-trip exactly.
        assert_eq!(round_req.foreign_source, r.foreign_source);
        assert_eq!(round_req.node.len(), r.node.len());
        assert_eq!(round_req.node[0].foreign_id, r.node[0].foreign_id);
        assert_eq!(round_req.node[0].node_label, r.node[0].node_label);
        assert_eq!(round_req.node[0].interface.len(), r.node[0].interface.len());

        // Wire fields the local YAML doesn't model are intentionally
        // null/empty after round-trip — documented loss.
        assert!(round_req.node[0].location.is_none());
        assert!(round_req.node[0].meta_data.is_empty());
        for iface in &round_req.node[0].interface {
            assert!(iface.status.is_none());
            assert!(iface.descr.is_none());
            assert!(iface.category.is_empty());
        }

        // ForeignSource round-trips detectors and policies completely
        // (no unmodeled fields on those types).
        assert_eq!(round_fs.detectors.len(), fs.detectors.len());
        assert_eq!(round_fs.policies.len(), fs.policies.len());
        assert_eq!(round_fs.scan_interval, fs.scan_interval);
    }
}
