/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local model → wire request conversion and the reconcile diff.
//!
//! The handler resolves every name reference to a numeric id (and expands any
//! reduction-key template), producing a fully-resolved [`server::BusinessServiceRequest`].
//! [`unchanged`] then compares that desired request against the live
//! [`server::BusinessServiceResponse`] via an order-insensitive [`Snapshot`], so
//! re-applying identical YAML is a no-op regardless of edge ordering.

use std::collections::{BTreeMap, BTreeSet};

use crate::model;
use crate::server::{self, BusinessServiceRequest, BusinessServiceResponse, FunctionDto};

/// The default map function the OpenNMS server requires on an edge — it has no
/// implicit default and 500s (`Objects.requireNonNull`) on an absent
/// `map-function`, so onmsctl materializes `Identity` when the YAML omits it.
pub const DEFAULT_MAP: &str = "Identity";
/// The default reduce function the server requires on a service (same 500 rule).
pub const DEFAULT_REDUCE: &str = "HighestSeverity";

/// Convert a local function to its wire DTO (same shape).
pub fn to_fn_dto(f: &model::Function) -> FunctionDto {
    FunctionDto {
        type_: f.type_.clone(),
        properties: f.properties.clone(),
    }
}

fn fn_dto_of_type(type_: &str) -> FunctionDto {
    FunctionDto {
        type_: type_.to_string(),
        properties: BTreeMap::new(),
    }
}

/// The wire `map-function` for an edge, defaulting to `Identity` when omitted
/// (the server rejects a null map function).
pub fn map_dto(f: &Option<model::Function>) -> FunctionDto {
    f.as_ref()
        .map(to_fn_dto)
        .unwrap_or_else(|| fn_dto_of_type(DEFAULT_MAP))
}

/// The wire `reduce-function` for a service, defaulting to `HighestSeverity`
/// when omitted (the server rejects a null reduce function).
pub fn reduce_dto(f: &Option<model::Function>) -> FunctionDto {
    f.as_ref()
        .map(to_fn_dto)
        .unwrap_or_else(|| fn_dto_of_type(DEFAULT_REDUCE))
}

/// Canonical, comparable form of a function: `(type, sorted (k,v) pairs)`.
type FnNorm = (String, Vec<(String, String)>);

/// Normalize a function for diffing, defaulting an absent one to `default_type`
/// — applied to BOTH the desired request and the live response so that an
/// omitted function (which onmsctl materializes, and the server stores) compares
/// equal regardless of which side spells out the default.
fn fn_norm(f: &Option<FunctionDto>, default_type: &str) -> FnNorm {
    match f {
        Some(f) => (
            f.type_.clone(),
            f.properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        None => (default_type.to_string(), Vec::new()),
    }
}

fn map_norm(f: &Option<FunctionDto>) -> FnNorm {
    fn_norm(f, DEFAULT_MAP)
}

fn reduce_norm(f: &Option<FunctionDto>) -> FnNorm {
    fn_norm(f, DEFAULT_REDUCE)
}

/// An order-insensitive snapshot of a Business Service's reconcilable state.
#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    attributes: BTreeMap<String, String>,
    reduce: FnNorm,
    child: BTreeSet<(i64, i64, FnNorm)>,
    ip: BTreeSet<(i64, i64, Option<String>, FnNorm)>,
    app: BTreeSet<(i64, i64, FnNorm)>,
    rkey: BTreeSet<(Vec<String>, i64, Option<String>, FnNorm)>,
}

/// Snapshot of the desired request.
pub fn snapshot_request(r: &BusinessServiceRequest) -> Snapshot {
    Snapshot {
        attributes: r.attributes.to_map(),
        reduce: reduce_norm(&r.reduce_function),
        child: r
            .child_edges
            .iter()
            .map(|e| (e.child_id, e.weight, map_norm(&e.map_function)))
            .collect(),
        ip: r
            .ip_service_edges
            .iter()
            .map(|e| {
                (
                    e.ip_service_id,
                    e.weight,
                    e.friendly_name.clone(),
                    map_norm(&e.map_function),
                )
            })
            .collect(),
        app: r
            .application_edges
            .iter()
            .map(|e| (e.application_id, e.weight, map_norm(&e.map_function)))
            .collect(),
        rkey: r
            .reduction_key_edges
            .iter()
            .map(|e| {
                (
                    vec![e.reduction_key.clone()],
                    e.weight,
                    e.friendly_name.clone(),
                    map_norm(&e.map_function),
                )
            })
            .collect(),
    }
}

/// Snapshot of the live response.
pub fn snapshot_response(r: &BusinessServiceResponse) -> Snapshot {
    Snapshot {
        attributes: r.attributes.to_map(),
        reduce: reduce_norm(&r.reduce_function),
        child: r
            .child_edges
            .iter()
            .filter_map(|e| {
                server::as_i64(&e.child_id).map(|id| (id, e.weight, map_norm(&e.map_function)))
            })
            .collect(),
        ip: r
            .ip_service_edges
            .iter()
            .filter_map(|e| {
                server::as_i64(&e.ip_service.id).map(|id| {
                    (
                        id,
                        e.weight,
                        e.friendly_name.clone(),
                        map_norm(&e.map_function),
                    )
                })
            })
            .collect(),
        app: r
            .application_edges
            .iter()
            .filter_map(|e| {
                server::as_i64(&e.application.id)
                    .map(|id| (id, e.weight, map_norm(&e.map_function)))
            })
            .collect(),
        rkey: r
            .reduction_key_edges
            .iter()
            .map(|e| {
                let mut keys = e.reduction_keys.clone();
                keys.sort();
                (
                    keys,
                    e.weight,
                    e.friendly_name.clone(),
                    map_norm(&e.map_function),
                )
            })
            .collect(),
    }
}

/// True when the desired request and the live response describe the same
/// Business Service (so no write is needed).
pub fn unchanged(desired: &BusinessServiceRequest, current: &BusinessServiceResponse) -> bool {
    snapshot_request(desired) == snapshot_response(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::*;

    fn req_with_child() -> BusinessServiceRequest {
        BusinessServiceRequest {
            name: "web".into(),
            child_edges: vec![ChildEdgeRequest {
                child_id: 2,
                weight: 1,
                map_function: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn identical_request_and_response_are_unchanged() {
        let desired = req_with_child();
        let current: BusinessServiceResponse = serde_json::from_value(serde_json::json!({
            "id": 1, "name": "web",
            "child-edges": [ { "child-id": 2, "weight": 1 } ]
        }))
        .unwrap();
        assert!(unchanged(&desired, &current));
    }

    #[test]
    fn differing_weight_is_changed() {
        let desired = req_with_child();
        let current: BusinessServiceResponse = serde_json::from_value(serde_json::json!({
            "id": 1, "name": "web",
            "child-edges": [ { "child-id": 2, "weight": 9 } ]
        }))
        .unwrap();
        assert!(!unchanged(&desired, &current));
    }

    #[test]
    fn edge_order_does_not_matter() {
        let desired = BusinessServiceRequest {
            name: "web".into(),
            reduction_key_edges: vec![
                ReductionKeyEdgeRequest {
                    reduction_key: "a".into(),
                    weight: 1,
                    friendly_name: None,
                    map_function: None,
                },
                ReductionKeyEdgeRequest {
                    reduction_key: "b".into(),
                    weight: 1,
                    friendly_name: None,
                    map_function: None,
                },
            ],
            ..Default::default()
        };
        let current: BusinessServiceResponse = serde_json::from_value(serde_json::json!({
            "id": 1, "name": "web",
            "reduction-key-edges": [
                { "reduction-keys": ["b"], "weight": 1 },
                { "reduction-keys": ["a"], "weight": 1 }
            ]
        }))
        .unwrap();
        assert!(unchanged(&desired, &current));
    }
}
