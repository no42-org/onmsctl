/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Kind-precedence data (Decision C / D7).
//!
//! Pure `kind → rank` data — no capability types — so it can live in core.
//! Documents apply in ascending rank order. Today's kinds are independent, so
//! these ranks encode no real dependency; they establish the ordering
//! mechanism for a future dependent kind. Because the ordering is a strict
//! total order, "acyclic" reduces to "every rank is distinct"
//! ([`ranks_are_total_order`]), checked by a unit test.

pub const RANK_USER: u32 = 100;
pub const RANK_EVENT_SOURCE: u32 = 200;
pub const RANK_SNMP_CONFIG: u32 = 250;
pub const RANK_REQUISITION: u32 = 300;
/// Maintenance windows apply after `Requisition` so a co-located apply imports
/// nodes before a window resolves its node foreign references (the import is
/// async, so a reference may still need a follow-up apply — see the capability's
/// design D10).
pub const RANK_MAINTENANCE: u32 = 350;

/// The authoritative precedence table. The binary uses these ranks when wiring
/// handlers into the registry.
pub const KNOWN_RANKS: &[(&str, u32)] = &[
    ("User", RANK_USER),
    ("EventSource", RANK_EVENT_SOURCE),
    ("SnmpConfig", RANK_SNMP_CONFIG),
    ("Requisition", RANK_REQUISITION),
    ("Maintenance", RANK_MAINTENANCE),
];

/// The precedence rank for a known `kind`, or `None` if unknown.
pub fn default_rank(kind: &str) -> Option<u32> {
    KNOWN_RANKS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, r)| *r)
}

/// True when every rank in `ranks` is distinct — i.e. the ranks form a strict
/// total order with no ties (the "acyclic" property for a linear order).
pub fn ranks_are_total_order(ranks: &[(&str, u32)]) -> bool {
    let mut seen = std::collections::HashSet::new();
    ranks.iter().all(|(_, r)| seen.insert(*r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ranks_are_a_total_order() {
        assert!(ranks_are_total_order(KNOWN_RANKS));
    }

    #[test]
    fn default_rank_resolves_known_and_rejects_unknown() {
        assert_eq!(default_rank("EventSource"), Some(RANK_EVENT_SOURCE));
        assert_eq!(default_rank("Nope"), None);
    }

    #[test]
    fn ties_are_rejected() {
        assert!(!ranks_are_total_order(&[("A", 1), ("B", 1)]));
    }
}
