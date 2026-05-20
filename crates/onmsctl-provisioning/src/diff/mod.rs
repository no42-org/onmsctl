/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Three-level diff engine for the composite `kind: Requisition` model.
//!
//! - **L1** (this module today): canonicalization + byte-equality
//!   compare. Returns [`L1Result::Unchanged`] / [`L1Result::Changed`].
//!   Drives the early-exit "nothing to apply" path so a CI run that
//!   parses an unchanged YAML file does no mutating HTTP work.
//! - **L2** (task 4.3, future): per-node semantic delta — added /
//!   removed / modified — drives the `--diff` preview output and feeds
//!   the rescan-classification table.
//! - **L3** (task 4.4, future): per-leaf delta within modified nodes,
//!   yielding `{path, from, to}` records for `--explain-rescan` and
//!   `-o json` machine output.
//!
//! L1 canonicalization rules — applied recursively to the JSON
//! representation of [`crate::model::RequisitionLocal`]:
//!
//! | Path                                | Rule                          |
//! |-------------------------------------|-------------------------------|
//! | (any object's keys)                 | sorted alphabetically         |
//! | `spec.nodes[]`                      | sorted by `foreignId`         |
//! | `*.interfaces[]` (on a node)        | sorted by `ip`                |
//! | `*.parameters[]` (detector/policy)  | sorted by `key`               |
//! | `spec.nodes[*].categories[]`        | sorted + deduplicated         |
//! | `*.interfaces[*].services[]`        | sorted + deduplicated         |
//! | `spec.foreignSource.detectors[]`    | preserved (ordered semantic)  |
//! | `spec.foreignSource.policies[]`     | preserved (ordered semantic)  |
//!
//! The intent: two YAML documents that differ only in cosmetic ways
//! (reordered keys, reordered set members, whitespace) produce
//! byte-identical canonical output, so L1 short-circuits without
//! disturbing server state. Two YAML documents that differ on an
//! ordered list (detector order) produce different canonical output —
//! provisiond's order-sensitive evaluation requires this.

use serde_json::{Map, Value};

use crate::model::RequisitionLocal;

/// L1 comparison outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L1Result {
    /// Local and remote canonicalize to byte-identical output — no
    /// mutating HTTP call is needed.
    Unchanged,
    /// Local and remote differ in some semantically meaningful way.
    /// The caller proceeds to L2 to characterise the change.
    Changed,
}

/// Compare two `RequisitionLocal` values for L1 equality. Returns
/// [`L1Result::Unchanged`] iff their canonical byte representations
/// are identical.
pub fn l1_compare(local: &RequisitionLocal, remote: &RequisitionLocal) -> L1Result {
    if canonicalize(local) == canonicalize(remote) {
        L1Result::Unchanged
    } else {
        L1Result::Changed
    }
}

/// Emit canonical bytes for a `RequisitionLocal`. Same logical content
/// produces byte-identical output regardless of input YAML ordering,
/// whitespace, set-member ordering, or duplicate set members.
///
/// Implementation: convert to `serde_json::Value` (which uses a
/// sorted-key map representation by default), then walk the tree
/// applying per-path normalization rules — sort node arrays by
/// identity key, dedupe + sort set arrays, preserve ordered arrays
/// as-is. Finally serialize back to JSON bytes (compact form, no
/// whitespace — only structural content matters).
pub fn canonicalize(req: &RequisitionLocal) -> Vec<u8> {
    let mut value: Value = serde_json::to_value(req).expect("RequisitionLocal serializes as JSON");
    normalize(&mut value, &mut Vec::new());
    serde_json::to_vec(&value).expect("canonical Value serializes as JSON")
}

/// Walk a JSON `Value` recursively applying canonicalization rules.
/// The `path` stack tracks the current location as field-name strings;
/// array indices are NOT pushed (we apply array rules based on the
/// enclosing field name, not on per-element position).
fn normalize(v: &mut Value, path: &mut Vec<&'static str>) {
    match v {
        Value::Object(map) => normalize_object(map, path),
        Value::Array(items) => normalize_array(items, path),
        _ => {}
    }
}

fn normalize_object(map: &mut Map<String, Value>, path: &mut Vec<&'static str>) {
    // Object key order: serde_json's Map (default features) is
    // BTreeMap-backed, so keys are already sorted on serialize. We
    // still walk into each value to normalize nested arrays.
    //
    // The `static_lifetime` trick: every field name in our model is
    // statically known. We look up each child's name in a static
    // table; if not modeled, we still recurse with the current path
    // unchanged (the array rules use suffix matching on the most
    // recently-pushed name).
    //
    // SAFETY: we don't mutate the keys themselves, just walk values.
    let keys: Vec<String> = map.keys().cloned().collect();
    for k in keys {
        let static_name = STATIC_NAMES
            .iter()
            .find(|&&n| n == k)
            .copied()
            .unwrap_or("");
        path.push(static_name);
        if let Some(val) = map.get_mut(&k) {
            normalize(val, path);
        }
        path.pop();
    }
}

/// Static interned set of every field name our model emits, for
/// stack-friendly path tracking without heap allocation per push.
/// Add a row here when a new field name is introduced in the model.
const STATIC_NAMES: &[&str] = &[
    "apiVersion",
    "kind",
    "metadata",
    "spec",
    "name",
    "foreignSource",
    "scanInterval",
    "detectors",
    "policies",
    "class",
    "parameters",
    "key",
    "value",
    "nodes",
    "foreignId",
    "label",
    "interfaces",
    "categories",
    "assets",
    "ip",
    "services",
    "snmpPrimary",
];

fn normalize_array(items: &mut Vec<Value>, path: &mut [&'static str]) {
    // Recurse into elements first so nested arrays/objects are already
    // canonical before we sort the outer array.
    for item in items.iter_mut() {
        let mut p = path.to_vec();
        normalize(item, &mut p);
    }

    // Per-field-name array rules. We match on the most recently pushed
    // field name (the array belongs to that field on its enclosing
    // object).
    let last = path.last().copied().unwrap_or("");
    match last {
        // Keyed-but-unordered: sort by identity key. Uniqueness is
        // already enforced at parse-time (D1 + DN1 from review pass 3).
        "nodes" => sort_by_string_field(items, "foreignId"),
        "interfaces" => sort_by_string_field(items, "ip"),
        "parameters" => sort_by_string_field(items, "key"),

        // Set-like: dedupe + sort. These hold string scalars.
        "categories" | "services" => sort_dedup_strings(items),

        // Ordered (detectors, policies, and anything else): preserve
        // input order — provisiond's evaluation is order-sensitive.
        _ => {}
    }
}

fn sort_by_string_field(items: &mut [Value], field: &str) {
    items.sort_by(|a, b| {
        let ka = a.get(field).and_then(Value::as_str).unwrap_or("");
        let kb = b.get(field).and_then(Value::as_str).unwrap_or("");
        ka.cmp(kb)
    });
}

fn sort_dedup_strings(items: &mut Vec<Value>) {
    items.sort_by(|a, b| {
        let sa = a.as_str().unwrap_or("");
        let sb = b.as_str().unwrap_or("");
        sa.cmp(sb)
    });
    items.dedup_by(|a, b| match (a.as_str(), b.as_str()) {
        (Some(s1), Some(s2)) => s1 == s2,
        _ => false,
    });
}

// ---------------------------------------------------------------------------
// Tests — golden-fixture invariants for L1 (task 4.6)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RequisitionLocal;

    fn parse(yaml: &str) -> RequisitionLocal {
        serde_norway::from_str(yaml).expect("YAML parses")
    }

    fn doc(extra_spec: &str) -> RequisitionLocal {
        let yaml = format!(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n{extra_spec}"
        );
        parse(&yaml)
    }

    // -- Invariants: same logical content -> same canonical bytes ---------

    #[test]
    fn reordered_object_keys_produce_same_canonical_bytes() {
        let a = parse(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  nodes:\n    - foreignId: web01\n      label: w\n",
        );
        // Same content, different YAML key order
        let b = parse(
            "kind: Requisition\n\
             spec:\n  nodes:\n    - label: w\n      foreignId: web01\n\
             metadata:\n  name: acme-prod\n\
             apiVersion: provisioning.opennms.org/v1\n",
        );
        assert_eq!(canonicalize(&a), canonicalize(&b));
        assert_eq!(l1_compare(&a, &b), L1Result::Unchanged);
    }

    #[test]
    fn reordered_categories_produce_same_canonical_bytes() {
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      categories: [Production, Web]\n",
        );
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      categories: [Web, Production]\n",
        );
        assert_eq!(canonicalize(&a), canonicalize(&b));
        assert_eq!(l1_compare(&a, &b), L1Result::Unchanged);
    }

    #[test]
    fn duplicate_categories_canonicalize_away() {
        // `set` semantics — duplicates are dropped at canonicalization.
        // (Note: structural-uniqueness for `foreignId`/`ip` rejects at
        // parse, but `categories` was a DN1 acceptable-dupe; canonical
        // form still dedupes.)
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      categories: [Production, Production, Web]\n",
        );
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      categories: [Production, Web]\n",
        );
        assert_eq!(canonicalize(&a), canonicalize(&b));
    }

    #[test]
    fn reordered_services_produce_same_canonical_bytes() {
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      interfaces:\n        - ip: 10.0.0.5\n          services: [HTTP, ICMP, SNMP]\n",
        );
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      interfaces:\n        - ip: 10.0.0.5\n          services: [SNMP, ICMP, HTTP]\n",
        );
        assert_eq!(canonicalize(&a), canonicalize(&b));
    }

    #[test]
    fn reordered_nodes_produce_same_canonical_bytes() {
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: web01\n    - foreignId: web02\n      label: web02\n",
        );
        let b = doc(
            "  nodes:\n    - foreignId: web02\n      label: web02\n    - foreignId: web01\n      label: web01\n",
        );
        assert_eq!(canonicalize(&a), canonicalize(&b));
    }

    #[test]
    fn reordered_interfaces_within_a_node_produce_same_canonical_bytes() {
        // (Same node, two interfaces with distinct IPs — uniqueness
        // already enforced at parse; we just exercise the sort step.)
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      interfaces:\n        - ip: 10.0.0.5\n        - ip: 10.0.0.6\n",
        );
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      interfaces:\n        - ip: 10.0.0.6\n        - ip: 10.0.0.5\n",
        );
        assert_eq!(canonicalize(&a), canonicalize(&b));
    }

    #[test]
    fn reordered_parameters_within_a_detector_produce_same_canonical_bytes() {
        let a = doc(
            "  foreignSource:\n    detectors:\n      - name: SNMP\n        parameters:\n          - key: timeout\n            value: \"1000\"\n          - key: retries\n            value: \"2\"\n  nodes: []\n",
        );
        let b = doc(
            "  foreignSource:\n    detectors:\n      - name: SNMP\n        parameters:\n          - key: retries\n            value: \"2\"\n          - key: timeout\n            value: \"1000\"\n  nodes: []\n",
        );
        assert_eq!(canonicalize(&a), canonicalize(&b));
    }

    // -- Invariants: ordered lists DO matter ------------------------------

    #[test]
    fn reordered_detectors_produce_different_canonical_bytes() {
        let a = doc(
            "  foreignSource:\n    detectors:\n      - name: ICMP\n      - name: SNMP\n  nodes: []\n",
        );
        let b = doc(
            "  foreignSource:\n    detectors:\n      - name: SNMP\n      - name: ICMP\n  nodes: []\n",
        );
        assert_ne!(canonicalize(&a), canonicalize(&b));
        assert_eq!(l1_compare(&a, &b), L1Result::Changed);
    }

    #[test]
    fn reordered_policies_produce_different_canonical_bytes() {
        let a = doc(
            "  foreignSource:\n    policies:\n      - name: Alpha\n        class: o.X\n      - name: Beta\n        class: o.Y\n  nodes: []\n",
        );
        let b = doc(
            "  foreignSource:\n    policies:\n      - name: Beta\n        class: o.Y\n      - name: Alpha\n        class: o.X\n  nodes: []\n",
        );
        assert_ne!(canonicalize(&a), canonicalize(&b));
    }

    // -- Invariants: substantive content changes -------------------------

    #[test]
    fn adding_a_node_changes_canonical_bytes() {
        let a = doc("  nodes:\n    - foreignId: web01\n      label: w\n");
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n    - foreignId: web02\n      label: w2\n",
        );
        assert_ne!(canonicalize(&a), canonicalize(&b));
        assert_eq!(l1_compare(&a, &b), L1Result::Changed);
    }

    #[test]
    fn changing_a_label_changes_canonical_bytes() {
        let a = doc("  nodes:\n    - foreignId: web01\n      label: original\n");
        let b = doc("  nodes:\n    - foreignId: web01\n      label: changed\n");
        assert_ne!(canonicalize(&a), canonicalize(&b));
    }

    #[test]
    fn changing_snmp_primary_changes_canonical_bytes() {
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      interfaces:\n        - ip: 10.0.0.5\n          snmpPrimary: P\n",
        );
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      interfaces:\n        - ip: 10.0.0.5\n          snmpPrimary: S\n",
        );
        assert_ne!(canonicalize(&a), canonicalize(&b));
    }

    // -- Determinism --------------------------------------------------------

    #[test]
    fn canonicalize_is_deterministic_across_repeated_calls() {
        let d = doc("  nodes:\n    - foreignId: web01\n      label: w\n");
        let bytes_a = canonicalize(&d);
        let bytes_b = canonicalize(&d);
        let bytes_c = canonicalize(&d);
        assert_eq!(bytes_a, bytes_b);
        assert_eq!(bytes_b, bytes_c);
    }

    #[test]
    fn l1_compare_reflexive() {
        let d = doc("  nodes: []\n");
        assert_eq!(l1_compare(&d, &d), L1Result::Unchanged);
    }
}
