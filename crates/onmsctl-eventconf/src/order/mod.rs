/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `kind: EventSourceOrder` — declare which event sources are evaluated
//! first.
//!
//! Singleton (`metadata.name: default`). `spec.first` lists source names in
//! the order they are evaluated. Every other source keeps its current
//! relative order below them, and `opennms.catch-all.events` stays last. The
//! document owns only the positions of the sources it lists.
//!
//! This module holds the local model and its validation plus the pure core
//! shared by plan, execute and diff: [`target_order`], [`predict_order`] and
//! [`render_diff`]. The router adapter is [`handler::EventSourceOrderHandler`].

pub mod handler;

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use onmsctl_core::{Error, RawDoc, Result};

use crate::apply::local::{Metadata, validate_source_name};

pub use handler::EventSourceOrderHandler;

/// Kind literal for EventSourceOrder documents.
pub const KIND: &str = "EventSourceOrder";

/// The only accepted `metadata.name`.
pub const SINGLETON_NAME: &str = "default";

/// The source Horizon always evaluates last.
pub const CATCH_ALL: &str = "opennms.catch-all.events";

const API_VERSION: &str = "eventconf.opennms.org/v1";

/// An `EventSourceOrder` document: which event sources are evaluated first.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventSourceOrderLocal {
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: EventSourceOrderSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventSourceOrderSpec {
    /// Sources evaluated first, in this order. Every other source keeps its
    /// current relative order below them.
    pub first: Vec<String>,
}

impl EventSourceOrderLocal {
    /// Strictly parse one router document and validate it.
    pub fn from_raw(doc: &RawDoc) -> Result<Self> {
        // Parse the text, not the `Value`, so a plain scalar such as `12345`
        // reads as the string it was in the file (see `EventSourceLocal`).
        let text = serde_norway::to_string(&doc.value).map_err(|e| {
            Error::Config(format!(
                "{}: could not re-serialize document: {e}",
                doc.label()
            ))
        })?;
        let local: Self = serde_norway::from_str(&text).map_err(|e| {
            Error::Config(format!(
                "{}: invalid `kind: {KIND}` document: {e}",
                doc.label()
            ))
        })?;
        local
            .validate()
            .map_err(|e| Error::Config(format!("{}: {}", doc.label(), config_msg(e))))?;
        Ok(local)
    }

    /// Validate every local rule. No HTTP.
    pub fn validate(&self) -> Result<()> {
        if self.api_version != API_VERSION {
            return Err(Error::Config(format!(
                "apiVersion must be {API_VERSION:?}, got {:?}",
                self.api_version
            )));
        }
        if self.kind != KIND {
            return Err(Error::Config(format!(
                "kind must be {KIND:?}, got {:?}",
                self.kind
            )));
        }
        if self.metadata.name != SINGLETON_NAME {
            return Err(Error::Config(format!(
                "metadata.name must be {SINGLETON_NAME:?} ({KIND} is a singleton), got {:?}",
                self.metadata.name
            )));
        }
        if self.spec.first.is_empty() {
            return Err(Error::Config(
                "spec.first is empty. List at least one source to evaluate first.".into(),
            ));
        }
        for (i, name) in self.spec.first.iter().enumerate() {
            let field = format!("spec.first[{i}]");
            if name.eq_ignore_ascii_case(CATCH_ALL) {
                return Err(Error::Config(format!(
                    "{field}: '{CATCH_ALL}' is always evaluated last and cannot be listed"
                )));
            }
            validate_source_name(&field, name)?;
            if let Some(j) = self.spec.first[..i].iter().position(|n| n == name) {
                return Err(Error::Config(format!(
                    "{field}: duplicate entry '{name}' (also at spec.first[{j}])"
                )));
            }
        }
        Ok(())
    }
}

fn config_msg(e: Error) -> String {
    match e {
        Error::Config(m) => m,
        other => other.to_string(),
    }
}

/// The order `first` asks for, given the `current` order (evaluated-first
/// first): `first` in list order, then every other source of `current` in
/// its relative order, then the catch-all when `current` has it.
pub fn target_order(current: &[String], first: &[String]) -> Vec<String> {
    let listed: HashSet<&str> = first.iter().map(String::as_str).collect();
    let mut out: Vec<String> = first.to_vec();
    out.extend(
        current
            .iter()
            .filter(|n| !listed.contains(n.as_str()) && n.as_str() != CATCH_ALL)
            .cloned(),
    );
    if current.iter().any(|n| n == CATCH_ALL) {
        out.push(CATCH_ALL.to_string());
    }
    out
}

/// The order expected after the `EventSource` bucket runs: sources the same
/// apply creates land on top of the `live` order. `created` must not overlap
/// `live`. Their order among themselves is a preview approximation only;
/// execute recomputes from the real live order.
pub fn predict_order(live: &[String], created: &[String]) -> Vec<String> {
    created.iter().chain(live).cloned().collect()
}

/// Render the `--diff` text for a planned reorder from `before` to `target`.
/// Prints one line per listed source whose position changes and counts the
/// unlisted sources that shift. Positions are 1-based evaluation positions.
pub fn render_diff(
    before: &[String],
    target: &[String],
    first: &[String],
    created: &[String],
) -> String {
    let pos = |order: &[String], name: &str| order.iter().position(|n| n == name).map(|i| i + 1);
    let listed: HashSet<&str> = first.iter().map(String::as_str).collect();

    let moves: Vec<(&str, usize, usize)> = first
        .iter()
        .filter_map(|n| match (pos(before, n), pos(target, n)) {
            (Some(b), Some(a)) if b != a => Some((n.as_str(), b, a)),
            _ => None,
        })
        .collect();
    let shifted = before
        .iter()
        .filter(|n| !listed.contains(n.as_str()) && pos(before, n) != pos(target, n))
        .count();

    let mut out = format!(
        "{KIND}/{SINGLETON_NAME}: {} listed {} {}, {} {} {} down",
        moves.len(),
        plural(moves.len(), "source", "sources"),
        plural(moves.len(), "moves", "move"),
        shifted,
        plural(shifted, "other", "others"),
        plural(shifted, "shifts", "shift"),
    );
    let width = moves.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
    for (name, b, a) in moves {
        out.push_str(&format!("\n  ~ {name:<width$}  {b} -> {a}"));
        if created.iter().any(|c| c == name) {
            out.push_str("  (created by this apply)");
        }
    }
    out
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 { one } else { many }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::parse_documents;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn doc(first: &str) -> String {
        format!(
            "apiVersion: eventconf.opennms.org/v1\nkind: EventSourceOrder\nmetadata:\n  name: default\nspec:\n  first: {first}\n"
        )
    }

    fn parse(yaml: &str) -> Result<EventSourceOrderLocal> {
        let docs = parse_documents("order.yaml", yaml).unwrap();
        EventSourceOrderLocal::from_raw(&docs[0])
    }

    fn err(yaml: &str) -> String {
        match parse(yaml) {
            Err(Error::Config(m)) => m,
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    // -- validation --

    #[test]
    fn valid_document_parses() {
        let local = parse(&doc("[cisco.custom, acme.traps]")).unwrap();
        assert_eq!(local.spec.first, names(&["cisco.custom", "acme.traps"]));
    }

    #[test]
    fn numeric_looking_name_is_read_as_a_string() {
        let local = parse(&doc("[12345]")).unwrap();
        assert_eq!(local.spec.first, names(&["12345"]));
    }

    #[test]
    fn rejects_wrong_api_version() {
        let m = err(&doc("[a.b]").replace("eventconf.opennms.org/v1", "v2"));
        assert!(m.contains("apiVersion"), "got: {m}");
    }

    #[test]
    fn rejects_non_default_name() {
        let m = err(&doc("[a.b]").replace("name: default", "name: mine"));
        assert!(
            m.contains("metadata.name") && m.contains("singleton"),
            "got: {m}"
        );
    }

    #[test]
    fn rejects_empty_first() {
        let m = err(&doc("[]"));
        assert!(m.contains("spec.first is empty"), "got: {m}");
    }

    #[test]
    fn rejects_duplicate_entry() {
        let m = err(&doc("[a.b, c.d, a.b]"));
        assert!(
            m.contains("spec.first[2]") && m.contains("duplicate"),
            "got: {m}"
        );
    }

    #[test]
    fn rejects_invalid_source_name() {
        let m = err(&doc("['bad name']"));
        assert!(
            m.contains("spec.first[0]") && m.contains("invalid characters"),
            "got: {m}"
        );
    }

    #[test]
    fn rejects_catch_all_case_insensitively() {
        for n in [CATCH_ALL, "OPENNMS.Catch-All.events"] {
            let m = err(&doc(&format!("[{n}]")));
            assert!(m.contains("always evaluated last"), "got: {m}");
        }
    }

    #[test]
    fn rejects_unknown_key() {
        let m = err(&doc("[a.b]").replace("spec:\n", "spec:\n  strict: true\n"));
        assert!(m.contains("unknown field"), "got: {m}");
    }

    #[test]
    fn rejects_missing_first() {
        let m = err(&doc("[a.b]").replace("  first: [a.b]\n", "  {}\n"));
        assert!(m.contains("first"), "got: {m}");
    }

    #[test]
    fn published_example_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/event-source-order.yaml");
        let text = std::fs::read_to_string(&path).unwrap();
        let local = parse(&text).unwrap();
        assert_eq!(local.spec.first.len(), 2);
    }

    // -- target_order --

    #[test]
    fn listed_sources_come_first_in_list_order() {
        let current = names(&["a", "b", "c", "d", CATCH_ALL]);
        assert_eq!(
            target_order(&current, &names(&["c", "a"])),
            names(&["c", "a", "b", "d", CATCH_ALL])
        );
    }

    #[test]
    fn unlisted_sources_keep_their_relative_order() {
        let current = names(&["x", "cisco.syslog", "y", "cisco.custom", "z", CATCH_ALL]);
        let t = target_order(&current, &names(&["cisco.custom"]));
        assert_eq!(
            t,
            names(&["cisco.custom", "x", "cisco.syslog", "y", "z", CATCH_ALL])
        );
    }

    #[test]
    fn catch_all_stays_last_even_when_not_last_live() {
        let current = names(&["a", CATCH_ALL, "b"]);
        assert_eq!(
            target_order(&current, &names(&["b"])),
            names(&["b", "a", CATCH_ALL])
        );
    }

    #[test]
    fn already_ordered_is_a_no_op() {
        let current = names(&["a", "b", "c", CATCH_ALL]);
        assert_eq!(target_order(&current, &names(&["a", "b"])), current);
    }

    #[test]
    fn prediction_prepends_created_sources() {
        let live = names(&["a", "b", CATCH_ALL]);
        let predicted = predict_order(&live, &names(&["new.one"]));
        assert_eq!(predicted, names(&["new.one", "a", "b", CATCH_ALL]));
        assert_eq!(
            target_order(&predicted, &names(&["b", "new.one"])),
            names(&["b", "new.one", "a", CATCH_ALL])
        );
    }

    // -- diff --

    #[test]
    fn diff_shows_listed_moves_and_counts_shifted_others() {
        let before = names(&["a", "b", "c", "d", "e", "f", "cisco.custom", CATCH_ALL]);
        let first = names(&["cisco.custom"]);
        let target = target_order(&before, &first);
        let d = render_diff(&before, &target, &first, &[]);
        assert!(
            d.starts_with("EventSourceOrder/default: 1 listed source moves, 6 others shift down"),
            "got: {d}"
        );
        assert!(d.contains("~ cisco.custom  7 -> 1"), "got: {d}");
        assert!(!d.contains("created by this apply"));
        assert!(!d.contains(" a "), "unlisted sources are not listed: {d}");
    }

    #[test]
    fn diff_marks_sources_created_by_this_apply() {
        let predicted = predict_order(&names(&["a", "b", CATCH_ALL]), &names(&["acme.traps"]));
        let first = names(&["b", "acme.traps"]);
        let target = target_order(&predicted, &first);
        let d = render_diff(&predicted, &target, &first, &names(&["acme.traps"]));
        assert!(
            d.contains("2 listed sources move, 1 other shifts down"),
            "got: {d}"
        );
        assert!(d.contains("~ b           3 -> 1"), "got: {d}");
        assert!(
            d.contains("~ acme.traps  1 -> 2  (created by this apply)"),
            "got: {d}"
        );
    }

    #[test]
    fn diff_omits_listed_sources_that_do_not_move() {
        let before = names(&["a", "c", "b", CATCH_ALL]);
        let first = names(&["a", "b"]);
        let target = target_order(&before, &first);
        let d = render_diff(&before, &target, &first, &[]);
        assert!(
            d.contains("1 listed source moves, 1 other shifts down"),
            "got: {d}"
        );
        assert!(!d.contains("~ a"), "got: {d}");
        assert!(d.contains("~ b  3 -> 2"), "got: {d}");
    }
}
