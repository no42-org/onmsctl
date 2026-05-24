/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Three-level diff engine for the composite `kind: Requisition` model.
//!
//! - **L1** ([`l1_compare`]): canonicalization + byte-equality
//!   compare. Returns [`L1Result::Unchanged`] / [`L1Result::Changed`].
//!   Drives the early-exit "nothing to apply" path so a CI run that
//!   parses an unchanged YAML file does no mutating HTTP work.
//! - **L2** ([`diff_requisition`]): per-node semantic delta — added /
//!   removed / modified — keyed by `foreignId`. Feeds the
//!   `--diff` preview output (task 4.5, future).
//! - **L3** ([`LeafChange`]): per-leaf detail within modified nodes
//!   (and within `spec.foreignSource`), yielding `{path, from, to}`
//!   records that feed the rescan-relevance classifier and
//!   `--explain-rescan` / `-o json` output.
//! - **Rescan classifier** ([`classify_leaf`] / [`aggregate_rescan_decision`]):
//!   maps leaf paths to `ScanRelevance::{Relevant, Irrelevant}` per
//!   design D3, used by the apply path to pick `rescanExisting=true|false`.
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

use serde::Serialize;
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
    serde_json::to_vec(&canonical_value(req)).expect("canonical Value serializes as JSON")
}

/// Produce the canonical [`serde_json::Value`] for a `RequisitionLocal`.
/// Same normalization rules as [`canonicalize`]; exposed so the L2/L3
/// diff machinery can compare canonical forms directly without
/// round-tripping through bytes.
pub fn canonical_value(req: &RequisitionLocal) -> Value {
    let mut value: Value = serde_json::to_value(req).expect("RequisitionLocal serializes as JSON");
    normalize(&mut value, &mut Vec::new());
    value
}

// ---------------------------------------------------------------------------
// L2 per-node delta + L3 per-leaf delta (tasks 4.3, 4.4)
// ---------------------------------------------------------------------------

/// Whole-requisition diff. L2 partitions the node list into added /
/// removed / modified subsets (keyed by `foreignId`); L3 supplies the
/// per-leaf detail attached to each modified node. Foreign-source
/// changes are surfaced as a separate leaf list since the foreignSource
/// half lives outside the node array.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub struct RequisitionDelta {
    /// Leaf changes within `spec.foreignSource` (or whole-block
    /// add/remove when one side omits the field). Paths are rooted at
    /// `spec.foreignSource`.
    pub foreign_source_changes: Vec<LeafChange>,
    /// Nodes present locally but not remotely (by `foreignId`).
    pub nodes_added: Vec<NodeRef>,
    /// Nodes present remotely but not locally.
    pub nodes_removed: Vec<NodeRef>,
    /// Nodes present in both but differing.
    pub nodes_modified: Vec<NodeModification>,
}

impl RequisitionDelta {
    /// `true` when there are no changes — equivalent to L1's
    /// `Unchanged` outcome. Useful when callers want the structured
    /// diff anyway (e.g. for `--dry-run`).
    pub fn is_empty(&self) -> bool {
        self.foreign_source_changes.is_empty()
            && self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && self.nodes_modified.is_empty()
    }

    /// Iterate every leaf-path string in the delta, regardless of
    /// which bucket it lives in. Used by the apply path to feed the
    /// rescan-relevance classifier — node add/remove emits a synthetic
    /// path so the classifier sees them as `spec.nodes[*]` events
    /// (which default to `Relevant`).
    pub fn iter_paths(&self) -> impl Iterator<Item = &str> {
        let fs = self.foreign_source_changes.iter().map(|c| c.path.as_str());
        // Node add/remove → synthetic `spec.nodes[*]` path so the
        // classifier treats them as scan-relevant.
        let added = self.nodes_added.iter().map(|_| "spec.nodes[*]");
        let removed = self.nodes_removed.iter().map(|_| "spec.nodes[*]");
        let modified = self
            .nodes_modified
            .iter()
            .flat_map(|m| m.leaves.iter().map(|c| c.path.as_str()));
        fs.chain(added).chain(removed).chain(modified)
    }
}

/// A node identified by its `foreignId`, used as a summary entry for
/// L2's add/remove buckets. The canonical-form JSON value is attached
/// so consumers that need richer detail (e.g. `-o json` output) can
/// render the whole node without re-fetching.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NodeRef {
    pub foreign_id: String,
    pub value: Value,
}

/// A node that exists in both local and remote with substantive
/// differences. `leaves` is the L3 per-leaf delta computed for the
/// node's body; paths are rooted at `spec.nodes[*]` so they feed the
/// rescan classifier directly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NodeModification {
    pub foreign_id: String,
    pub leaves: Vec<LeafChange>,
}

/// A single leaf-path change. `from` is the remote (server) side;
/// `to` is the local (YAML) side. Either side may be `Value::Null`
/// when the leaf was added or removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LeafChange {
    /// Dotted path from the document root, with `[N]` for array
    /// indices. Example: `spec.nodes[*].interfaces[0].snmpPrimary`.
    pub path: String,
    /// Value present on the remote side. `Value::Null` means the
    /// leaf was absent remotely (i.e. the local change adds it).
    pub from: Value,
    /// Value present on the local side. `Value::Null` means the
    /// leaf is absent locally (i.e. the local change removes it).
    pub to: Value,
}

/// Compute the L2+L3 diff between two requisitions. Both sides are
/// canonicalized first so:
///   - Reordered set members (categories, services) don't surface as
///     changes.
///   - Reordered node arrays / interface arrays don't either
///     (they're keyed by `foreignId` / `ip` and sorted).
///   - Reordered detector / policy arrays DO surface — they're
///     order-sensitive.
pub fn diff_requisition(local: &RequisitionLocal, remote: &RequisitionLocal) -> RequisitionDelta {
    let local_v = canonical_value(local);
    let remote_v = canonical_value(remote);

    let mut delta = RequisitionDelta::default();

    // --- spec.foreignSource ---
    let local_fs = local_v
        .pointer("/spec/foreignSource")
        .unwrap_or(&Value::Null);
    let remote_fs = remote_v
        .pointer("/spec/foreignSource")
        .unwrap_or(&Value::Null);
    diff_value(
        local_fs,
        remote_fs,
        "spec.foreignSource",
        &mut delta.foreign_source_changes,
    );

    // --- spec.nodes ---
    let empty: Vec<Value> = Vec::new();
    let local_nodes = local_v
        .pointer("/spec/nodes")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let remote_nodes = remote_v
        .pointer("/spec/nodes")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    // Index by foreignId. After canonicalization both arrays are
    // already sorted by foreignId, but we still index for O(1) lookup
    // during partitioning.
    let local_by_fid: std::collections::BTreeMap<&str, &Value> =
        local_nodes.iter().map(|n| (foreign_id_of(n), n)).collect();
    let remote_by_fid: std::collections::BTreeMap<&str, &Value> =
        remote_nodes.iter().map(|n| (foreign_id_of(n), n)).collect();

    for (fid, local_node) in &local_by_fid {
        match remote_by_fid.get(fid) {
            Some(remote_node) if local_node == remote_node => {
                // No change for this node.
            }
            Some(remote_node) => {
                let mut leaves = Vec::new();
                diff_value(local_node, remote_node, "spec.nodes[*]", &mut leaves);
                delta.nodes_modified.push(NodeModification {
                    foreign_id: fid.to_string(),
                    leaves,
                });
            }
            None => delta.nodes_added.push(NodeRef {
                foreign_id: fid.to_string(),
                value: (*local_node).clone(),
            }),
        }
    }
    for (fid, remote_node) in &remote_by_fid {
        if !local_by_fid.contains_key(fid) {
            delta.nodes_removed.push(NodeRef {
                foreign_id: fid.to_string(),
                value: (*remote_node).clone(),
            });
        }
    }

    delta
}

/// Pull the `foreignId` string out of a canonical node Value. Returns
/// the empty string if absent or non-string (parse-time validation
/// should make this case unreachable for well-formed input).
fn foreign_id_of(node: &Value) -> &str {
    node.get("foreignId").and_then(Value::as_str).unwrap_or("")
}

/// Recursively diff two `Value`s, emitting one [`LeafChange`] per
/// substantive difference. Walks objects key-by-key and arrays
/// position-by-position. The canonicalization step ensures arrays are
/// already in stable order — for set-like arrays a reordering looks
/// identical; for ordered arrays (detectors / policies) the diff
/// surfaces position-mismatches as the user expects.
///
/// Path format: dotted segments with `[N]` for array indices. Example
/// path produced inside a node modification:
/// `spec.nodes[*].interfaces[0].snmpPrimary`.
fn diff_value(local: &Value, remote: &Value, path: &str, out: &mut Vec<LeafChange>) {
    if local == remote {
        return;
    }
    match (local, remote) {
        (Value::Object(l), Value::Object(r)) => {
            // Walk the union of keys so missing-on-either-side surfaces
            // as a leaf change at the child path.
            let mut keys: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            keys.extend(l.keys().map(String::as_str));
            keys.extend(r.keys().map(String::as_str));
            for k in keys {
                let lv = l.get(k).unwrap_or(&Value::Null);
                let rv = r.get(k).unwrap_or(&Value::Null);
                let child_path = if path.is_empty() {
                    k.to_string()
                } else {
                    format!("{path}.{k}")
                };
                diff_value(lv, rv, &child_path, out);
            }
        }
        (Value::Array(l), Value::Array(r)) => {
            let max = l.len().max(r.len());
            for i in 0..max {
                let lv = l.get(i).unwrap_or(&Value::Null);
                let rv = r.get(i).unwrap_or(&Value::Null);
                let child_path = format!("{path}[{i}]");
                diff_value(lv, rv, &child_path, out);
            }
        }
        // Leaf-level difference (scalars, mixed-type, or one-side-null).
        _ => out.push(LeafChange {
            path: path.to_string(),
            from: remote.clone(),
            to: local.clone(),
        }),
    }
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
// Rescan-relevance classification (task 4.7, design.md §D3)
// ---------------------------------------------------------------------------
//
// The future L3 layer emits a set of leaf-path strings describing every
// change between local and remote. The apply path then needs to decide
// whether to import with `rescanExisting=true` (re-scans every node,
// expensive) or `rescanExisting=false` (cheap, only imports newly-added
// or definition-changed nodes). The classifier below decides per-leaf;
// the aggregator does the OR — any single Relevant leaf forces a full
// rescan.
//
// Unknown leaf paths fall through to `Relevant` (conservative default):
// better to do an unnecessary rescan than to silently skip one that
// turns out to be needed.

/// Whether a single leaf change requires re-scanning existing nodes on
/// the server. Per design D3, scan-relevant changes are those that
/// affect what provisiond would discover (node identity, interface
/// services, SNMP discriminator, detection/policy logic); scan-
/// irrelevant changes are pure metadata (display labels, asset records,
/// scan-interval, category tags — provisiond doesn't re-collect data
/// for them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanRelevance {
    /// Touching this leaf changes a scan input. Apply MUST trigger
    /// `rescanExisting=true` so already-imported nodes pick up the
    /// new value on the next scan cycle.
    Relevant,
    /// Touching this leaf does not affect scan inputs. Apply MAY use
    /// `rescanExisting=false` if no other change in the diff is
    /// `Relevant`.
    Irrelevant,
}

/// Classify a single leaf-path string against the rescan-relevance
/// table in `design.md §D3`. Paths are JSON-Pointer-like with `[N]` for
/// concrete array indices; they're normalized to `[*]` before matching
/// so the classifier doesn't care which specific node/interface index
/// is affected.
///
/// The table is encoded by *irrelevant* paths (small, finite set);
/// everything else defaults to `Relevant`. This biases toward
/// conservative correctness — a new leaf path that the classifier
/// doesn't recognize triggers a rescan rather than silently skipping
/// one.
pub fn classify_leaf(path: &str) -> ScanRelevance {
    let normalized = normalize_indices(path);
    let s = normalized.as_str();

    // Irrelevant paths (design.md §D3). Match either exactly or as a
    // prefix when the path could end at a nested field (e.g.
    // `spec.nodes[*].assets.building` is irrelevant because
    // `spec.nodes[*].assets` is).
    const IRRELEVANT: &[&str] = &[
        "spec.nodes[*].label",
        "spec.nodes[*].categories",
        "spec.nodes[*].assets",
        "spec.foreignSource.scanInterval",
    ];

    for &prefix in IRRELEVANT {
        if s == prefix
            || s.starts_with(&format!("{prefix}."))
            || s.starts_with(&format!("{prefix}["))
        {
            return ScanRelevance::Irrelevant;
        }
    }

    // Everything else: relevant. This covers `spec.nodes[*]` (whole-
    // node add/remove), `spec.nodes[*].interfaces[*]` (interfaces and
    // their fields), `spec.foreignSource.detectors[*]` /
    // `spec.foreignSource.policies[*]` (detection logic), and the
    // conservative-default catch-all.
    ScanRelevance::Relevant
}

/// Collect every leaf path that classifies as scan-relevant. Used by
/// `--explain-rescan` so the operator sees which paths drove the auto
/// decision, and by `aggregate_rescan_decision` as its single source
/// of truth. Returns an empty vector if no leaf is relevant.
pub fn scan_relevant_paths<'a, I>(paths: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    paths
        .into_iter()
        .filter(|p| classify_leaf(p) == ScanRelevance::Relevant)
        .map(String::from)
        .collect()
}

/// Aggregate per-leaf classifications into a single `rescanExisting`
/// boolean for the import call. Returns `true` iff at least one leaf
/// in the input is `Relevant`. Empty input returns `false` — but the
/// apply path should have L1-short-circuited before this point.
///
/// Delegates to [`scan_relevant_paths`] so there is a single
/// classifier source-of-truth. Allocates a `Vec` then drops it; for
/// the apply path this overhead is dwarfed by the HTTP I/O it gates.
pub fn aggregate_rescan_decision<'a, I>(paths: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    !scan_relevant_paths(paths).is_empty()
}

/// Replace `[N]` (concrete array indices) with `[*]` (wildcards) for
/// classification matching. Used internally by [`classify_leaf`].
fn normalize_indices(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            // Look ahead for digits-then-`]`
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b']' {
                out.push_str("[*]");
                i = j + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
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

    // -- Rescan classifier (task 4.7) -------------------------------------

    #[test]
    fn rescan_classifier_scan_relevant_leaves() {
        // Per design.md §D3 — these MUST trigger rescanExisting=true.
        let relevant = [
            "spec.nodes[0]",                                // whole node add/remove
            "spec.nodes[0].interfaces[0]",                  // whole interface
            "spec.nodes[0].interfaces[0].ip",               // IP change
            "spec.nodes[0].interfaces[0].services",         // services array change
            "spec.nodes[0].interfaces[0].services[2]",      // individual service
            "spec.nodes[0].interfaces[0].snmpPrimary",      // SNMP discriminator
            "spec.foreignSource.detectors",                 // detectors array
            "spec.foreignSource.detectors[0]",              // individual detector
            "spec.foreignSource.detectors[0].class",        // detector property
            "spec.foreignSource.policies",                  // policies array
            "spec.foreignSource.policies[1].parameters[0]", // deep policy edit
        ];
        for p in relevant {
            assert_eq!(
                classify_leaf(p),
                ScanRelevance::Relevant,
                "{p} should be scan-relevant"
            );
        }
    }

    #[test]
    fn rescan_classifier_scan_irrelevant_leaves() {
        // Per design.md §D3 — these do NOT need rescanExisting=true.
        let irrelevant = [
            "spec.nodes[0].label",             // display name
            "spec.nodes[0].categories",        // category array (whole)
            "spec.nodes[0].categories[0]",     // individual category
            "spec.nodes[0].assets",            // asset block (whole)
            "spec.nodes[0].assets.building",   // individual asset field
            "spec.foreignSource.scanInterval", // scan cadence (next scan picks up)
        ];
        for p in irrelevant {
            assert_eq!(
                classify_leaf(p),
                ScanRelevance::Irrelevant,
                "{p} should be scan-irrelevant"
            );
        }
    }

    #[test]
    fn rescan_classifier_unknown_path_defaults_relevant() {
        // Conservative default: an unrecognized leaf triggers a rescan.
        // Better to over-rescan once than to silently skip one we
        // should have done.
        let unknown = [
            "spec.somethingFuture",
            "spec.nodes[0].newField",
            "spec.foreignSource.newField",
            "topLevel.elsewhere",
        ];
        for p in unknown {
            assert_eq!(
                classify_leaf(p),
                ScanRelevance::Relevant,
                "{p} should default to scan-relevant"
            );
        }
    }

    #[test]
    fn rescan_classifier_normalizes_indices() {
        // Concrete indices in the input path are normalized to `[*]`
        // before matching, so the same field on node 0 vs node 999
        // classifies identically.
        assert_eq!(
            classify_leaf("spec.nodes[0].label"),
            classify_leaf("spec.nodes[999].label")
        );
        assert_eq!(
            classify_leaf("spec.nodes[0].interfaces[0].ip"),
            classify_leaf("spec.nodes[42].interfaces[7].ip")
        );
    }

    #[test]
    fn rescan_aggregator_any_relevant_means_true() {
        let paths = [
            "spec.nodes[0].label",                  // irrelevant
            "spec.nodes[0].categories",             // irrelevant
            "spec.foreignSource.detectors[0].name", // RELEVANT
        ];
        assert!(aggregate_rescan_decision(paths.iter().copied()));
    }

    #[test]
    fn rescan_aggregator_all_irrelevant_means_false() {
        let paths = [
            "spec.nodes[0].label",
            "spec.nodes[0].categories[0]",
            "spec.nodes[0].assets.building",
            "spec.foreignSource.scanInterval",
        ];
        assert!(!aggregate_rescan_decision(paths.iter().copied()));
    }

    #[test]
    fn rescan_aggregator_empty_is_false() {
        // No changes → no rescan. (The L1 layer should have
        // short-circuited before reaching this, but the aggregator's
        // contract must still be defined for the empty input.)
        let empty: [&str; 0] = [];
        assert!(!aggregate_rescan_decision(empty.iter().copied()));
    }

    // -- L2 + L3: per-node delta + per-leaf delta (tasks 4.3, 4.4) -------

    #[test]
    fn diff_empty_for_identical_requisitions() {
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      interfaces:\n        - ip: 10.0.0.5\n",
        );
        let b = a.clone();
        let d = diff_requisition(&a, &b);
        assert!(d.is_empty(), "identical inputs produce empty delta");
        assert_eq!(d.iter_paths().count(), 0);
    }

    #[test]
    fn diff_detects_added_node() {
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: w1\n    - foreignId: web02\n      label: w2\n",
        );
        let b = doc("  nodes:\n    - foreignId: web01\n      label: w1\n");
        let d = diff_requisition(&a, &b);
        assert_eq!(d.nodes_added.len(), 1);
        assert_eq!(d.nodes_added[0].foreign_id, "web02");
        assert!(d.nodes_removed.is_empty());
        assert!(d.nodes_modified.is_empty());
        // iter_paths emits a synthetic spec.nodes[*] for the add so the
        // classifier flags it as scan-relevant.
        assert!(d.iter_paths().any(|p| p == "spec.nodes[*]"));
    }

    #[test]
    fn diff_detects_removed_node() {
        let a = doc("  nodes:\n    - foreignId: web01\n      label: w1\n");
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: w1\n    - foreignId: web02\n      label: w2\n",
        );
        let d = diff_requisition(&a, &b);
        assert_eq!(d.nodes_removed.len(), 1);
        assert_eq!(d.nodes_removed[0].foreign_id, "web02");
        assert!(d.nodes_added.is_empty());
    }

    #[test]
    fn diff_detects_modified_label_as_irrelevant_leaf() {
        let a = doc("  nodes:\n    - foreignId: web01\n      label: changed\n");
        let b = doc("  nodes:\n    - foreignId: web01\n      label: original\n");
        let d = diff_requisition(&a, &b);
        assert_eq!(d.nodes_modified.len(), 1);
        let m = &d.nodes_modified[0];
        assert_eq!(m.foreign_id, "web01");
        assert!(m.leaves.iter().any(|c| c.path.ends_with(".label")));
        // Aggregator: only label changed → no rescan.
        assert!(!aggregate_rescan_decision(d.iter_paths()));
    }

    #[test]
    fn diff_detects_modified_snmp_primary_as_relevant_leaf() {
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      interfaces:\n        - ip: 10.0.0.5\n          snmpPrimary: P\n",
        );
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      interfaces:\n        - ip: 10.0.0.5\n          snmpPrimary: S\n",
        );
        let d = diff_requisition(&a, &b);
        assert_eq!(d.nodes_modified.len(), 1);
        let m = &d.nodes_modified[0];
        assert!(
            m.leaves.iter().any(|c| c.path.ends_with(".snmpPrimary")),
            "leaves = {:?}",
            m.leaves
        );
        // Aggregator: snmpPrimary is scan-relevant → rescan required.
        assert!(aggregate_rescan_decision(d.iter_paths()));
    }

    #[test]
    fn diff_detects_foreign_source_detector_reorder() {
        let a = doc(
            "  foreignSource:\n    detectors:\n      - name: ICMP\n      - name: SNMP\n  nodes: []\n",
        );
        let b = doc(
            "  foreignSource:\n    detectors:\n      - name: SNMP\n      - name: ICMP\n  nodes: []\n",
        );
        let d = diff_requisition(&a, &b);
        assert!(
            !d.foreign_source_changes.is_empty(),
            "ordered list reorder must surface"
        );
        // Aggregator: detector change is scan-relevant.
        assert!(aggregate_rescan_decision(d.iter_paths()));
    }

    #[test]
    fn diff_ignores_category_reorder() {
        // Set-like: canonicalization sorts before diff, so reordering
        // surfaces no change.
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      categories: [Production, Web]\n",
        );
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: w\n      categories: [Web, Production]\n",
        );
        let d = diff_requisition(&a, &b);
        assert!(d.is_empty(), "set reorder does not surface in L2");
    }

    #[test]
    fn diff_paths_are_rooted_at_spec() {
        // Leaf paths from node modifications must be rooted at
        // `spec.nodes[*]` so the rescan classifier sees them in their
        // full form.
        let a = doc("  nodes:\n    - foreignId: web01\n      label: changed\n");
        let b = doc("  nodes:\n    - foreignId: web01\n      label: original\n");
        let d = diff_requisition(&a, &b);
        for leaf in &d.nodes_modified[0].leaves {
            assert!(
                leaf.path.starts_with("spec.nodes[*]"),
                "leaf path '{}' should start with spec.nodes[*]",
                leaf.path
            );
        }
    }

    #[test]
    fn diff_foreign_source_paths_are_rooted() {
        let a = doc("  foreignSource:\n    scanInterval: 1d\n  nodes: []\n");
        let b = doc("  foreignSource:\n    scanInterval: 2h\n  nodes: []\n");
        let d = diff_requisition(&a, &b);
        assert!(!d.foreign_source_changes.is_empty());
        for leaf in &d.foreign_source_changes {
            assert!(
                leaf.path.starts_with("spec.foreignSource"),
                "leaf path '{}' should start with spec.foreignSource",
                leaf.path
            );
        }
        // Aggregator: only scanInterval changed → irrelevant → no rescan.
        assert!(!aggregate_rescan_decision(d.iter_paths()));
    }

    #[test]
    fn diff_mixed_buckets_at_once() {
        // Add web03, remove web04, modify web01 (label only — irrelevant
        // leaf) so we exercise all three node-buckets in one diff.
        let a = doc(
            "  nodes:\n    - foreignId: web01\n      label: NEW\n    - foreignId: web03\n      label: w3\n",
        );
        let b = doc(
            "  nodes:\n    - foreignId: web01\n      label: OLD\n    - foreignId: web04\n      label: w4\n",
        );
        let d = diff_requisition(&a, &b);
        let added_ids: Vec<_> = d
            .nodes_added
            .iter()
            .map(|n| n.foreign_id.as_str())
            .collect();
        let removed_ids: Vec<_> = d
            .nodes_removed
            .iter()
            .map(|n| n.foreign_id.as_str())
            .collect();
        let modified_ids: Vec<_> = d
            .nodes_modified
            .iter()
            .map(|n| n.foreign_id.as_str())
            .collect();
        assert_eq!(added_ids, vec!["web03"]);
        assert_eq!(removed_ids, vec!["web04"]);
        assert_eq!(modified_ids, vec!["web01"]);
        // Aggregator: web03 added + web04 removed are scan-relevant (synthetic spec.nodes[*]).
        assert!(aggregate_rescan_decision(d.iter_paths()));
    }

    #[test]
    fn diff_leaf_change_carries_from_and_to() {
        let a = doc("  nodes:\n    - foreignId: web01\n      label: NEW\n");
        let b = doc("  nodes:\n    - foreignId: web01\n      label: OLD\n");
        let d = diff_requisition(&a, &b);
        let label_change = d.nodes_modified[0]
            .leaves
            .iter()
            .find(|c| c.path.ends_with(".label"))
            .expect("label leaf change present");
        assert_eq!(label_change.from, Value::String("OLD".into()));
        assert_eq!(label_change.to, Value::String("NEW".into()));
    }

    #[test]
    fn diff_added_foreign_source_block_surfaces() {
        // local has foreignSource, remote omits it → foreign-source-side
        // change surfaces with one or more leaves.
        let a = doc("  foreignSource:\n    scanInterval: 1d\n  nodes: []\n");
        let b = doc("  nodes: []\n");
        let d = diff_requisition(&a, &b);
        assert!(
            !d.foreign_source_changes.is_empty(),
            "adding foreignSource must surface"
        );
        // Aggregator: foreignSource-level change defaults to relevant.
        assert!(aggregate_rescan_decision(d.iter_paths()));
    }
}
