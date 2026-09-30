/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Kind-precedence data (Decision C / D7).
//!
//! Pure `kind → rank` data — no capability types — so it can live in core.
//! Documents apply in ascending rank order. Most kinds are independent and
//! their ranks only fix a deterministic order. `EventSourceOrder` after
//! `EventSource` is a real dependency: an order document places sources the
//! same apply may create. So are the threshold kinds: groups after event
//! sources, packages after groups, and `Maintenance` after packages.
//! Because the ordering is a strict
//! total order, "acyclic" reduces to "every rank is distinct"
//! ([`ranks_are_total_order`]), checked by a unit test.

pub const RANK_USER: u32 = 100;
pub const RANK_EVENT_SOURCE: u32 = 200;
/// Event-source order applies after `EventSource` so it can place sources
/// created by the same apply. This is a real dependency.
pub const RANK_EVENT_SOURCE_ORDER: u32 = 210;
/// Threshold groups apply after event sources, because a threshold's
/// `triggeredUEI` may name an event the same apply creates.
pub const RANK_THRESHOLD_GROUP: u32 = 220;
/// threshd packages apply after the groups they bind and before
/// `Maintenance`, which attaches outages to packages.
pub const RANK_THRESHD_PACKAGE: u32 = 230;
pub const RANK_SNMP_CONFIG: u32 = 250;
pub const RANK_REQUISITION: u32 = 300;
/// Maintenance windows apply after `Requisition` so a co-located apply imports
/// nodes before a window resolves its node foreign references (the import is
/// async, so a reference may still need a follow-up apply — see the capability's
/// design D10).
pub const RANK_MAINTENANCE: u32 = 350;
/// Data-collection sources are independent of the other kinds (their own REST
/// base, no cross-kind references); the rank only fixes a deterministic apply
/// order after `SnmpConfig`/`Maintenance`.
pub const RANK_DATACOLLECTION: u32 = 375;
/// Business services are independent of the other kinds (their own REST base;
/// intra-kind child references are ordered by the handler's two-pass execute,
/// not by this table). The rank only fixes a deterministic apply order last.
pub const RANK_BUSINESS_SERVICE: u32 = 400;

/// The authoritative precedence table. The binary uses these ranks when wiring
/// handlers into the registry.
pub const KNOWN_RANKS: &[(&str, u32)] = &[
    ("User", RANK_USER),
    ("EventSource", RANK_EVENT_SOURCE),
    ("EventSourceOrder", RANK_EVENT_SOURCE_ORDER),
    ("ThresholdGroup", RANK_THRESHOLD_GROUP),
    ("ThreshdPackage", RANK_THRESHD_PACKAGE),
    ("SnmpConfig", RANK_SNMP_CONFIG),
    ("Requisition", RANK_REQUISITION),
    ("Maintenance", RANK_MAINTENANCE),
    ("DataCollectionSource", RANK_DATACOLLECTION),
    ("BusinessService", RANK_BUSINESS_SERVICE),
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
    fn event_source_order_ranks_between_event_source_and_snmp_config() {
        let r = default_rank("EventSourceOrder").unwrap();
        assert!(RANK_EVENT_SOURCE < r && r < RANK_SNMP_CONFIG);
    }

    #[test]
    fn threshold_kinds_rank_after_event_sources_and_before_snmp_and_maintenance() {
        let g = default_rank("ThresholdGroup").unwrap();
        let p = default_rank("ThreshdPackage").unwrap();
        assert!(RANK_EVENT_SOURCE_ORDER < g && g < p && p < RANK_SNMP_CONFIG);
        assert!(p < RANK_MAINTENANCE);
    }

    #[test]
    fn ties_are_rejected() {
        assert!(!ranks_are_total_order(&[("A", 1), ("B", 1)]));
    }
}
