/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Human-readable diff renderer for [`ApplyOutcome`].
//!
//! Turns the structured `ApplyOutcome` returned by
//! [`crate::apply::apply_requisition`] into a multi-line text preview
//! suitable for `requisition apply --diff` and `--dry-run`. Output is
//! deterministic (sorted iteration on every collection) so it can be
//! diff-tested and piped into review tooling.
//!
//! Format sketch:
//!
//! ```text
//! Requisition/<name> (<state>)
//!   rescanExisting: <bool>
//!
//!   - foreignSource: deleting custom (will revert to Horizon's default)
//!     displaced detectors:
//!       - <name> (<class>)
//!     displaced policies:
//!       - <name> (<class>)
//!
//!   nodes:
//!     + <foreignId>
//!     - <foreignId>
//!     ~ <foreignId>
//!         <leaf-path>: <from> → <to>
//!
//!   Note: local YAML omits spec.foreignSource — uses Horizon's default foreignSource.
//! ```
//!
//! JSON form for `-o json` (task 4.5) is intentionally NOT exposed here;
//! it serializes [`ApplyOutcome`] directly via `serde_json` once the
//! CLI layer wires it up.

use crate::apply::{ApplyOutcome, ApplyState, ForeignSourceAction};
use crate::diff::{LeafChange, RequisitionDelta};
use crate::model::RequisitionLocal;
use std::fmt::Write as _;

/// Render a human-readable diff for the apply outcome. Returns an
/// empty-changes summary when nothing would change, otherwise a
/// multi-line preview describing FS-side and node-side changes.
pub fn render_apply_diff(local: &RequisitionLocal, outcome: &ApplyOutcome) -> String {
    let mut out = String::new();
    let name = &local.metadata.name;
    let state = state_label(outcome.state);

    writeln!(out, "Requisition/{name} ({state})").ok();

    if outcome.state == ApplyState::Unchanged {
        writeln!(out, "  (no changes)").ok();
        return out;
    }

    writeln!(out, "  rescanExisting: {}", outcome.rescan_existing).ok();
    // PR001 unmodeled annotation summary (per
    // `harden-provisioning-and-eventconf-parity` design §D1). If the
    // local YAML carries an `x-onmsctl-unmodeled` block (typically
    // from `requisition convert`), surface a one-line count so the
    // operator sees the annotation exists without drowning the diff
    // body in raw key/value pairs. The annotation is stripped from
    // the wire body — this line is purely informational.
    if let Some(unmodeled) = &local.metadata.unmodeled
        && !unmodeled.is_empty()
    {
        let leaves = count_unmodeled_leaves(unmodeled);
        let word = if leaves == 1 { "entry" } else { "entries" };
        writeln!(
            out,
            "  metadata.x-onmsctl-unmodeled: {leaves} {word}"
        )
        .ok();
    }
    render_fs_section(&mut out, outcome);
    render_node_section(&mut out, &outcome.delta);

    // Footnote per design D1: emit only when the apply isn't itself
    // changing the foreign-source side. The Created/Updated/Deleted
    // arms already explain what's happening; the footnote exists to
    // warn that out-of-band changes to Horizon's default-FS are not
    // visible in this diff.
    if local.spec.foreign_source.is_none()
        && outcome.foreign_source_action == ForeignSourceAction::NoChange
    {
        writeln!(out).ok();
        writeln!(
            out,
            "  note: uses Horizon's default foreign-source; out-of-band changes to"
        )
        .ok();
        writeln!(out, "  the default are not surfaced by this diff").ok();
    }

    out
}

/// Count terminal entries in the unmodeled-annotation tree.
///
/// Nested structural containers (Mapping / Sequence) recurse so the
/// count reflects the actual number of preserved leaf values:
/// - A `Mapping` whose keys are structural (`nodes`, `<foreign-id>`,
///   `interfaces`, `<ip>`, `services`) is descended into, summing the
///   leaves under each entry.
/// - A `Sequence` (e.g. a `meta-data: [...]` block of N elements, or
///   a `future-extension: [...]` aggregation of repeated unknown
///   siblings) recurses into each element, summing their leaves.
/// - Scalar values (String / Number / Bool / Null) count as one
///   leaf each.
fn count_unmodeled_leaves(m: &serde_norway::Mapping) -> usize {
    m.values().map(count_value_leaves).sum()
}

fn count_value_leaves(v: &serde_norway::Value) -> usize {
    match v {
        serde_norway::Value::Mapping(inner) => count_unmodeled_leaves(inner),
        serde_norway::Value::Sequence(seq) => seq.iter().map(count_value_leaves).sum(),
        _ => 1,
    }
}

fn state_label(state: ApplyState) -> &'static str {
    match state {
        ApplyState::Unchanged => "unchanged",
        ApplyState::DryRun => "dry-run",
        ApplyState::Created => "create",
        ApplyState::Updated => "update",
    }
}

fn render_fs_section(out: &mut String, outcome: &ApplyOutcome) {
    let delta = &outcome.delta;
    match outcome.foreign_source_action {
        ForeignSourceAction::NoChange => {
            // FS-side leaf changes can still appear when the diff
            // baseline (default-FS substitution) differs from local.
            // Render them under a neutral header so the operator can
            // see them without claiming a write will happen.
            if !delta.foreign_source_changes.is_empty() {
                writeln!(out).ok();
                writeln!(out, "  foreignSource:").ok();
                for change in &delta.foreign_source_changes {
                    render_leaf(out, change, "    ");
                }
            }
        }
        ForeignSourceAction::Created => {
            writeln!(out).ok();
            writeln!(out, "  + foreignSource: creating custom").ok();
            for change in &delta.foreign_source_changes {
                render_leaf(out, change, "    ");
            }
        }
        ForeignSourceAction::Updated => {
            writeln!(out).ok();
            writeln!(out, "  ~ foreignSource: updating").ok();
            for change in &delta.foreign_source_changes {
                render_leaf(out, change, "    ");
            }
        }
        ForeignSourceAction::Deleted => {
            writeln!(out).ok();
            writeln!(
                out,
                "  - foreignSource: deleting custom (will revert to Horizon's default)"
            )
            .ok();
            if let Some(orig) = &outcome.original_remote_fs {
                if !orig.detectors.is_empty() {
                    writeln!(out, "    displaced detectors:").ok();
                    for d in &orig.detectors {
                        writeln!(out, "      - {} ({})", d.name, d.class).ok();
                    }
                }
                if !orig.policies.is_empty() {
                    writeln!(out, "    displaced policies:").ok();
                    for p in &orig.policies {
                        writeln!(out, "      - {} ({})", p.name, p.class).ok();
                    }
                }
            }
        }
    }
}

fn render_node_section(out: &mut String, delta: &RequisitionDelta) {
    if delta.nodes_added.is_empty()
        && delta.nodes_removed.is_empty()
        && delta.nodes_modified.is_empty()
    {
        return;
    }
    writeln!(out).ok();
    writeln!(out, "  nodes:").ok();
    for n in &delta.nodes_added {
        writeln!(out, "    + {}", n.foreign_id).ok();
    }
    for n in &delta.nodes_removed {
        writeln!(out, "    - {}", n.foreign_id).ok();
    }
    for m in &delta.nodes_modified {
        writeln!(out, "    ~ {}", m.foreign_id).ok();
        for leaf in &m.leaves {
            render_leaf(out, leaf, "        ");
        }
    }
}

fn render_leaf(out: &mut String, change: &LeafChange, indent: &str) {
    writeln!(
        out,
        "{indent}{}: {} → {}",
        change.path,
        render_value(&change.from),
        render_value(&change.to),
    )
    .ok();
}

fn render_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "<absent>".into(),
        // serde_json's string serializer escapes newlines, quotes,
        // backslashes, and control characters — preserving the
        // kubectl-style indent when an asset value or label contains
        // them. Falls back to v.to_string() for non-string scalars
        // and collections, which produce unambiguous JSON literals.
        serde_json::Value::String(_) => serde_json::to_string(v).unwrap_or_default(),
        _ => v.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::{ApplyOutcome, ApplyState, ForeignSourceAction};
    use crate::diff::{LeafChange, NodeModification, NodeRef, RequisitionDelta};
    use crate::model::server::{DetectorServer, ForeignSourceServer, PolicyServer};
    use serde_json::json;

    fn parse_local(yaml: &str) -> RequisitionLocal {
        serde_norway::from_str(yaml).expect("YAML parses")
    }

    fn minimal_local() -> RequisitionLocal {
        parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  nodes: []\n",
        )
    }

    fn local_with_fs() -> RequisitionLocal {
        parse_local(
            "apiVersion: provisioning.opennms.org/v1\n\
             kind: Requisition\n\
             metadata:\n  name: acme-prod\n\
             spec:\n  foreignSource:\n    scanInterval: 1d\n  nodes: []\n",
        )
    }

    fn outcome(
        state: ApplyState,
        rescan: bool,
        fs_action: ForeignSourceAction,
        delta: RequisitionDelta,
        original_remote_fs: Option<ForeignSourceServer>,
    ) -> ApplyOutcome {
        ApplyOutcome {
            state,
            delta,
            rescan_existing: rescan,
            foreign_source_action: fs_action,
            original_remote_fs,
            pre_trigger_last_import_ms: None,
            scan_relevant_leaves: vec![],
        }
    }

    #[test]
    fn unchanged_short_summary_no_details() {
        let local = local_with_fs();
        let o = outcome(
            ApplyState::Unchanged,
            false,
            ForeignSourceAction::NoChange,
            RequisitionDelta::default(),
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert_eq!(
            rendered,
            "Requisition/acme-prod (unchanged)\n  (no changes)\n"
        );
    }

    #[test]
    fn create_with_added_node_renders_plus_prefix() {
        let local = local_with_fs();
        let delta = RequisitionDelta {
            nodes_added: vec![NodeRef {
                foreign_id: "web01".into(),
                value: json!({}),
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Created,
            true,
            ForeignSourceAction::Created,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(rendered.contains("Requisition/acme-prod (create)"));
        assert!(rendered.contains("  rescanExisting: true"));
        assert!(rendered.contains("  + foreignSource: creating custom"));
        assert!(rendered.contains("    + web01"));
    }

    #[test]
    fn updated_with_modified_node_shows_leaf_from_to() {
        let local = local_with_fs();
        let delta = RequisitionDelta {
            nodes_modified: vec![NodeModification {
                foreign_id: "web01".into(),
                leaves: vec![LeafChange {
                    path: "spec.nodes[*].interfaces[0].snmpPrimary".into(),
                    from: json!("S"),
                    to: json!("P"),
                }],
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Updated,
            true,
            ForeignSourceAction::NoChange,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(rendered.contains("    ~ web01"));
        assert!(
            rendered.contains("        spec.nodes[*].interfaces[0].snmpPrimary: \"S\" → \"P\"")
        );
    }

    #[test]
    fn fs_deletion_enumerates_displaced_detectors_and_policies() {
        let local = minimal_local(); // omits spec.foreignSource
        let orig_fs = ForeignSourceServer {
            name: "acme-prod".into(),
            date_stamp: None,
            scan_interval: Some("30m".into()),
            detectors: vec![
                DetectorServer {
                    name: "SNMP".into(),
                    class: "org.opennms.netmgt.provision.detector.snmp.SnmpDetector".into(),
                    parameter: vec![],
                },
                DetectorServer {
                    name: "ICMP".into(),
                    class: "org.opennms.netmgt.provision.detector.icmp.IcmpDetector".into(),
                    parameter: vec![],
                },
            ],
            policies: vec![PolicyServer {
                name: "Production tag".into(),
                class: "org.opennms.netmgt.provision.persist.policies.NodeCategorySettingPolicy"
                    .into(),
                parameter: vec![],
            }],
        };
        let o = outcome(
            ApplyState::Updated,
            false,
            ForeignSourceAction::Deleted,
            RequisitionDelta::default(),
            Some(orig_fs),
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(rendered.contains("  - foreignSource: deleting custom"));
        assert!(rendered.contains("    displaced detectors:"));
        assert!(
            rendered
                .contains("      - SNMP (org.opennms.netmgt.provision.detector.snmp.SnmpDetector)")
        );
        assert!(
            rendered
                .contains("      - ICMP (org.opennms.netmgt.provision.detector.icmp.IcmpDetector)")
        );
        assert!(rendered.contains("    displaced policies:"));
        assert!(rendered.contains(
            "      - Production tag (org.opennms.netmgt.provision.persist.policies.NodeCategorySettingPolicy)"
        ));
    }

    #[test]
    fn omitting_foreign_source_emits_default_footnote() {
        let local = minimal_local();
        let delta = RequisitionDelta {
            nodes_added: vec![NodeRef {
                foreign_id: "web01".into(),
                value: json!({}),
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Created,
            true,
            ForeignSourceAction::NoChange,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        // Footnote text is contract-pinned by design D1; assert
        // both lines verbatim so a reworded "improvement" can't
        // silently drift away from the spec.
        assert!(rendered.contains(
            "  note: uses Horizon's default foreign-source; out-of-band changes to\n\
             \x20 the default are not surfaced by this diff\n"
        ));
    }

    #[test]
    fn footnote_suppressed_when_fs_action_is_not_no_change() {
        // Local omits foreignSource AND action is Deleted (server
        // had a custom FS): the FS-deletion section already explains
        // the transition, so the footnote would be redundant and
        // contradictory. D1 + design intent.
        let local = minimal_local();
        let o = outcome(
            ApplyState::Updated,
            false,
            ForeignSourceAction::Deleted,
            RequisitionDelta::default(),
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(!rendered.contains("note: uses Horizon's default"));
    }

    #[test]
    fn yaml_with_explicit_fs_does_not_emit_footnote() {
        let local = local_with_fs();
        let delta = RequisitionDelta {
            nodes_added: vec![NodeRef {
                foreign_id: "web01".into(),
                value: json!({}),
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Created,
            true,
            ForeignSourceAction::Created,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(!rendered.contains("Note: local YAML omits"));
    }

    #[test]
    fn fs_deletion_with_no_original_fs_renders_header_only() {
        // Edge: action=Deleted but original_remote_fs is None — should
        // not panic, just render the header without enumerated lists.
        let local = minimal_local();
        let o = outcome(
            ApplyState::Updated,
            false,
            ForeignSourceAction::Deleted,
            RequisitionDelta::default(),
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(rendered.contains("  - foreignSource: deleting custom"));
        assert!(!rendered.contains("displaced detectors"));
        assert!(!rendered.contains("displaced policies"));
    }

    #[test]
    fn removed_node_renders_minus_prefix() {
        let local = local_with_fs();
        let delta = RequisitionDelta {
            nodes_removed: vec![NodeRef {
                foreign_id: "old-host".into(),
                value: json!({}),
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Updated,
            true,
            ForeignSourceAction::NoChange,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(rendered.contains("    - old-host"));
    }

    #[test]
    fn leaf_with_absent_from_renders_as_absent_marker() {
        let local = local_with_fs();
        let delta = RequisitionDelta {
            nodes_modified: vec![NodeModification {
                foreign_id: "web01".into(),
                leaves: vec![LeafChange {
                    path: "spec.nodes[*].label".into(),
                    from: serde_json::Value::Null,
                    to: json!("web01.acme"),
                }],
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Updated,
            false,
            ForeignSourceAction::NoChange,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(rendered.contains("spec.nodes[*].label: <absent> → \"web01.acme\""));
    }

    #[test]
    fn dry_run_state_label_appears() {
        let local = local_with_fs();
        let delta = RequisitionDelta {
            nodes_added: vec![NodeRef {
                foreign_id: "web01".into(),
                value: json!({}),
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::DryRun,
            true,
            ForeignSourceAction::Created,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(rendered.starts_with("Requisition/acme-prod (dry-run)"));
    }

    #[test]
    fn diff_collapses_unmodeled_annotation_to_one_line_summary() {
        // PR001 unmodeled annotation: the diff renderer surfaces a
        // count line for non-empty `metadata.x-onmsctl-unmodeled`
        // instead of dumping every entry. Operators reviewing a
        // migration `--diff` see "metadata.x-onmsctl-unmodeled: N
        // entries" — enough to know the annotation exists without
        // drowning the diff in raw key/value pairs.
        let mut local = local_with_fs();
        // Nested-Mapping shape: three scalar leaves under
        // `nodes.web01` → diff renders "3 entries".
        let mut node_inner = serde_norway::Mapping::new();
        node_inner.insert(
            "location".into(),
            serde_norway::Value::String("HQ".into()),
        );
        node_inner.insert(
            "city".into(),
            serde_norway::Value::String("NYC".into()),
        );
        node_inner.insert(
            "legacy-tag".into(),
            serde_norway::Value::String("tag-1".into()),
        );
        let mut unmodeled = serde_norway::Mapping::new();
        unmodeled.insert(
            "nodes".into(),
            serde_norway::Value::Mapping({
                let mut m = serde_norway::Mapping::new();
                m.insert("web01".into(), serde_norway::Value::Mapping(node_inner));
                m
            }),
        );
        local.metadata.unmodeled = Some(unmodeled);

        let delta = RequisitionDelta {
            nodes_added: vec![NodeRef {
                foreign_id: "web01".into(),
                value: json!({}),
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Updated,
            false,
            ForeignSourceAction::NoChange,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(
            rendered.contains("metadata.x-onmsctl-unmodeled: 3 entries"),
            "expected one-line summary, got:\n{rendered}"
        );
        // Should NOT recurse into the annotation map.
        assert!(!rendered.contains("nodes.web01.@location"));
        assert!(!rendered.contains("HQ"));
    }

    #[test]
    fn diff_unmodeled_count_recurses_into_sequences() {
        // A `meta-data: [...]` block with 2 entries (each a Mapping
        // with 3 scalar keys: context/key/value) and 1 sibling scalar
        // leaf totals 1 + (2 * 3) = 7 leaves. Without recursion
        // through Sequences, this would mis-report as "2 entries".
        let mut local = local_with_fs();
        let entry = |ctx: &str, k: &str, v: &str| {
            let mut m = serde_norway::Mapping::new();
            m.insert("context".into(), serde_norway::Value::String(ctx.into()));
            m.insert("key".into(), serde_norway::Value::String(k.into()));
            m.insert("value".into(), serde_norway::Value::String(v.into()));
            serde_norway::Value::Mapping(m)
        };
        let mut node_inner = serde_norway::Mapping::new();
        node_inner.insert(
            "location".into(),
            serde_norway::Value::String("HQ".into()),
        );
        node_inner.insert(
            "meta-data".into(),
            serde_norway::Value::Sequence(vec![
                entry("r", "owner", "ops"),
                entry("r", "tier", "1"),
            ]),
        );
        let mut unmodeled = serde_norway::Mapping::new();
        unmodeled.insert(
            "nodes".into(),
            serde_norway::Value::Mapping({
                let mut m = serde_norway::Mapping::new();
                m.insert("web01".into(), serde_norway::Value::Mapping(node_inner));
                m
            }),
        );
        local.metadata.unmodeled = Some(unmodeled);

        let delta = RequisitionDelta {
            nodes_added: vec![NodeRef {
                foreign_id: "web01".into(),
                value: json!({}),
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Updated,
            false,
            ForeignSourceAction::NoChange,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(
            rendered.contains("metadata.x-onmsctl-unmodeled: 7 entries"),
            "expected 7-leaf count (1 scalar + 2 meta-data * 3 keys), got:\n{rendered}"
        );
    }

    #[test]
    fn diff_pluralizes_unmodeled_single_entry_as_entry_not_entries() {
        let mut local = local_with_fs();
        // One leaf entry: `nodes.web01.location`.
        let mut node_inner = serde_norway::Mapping::new();
        node_inner.insert(
            "location".into(),
            serde_norway::Value::String("HQ".into()),
        );
        let mut unmodeled = serde_norway::Mapping::new();
        unmodeled.insert(
            "nodes".into(),
            serde_norway::Value::Mapping({
                let mut m = serde_norway::Mapping::new();
                m.insert("web01".into(), serde_norway::Value::Mapping(node_inner));
                m
            }),
        );
        local.metadata.unmodeled = Some(unmodeled);

        let delta = RequisitionDelta {
            nodes_added: vec![NodeRef {
                foreign_id: "web01".into(),
                value: json!({}),
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Updated,
            false,
            ForeignSourceAction::NoChange,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(rendered.contains("metadata.x-onmsctl-unmodeled: 1 entry"));
    }

    #[test]
    fn diff_omits_unmodeled_line_when_annotation_is_absent() {
        let local = local_with_fs();
        // No unmodeled annotation on this fixture.
        assert!(local.metadata.unmodeled.is_none());
        let delta = RequisitionDelta {
            nodes_added: vec![NodeRef {
                foreign_id: "web01".into(),
                value: json!({}),
            }],
            ..Default::default()
        };
        let o = outcome(
            ApplyState::Updated,
            false,
            ForeignSourceAction::NoChange,
            delta,
            None,
        );
        let rendered = render_apply_diff(&local, &o);
        assert!(!rendered.contains("x-onmsctl-unmodeled"));
    }
}
