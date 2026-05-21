/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Wire-format DTOs for Horizon's `/rest/requisitions/{fs}` and
//! `/rest/foreignSources/{fs}` (plus `/foreignSources/default`)
//! endpoints.
//!
//! Field naming follows the JSON wire shape captured from a live
//! Horizon 36 instance: kebab-case throughout (`foreign-id`,
//! `node-label`, `ip-addr`, `snmp-primary`, `monitored-service`,
//! `meta-data`, `scan-interval`). The Rust types use snake_case and
//! rely on `#[serde(rename_all = "kebab-case")]` per struct.
//!
//! These DTOs are intentionally **permissive** on deserialize:
//! unknown fields are silently ignored (no `deny_unknown_fields`) so
//! a future Horizon release adding a field won't break parse. This
//! matches the cli-core spec's "permissive on deserialization,
//! strict on local parse" rule — local YAML stays strict, server
//! responses stay forward-compatible.
//!
//! Conversions to and from the local YAML [`crate::model::RequisitionLocal`]
//! live in [`crate::model::convert`].

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Requisition response — GET /rest/requisitions/{fs}
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct RequisitionServer {
    /// Foreign-source identifier. Matches the `{fs}` URL path segment.
    pub foreign_source: String,
    /// Epoch milliseconds — server-managed, omitted on POST.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_stamp: Option<i64>,
    /// Epoch milliseconds — server-managed, omitted on POST.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_import: Option<i64>,
    /// Nodes in this requisition. Wire field is singular `node` for
    /// historical reasons; it's always an array.
    #[serde(default)]
    pub node: Vec<NodeServer>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct NodeServer {
    pub foreign_id: String,
    pub node_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Legacy free-form fields preserved by Horizon — not modeled in
    /// the local YAML. We keep them on the wire DTO so server →
    /// local → server doesn't strip them when we're round-tripping
    /// foreign data we didn't author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub building: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_foreign_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_foreign_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_node_label: Option<String>,
    #[serde(default)]
    pub interface: Vec<InterfaceServer>,
    #[serde(default)]
    pub category: Vec<CategoryRef>,
    #[serde(default)]
    pub asset: Vec<AssetEntry>,
    #[serde(default)]
    pub meta_data: Vec<MetaDataEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct InterfaceServer {
    pub ip_addr: String,
    /// `P` / `S` / `N`. Always present in wire responses.
    pub snmp_primary: String,
    /// Status code — 1 typically means "managed/active". Not modeled
    /// in local YAML (server-derived).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    #[serde(default)]
    pub monitored_service: Vec<MonitoredServiceServer>,
    #[serde(default)]
    pub category: Vec<CategoryRef>,
    #[serde(default)]
    pub meta_data: Vec<MetaDataEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct MonitoredServiceServer {
    pub service_name: String,
    #[serde(default)]
    pub category: Vec<CategoryRef>,
    #[serde(default)]
    pub meta_data: Vec<MetaDataEntry>,
}

/// Category reference: `{"name": "..."}`. Used on nodes, interfaces,
/// and services.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CategoryRef {
    pub name: String,
}

/// Asset entry: `{"name": "<asset-field>", "value": "<asset-value>"}`.
/// Horizon's asset record uses this key/value-pair shape on the wire
/// rather than an object — historical reasons.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AssetEntry {
    pub name: String,
    pub value: String,
}

/// Meta-data triple: `{"context", "key", "value"}`. Used on nodes,
/// interfaces, and services. The local YAML model does NOT expose
/// this surface today — fields are preserved on the wire DTO so
/// server → local → server round-trips don't drop data we didn't
/// author.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MetaDataEntry {
    pub context: String,
    pub key: String,
    pub value: String,
}

// ---------------------------------------------------------------------------
// Foreign-source response — GET /rest/foreignSources/{fs} and /default
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ForeignSourceServer {
    pub name: String,
    /// Epoch milliseconds — server-managed, omitted on POST.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_stamp: Option<i64>,
    /// Humanized interval string like `1d`, `30m`. Optional on the
    /// wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_interval: Option<String>,
    #[serde(default)]
    pub detectors: Vec<DetectorServer>,
    #[serde(default)]
    pub policies: Vec<PolicyServer>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DetectorServer {
    pub name: String,
    pub class: String,
    /// Wire field is singular `parameter` for historical reasons;
    /// content is always an array.
    #[serde(default)]
    pub parameter: Vec<ParameterEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PolicyServer {
    pub name: String,
    pub class: String,
    #[serde(default)]
    pub parameter: Vec<ParameterEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ParameterEntry {
    pub key: String,
    pub value: String,
}

// ---------------------------------------------------------------------------
// Tests — verify deserialization against the captured fixtures
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const REQUISITION_FIXTURE: &str = include_str!("../../tests/fixtures/requisition.json");
    const FOREIGN_SOURCE_FIXTURE: &str = include_str!("../../tests/fixtures/foreign_source.json");
    const FOREIGN_SOURCE_DEFAULT_FIXTURE: &str =
        include_str!("../../tests/fixtures/foreign_source_default.json");

    #[test]
    fn requisition_fixture_deserializes() {
        let r: RequisitionServer =
            serde_json::from_str(REQUISITION_FIXTURE).expect("requisition fixture parses");
        assert_eq!(r.foreign_source, "acme-prod");
        assert_eq!(r.node.len(), 1);
        let n = &r.node[0];
        assert_eq!(n.foreign_id, "1779349515116");
        assert_eq!(n.node_label, "my-blinky-node");
        assert_eq!(n.interface.len(), 3);
        assert_eq!(n.category.len(), 1);
        assert_eq!(n.category[0].name, "Routers");
        assert_eq!(n.asset.len(), 1);
        assert_eq!(n.asset[0].name, "city");
        assert_eq!(n.asset[0].value, "Heilbronn");
        assert_eq!(n.meta_data.len(), 1);

        let primary_iface = n
            .interface
            .iter()
            .find(|i| i.snmp_primary == "P")
            .expect("primary interface present");
        assert_eq!(primary_iface.ip_addr, "127.0.23.23");
        assert_eq!(primary_iface.monitored_service.len(), 2);
        let svc_names: Vec<_> = primary_iface
            .monitored_service
            .iter()
            .map(|s| s.service_name.as_str())
            .collect();
        assert_eq!(svc_names, vec!["ICMP", "SNMP"]);
    }

    #[test]
    fn foreign_source_fixture_deserializes() {
        let fs: ForeignSourceServer =
            serde_json::from_str(FOREIGN_SOURCE_FIXTURE).expect("FS fixture parses");
        assert_eq!(fs.name, "acme-prod");
        assert_eq!(fs.scan_interval.as_deref(), Some("1d"));
        assert!(!fs.detectors.is_empty());
        assert_eq!(fs.policies.len(), 1);

        let icmp = fs
            .detectors
            .iter()
            .find(|d| d.name == "ICMP")
            .expect("ICMP detector present");
        assert_eq!(
            icmp.class,
            "org.opennms.netmgt.provision.detector.icmp.IcmpDetector"
        );
        assert!(icmp.parameter.is_empty());

        let jvm = fs
            .detectors
            .iter()
            .find(|d| d.name == "OpenNMS-JVM")
            .expect("JVM detector present");
        assert!(
            jvm.parameter
                .iter()
                .any(|p| p.key == "port" && p.value == "18980")
        );

        let pol = &fs.policies[0];
        assert_eq!(pol.name, "enable-interface-collection");
        assert!(pol.parameter.iter().any(|p| p.key == "matchBehavior"));
    }

    #[test]
    fn foreign_source_default_fixture_deserializes() {
        let fs: ForeignSourceServer = serde_json::from_str(FOREIGN_SOURCE_DEFAULT_FIXTURE)
            .expect("default FS fixture parses");
        assert_eq!(fs.name, "default");
        assert!(fs.policies.is_empty());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // Per cli-core "permissive on deserialization" — a future
        // Horizon release adding a field must not break parse.
        let with_extra = r#"{
            "foreign-source": "x",
            "node": [],
            "totally-new-server-field": "ignored"
        }"#;
        let r: RequisitionServer = serde_json::from_str(with_extra).expect("forward-compat");
        assert_eq!(r.foreign_source, "x");
    }
}
