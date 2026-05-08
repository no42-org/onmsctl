/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! UEI-bucket diff for `EventSource` apply.
//!
//! Per `design.md §5.3` algorithm:
//!
//!   1. Bucket local and server events by `uei`.
//!   2. For UEIs present in both buckets where each side has exactly one
//!      event, match the pair and produce a per-field diff (`~` for
//!      modified, `=` for unchanged).
//!   3. For UEIs only in local: emit `+`.
//!   4. For UEIs only in server: emit `-`.
//!   5. For UEIs with multiple entries on either side (duplicate-UEI
//!      cluster), emit the cluster as an opaque add-or-remove block —
//!      no per-event matching attempted.
//!
//! The matching algorithm is observable: two implementations producing
//! different diffs for the same input are non-conformant.

use std::collections::BTreeMap;

use crate::apply::local::EventDef;
use crate::dto::Event;

/// Structured diff between a local YAML event set and the parsed remote
/// event set. Carries enough information for both the
/// [`onmsctl_core::Diff`] display path and outcome reporting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventSetDiff {
    /// UEIs present only in local. The associated `EventDef` is the
    /// local definition that would be uploaded.
    pub added: Vec<EventDef>,
    /// UEIs present only on the server. The associated `Event` is the
    /// server-side definition (after wire→local conversion).
    pub removed: Vec<Event>,
    /// UEIs with exactly one entry on each side, where the per-field
    /// comparison reports differences.
    pub modified: Vec<EventModification>,
    /// UEIs with exactly one entry on each side that compare equal.
    pub unchanged_uei_count: usize,
    /// UEIs with multiple entries on at least one side. Treated as opaque
    /// blocks — both sides show as add-or-remove without per-event
    /// matching.
    pub duplicate_clusters: Vec<DuplicateCluster>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventModification {
    pub uei: String,
    /// Field-level differences. Each entry: (field path, before, after).
    pub field_changes: Vec<FieldChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldChange {
    pub path: String,
    pub before: serde_json::Value,
    pub after: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateCluster {
    pub uei: String,
    pub local_count: usize,
    pub remote_count: usize,
}

impl EventSetDiff {
    /// True when the diff describes no changes at all.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.modified.is_empty()
            && self.duplicate_clusters.is_empty()
    }

    /// Counts for the summary line.
    pub fn summary_counts(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.added.len(),
            self.removed.len(),
            self.modified.len(),
            self.unchanged_uei_count,
            self.duplicate_clusters.len(),
        )
    }
}

/// Bucket local and remote events by UEI and produce the structured
/// diff. The implementation is the load-bearing observable algorithm
/// (per design.md §5.3).
pub fn diff_event_sets(local: &[EventDef], remote: &[Event]) -> EventSetDiff {
    // -- Bucketing ----
    let mut local_by_uei: BTreeMap<String, Vec<&EventDef>> = BTreeMap::new();
    for e in local {
        local_by_uei.entry(e.uei.clone()).or_default().push(e);
    }
    let mut remote_by_uei: BTreeMap<String, Vec<&Event>> = BTreeMap::new();
    for e in remote {
        if let Some(uei) = &e.uei {
            remote_by_uei.entry(uei.clone()).or_default().push(e);
        }
    }

    let mut diff = EventSetDiff::default();
    // Walk a sorted union so output is deterministic.
    let all_ueis: std::collections::BTreeSet<String> = local_by_uei
        .keys()
        .chain(remote_by_uei.keys())
        .cloned()
        .collect();

    for uei in all_ueis {
        let l = local_by_uei.get(&uei);
        let r = remote_by_uei.get(&uei);
        match (l, r) {
            (Some(ls), None) if ls.len() == 1 => {
                diff.added.push((*ls[0]).clone());
            }
            (None, Some(rs)) if rs.len() == 1 => {
                diff.removed.push((*rs[0]).clone());
            }
            (Some(ls), Some(rs)) if ls.len() == 1 && rs.len() == 1 => {
                let local_event = ls[0];
                let remote_event = rs[0];
                let changes = field_diff(local_event, remote_event);
                if changes.is_empty() {
                    diff.unchanged_uei_count += 1;
                } else {
                    diff.modified.push(EventModification {
                        uei: uei.clone(),
                        field_changes: changes,
                    });
                }
            }
            _ => {
                // Duplicate-UEI cluster. Either side has >1 entry for
                // this UEI; no per-event matching attempted.
                diff.duplicate_clusters.push(DuplicateCluster {
                    uei: uei.clone(),
                    local_count: l.map(|v| v.len()).unwrap_or(0),
                    remote_count: r.map(|v| v.len()).unwrap_or(0),
                });
            }
        }
    }
    diff
}

/// Per-field diff between a local `EventDef` and a remote wire `Event`.
/// Both are serialised through their canonical JSON shapes; the local
/// shape is converted to the wire shape first so the comparison happens
/// at a single level.
fn field_diff(local: &EventDef, remote: &Event) -> Vec<FieldChange> {
    let local_wire: Event = Event::from(local);
    // Round-trip through serde_json::Value so we can walk the maps
    // structurally and produce stable field paths.
    let local_v = serde_json::to_value(&local_wire).expect("Event always serialises to JSON");
    let remote_v = serde_json::to_value(remote).expect("Event always serialises to JSON");
    let mut out = Vec::new();
    walk(&local_v, &remote_v, "", &mut out);
    out
}

/// Recursively walk two JSON values and accumulate per-field differences.
/// Path segments use `.` for objects and `[i]` for arrays.
fn walk(
    local: &serde_json::Value,
    remote: &serde_json::Value,
    path: &str,
    out: &mut Vec<FieldChange>,
) {
    use serde_json::Value;
    match (local, remote) {
        (Value::Object(la), Value::Object(ra)) => {
            let mut keys: std::collections::BTreeSet<&String> = la.keys().collect();
            keys.extend(ra.keys());
            for k in keys {
                let l = la.get(k).unwrap_or(&Value::Null);
                let r = ra.get(k).unwrap_or(&Value::Null);
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(l, r, &child, out);
            }
        }
        (Value::Array(la), Value::Array(ra)) => {
            let n = la.len().max(ra.len());
            for i in 0..n {
                let l = la.get(i).unwrap_or(&Value::Null);
                let r = ra.get(i).unwrap_or(&Value::Null);
                let child = format!("{path}[{i}]");
                walk(l, r, &child, out);
            }
        }
        (l, r) if l == r => {}
        (l, r) => out.push(FieldChange {
            path: path.to_string(),
            before: r.clone(),
            after: l.clone(),
        }),
    }
}

/// Render a structured diff as a human-readable string. The format is
/// stable but informational — see design.md §5.3 for the markers.
pub fn render_diff(d: &EventSetDiff) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let (a, r, m, u, c) = d.summary_counts();
    writeln!(
        s,
        "spec.events: {} → {}   (+{a} added, ~{m} modified, ={u} unchanged, -{r} removed{})",
        u + r + m,
        u + a + m,
        if c > 0 {
            format!(", {c} duplicate-UEI cluster(s)")
        } else {
            String::new()
        }
    )
    .unwrap();
    for e in &d.added {
        writeln!(s, "  + {}    severity={}", e.uei, e.severity).unwrap();
    }
    for e in &d.removed {
        writeln!(
            s,
            "  - {}    severity={}",
            e.uei.as_deref().unwrap_or("?"),
            e.severity.as_deref().unwrap_or("?")
        )
        .unwrap();
    }
    for m in &d.modified {
        writeln!(s, "  ~ {}", m.uei).unwrap();
        for fc in &m.field_changes {
            let before = render_value(&fc.before);
            let after = render_value(&fc.after);
            writeln!(s, "      {}: {before} → {after}", fc.path).unwrap();
        }
    }
    for c in &d.duplicate_clusters {
        writeln!(
            s,
            "  ! {}    [duplicate-UEI cluster: local={}, remote={}]",
            c.uei, c.local_count, c.remote_count
        )
        .unwrap();
    }
    s
}

fn render_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "<absent>".into(),
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::local::EventDef;

    fn local(uei: &str, severity: &str, label: &str) -> EventDef {
        EventDef {
            uei: uei.into(),
            label: label.into(),
            severity: severity.into(),
            ..EventDef::default()
        }
    }

    fn remote(uei: &str, severity: &str, label: &str) -> Event {
        Event {
            uei: Some(uei.into()),
            event_label: Some(label.into()),
            severity: Some(severity.into()),
            ..Event::default()
        }
    }

    #[test]
    fn empty_inputs_produce_empty_diff() {
        let d = diff_event_sets(&[], &[]);
        assert!(d.is_empty());
    }

    #[test]
    fn local_only_uei_yields_addition() {
        let l = vec![local("uei.a", "Warning", "a")];
        let d = diff_event_sets(&l, &[]);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].uei, "uei.a");
        assert!(d.removed.is_empty());
        assert!(d.modified.is_empty());
    }

    #[test]
    fn remote_only_uei_yields_removal() {
        let r = vec![remote("uei.b", "Major", "b")];
        let d = diff_event_sets(&[], &r);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].uei.as_deref(), Some("uei.b"));
    }

    #[test]
    fn matching_pair_with_same_fields_is_unchanged() {
        let l = vec![local("uei.a", "Warning", "a")];
        let r = vec![remote("uei.a", "Warning", "a")];
        let d = diff_event_sets(&l, &r);
        assert!(d.is_empty());
        assert_eq!(d.unchanged_uei_count, 1);
    }

    #[test]
    fn matching_pair_with_severity_change_yields_modification() {
        let l = vec![local("uei.a", "Major", "a")];
        let r = vec![remote("uei.a", "Warning", "a")];
        let d = diff_event_sets(&l, &r);
        assert_eq!(d.modified.len(), 1);
        let m = &d.modified[0];
        assert_eq!(m.uei, "uei.a");
        // The severity change is the relevant field.
        assert!(
            m.field_changes.iter().any(|fc| fc.path == "severity"
                && fc.before == serde_json::json!("Warning")
                && fc.after == serde_json::json!("Major")),
            "expected severity change in {:?}",
            m.field_changes
        );
    }

    #[test]
    fn duplicate_uei_on_local_yields_cluster() {
        let l = vec![
            local("uei.dup", "Warning", "first"),
            local("uei.dup", "Major", "second"),
        ];
        let r = vec![remote("uei.dup", "Warning", "first")];
        let d = diff_event_sets(&l, &r);
        assert_eq!(d.duplicate_clusters.len(), 1);
        let c = &d.duplicate_clusters[0];
        assert_eq!(c.uei, "uei.dup");
        assert_eq!(c.local_count, 2);
        assert_eq!(c.remote_count, 1);
        // No per-event matching for the cluster — modified/added/removed
        // should be empty.
        assert!(d.added.is_empty());
        assert!(d.removed.is_empty());
        assert!(d.modified.is_empty());
    }

    #[test]
    fn duplicate_uei_on_remote_yields_cluster() {
        let l = vec![local("uei.dup", "Warning", "x")];
        let r = vec![
            remote("uei.dup", "Warning", "x"),
            remote("uei.dup", "Major", "y"),
        ];
        let d = diff_event_sets(&l, &r);
        assert_eq!(d.duplicate_clusters.len(), 1);
        assert_eq!(d.duplicate_clusters[0].local_count, 1);
        assert_eq!(d.duplicate_clusters[0].remote_count, 2);
    }

    #[test]
    fn mixed_diff_categorises_each_uei_correctly() {
        let l = vec![
            local("uei.add", "Warning", "added"), // addition
            local("uei.same", "Normal", "same"),  // unchanged
            local("uei.mod", "Major", "mod"),     // modified (severity)
        ];
        let r = vec![
            remote("uei.same", "Normal", "same"),
            remote("uei.mod", "Warning", "mod"),
            remote("uei.gone", "Cleared", "gone"), // removal
        ];
        let d = diff_event_sets(&l, &r);
        let (a, rm, m, u, c) = d.summary_counts();
        assert_eq!(a, 1);
        assert_eq!(rm, 1);
        assert_eq!(m, 1);
        assert_eq!(u, 1);
        assert_eq!(c, 0);
    }

    #[test]
    fn diff_output_is_deterministic_across_runs() {
        let l = vec![
            local("uei.b", "Warning", "b"),
            local("uei.a", "Warning", "a"),
        ];
        let r = vec![
            remote("uei.a", "Warning", "a"),
            remote("uei.c", "Warning", "c"),
        ];
        let d1 = diff_event_sets(&l, &r);
        let d2 = diff_event_sets(&l, &r);
        assert_eq!(format!("{d1:?}"), format!("{d2:?}"));
        // Bucketed by UEI in BTreeMap, so output order is sorted.
        assert_eq!(d1.added[0].uei, "uei.b");
        assert_eq!(d1.removed[0].uei.as_deref(), Some("uei.c"));
    }

    #[test]
    fn render_diff_produces_summary_and_markers() {
        let l = vec![
            local("uei.add", "Warning", "added"),
            local("uei.mod", "Major", "mod"),
        ];
        let r = vec![
            remote("uei.mod", "Warning", "mod"),
            remote("uei.gone", "Cleared", "gone"),
        ];
        let d = diff_event_sets(&l, &r);
        let s = render_diff(&d);
        // Summary line
        assert!(s.contains("+1 added"));
        assert!(s.contains("~1 modified"));
        assert!(s.contains("-1 removed"));
        // Per-UEI markers
        assert!(s.contains("+ uei.add"));
        assert!(s.contains("~ uei.mod"));
        assert!(s.contains("- uei.gone"));
        // Per-field detail for the modified event
        assert!(s.contains("severity"));
        assert!(s.contains("Warning"));
        assert!(s.contains("Major"));
    }

    #[test]
    fn render_diff_marks_duplicate_clusters() {
        let l = vec![
            local("uei.dup", "Warning", "x"),
            local("uei.dup", "Major", "y"),
        ];
        let r = vec![remote("uei.dup", "Warning", "x")];
        let d = diff_event_sets(&l, &r);
        let s = render_diff(&d);
        assert!(s.contains("duplicate-UEI cluster"));
        assert!(s.contains("uei.dup"));
    }
}
