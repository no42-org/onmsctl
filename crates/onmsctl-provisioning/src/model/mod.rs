/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Composite `kind: Requisition` data model — the local (YAML) DTOs.
//!
//! Top-level document: [`RequisitionLocal`]. The composite kind embeds
//! both the requisition data ([`Node`] / [`Interface`] / [`Service`]) and
//! the foreign-source definition ([`ForeignSourceSpec`] / [`Detector`] /
//! [`Policy`]) per design D1.
//!
//! All structs use `#[serde(deny_unknown_fields)]` so unknown keys at any
//! nesting level fail parse rather than being silently dropped — matches
//! the cli-core "strict on serialization, strict on local parse"
//! convention. `apiVersion` and `kind` use custom deserialization that
//! validates the exact literal so a typo or wrong-version document is
//! rejected with a clear error before any HTTP call.
//!
//! Server-side wire-format DTOs (`RequisitionServer` /
//! `ForeignSourceServer`) and the `From`/`Into` conversions are filled in
//! by task 2.4 of the `add-provisioning-capability` change once a sample
//! Horizon payload is available to mirror.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod convert;
pub mod server;

pub use convert::{requisition_from_wire, requisition_to_wire};

// ---------------------------------------------------------------------------
// Validation helpers (used via `#[serde(deserialize_with = ...)]`)
// ---------------------------------------------------------------------------

/// Deserialize a `String` and reject if it is empty. Pinned at parse time
/// so the strict-on-local-parse convention isn't violated by empty
/// required strings (which would build malformed REST paths or be silently
/// rejected by the server with cryptic errors).
fn deserialize_non_empty<'de, D>(d: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Err(serde::de::Error::custom("must be a non-empty string"));
    }
    Ok(s)
}

/// Deserialize `spec.nodes` and reject structural collisions:
///
/// 1. Duplicate `foreignId` across nodes (would silently overwrite at apply time).
/// 2. Duplicate `ip` across interfaces on the same node (same risk per node).
///
/// Other uniqueness invariants (duplicate categories, duplicate parameter
/// keys, duplicate detector/policy names) are intentionally NOT enforced
/// here per the third-pass review decision — the server tolerates them or
/// operators dedup intentionally.
fn deserialize_nodes_strict<'de, D>(d: D) -> Result<Vec<Node>, D::Error>
where
    D: Deserializer<'de>,
{
    let nodes = Vec::<Node>::deserialize(d)?;
    let mut seen_fids: HashSet<&str> = HashSet::new();
    for n in &nodes {
        if !seen_fids.insert(n.foreign_id.as_str()) {
            return Err(serde::de::Error::custom(format!(
                "duplicate foreignId {:?} across spec.nodes",
                n.foreign_id
            )));
        }
        let mut seen_ips: HashSet<&str> = HashSet::new();
        for iface in &n.interfaces {
            if !seen_ips.insert(iface.ip.as_str()) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate interface ip {:?} on node {:?}",
                    iface.ip, n.foreign_id
                )));
            }
        }
    }
    Ok(nodes)
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The only accepted `apiVersion` literal at this revision.
pub const API_VERSION: &str = "provisioning.opennms.org/v1";

/// The only accepted `kind` literal at this revision.
pub const KIND: &str = "Requisition";

// ---------------------------------------------------------------------------
// Top-level document
// ---------------------------------------------------------------------------

/// Composite `kind: Requisition` YAML document (top-level).
///
/// Parse-time invariants:
/// - `apiVersion` must equal [`API_VERSION`] exactly (custom deserialization)
/// - `kind` must equal [`KIND`] exactly (custom deserialization)
/// - `metadata.name` must be present (no default)
/// - `spec.nodes` must be present (no default; may be an empty `[]`)
/// - `spec.foreignSource` is optional per design D1 (default-FS inherit)
/// - Unknown fields at any nesting level fail parse
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequisitionLocal {
    #[serde(rename = "apiVersion")]
    pub api_version: ApiVersion,
    pub kind: Kind,
    pub metadata: Metadata,
    pub spec: Spec,
}

// ---------------------------------------------------------------------------
// Validated literal types: ApiVersion and Kind
// ---------------------------------------------------------------------------

/// Newtype wrapping the `apiVersion` literal. Deserialization rejects any
/// value other than [`API_VERSION`] so parse fails fast with a clear error
/// for typos or wrong-version documents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiVersion;

impl Serialize for ApiVersion {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(API_VERSION)
    }
}

impl<'de> Deserialize<'de> for ApiVersion {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        if s != API_VERSION {
            return Err(serde::de::Error::custom(format!(
                "unsupported apiVersion {s:?}; expected {API_VERSION:?}"
            )));
        }
        Ok(ApiVersion)
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(API_VERSION)
    }
}

impl JsonSchema for ApiVersion {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ApiVersion".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "const": API_VERSION,
            "description": "API version literal. Must equal 'provisioning.opennms.org/v1'."
        })
    }
}

/// Newtype wrapping the `kind` literal. Same shape as [`ApiVersion`] —
/// deserialization rejects anything other than [`KIND`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind;

impl Serialize for Kind {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(KIND)
    }
}

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        if s != KIND {
            return Err(serde::de::Error::custom(format!(
                "unsupported kind {s:?}; expected {KIND:?}"
            )));
        }
        Ok(Kind)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(KIND)
    }
}

impl JsonSchema for Kind {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Kind".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "const": KIND,
            "description": "Kind literal. Must equal 'Requisition'."
        })
    }
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// Document metadata. `name` is the foreign-source identifier — the value
/// used in the REST path `/requisitions/{fs}` and `/foreignSources/{fs}`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    #[serde(deserialize_with = "deserialize_non_empty")]
    pub name: String,
}

// ---------------------------------------------------------------------------
// Spec — composite body
// ---------------------------------------------------------------------------

/// Composite spec carrying both the foreign-source definition and the
/// requisition data. `foreignSource` is optional per D1: omission means
/// "inherit Horizon's default foreign-source." `nodes` is required (use
/// `[]` for a requisition with no nodes yet).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spec {
    /// Optional per D1. Absence triggers default-FS inheritance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_source: Option<ForeignSourceSpec>,

    /// Required. May be empty. Parse-time invariants enforced via
    /// [`deserialize_nodes_strict`]: no duplicate `foreignId` across the
    /// array, no duplicate `ip` across interfaces within the same node.
    #[serde(deserialize_with = "deserialize_nodes_strict")]
    pub nodes: Vec<Node>,
}

// ---------------------------------------------------------------------------
// Foreign-source half
// ---------------------------------------------------------------------------

/// Foreign-source definition: scan recipe + detection / policy logic
/// applied during import. All fields optional — a `ForeignSourceSpec`
/// with all defaults is a valid declaration to "use Horizon's behavior"
/// while still pinning that intent in the YAML.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForeignSourceSpec {
    /// Humanized scan-interval like `1d`, `30m`. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_interval: Option<String>,

    /// Ordered list of detectors (provisiond runs them top-down).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detectors: Vec<Detector>,

    /// Ordered list of policies (evaluated in order; first match wins
    /// per provisiond semantics).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<Policy>,
}

/// A detector entry. `class` is optional only because the convert
/// migrator may surface a name-only entry from sparse XML; in practice
/// production declarations carry the class.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Detector {
    #[serde(deserialize_with = "deserialize_non_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
}

/// A policy entry. `class` is required — policies without a class are
/// not exercisable by provisiond.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    #[serde(deserialize_with = "deserialize_non_empty")]
    pub name: String,
    #[serde(deserialize_with = "deserialize_non_empty")]
    pub class: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
}

/// `{key, value}` pair as used by detector / policy `parameters` arrays.
/// OpenNMS XML uses this shape verbatim.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    pub key: String,
    pub value: String,
}

// ---------------------------------------------------------------------------
// Requisition half — nodes / interfaces / services / categories / assets
// ---------------------------------------------------------------------------

/// A node within the requisition. `foreignId` is the stable identifier
/// in the source-of-truth (e.g. CMDB id, hostname). `label` is the
/// human-readable display name (becomes Horizon's `nodeLabel`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    #[serde(deserialize_with = "deserialize_non_empty")]
    pub foreign_id: String,
    #[serde(deserialize_with = "deserialize_non_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<Interface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// Asset record (free-form `key: value` pairs). Stored as `BTreeMap`
    /// so the YAML output is sorted and diffs stay stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, String>,
}

/// A network interface on a node. `ip` is required; `services` and
/// `snmpPrimary` are optional. The IP is parsed only at apply time
/// (the model accepts a string so the YAML schema can be edited
/// off-server without an IP-parser dependency at this layer).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Interface {
    #[serde(deserialize_with = "deserialize_non_empty")]
    pub ip: String,
    /// Service names as known to provisiond (`ICMP`, `SNMP`, `HTTP`, …).
    /// Strings rather than an enum because the catalog is server-side
    /// and grows independently of this CLI.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snmp_primary: Option<SnmpPrimary>,
}

/// SNMP-primary discriminator on an interface.
///
/// - `P` — primary SNMP interface for data collection
/// - `S` — secondary SNMP interface
/// - `N` — not eligible for SNMP
///
/// Variants are renamed explicitly via `#[serde(rename = ...)]` so the
/// wire contract (`P` / `S` / `N`) is pinned independent of the Rust
/// variant identifiers. Deserialization accepts only the three
/// single-character literals; any other value (`Primary`, `primary`,
/// `1`) is rejected as an unknown variant.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[schemars(
    description = "SNMP-primary discriminator on an interface. `P` = primary SNMP \
                   interface for data collection; `S` = secondary SNMP interface; \
                   `N` = not eligible for SNMP."
)]
pub enum SnmpPrimary {
    #[serde(rename = "P")]
    P,
    #[serde(rename = "S")]
    S,
    #[serde(rename = "N")]
    N,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Realistic full-shape document — used as the canonical positive case.
    const FULL_YAML: &str = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  foreignSource:
    scanInterval: 1d
    detectors:
      - name: ICMP
      - name: SNMP
        class: org.opennms.netmgt.provision.detector.snmp.SnmpDetector
    policies:
      - name: MatchByCategory
        class: org.opennms.netmgt.provision.persist.policies.NodeCategorySettingPolicy
        parameters:
          - { key: matchBehavior, value: ALL_PARAMETERS }
          - { key: category,      value: Production }
  nodes:
    - foreignId: web01
      label: web01.acme
      interfaces:
        - ip: 10.0.0.5
          snmpPrimary: P
          services: [ICMP, SNMP, HTTP]
      categories: [Production, Web]
      assets:
        building: HQ
        rack: A4
"#;

    fn parse(yaml: &str) -> Result<RequisitionLocal, serde_norway::Error> {
        serde_norway::from_str(yaml)
    }

    // -- Positive cases ----------------------------------------------------

    #[test]
    fn composite_document_is_accepted() {
        let doc = parse(FULL_YAML).expect("full YAML parses");
        assert_eq!(doc.metadata.name, "acme-prod");
        assert_eq!(doc.spec.nodes.len(), 1);
        let n = &doc.spec.nodes[0];
        assert_eq!(n.foreign_id, "web01");
        assert_eq!(n.label, "web01.acme");
        assert_eq!(n.interfaces.len(), 1);
        assert_eq!(n.interfaces[0].ip, "10.0.0.5");
        assert_eq!(n.interfaces[0].snmp_primary, Some(SnmpPrimary::P));
        assert_eq!(n.interfaces[0].services, vec!["ICMP", "SNMP", "HTTP"]);
        assert_eq!(n.categories, vec!["Production", "Web"]);
        assert_eq!(n.assets.get("building"), Some(&"HQ".to_string()));
        assert_eq!(n.assets.get("rack"), Some(&"A4".to_string()));
        let fs = doc
            .spec
            .foreign_source
            .as_ref()
            .expect("foreignSource present");
        assert_eq!(fs.scan_interval.as_deref(), Some("1d"));
        assert_eq!(fs.detectors.len(), 2);
        assert_eq!(fs.policies.len(), 1);
    }

    #[test]
    fn omitted_foreign_source_is_accepted() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
      label: web01.acme
"#;
        let doc = parse(yaml).expect("portable-style document parses");
        assert!(doc.spec.foreign_source.is_none());
    }

    #[test]
    fn empty_nodes_array_is_accepted() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes: []
"#;
        let doc = parse(yaml).expect("empty-nodes document parses");
        assert!(doc.spec.nodes.is_empty());
    }

    #[test]
    fn round_trip_preserves_content() {
        let original = parse(FULL_YAML).expect("parse");
        let serialized = serde_norway::to_string(&original).expect("serialize");
        let reparsed = parse(&serialized).expect("re-parse");
        assert_eq!(original, reparsed);
    }

    // -- Negative cases: spec scenarios ------------------------------------

    #[test]
    fn missing_foreign_id_is_rejected_at_parse() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - label: web01.acme
"#;
        let err = parse(yaml).expect_err("missing foreignId rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("foreignId"),
            "error should name the missing field; got: {msg}"
        );
    }

    #[test]
    fn missing_label_is_rejected_at_parse() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
"#;
        let err = parse(yaml).expect_err("missing label rejected");
        assert!(err.to_string().contains("label"));
    }

    #[test]
    fn missing_metadata_name_is_rejected_at_parse() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata: {}
spec:
  nodes: []
"#;
        let err = parse(yaml).expect_err("missing metadata.name rejected");
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn wrong_api_version_is_rejected_at_parse() {
        let yaml = r#"
apiVersion: v2
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes: []
"#;
        let err = parse(yaml).expect_err("wrong apiVersion rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported apiVersion") && msg.contains("v2"),
            "error should name the bad value; got: {msg}"
        );
    }

    #[test]
    fn wrong_kind_is_rejected_at_parse() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Source
metadata:
  name: acme-prod
spec:
  nodes: []
"#;
        let err = parse(yaml).expect_err("wrong kind rejected");
        assert!(err.to_string().contains("unsupported kind"));
    }

    #[test]
    fn unknown_field_at_root_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes: []
extra: oops
"#;
        let err = parse(yaml).expect_err("unknown root field rejected");
        assert!(err.to_string().contains("extra"));
    }

    #[test]
    fn unknown_field_on_node_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
      label: web01.acme
      mystery: yes
"#;
        let err = parse(yaml).expect_err("unknown nested field rejected");
        assert!(err.to_string().contains("mystery"));
    }

    #[test]
    fn invalid_snmp_primary_value_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
      label: web01.acme
      interfaces:
        - ip: 10.0.0.5
          snmpPrimary: Primary
"#;
        let err = parse(yaml).expect_err("non-PSN snmpPrimary rejected");
        let msg = err.to_string();
        // Serde rejects unknown enum variants with an "unknown variant"
        // phrase; the variant literal `Primary` must appear so the
        // operator can locate the bad value. Substring `"primary"` alone
        // is too loose — `snmpPrimary` would falsely match.
        assert!(
            msg.contains("unknown variant") && msg.contains("Primary"),
            "expected an 'unknown variant `Primary`'-style error, got: {msg}"
        );
    }

    // -- Asserting on the literal types ------------------------------------

    #[test]
    fn api_version_constants_are_exact() {
        assert_eq!(API_VERSION, "provisioning.opennms.org/v1");
        assert_eq!(KIND, "Requisition");
    }

    // -- Non-empty validation (P3) -----------------------------------------

    #[test]
    fn empty_metadata_name_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: ""
spec:
  nodes: []
"#;
        let err = parse(yaml).expect_err("empty metadata.name rejected");
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn empty_node_foreign_id_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: ""
      label: web01.acme
"#;
        let err = parse(yaml).expect_err("empty foreignId rejected");
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn empty_interface_ip_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
      label: web01.acme
      interfaces:
        - ip: ""
"#;
        let err = parse(yaml).expect_err("empty interface.ip rejected");
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn empty_policy_class_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  foreignSource:
    policies:
      - name: MatchByCategory
        class: ""
  nodes: []
"#;
        let err = parse(yaml).expect_err("empty policy.class rejected");
        assert!(err.to_string().contains("non-empty"));
    }

    // -- Structural uniqueness (P10 / decision DN1) ------------------------

    #[test]
    fn duplicate_foreign_id_across_nodes_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
      label: web01.acme
    - foreignId: web01
      label: web01-dup.acme
"#;
        let err = parse(yaml).expect_err("duplicate foreignId rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate foreignId") && msg.contains("web01"),
            "got: {msg}"
        );
    }

    #[test]
    fn duplicate_interface_ip_on_same_node_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
      label: web01.acme
      interfaces:
        - ip: 10.0.0.5
        - ip: 10.0.0.5
"#;
        let err = parse(yaml).expect_err("duplicate interface ip rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate interface ip") && msg.contains("10.0.0.5"),
            "got: {msg}"
        );
    }

    #[test]
    fn duplicate_ip_across_different_nodes_is_accepted() {
        // Uniqueness is per-node, not workspace-wide. Two nodes claiming
        // 10.0.0.5 is unusual but not a structural collision at the
        // requisition layer.
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
      label: web01.acme
      interfaces:
        - ip: 10.0.0.5
    - foreignId: web02
      label: web02.acme
      interfaces:
        - ip: 10.0.0.5
"#;
        parse(yaml).expect("per-node uniqueness only; cross-node IP dup allowed");
    }

    // -- Missing-required-field tests (P5) ---------------------------------

    #[test]
    fn missing_api_version_is_rejected() {
        let yaml = r#"
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes: []
"#;
        let err = parse(yaml).expect_err("missing apiVersion rejected");
        assert!(err.to_string().contains("apiVersion"));
    }

    #[test]
    fn missing_kind_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
metadata:
  name: acme-prod
spec:
  nodes: []
"#;
        let err = parse(yaml).expect_err("missing kind rejected");
        assert!(err.to_string().contains("kind"));
    }

    #[test]
    fn missing_metadata_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
spec:
  nodes: []
"#;
        let err = parse(yaml).expect_err("missing metadata rejected");
        assert!(err.to_string().contains("metadata"));
    }

    #[test]
    fn missing_spec_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
"#;
        let err = parse(yaml).expect_err("missing spec rejected");
        assert!(err.to_string().contains("spec"));
    }

    #[test]
    fn missing_spec_nodes_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec: {}
"#;
        let err = parse(yaml).expect_err("missing spec.nodes rejected");
        assert!(err.to_string().contains("nodes"));
    }

    // -- Deny-unknown-fields at every nesting level (P4) -------------------

    #[test]
    fn unknown_field_on_metadata_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
  mystery: yes
spec:
  nodes: []
"#;
        let err = parse(yaml).expect_err("unknown metadata field rejected");
        assert!(err.to_string().contains("mystery"));
    }

    #[test]
    fn unknown_field_on_spec_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes: []
  mystery: yes
"#;
        let err = parse(yaml).expect_err("unknown spec field rejected");
        assert!(err.to_string().contains("mystery"));
    }

    #[test]
    fn unknown_field_on_foreign_source_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  foreignSource:
    mystery: yes
  nodes: []
"#;
        let err = parse(yaml).expect_err("unknown foreignSource field rejected");
        assert!(err.to_string().contains("mystery"));
    }

    #[test]
    fn unknown_field_on_interface_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
      label: web01.acme
      interfaces:
        - ip: 10.0.0.5
          mystery: yes
"#;
        let err = parse(yaml).expect_err("unknown interface field rejected");
        assert!(err.to_string().contains("mystery"));
    }

    #[test]
    fn unknown_field_on_detector_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  foreignSource:
    detectors:
      - name: ICMP
        mystery: yes
  nodes: []
"#;
        let err = parse(yaml).expect_err("unknown detector field rejected");
        assert!(err.to_string().contains("mystery"));
    }

    #[test]
    fn unknown_field_on_policy_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  foreignSource:
    policies:
      - name: P1
        class: org.opennms.example.Policy
        mystery: yes
  nodes: []
"#;
        let err = parse(yaml).expect_err("unknown policy field rejected");
        assert!(err.to_string().contains("mystery"));
    }

    #[test]
    fn unknown_field_on_parameter_is_rejected() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  foreignSource:
    detectors:
      - name: SNMP
        parameters:
          - key: timeout
            value: "1000"
            mystery: yes
  nodes: []
"#;
        let err = parse(yaml).expect_err("unknown parameter field rejected");
        assert!(err.to_string().contains("mystery"));
    }

    // -- MAY-optional acceptance (P6) --------------------------------------

    #[test]
    fn interface_with_omitted_snmp_primary_and_services_is_accepted() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  nodes:
    - foreignId: web01
      label: web01.acme
      interfaces:
        - ip: 10.0.0.5
"#;
        let doc = parse(yaml).expect("MAY-optional interface fields can be omitted");
        let iface = &doc.spec.nodes[0].interfaces[0];
        assert!(iface.snmp_primary.is_none());
        assert!(iface.services.is_empty());
    }

    #[test]
    fn empty_foreign_source_block_is_accepted() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  foreignSource: {}
  nodes: []
"#;
        let doc = parse(yaml).expect("empty foreignSource block is valid");
        let fs = doc.spec.foreign_source.expect("present");
        assert!(fs.scan_interval.is_none());
        assert!(fs.detectors.is_empty());
        assert!(fs.policies.is_empty());
    }

    // -- Wire-shape pinning for SnmpPrimary (P9) ---------------------------

    #[test]
    fn snmp_primary_serializes_to_single_char() {
        // serde_norway emits YAML scalar plus trailing newline; trim
        // before comparing.
        let p = serde_norway::to_string(&SnmpPrimary::P).unwrap();
        let s = serde_norway::to_string(&SnmpPrimary::S).unwrap();
        let n = serde_norway::to_string(&SnmpPrimary::N).unwrap();
        assert_eq!(p.trim(), "P");
        assert_eq!(s.trim(), "S");
        assert_eq!(n.trim(), "N");
    }

    // -- Idempotent serialization (P8) -------------------------------------

    #[test]
    fn serialize_is_idempotent_on_reparse() {
        // yaml → struct → yaml(1) → struct → yaml(2) must produce
        // yaml(1) == yaml(2). Surfaces non-determinism (insertion-order
        // dependence, hash-map iteration, …) before it lands in shipped
        // diffs.
        let original = parse(FULL_YAML).expect("parse");
        let s1 = serde_norway::to_string(&original).expect("serialize once");
        let reparsed = parse(&s1).expect("re-parse");
        let s2 = serde_norway::to_string(&reparsed).expect("serialize twice");
        assert_eq!(s1, s2, "serialize must be idempotent on re-parse");
    }
}
