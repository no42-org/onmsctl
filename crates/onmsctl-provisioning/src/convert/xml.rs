/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! XML reader DTOs for the requisition + foreign-source migrator.
//!
//! Two root types match the two input files OpenNMS's `provision.pl`
//! workflow produces:
//!
//! - [`RequisitionXml`] — `<model-import>` root, equivalent to the
//!   wire shape of `GET /rest/requisitions/{fs}` (the legacy CLI's
//!   `model-import.xsd`).
//! - [`ForeignSourceXml`] — `<foreign-source>` root, equivalent to
//!   `etc/foreign-sources/*.xml` (the legacy CLI's
//!   `foreign-source.xsd`).
//!
//! Parsing uses `quick_xml::de::from_str`. The DTOs are intentionally
//! permissive (`#[serde(default)]` on every list, `Option<…>` on
//! optional attributes) so legacy XML with omitted-but-meaningful
//! elements still parses. Unknown elements / attributes flow through
//! to the PR001 finding via a wrapper pass at the pipeline layer.

use serde::Deserialize;

// ---------------------------------------------------------------------------
// <model-import> — requisition XML
// ---------------------------------------------------------------------------

/// Root element of a requisition XML file. Maps 1:1 to
/// `crate::model::server::RequisitionServer` once converted, with the
/// addition of XML-specific attributes (date-stamp / last-import on
/// the root, which the REST API surfaces as JSON fields).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RequisitionXml {
    /// Required: the foreign-source name. Matches the filename
    /// convention `<fs>.xml`.
    #[serde(rename = "@foreign-source")]
    pub foreign_source: String,
    /// Optional ISO-8601 string; the REST API surfaces this as
    /// epoch ms in the JSON shape.
    #[serde(rename = "@date-stamp", default)]
    pub date_stamp: Option<String>,
    /// Optional ISO-8601 string; ditto.
    #[serde(rename = "@last-import", default)]
    pub last_import: Option<String>,
    /// Nodes in the requisition. Empty is legal (a requisition with
    /// no nodes — operator probably wants to delete it, but the
    /// migrator preserves the shape).
    #[serde(default, rename = "node")]
    pub nodes: Vec<NodeXml>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct NodeXml {
    #[serde(rename = "@foreign-id")]
    pub foreign_id: String,
    #[serde(rename = "@node-label")]
    pub node_label: String,
    /// Geographic / Minion location. Optional in the XML; defaults
    /// to `Default` on the server.
    #[serde(rename = "@location", default)]
    pub location: Option<String>,
    #[serde(rename = "@city", default)]
    pub city: Option<String>,
    #[serde(default, rename = "interface")]
    pub interfaces: Vec<InterfaceXml>,
    #[serde(default, rename = "category")]
    pub categories: Vec<CategoryXml>,
    #[serde(default, rename = "asset")]
    pub assets: Vec<AssetXml>,
    #[serde(default, rename = "meta-data")]
    pub meta_data: Vec<MetaDataXml>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct InterfaceXml {
    #[serde(rename = "@ip-addr")]
    pub ip_addr: String,
    /// `"P"` (primary) / `"S"` (secondary) / `"N"` (not eligible).
    /// Optional in the XML; the REST API defaults to `"N"`.
    #[serde(rename = "@snmp-primary", default)]
    pub snmp_primary: Option<String>,
    /// `"1"` (managed) / `"3"` (unmanaged). Optional.
    #[serde(rename = "@status", default)]
    pub status: Option<String>,
    /// Optional human-readable description.
    #[serde(rename = "@descr", default)]
    pub descr: Option<String>,
    #[serde(default, rename = "monitored-service")]
    pub monitored_services: Vec<MonitoredServiceXml>,
    #[serde(default, rename = "meta-data")]
    pub meta_data: Vec<MetaDataXml>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MonitoredServiceXml {
    #[serde(rename = "@service-name")]
    pub service_name: String,
    #[serde(default, rename = "meta-data")]
    pub meta_data: Vec<MetaDataXml>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CategoryXml {
    #[serde(rename = "@name")]
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AssetXml {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@value")]
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MetaDataXml {
    #[serde(rename = "@context")]
    pub context: String,
    #[serde(rename = "@key")]
    pub key: String,
    #[serde(rename = "@value")]
    pub value: String,
}

// ---------------------------------------------------------------------------
// <foreign-source> — foreign-source XML
// ---------------------------------------------------------------------------

/// Root element of a foreign-source XML file. Maps to
/// `crate::model::server::ForeignSourceServer` once converted.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ForeignSourceXml {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@date-stamp", default)]
    pub date_stamp: Option<String>,
    /// Humanized duration string like `1d` / `30m`. Optional.
    #[serde(rename = "scan-interval", default)]
    pub scan_interval: Option<String>,
    /// Detector list, wrapped in a `<detectors>` element in the
    /// XML. Empty is legal.
    #[serde(default)]
    pub detectors: DetectorsXml,
    #[serde(default)]
    pub policies: PoliciesXml,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct DetectorsXml {
    #[serde(default, rename = "detector")]
    pub detector: Vec<DetectorXml>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct PoliciesXml {
    #[serde(default, rename = "policy")]
    pub policy: Vec<PolicyXml>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DetectorXml {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@class")]
    pub class: String,
    #[serde(default, rename = "parameter")]
    pub parameter: Vec<ParameterXml>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PolicyXml {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@class")]
    pub class: String,
    #[serde(default, rename = "parameter")]
    pub parameter: Vec<ParameterXml>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ParameterXml {
    #[serde(rename = "@key")]
    pub key: String,
    #[serde(rename = "@value")]
    pub value: String,
}

// ---------------------------------------------------------------------------
// Parse entry points
// ---------------------------------------------------------------------------

/// Parse a `<model-import>` requisition XML document.
pub fn parse_requisition(xml: &str) -> Result<RequisitionXml, quick_xml::DeError> {
    quick_xml::de::from_str(xml)
}

/// Parse a `<foreign-source>` XML document.
pub fn parse_foreign_source(xml: &str) -> Result<ForeignSourceXml, quick_xml::DeError> {
    quick_xml::de::from_str(xml)
}

// ---------------------------------------------------------------------------
// Tests — synthetic fixtures based on the documented XSD shapes
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const REQUISITION_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model-import foreign-source="acme-prod" date-stamp="2024-01-15T10:30:00Z" last-import="2024-01-15T10:31:00Z">
  <node foreign-id="web01" node-label="web01.acme.com" location="HQ">
    <interface ip-addr="10.0.0.1" snmp-primary="P" status="1">
      <monitored-service service-name="HTTP"/>
      <monitored-service service-name="HTTPS"/>
      <meta-data context="requisition" key="role" value="frontend"/>
    </interface>
    <interface ip-addr="10.0.0.2" snmp-primary="S" status="1">
      <monitored-service service-name="SNMP"/>
    </interface>
    <category name="Production"/>
    <category name="Web"/>
    <asset name="city" value="NYC"/>
    <asset name="rack" value="R3"/>
    <meta-data context="requisition" key="owner" value="ops"/>
  </node>
  <node foreign-id="db01" node-label="db01.acme.com">
    <interface ip-addr="10.0.1.1"/>
    <category name="Production"/>
    <category name="Database"/>
  </node>
</model-import>"#;

    const FOREIGN_SOURCE_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<foreign-source xmlns="http://xmlns.opennms.org/xsd/config/foreign-source" name="acme-prod" date-stamp="2024-01-15T10:30:00Z">
  <scan-interval>1d</scan-interval>
  <detectors>
    <detector name="SNMP" class="org.opennms.netmgt.provision.detector.snmp.SnmpDetector">
      <parameter key="port" value="161"/>
      <parameter key="timeout" value="2000"/>
    </detector>
    <detector name="ICMP" class="org.opennms.netmgt.provision.detector.icmp.IcmpDetector"/>
  </detectors>
  <policies>
    <policy name="Production tag" class="org.opennms.netmgt.provision.persist.policies.NodeCategorySettingPolicy">
      <parameter key="category" value="Production"/>
      <parameter key="matchBehavior" value="ALL_PARAMETERS"/>
    </policy>
  </policies>
</foreign-source>"#;

    const EMPTY_REQUISITION: &str = r#"<?xml version="1.0"?>
<model-import foreign-source="empty"></model-import>"#;

    const FOREIGN_SOURCE_NO_POLICIES: &str = r#"<?xml version="1.0"?>
<foreign-source name="basic">
  <scan-interval>30m</scan-interval>
  <detectors>
    <detector name="ICMP" class="org.opennms.netmgt.provision.detector.icmp.IcmpDetector"/>
  </detectors>
</foreign-source>"#;

    #[test]
    fn requisition_root_attrs_parse() {
        let r = parse_requisition(REQUISITION_FIXTURE).expect("requisition parses");
        assert_eq!(r.foreign_source, "acme-prod");
        assert_eq!(r.date_stamp.as_deref(), Some("2024-01-15T10:30:00Z"));
        assert_eq!(r.last_import.as_deref(), Some("2024-01-15T10:31:00Z"));
    }

    #[test]
    fn requisition_node_count_matches_fixture() {
        let r = parse_requisition(REQUISITION_FIXTURE).unwrap();
        assert_eq!(r.nodes.len(), 2);
        assert_eq!(r.nodes[0].foreign_id, "web01");
        assert_eq!(r.nodes[0].node_label, "web01.acme.com");
        assert_eq!(r.nodes[0].location.as_deref(), Some("HQ"));
        assert_eq!(r.nodes[1].foreign_id, "db01");
        assert!(r.nodes[1].location.is_none());
    }

    #[test]
    fn requisition_interface_attrs_parse() {
        let r = parse_requisition(REQUISITION_FIXTURE).unwrap();
        let web = &r.nodes[0];
        assert_eq!(web.interfaces.len(), 2);
        assert_eq!(web.interfaces[0].ip_addr, "10.0.0.1");
        assert_eq!(web.interfaces[0].snmp_primary.as_deref(), Some("P"));
        assert_eq!(web.interfaces[0].status.as_deref(), Some("1"));
        assert_eq!(web.interfaces[0].monitored_services.len(), 2);
        assert_eq!(web.interfaces[0].monitored_services[0].service_name, "HTTP");
    }

    #[test]
    fn requisition_categories_assets_metadata_parse() {
        let r = parse_requisition(REQUISITION_FIXTURE).unwrap();
        let web = &r.nodes[0];
        assert_eq!(web.categories.len(), 2);
        assert_eq!(web.categories[0].name, "Production");
        assert_eq!(web.assets.len(), 2);
        assert_eq!(web.assets[0].name, "city");
        assert_eq!(web.assets[0].value, "NYC");
        assert_eq!(web.meta_data.len(), 1);
        assert_eq!(web.meta_data[0].context, "requisition");
        assert_eq!(web.meta_data[0].key, "owner");
    }

    #[test]
    fn interface_metadata_under_interface_parses() {
        let r = parse_requisition(REQUISITION_FIXTURE).unwrap();
        let web_iface_0 = &r.nodes[0].interfaces[0];
        assert_eq!(web_iface_0.meta_data.len(), 1);
        assert_eq!(web_iface_0.meta_data[0].value, "frontend");
    }

    #[test]
    fn empty_requisition_parses_with_zero_nodes() {
        let r = parse_requisition(EMPTY_REQUISITION).expect("empty requisition parses");
        assert_eq!(r.foreign_source, "empty");
        assert_eq!(r.nodes.len(), 0);
    }

    #[test]
    fn foreign_source_root_attrs_parse() {
        let f = parse_foreign_source(FOREIGN_SOURCE_FIXTURE).expect("FS parses");
        assert_eq!(f.name, "acme-prod");
        assert_eq!(f.date_stamp.as_deref(), Some("2024-01-15T10:30:00Z"));
        assert_eq!(f.scan_interval.as_deref(), Some("1d"));
    }

    #[test]
    fn foreign_source_detectors_parse() {
        let f = parse_foreign_source(FOREIGN_SOURCE_FIXTURE).unwrap();
        assert_eq!(f.detectors.detector.len(), 2);
        let snmp = &f.detectors.detector[0];
        assert_eq!(snmp.name, "SNMP");
        assert_eq!(
            snmp.class,
            "org.opennms.netmgt.provision.detector.snmp.SnmpDetector"
        );
        assert_eq!(snmp.parameter.len(), 2);
        assert_eq!(snmp.parameter[0].key, "port");
        assert_eq!(snmp.parameter[0].value, "161");
        // Parameter-less detector parses with zero parameters.
        assert_eq!(f.detectors.detector[1].parameter.len(), 0);
    }

    #[test]
    fn foreign_source_policies_parse() {
        let f = parse_foreign_source(FOREIGN_SOURCE_FIXTURE).unwrap();
        assert_eq!(f.policies.policy.len(), 1);
        let p = &f.policies.policy[0];
        assert_eq!(p.name, "Production tag");
        assert_eq!(p.parameter.len(), 2);
    }

    #[test]
    fn foreign_source_without_policies_parses_empty() {
        let f = parse_foreign_source(FOREIGN_SOURCE_NO_POLICIES).unwrap();
        assert_eq!(f.policies.policy.len(), 0);
        assert_eq!(f.detectors.detector.len(), 1);
    }

    #[test]
    fn malformed_xml_returns_error() {
        let bad = "<model-import><node foreign-id=\"x\"";
        assert!(parse_requisition(bad).is_err());
    }
}
