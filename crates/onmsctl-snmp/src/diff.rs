/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! L1 idempotency comparison for the SNMP config.
//!
//! `apply` compares the desired wire config (from [`crate::convert::to_wire`],
//! secrets left `None`) against the deployed one (`GET /snmp-config`). Secrets
//! are **excluded** from the comparison — they are write-only, so a redacted or
//! echoed secret on the deployed side must not surface as a spurious diff (a
//! secret rotation is re-sent with `--force`, per design D3).
//!
//! Canonicalization: blank every secret field, sort the order-insensitive
//! lists (definitions, profiles, and each definition's selectors) into a
//! deterministic order, then serialize. Struct field order is fixed by
//! declaration, so equal canonical values produce byte-identical JSON.

use crate::server;

fn blank_secrets(c: &mut server::Configuration) {
    c.read_community = None;
    c.write_community = None;
    c.auth_passphrase = None;
    c.privacy_passphrase = None;
    // `encrypted` is server-managed and not part of the desired state.
    c.encrypted = None;
}

/// Canonical, secret-free byte form of a wire config. Same logical content →
/// identical bytes regardless of secret values or list ordering.
pub fn canonical_nonsecret(cfg: &server::SnmpConfig) -> Vec<u8> {
    let mut c = cfg.clone();
    blank_secrets(&mut c.defaults);

    for d in &mut c.definition {
        blank_secrets(&mut d.config);
        d.specific.sort();
        d.ip_match.sort();
        d.range
            .sort_by(|a, b| a.begin.cmp(&b.begin).then_with(|| a.end.cmp(&b.end)));
    }
    // Definitions are order-insensitive for config purposes — sort by a stable
    // key built from location + selectors.
    c.definition.sort_by_key(def_key);

    for p in &mut c.profiles.profile {
        blank_secrets(&mut p.config);
    }
    c.profiles.profile.sort_by(|a, b| a.label.cmp(&b.label));

    serde_json::to_vec(&c).expect("wire config serializes as JSON")
}

/// Stable sort key for a definition: location, then joined selectors.
fn def_key(d: &server::Definition) -> (String, String, String, String) {
    let ranges = d
        .range
        .iter()
        .map(|r| format!("{}-{}", r.begin, r.end))
        .collect::<Vec<_>>()
        .join(",");
    (
        d.location.clone().unwrap_or_default(),
        d.specific.join(","),
        d.ip_match.join(","),
        ranges,
    )
}

/// `true` when the two configs are equivalent ignoring secret values.
pub fn unchanged(desired: &server::SnmpConfig, deployed: &server::SnmpConfig) -> bool {
    canonical_nonsecret(desired) == canonical_nonsecret(deployed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(read_community: Option<&str>, defs: Vec<server::Definition>) -> server::SnmpConfig {
        server::SnmpConfig {
            defaults: server::Configuration {
                version: Some("v2c".into()),
                read_community: read_community.map(String::from),
                ..Default::default()
            },
            definition: defs,
            ..Default::default()
        }
    }

    fn def(location: &str, specifics: &[&str]) -> server::Definition {
        server::Definition {
            location: Some(location.into()),
            specific: specifics.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn secret_only_difference_is_unchanged() {
        let a = cfg_with(Some("public"), vec![]);
        let b = cfg_with(Some("ROTATED"), vec![]);
        assert!(
            unchanged(&a, &b),
            "secret-only change must not register as a diff"
        );
    }

    #[test]
    fn non_secret_difference_is_changed() {
        let a = cfg_with(Some("public"), vec![def("hq", &["10.0.0.1"])]);
        let b = cfg_with(Some("public"), vec![def("hq", &["10.0.0.2"])]);
        assert!(!unchanged(&a, &b));
    }

    #[test]
    fn definition_and_selector_reorder_is_unchanged() {
        let a = cfg_with(
            None,
            vec![
                def("hq", &["10.0.0.1", "10.0.0.2"]),
                def("dc", &["10.0.1.1"]),
            ],
        );
        // Same content, definitions and specifics reordered.
        let b = cfg_with(
            None,
            vec![
                def("dc", &["10.0.1.1"]),
                def("hq", &["10.0.0.2", "10.0.0.1"]),
            ],
        );
        assert!(unchanged(&a, &b), "ordering must not register as a diff");
    }

    #[test]
    fn adding_a_definition_is_changed() {
        let a = cfg_with(None, vec![def("hq", &["10.0.0.1"])]);
        let b = cfg_with(
            None,
            vec![def("hq", &["10.0.0.1"]), def("dc", &["10.0.1.1"])],
        );
        assert!(!unchanged(&a, &b));
    }
}
