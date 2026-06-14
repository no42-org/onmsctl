/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Wire-format DTOs for `/rest/sched-outages` (the v1 scheduled-outages service).
//!
//! Mirrored from the OpenNMS `poll-outages` model (`Outage extends
//! BasicSchedule`, `Outages`, `Time`, `Interface`, `Node`). The v1 service
//! `@Produces`/`@Consumes` JSON; JAXB→JSON may serialize a single
//! `time`/`interface`/`node`/`outage` element as a bare object rather than a
//! 1-element array, so those fields deserialize **one-or-many** (and tolerate an
//! absent/`null` collection). Permissive on deserialize; omits empties on
//! serialize.
//!
//! NOTE: derived from the model source, not yet a captured live payload —
//! confirm the JSON shape (one-or-many, attribute casing) against a real Horizon
//! (change task 1/9).

use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize a field that may arrive as a single object, an array, or be
/// absent/`null`, into a `Vec<T>`.
fn one_or_many<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    // `Many` (a sequence) MUST be tried before `One`: serde can deserialize a
    // struct positionally from a sequence, so a single-element array like
    // `[{ "id": 7 }]` would otherwise match `One(T)` with the field bound to the
    // wrong (nested) value. Trying `Many` first parses arrays correctly; a bare
    // single object falls through to `One`.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        Many(Vec<T>),
        One(T),
    }
    Ok(match Option::<OneOrMany<T>>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(OneOrMany::Many(v)) => v,
        Some(OneOrMany::One(x)) => vec![x],
    })
}

/// A scheduled outage (`Outage extends BasicSchedule`): name + type + the time
/// windows, plus the interface/node selectors. The daemon/package *attachments*
/// are NOT part of this object (they live in the daemon configs).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Outage {
    pub name: String,
    /// `specific` / `daily` / `weekly` / `monthly`.
    #[serde(rename = "type")]
    pub schedule_type: String,
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub time: Vec<Time>,
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub interface: Vec<Interface>,
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub node: Vec<Node>,
}

/// One start/end window. `day` is set for weekly/monthly; `id` is server-assigned.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Time {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    pub begins: String,
    pub ends: String,
}

/// An interface selector: an IP `address`, or the literal `match-any`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Interface {
    pub address: String,
}

/// A node selector by server nodeId.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Node {
    pub id: i64,
}

/// Minimal view of the v2 `GET /api/v2/nodes?_s=…` list response: the node ids
/// plus the paging counts used to detect truncation. The `node` element may
/// arrive as a single object or be absent; `id` is a string or a number on the
/// wire. Permissive — we ignore everything else the node carries.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeList {
    #[serde(
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub node: Vec<NodeIdRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<i64>,
}

/// One node entry — only the `id` is modeled (string or number on the wire).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeIdRef {
    pub id: serde_json::Value,
}

/// The `GET /rest/sched-outages` collection wrapper (`{ "outage": [ … ] }`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Outages {
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub outage: Vec<Outage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outage_round_trips_camelcase() {
        let j = r#"{
            "name": "weekend", "type": "specific",
            "time": [ { "begins": "20-Jun-2026 22:00:00", "ends": "21-Jun-2026 04:00:00" } ],
            "interface": [ { "address": "192.168.8.8" } ],
            "node": [ { "id": 12 } ]
        }"#;
        let o: Outage = serde_json::from_str(j).expect("parses");
        assert_eq!(o.name, "weekend");
        assert_eq!(o.schedule_type, "specific");
        assert_eq!(o.time.len(), 1);
        assert_eq!(o.node[0].id, 12);
        let out = serde_json::to_string(&o).unwrap();
        assert!(out.contains("\"type\":\"specific\""));
        let re: Outage = serde_json::from_str(&out).unwrap();
        assert_eq!(o, re);
    }

    #[test]
    fn one_or_many_tolerates_single_object_and_null() {
        // A single `time`/`interface` as a bare object (JAXB-JSON), node null.
        let j = r#"{
            "name": "w", "type": "daily",
            "time": { "begins": "22:00:00", "ends": "23:00:00" },
            "interface": { "address": "match-any" },
            "node": null
        }"#;
        let o: Outage = serde_json::from_str(j).expect("single-object + null tolerated");
        assert_eq!(o.time.len(), 1);
        assert_eq!(o.interface[0].address, "match-any");
        assert!(o.node.is_empty());
    }

    #[test]
    fn one_or_many_single_element_array_is_not_misparsed() {
        // Regression: a SINGLE-element array must not be deserialized as a
        // positional struct (serde allows struct-from-sequence). Both a
        // one-element and a single-object `interface` must yield the same value.
        let arr: Outage = serde_json::from_str(
            r#"{ "name": "w", "type": "daily", "interface": [ { "address": "10.0.0.1" } ] }"#,
        )
        .unwrap();
        let obj: Outage = serde_json::from_str(
            r#"{ "name": "w", "type": "daily", "interface": { "address": "10.0.0.1" } }"#,
        )
        .unwrap();
        assert_eq!(
            arr.interface,
            vec![Interface {
                address: "10.0.0.1".into()
            }]
        );
        assert_eq!(arr.interface, obj.interface);
    }

    #[test]
    fn outages_wrapper_parses() {
        let j = r#"{ "outage": [ { "name": "a", "type": "daily", "time": { "begins": "01:00:00", "ends": "02:00:00" } } ] }"#;
        let os: Outages = serde_json::from_str(j).expect("parses");
        assert_eq!(os.outage.len(), 1);
        assert_eq!(os.outage[0].name, "a");
    }
}
