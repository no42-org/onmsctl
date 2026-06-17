/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local (YAML) model for `kind: BusinessService` — the operator-facing
//! document that `onmsctl apply -f` reconciles into an OpenNMS Business Service.
//!
//! Named, multi-instance: `metadata.name` is the Business Service name. The
//! `spec` carries `attributes`, a per-service `reduceFunction`, and four edge
//! collections (`childServices` / `ipServices` / `applications` /
//! `reductionKeys`). All references are by NAME — child services and
//! applications by name, IP services by a structured `node` reference plus
//! `ipAddress`+`service`, reduction keys by literal string (optionally
//! templated). Everything that can be validated without the server happens in
//! [`BusinessServiceLocal::validate`], before any HTTP request.

use std::collections::BTreeMap;

use onmsctl_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only accepted `apiVersion`.
pub const API_VERSION: &str = "bsm.opennms.org/v1";
/// The only accepted `kind`.
pub const KIND: &str = "BusinessService";
/// The default monitoring-location name when a `node` reference omits one.
pub const DEFAULT_LOCATION: &str = "Default";

/// The known map-function type names (per-edge severity transform).
pub const MAP_FUNCTIONS: &[&str] = &["Identity", "Increase", "Decrease", "Ignore", "SetTo"];
/// The known reduce-function type names (per-service aggregation).
pub const REDUCE_FUNCTIONS: &[&str] = &[
    "HighestSeverity",
    "Threshold",
    "HighestSeverityAbove",
    "ExponentialPropagation",
];
/// The OpenNMS severity vocabulary (used by `SetTo.status` and
/// `HighestSeverityAbove.threshold`), low → high.
pub const STATUSES: &[&str] = &[
    "Indeterminate",
    "Normal",
    "Warning",
    "Minor",
    "Major",
    "Critical",
];
/// The node-derived template tokens a reduction-key edge may use (case-insensitive).
pub const TEMPLATE_TOKENS: &[&str] = &["nodeId", "foreignSource", "foreignId", "nodeLabel"];

fn default_weight() -> i64 {
    1
}

/// A `kind: BusinessService` document.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BusinessServiceLocal {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: Spec,
}

/// Document metadata. `name` is the Business Service name.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
}

/// The Business Service body.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spec {
    /// Arbitrary key/value attributes (flat in YAML; the wire array-of-pairs
    /// wrapper is handled in [`crate::server`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
    /// The per-service reduce function. Optional in YAML; onmsctl defaults it to
    /// `HighestSeverity` when omitted (the server has no default and rejects an
    /// absent reduce function). Per-edge `mapFunction` defaults to `Identity` the
    /// same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduce_function: Option<Function>,
    /// Edges to other Business Services (by name).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_services: Vec<ChildEdge>,
    /// Edges to monitored IP services (by node reference + ip + service).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ip_services: Vec<IpServiceEdge>,
    /// Edges to OpenNMS Applications (by name).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applications: Vec<ApplicationEdge>,
    /// Edges to raw alarm reduction keys (literal string, optionally templated).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reduction_keys: Vec<ReductionKeyEdge>,
}

/// A map/reduce function: a `type` discriminator plus a free-form string-valued
/// `properties` map (the BSM wire shape).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Function {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

/// Edge → another Business Service, referenced by name.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChildEdge {
    pub name: String,
    #[serde(default = "default_weight")]
    pub weight: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_function: Option<Function>,
}

/// Edge → a monitored IP service. The node is resolved by label/location (or
/// foreignSource/foreignId), then `ip`+`service` resolve to the monitored
/// service's id (ifserviceid).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IpServiceEdge {
    pub node: NodeRef,
    pub ip_address: String,
    pub service: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_function: Option<Function>,
}

/// Edge → an OpenNMS Application, referenced by name.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationEdge {
    pub name: String,
    #[serde(default = "default_weight")]
    pub weight: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_function: Option<Function>,
}

/// Edge → a raw alarm reduction key. The `reductionKey` string MAY contain
/// node-derived `{{token}}`s, expanded at apply time from the optional `node`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReductionKeyEdge {
    pub reduction_key: String,
    /// Required only when `reductionKey` contains a `{{token}}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_function: Option<Function>,
}

/// A structured node reference. Either `{label, location?}` (ergonomic;
/// `location` defaults to [`DEFAULT_LOCATION`]) or `{foreignSource, foreignId}`
/// (the only guaranteed-unique node key). The two forms are mutually exclusive.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_id: Option<String>,
}

/// Which identification form a [`NodeRef`] uses, after validation.
pub enum NodeRefForm<'a> {
    /// `{label, location}` — `location` defaulted to [`DEFAULT_LOCATION`].
    LabelLocation { label: &'a str, location: &'a str },
    /// `{foreignSource, foreignId}`.
    Foreign {
        foreign_source: &'a str,
        foreign_id: &'a str,
    },
}

impl NodeRef {
    /// Resolve which form this reference uses, or a config error if the two
    /// forms are mixed / incomplete. `location` defaults to [`DEFAULT_LOCATION`].
    pub fn form(&self) -> Result<NodeRefForm<'_>> {
        let has_label = self.label.is_some();
        let has_foreign = self.foreign_source.is_some() || self.foreign_id.is_some();
        match (has_label, has_foreign) {
            (true, true) => Err(cfg(
                "node: use either {label, location} or {foreignSource, foreignId}, not both".into(),
            )),
            (false, false) => Err(cfg(
                "node: must set either `label` (with optional `location`) or both `foreignSource` and `foreignId`".into(),
            )),
            (true, false) => {
                if self.location.is_some() && self.label.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(cfg("node.label must not be empty".into()));
                }
                let label = self.label.as_deref().unwrap_or("");
                if label.trim().is_empty() {
                    return Err(cfg("node.label must not be empty".into()));
                }
                Ok(NodeRefForm::LabelLocation {
                    label,
                    location: self.location.as_deref().unwrap_or(DEFAULT_LOCATION),
                })
            }
            (false, true) => {
                let fs = self.foreign_source.as_deref().unwrap_or("");
                let fid = self.foreign_id.as_deref().unwrap_or("");
                if self.location.is_some() {
                    return Err(cfg(
                        "node.location applies only to a {label, location} reference".into(),
                    ));
                }
                if fs.trim().is_empty() || fid.trim().is_empty() {
                    return Err(cfg(
                        "node: foreignSource and foreignId must both be non-empty".into(),
                    ));
                }
                Ok(NodeRefForm::Foreign {
                    foreign_source: fs,
                    foreign_id: fid,
                })
            }
        }
    }
}

impl BusinessServiceLocal {
    /// Validate the document offline, returning the first user-actionable error.
    /// Covers the API literals, the function vocabulary + parameter ranges,
    /// edge weights, node references, and reduction-key templating.
    pub fn validate(&self) -> Result<()> {
        if self.api_version != API_VERSION {
            return Err(cfg(format!(
                "apiVersion must be {API_VERSION:?}, got {:?}",
                self.api_version
            )));
        }
        if self.kind != KIND {
            return Err(cfg(format!("kind must be {KIND:?}, got {:?}", self.kind)));
        }
        if self.metadata.name.trim().is_empty() {
            return Err(cfg("metadata.name must not be empty".into()));
        }

        if let Some(rf) = &self.spec.reduce_function {
            validate_reduce_function(rf).map_err(|m| cfg(format!("spec.reduceFunction: {m}")))?;
        }

        for (i, e) in self.spec.child_services.iter().enumerate() {
            if e.name.trim().is_empty() {
                return Err(cfg(format!(
                    "spec.childServices[{i}].name must not be empty"
                )));
            }
            validate_edge_common(e.weight, &e.map_function)
                .map_err(|m| cfg(format!("spec.childServices[{i}]: {m}")))?;
        }
        for (i, e) in self.spec.applications.iter().enumerate() {
            if e.name.trim().is_empty() {
                return Err(cfg(format!(
                    "spec.applications[{i}].name must not be empty"
                )));
            }
            validate_edge_common(e.weight, &e.map_function)
                .map_err(|m| cfg(format!("spec.applications[{i}]: {m}")))?;
        }
        for (i, e) in self.spec.ip_services.iter().enumerate() {
            let prefix = format!("spec.ipServices[{i}]");
            e.node
                .form()
                .map_err(|err| cfg(format!("{prefix}.{err}")))?;
            if e.ip_address.trim().parse::<std::net::IpAddr>().is_err() {
                return Err(cfg(format!(
                    "{prefix}.ipAddress: invalid IP {:?}",
                    e.ip_address
                )));
            }
            if e.service.trim().is_empty() {
                return Err(cfg(format!("{prefix}.service must not be empty")));
            }
            validate_edge_common(e.weight, &e.map_function)
                .map_err(|m| cfg(format!("{prefix}: {m}")))?;
        }
        for (i, e) in self.spec.reduction_keys.iter().enumerate() {
            let prefix = format!("spec.reductionKeys[{i}]");
            if e.reduction_key.trim().is_empty() {
                return Err(cfg(format!("{prefix}.reductionKey must not be empty")));
            }
            validate_edge_common(e.weight, &e.map_function)
                .map_err(|m| cfg(format!("{prefix}: {m}")))?;
            if let Some(n) = &e.node {
                n.form().map_err(|err| cfg(format!("{prefix}.{err}")))?;
            }
            validate_template(&e.reduction_key, e.node.as_ref())
                .map_err(|m| cfg(format!("{prefix}: {m}")))?;
        }
        Ok(())
    }
}

/// Validate an edge's `weight` and optional `mapFunction`.
fn validate_edge_common(
    weight: i64,
    map_function: &Option<Function>,
) -> std::result::Result<(), String> {
    if weight < 1 {
        return Err(format!("weight must be >= 1, got {weight}"));
    }
    if let Some(mf) = map_function {
        validate_map_function(mf)?;
    }
    Ok(())
}

/// Validate a map function (per-edge). Returns a message fragment on failure.
fn validate_map_function(f: &Function) -> std::result::Result<(), String> {
    if !MAP_FUNCTIONS.contains(&f.type_.as_str()) {
        return Err(format!(
            "mapFunction.type {:?} is not a known map function ({})",
            f.type_,
            MAP_FUNCTIONS.join(", ")
        ));
    }
    if f.type_ == "SetTo" {
        match f.properties.get("status") {
            None => return Err("mapFunction SetTo requires a `status` property".into()),
            Some(s) if !is_status(s) => {
                return Err(format!(
                    "mapFunction SetTo.status {s:?} is not a valid status"
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Validate a reduce function (per-service). Returns a message fragment on failure.
fn validate_reduce_function(f: &Function) -> std::result::Result<(), String> {
    if !REDUCE_FUNCTIONS.contains(&f.type_.as_str()) {
        return Err(format!(
            "type {:?} is not a known reduce function ({})",
            f.type_,
            REDUCE_FUNCTIONS.join(", ")
        ));
    }
    match f.type_.as_str() {
        "Threshold" => {
            let t = require_prop(f, "threshold")?;
            let v: f64 = t
                .parse()
                .map_err(|_| format!("threshold {t:?} is not a number"))?;
            if v <= 0.0 || v > 1.0 {
                return Err(format!("threshold must be in (0, 1], got {v}"));
            }
        }
        "HighestSeverityAbove" => {
            let t = require_prop(f, "threshold")?;
            if !is_status(t) {
                return Err(format!("threshold {t:?} is not a valid status"));
            }
        }
        "ExponentialPropagation" => {
            let b = require_prop(f, "base")?;
            let v: f64 = b
                .parse()
                .map_err(|_| format!("base {b:?} is not a number"))?;
            if v <= 1.0 {
                return Err(format!("base must be > 1.0, got {v}"));
            }
        }
        _ => {}
    }
    Ok(())
}

fn require_prop<'a>(f: &'a Function, key: &str) -> std::result::Result<&'a str, String> {
    f.properties
        .get(key)
        .map(|s| s.as_str())
        .ok_or_else(|| format!("{} requires a `{key}` property", f.type_))
}

/// True when `s` names an OpenNMS severity (case-insensitive).
fn is_status(s: &str) -> bool {
    STATUSES.iter().any(|k| k.eq_ignore_ascii_case(s.trim()))
}

/// Validate the `{{token}}`s in a reduction-key string: every token must be a
/// known node-derived token (case-insensitive); a token requires a node; a node
/// with no token is a likely mistake; and each token must be DERIVABLE from the
/// node's form (`{{foreignSource}}`/`{{foreignId}}` need a `{foreignSource,
/// foreignId}` node; `{{nodeLabel}}` needs a `{label, location}` node). The
/// form check is done here, offline, so a mismatch fails at plan rather than
/// deferring to apply-time expansion.
fn validate_template(key: &str, node: Option<&NodeRef>) -> std::result::Result<(), String> {
    let tokens = template_tokens(key);
    for t in &tokens {
        if !TEMPLATE_TOKENS.iter().any(|k| k.eq_ignore_ascii_case(t)) {
            return Err(format!(
                "reductionKey uses unsupported template token {{{{{t}}}}} (supported: {})",
                TEMPLATE_TOKENS.join(", ")
            ));
        }
    }
    match node {
        None => {
            if !tokens.is_empty() {
                return Err(
                    "reductionKey contains a {{token}} but no `node` reference was given to resolve it"
                        .into(),
                );
            }
        }
        Some(n) => {
            if tokens.is_empty() {
                return Err(
                    "a `node` reference was given but reductionKey contains no {{token}} to expand (likely a mistake)".into(),
                );
            }
            for t in &tokens {
                match t.to_ascii_lowercase().as_str() {
                    "foreignsource" if n.foreign_source.is_none() => {
                        return Err("{{foreignSource}} requires a {foreignSource, foreignId} node reference".into());
                    }
                    "foreignid" if n.foreign_id.is_none() => {
                        return Err(
                            "{{foreignId}} requires a {foreignSource, foreignId} node reference"
                                .into(),
                        );
                    }
                    "nodelabel" if n.label.is_none() => {
                        return Err(
                            "{{nodeLabel}} requires a {label, location} node reference".into()
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

/// Extract the `{{token}}` names from a string (the text between `{{` and `}}`,
/// trimmed). Tolerates surrounding whitespace inside the braces.
pub fn template_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        if let Some(close) = after.find("}}") {
            out.push(after[..close].trim().to_string());
            rest = &after[close + 2..];
        } else {
            break;
        }
    }
    out
}

fn cfg(m: String) -> Error {
    Error::Config(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> std::result::Result<BusinessServiceLocal, serde_norway::Error> {
        serde_norway::from_str(yaml)
    }

    const FULL: &str = r#"
apiVersion: bsm.opennms.org/v1
kind: BusinessService
metadata: { name: web-frontend }
spec:
  attributes: { owner: platform }
  reduceFunction:
    type: Threshold
    properties: { threshold: "0.75" }
  childServices:
    - { name: database-tier, weight: 2, mapFunction: { type: Increase } }
  ipServices:
    - node: { label: webhost01, location: Default }
      ipAddress: 10.0.0.10
      service: HTTP
      friendlyName: web-http
  applications:
    - { name: Webservers }
  reductionKeys:
    - reductionKey: "uei.opennms.org/nodes/nodeDown::1"
      mapFunction: { type: SetTo, properties: { status: Major } }
"#;

    #[test]
    fn full_document_parses_and_validates() {
        let doc = parse(FULL).expect("parses");
        doc.validate().expect("valid");
        assert_eq!(doc.spec.child_services[0].weight, 2);
        // weight defaults to 1 when omitted
        assert_eq!(doc.spec.ip_services[0].weight, 1);
        assert_eq!(
            doc.spec.ip_services[0].node.label.as_deref(),
            Some("webhost01")
        );
    }

    #[test]
    fn unknown_field_rejected() {
        let err = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  bogus: 1\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn raw_id_child_reference_rejected() {
        // A child referenced by numeric `id` instead of `name` is an unknown
        // field (and missing `name`) → parse error.
        let err = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  childServices:\n    - { id: 5 }\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn unknown_reduce_function_rejected() {
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reduceFunction: { type: Bogus }\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("not a known reduce function")
        );
    }

    #[test]
    fn threshold_out_of_range_rejected() {
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reduceFunction: { type: Threshold, properties: { threshold: \"1.5\" } }\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("in (0, 1]")
        );
    }

    #[test]
    fn exponential_base_must_exceed_one() {
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reduceFunction: { type: ExponentialPropagation, properties: { base: \"1.0\" } }\n",
        )
        .unwrap();
        assert!(doc.validate().unwrap_err().to_string().contains("> 1.0"));
    }

    #[test]
    fn setto_requires_valid_status() {
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reductionKeys:\n    - { reductionKey: k, mapFunction: { type: SetTo, properties: { status: Nope } } }\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("not a valid status")
        );
    }

    #[test]
    fn weight_must_be_positive() {
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  applications:\n    - { name: Web, weight: 0 }\n",
        )
        .unwrap();
        assert!(doc.validate().unwrap_err().to_string().contains("weight"));
    }

    #[test]
    fn node_ref_forms_are_mutually_exclusive() {
        let mixed = NodeRef {
            label: Some("a".into()),
            foreign_source: Some("fs".into()),
            foreign_id: Some("fid".into()),
            ..Default::default()
        };
        assert!(mixed.form().is_err());
        let empty = NodeRef::default();
        assert!(empty.form().is_err());
        let label = NodeRef {
            label: Some("a".into()),
            ..Default::default()
        };
        match label.form().unwrap() {
            NodeRefForm::LabelLocation { location, .. } => assert_eq!(location, "Default"),
            _ => panic!("expected label/location"),
        }
        let fr = NodeRef {
            foreign_source: Some("fs".into()),
            foreign_id: Some("fid".into()),
            ..Default::default()
        };
        assert!(matches!(fr.form().unwrap(), NodeRefForm::Foreign { .. }));
    }

    #[test]
    fn template_token_requires_node() {
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reductionKeys:\n    - { reductionKey: \"x::{{nodeId}}:y\" }\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("no `node`")
        );
    }

    #[test]
    fn unknown_template_token_rejected() {
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reductionKeys:\n    - { reductionKey: \"x::{{ifIndex}}\", node: { label: n } }\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("unsupported template token")
        );
    }

    #[test]
    fn token_incompatible_with_node_form_is_rejected_offline() {
        // {{foreignId}} with a label-form node has nothing to expand from.
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reductionKeys:\n    - { reductionKey: \"x::{{foreignId}}\", node: { label: n } }\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("requires a {foreignSource, foreignId}")
        );
        // {{nodeLabel}} with a foreign-form node likewise.
        let doc2 = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reductionKeys:\n    - { reductionKey: \"x::{{nodeLabel}}\", node: { foreignSource: fs, foreignId: fid } }\n",
        )
        .unwrap();
        assert!(
            doc2.validate()
                .unwrap_err()
                .to_string()
                .contains("requires a {label, location}")
        );
    }

    #[test]
    fn node_without_token_is_rejected() {
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reductionKeys:\n    - { reductionKey: \"literal::1\", node: { label: n } }\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("no {{token}}")
        );
    }

    #[test]
    fn template_tokens_extracts_names() {
        assert_eq!(
            template_tokens("a::{{nodeId}}:b:{{ foreignId }}"),
            vec!["nodeId".to_string(), "foreignId".to_string()]
        );
        assert!(template_tokens("no tokens here").is_empty());
    }

    #[test]
    fn case_insensitive_token_and_status() {
        assert!(is_status("major"));
        assert!(is_status("CRITICAL"));
        // case-insensitive token accepted (validate passes with node present)
        let doc = parse(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: { name: a }\nspec:\n  reductionKeys:\n    - { reductionKey: \"x::{{nodeid}}\", node: { label: n } }\n",
        )
        .unwrap();
        doc.validate().expect("lowercase token accepted");
    }
}
