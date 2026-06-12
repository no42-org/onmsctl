/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Concrete kind-handler wiring for the `onmsctl` binary.
//!
//! This is the one place that depends on every capability crate: it assembles
//! the core-owned [`Registry`] by registering each capability's `KindHandler`
//! at its precedence rank. Core defines the registry *shape* and the rank
//! table; the binary *populates* it (Decision B), so `onmsctl-core` never
//! references a capability type. Each handler is keyed by its own `kind()`
//! (the capability's exported `KIND` constant) inside `Registry::register` —
//! there are no `kind` string literals at this call site.

use onmsctl_core::Registry;
use onmsctl_core::kind::precedence::{
    RANK_EVENT_SOURCE, RANK_REQUISITION, RANK_SNMP_CONFIG, RANK_USER, ranks_are_total_order,
};

use onmsctl_eventconf::apply::EventSourceHandler;
use onmsctl_iam::apply::UserHandler;
use onmsctl_provisioning::apply::ProvisioningHandler;
use onmsctl_snmp::apply::SnmpConfigHandler;

/// Build the populated kind registry: every supported `kind` mapped to its
/// handler and static precedence rank.
///
/// Panics if the assembled ranks are not a strict total order. `Registry::register`
/// only `debug_assert`s against duplicate *kinds* (compiled out in release) and
/// never checks for duplicate *ranks*; a rank collision would make bucket
/// ordering depend on first-seen iteration order. This is a wiring bug, so we
/// fail loudly at startup rather than silently apply buckets in an arbitrary
/// order.
pub fn build() -> Registry {
    let mut reg = Registry::new();
    reg.register(RANK_USER, Box::new(UserHandler));
    reg.register(RANK_EVENT_SOURCE, Box::new(EventSourceHandler));
    reg.register(RANK_SNMP_CONFIG, Box::new(SnmpConfigHandler));
    reg.register(RANK_REQUISITION, Box::new(ProvisioningHandler));

    let ranks: Vec<(&str, u32)> = reg
        .known_kinds()
        .into_iter()
        .map(|k| (k, reg.rank(k).expect("kind was just registered")))
        .collect();
    assert!(
        ranks_are_total_order(&ranks),
        "kind-precedence ranks are not a strict total order: {ranks:?} — wiring bug"
    );

    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::KindHandler;

    #[test]
    fn build_registers_all_kinds_at_their_canonical_ranks() {
        let reg = build();
        assert_eq!(reg.len(), 4, "exactly the wired kinds are present");
        // Key off each handler's own `kind()` so the test can't drift from the
        // registration site if a KIND constant changes.
        assert_eq!(reg.rank(UserHandler.kind()), Some(RANK_USER));
        assert_eq!(reg.rank(EventSourceHandler.kind()), Some(RANK_EVENT_SOURCE));
        assert_eq!(reg.rank(SnmpConfigHandler.kind()), Some(RANK_SNMP_CONFIG));
        assert_eq!(reg.rank(ProvisioningHandler.kind()), Some(RANK_REQUISITION));
    }
}
