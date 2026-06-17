/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Wire DTOs for the v2 `/api/v2/business-services` REST surface.
//!
//! These mirror OpenNMS's `BusinessServiceRequestDTO` / `…ResponseDTO` exactly,
//! including the two serialization quirks confirmed against the project's own
//! marshal tests:
//!   - `attributes` serializes as an array-of-`{key,value}` wrapper
//!     (`{"attribute":[{"key":…,"value":…}]}`), NOT a flat object.
//!   - a map/reduce function's `properties` is a FLAT string-valued object
//!     (`{"threshold":"0.75"}`), and all values are strings on the wire.
//!
//! Request structs use `kebab-case` field names (`child-id`, `ip-service-id`,
//! `reduce-function`, …). The numeric ids the response carries may be a JSON
//! number or string depending on the field, so they deserialize via
//! [`as_i64`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A map/reduce function on the wire: `{ "type": …, "properties": { … } }`.
/// `properties` is a flat string→string object (NOT the attribute wrapper).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionDto {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
}

/// A single `{key, value}` attribute entry.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// The `attributes` wrapper — `{"attribute":[{"key":…,"value":…}]}`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttributeList {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribute: Vec<KeyValue>,
}

impl AttributeList {
    pub fn from_map(m: &BTreeMap<String, String>) -> Self {
        Self {
            attribute: m
                .iter()
                .map(|(k, v)| KeyValue {
                    key: k.clone(),
                    value: v.clone(),
                })
                .collect(),
        }
    }

    pub fn to_map(&self) -> BTreeMap<String, String> {
        self.attribute
            .iter()
            .map(|kv| (kv.key.clone(), kv.value.clone()))
            .collect()
    }
}

// -- Request DTOs ------------------------------------------------------------

/// `BusinessServiceRequestDTO` — the body for POST (create) and PUT (replace).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct BusinessServiceRequest {
    pub name: String,
    #[serde(default)]
    pub attributes: AttributeList,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce_function: Option<FunctionDto>,
    #[serde(default)]
    pub ip_service_edges: Vec<IpServiceEdgeRequest>,
    #[serde(default)]
    pub reduction_key_edges: Vec<ReductionKeyEdgeRequest>,
    #[serde(default)]
    pub child_edges: Vec<ChildEdgeRequest>,
    #[serde(default)]
    pub application_edges: Vec<ApplicationEdgeRequest>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ChildEdgeRequest {
    pub child_id: i64,
    pub weight: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_function: Option<FunctionDto>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct IpServiceEdgeRequest {
    pub ip_service_id: i64,
    pub weight: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_function: Option<FunctionDto>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ApplicationEdgeRequest {
    pub application_id: i64,
    pub weight: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_function: Option<FunctionDto>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ReductionKeyEdgeRequest {
    pub reduction_key: String,
    pub weight: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_function: Option<FunctionDto>,
}

// -- Response DTOs -----------------------------------------------------------

/// `BusinessServiceResponseDTO` — the body of `GET /{id}`. Only the fields the
/// reconcile needs are modeled; unknown fields (operational-status, location,
/// parent-services) are ignored.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BusinessServiceResponse {
    #[serde(default)]
    pub id: serde_json::Value,
    pub name: String,
    #[serde(default)]
    pub attributes: AttributeList,
    #[serde(default)]
    pub reduce_function: Option<FunctionDto>,
    #[serde(default)]
    pub ip_service_edges: Vec<IpServiceEdgeResponse>,
    #[serde(default)]
    pub reduction_key_edges: Vec<ReductionKeyEdgeResponse>,
    #[serde(default)]
    pub child_edges: Vec<ChildEdgeResponse>,
    #[serde(default)]
    pub application_edges: Vec<ApplicationEdgeResponse>,
}

impl BusinessServiceResponse {
    pub fn id(&self) -> Option<i64> {
        as_i64(&self.id)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ChildEdgeResponse {
    #[serde(default)]
    pub child_id: serde_json::Value,
    #[serde(default = "one")]
    pub weight: i64,
    #[serde(default)]
    pub map_function: Option<FunctionDto>,
}

/// Nested `ip-service` object inside an ip-service edge response.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct IpServiceRef {
    #[serde(default)]
    pub id: serde_json::Value,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct IpServiceEdgeResponse {
    #[serde(default)]
    pub ip_service: IpServiceRef,
    #[serde(default)]
    pub friendly_name: Option<String>,
    #[serde(default = "one")]
    pub weight: i64,
    #[serde(default)]
    pub map_function: Option<FunctionDto>,
}

/// Nested `application` object inside an application edge response.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ApplicationRef {
    #[serde(default)]
    pub id: serde_json::Value,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ApplicationEdgeResponse {
    #[serde(default)]
    pub application: ApplicationRef,
    #[serde(default = "one")]
    pub weight: i64,
    #[serde(default)]
    pub map_function: Option<FunctionDto>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ReductionKeyEdgeResponse {
    /// The response carries the resolved reduction key(s) as a set.
    #[serde(default)]
    pub reduction_keys: Vec<String>,
    #[serde(default)]
    pub friendly_name: Option<String>,
    #[serde(default = "one")]
    pub weight: i64,
    #[serde(default)]
    pub map_function: Option<FunctionDto>,
}

/// `BusinessServiceListDTO` — `GET /business-services` returns resource URIs
/// only (e.g. `/api/v2/business-services/1`), not full objects.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct BusinessServiceList {
    #[serde(rename = "business-services", default)]
    pub business_services: Vec<String>,
}

/// Minimal `GET /api/v2/applications` list view — id + name per application,
/// plus `totalCount` so a truncated/paginated response can be detected rather
/// than silently covering a subset.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ApplicationList {
    #[serde(default)]
    pub application: Vec<NamedRef>,
    #[serde(rename = "totalCount", default)]
    pub total_count: Option<i64>,
}

/// Minimal `GET /api/v2/nodes` list view.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct NodeList {
    #[serde(default)]
    pub node: Vec<NodeBrief>,
}

/// A node entry: id (string-encoded integer), label, location (plain string).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct NodeBrief {
    #[serde(default)]
    pub id: serde_json::Value,
}

/// An object with a numeric `id` and a `name` (applications, etc.).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct NamedRef {
    #[serde(default)]
    pub id: serde_json::Value,
    #[serde(default)]
    pub name: String,
}

fn one() -> i64 {
    1
}

/// Parse an OpenNMS id that REST may serialize as a JSON number or string.
pub fn as_i64(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

/// Parse the trailing numeric id from a business-service resource URI, e.g.
/// `/api/v2/business-services/12` → `12`.
pub fn id_from_uri(uri: &str) -> Option<i64> {
    uri.trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_marshal_as_array_of_pairs() {
        let mut m = BTreeMap::new();
        m.insert("dc".to_string(), "RDU".to_string());
        let list = AttributeList::from_map(&m);
        let json = serde_json::to_value(&list).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "attribute": [ { "key": "dc", "value": "RDU" } ] })
        );
    }

    #[test]
    fn function_properties_marshal_flat() {
        let f = FunctionDto {
            type_: "Threshold".into(),
            properties: BTreeMap::from([("threshold".to_string(), "0.75".to_string())]),
        };
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "Threshold", "properties": { "threshold": "0.75" } })
        );
    }

    #[test]
    fn request_uses_kebab_case_edge_fields() {
        let req = BusinessServiceRequest {
            name: "web".into(),
            child_edges: vec![ChildEdgeRequest {
                child_id: 2,
                weight: 5,
                map_function: None,
            }],
            ip_service_edges: vec![IpServiceEdgeRequest {
                ip_service_id: 1,
                weight: 1,
                friendly_name: Some("http".into()),
                map_function: None,
            }],
            ..Default::default()
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["child-edges"][0]["child-id"], 2);
        assert_eq!(json["ip-service-edges"][0]["ip-service-id"], 1);
        assert_eq!(json["ip-service-edges"][0]["friendly-name"], "http");
    }

    #[test]
    fn response_id_parses_number_or_string() {
        let r: BusinessServiceResponse =
            serde_json::from_value(serde_json::json!({ "id": 7, "name": "a" })).unwrap();
        assert_eq!(r.id(), Some(7));
        let r2: BusinessServiceResponse =
            serde_json::from_value(serde_json::json!({ "id": "9", "name": "b" })).unwrap();
        assert_eq!(r2.id(), Some(9));
    }

    #[test]
    fn id_from_uri_parses_trailing_segment() {
        assert_eq!(id_from_uri("/api/v2/business-services/12"), Some(12));
        assert_eq!(id_from_uri("/api/v2/business-services/3/"), Some(3));
        assert_eq!(id_from_uri("nonsense"), None);
    }
}
