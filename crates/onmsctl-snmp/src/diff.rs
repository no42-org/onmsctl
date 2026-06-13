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

/// Canonical, secret-free form of a wire config as a struct: secrets blanked
/// and the order-insensitive lists sorted into a deterministic order. Two
/// logically-equal configs produce equal canonical structs regardless of
/// secret values or list ordering. Both [`unchanged`] and the handler's
/// `--diff` summary build on it.
pub fn canonical_struct(cfg: &server::SnmpConfig) -> server::SnmpConfig {
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

    c
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

/// `true` when the non-secret params the `desired` config explicitly sets all
/// match `deployed`; params absent from `desired` are ignored. Under
/// whole-config replace the server fills unset params with its schema defaults,
/// so a freshly-applied minimal document — whose `GET` then comes back with
/// those defaults populated — must still read as unchanged (exact byte equality
/// would re-upload forever). Secrets are blanked by [`canonical_struct`] and so
/// always compare equal.
///
/// LIMITATION: because an absent param is "don't care", *removing* a previously
/// set param from the document to fall back to the server default is not
/// detected as a change here. Detecting that needs the server's schema defaults
/// from a live payload (change task 9.2).
fn config_subset_match(desired: &server::Configuration, deployed: &server::Configuration) -> bool {
    macro_rules! field_ok {
        ($($f:ident),+ $(,)?) => {
            $( (desired.$f.is_none() || desired.$f == deployed.$f) )&&+
        };
    }
    field_ok!(
        port,
        retry,
        timeout,
        ttl,
        proxy_host,
        version,
        read_community,
        write_community,
        max_vars_per_pdu,
        max_repetitions,
        max_request_size,
        encrypted,
        security_name,
        security_level,
        auth_protocol,
        auth_passphrase,
        privacy_protocol,
        privacy_passphrase,
        context_name,
        engine_id,
        context_engine_id,
        enterprise_id,
    )
}

/// Per-tier idempotency verdict `[defaults, definitions, profiles]`, each `true`
/// when that tier is unchanged. Definition/profile **membership** is compared
/// exactly (the upload is a whole replace, so a definition present on one side
/// only is a change), while each tier's SNMP params use the subset semantics of
/// [`config_subset_match`]. Both lists are canonicalized (sorted) first, so the
/// element-wise zip is aligned by selector/label.
pub fn tiers_match(desired: &server::SnmpConfig, deployed: &server::SnmpConfig) -> [bool; 3] {
    let want = canonical_struct(desired);
    let have = canonical_struct(deployed);

    let defaults = config_subset_match(&want.defaults, &have.defaults);

    let definitions = want.definition.len() == have.definition.len()
        && want.definition.iter().zip(&have.definition).all(|(a, b)| {
            a.specific == b.specific
                && a.range == b.range
                && a.ip_match == b.ip_match
                && a.location == b.location
                && a.profile_label == b.profile_label
                && config_subset_match(&a.config, &b.config)
        });

    let profiles = want.profiles.profile.len() == have.profiles.profile.len()
        && want
            .profiles
            .profile
            .iter()
            .zip(&have.profiles.profile)
            .all(|(a, b)| {
                a.label == b.label
                    && a.filter == b.filter
                    && config_subset_match(&a.config, &b.config)
            });

    [defaults, definitions, profiles]
}

/// `true` when the deployed config already satisfies the desired document
/// (every tier unchanged per [`tiers_match`]).
pub fn unchanged(desired: &server::SnmpConfig, deployed: &server::SnmpConfig) -> bool {
    tiers_match(desired, deployed) == [true, true, true]
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

    #[test]
    fn server_defaulted_fields_are_unchanged() {
        // Desired sets only `version`; the deployed config (as a real GET
        // returns it) carries server-filled defaults the document omits. The
        // subset comparison must treat those extras as "don't care".
        let desired = server::SnmpConfig {
            defaults: server::Configuration {
                version: Some("v2c".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let deployed = server::SnmpConfig {
            defaults: server::Configuration {
                version: Some("v2c".into()),
                port: Some(161),
                retry: Some(1),
                timeout: Some(1800),
                max_repetitions: Some(10),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            unchanged(&desired, &deployed),
            "server-defaulted fields the document omits must not trigger a re-upload"
        );
    }

    #[test]
    fn a_param_the_document_sets_must_match() {
        // The document explicitly sets `port`; the deployed value differs, so
        // this IS a change (subset only ignores params the document omits).
        let desired = server::SnmpConfig {
            defaults: server::Configuration {
                version: Some("v2c".into()),
                port: Some(1161),
                ..Default::default()
            },
            ..Default::default()
        };
        let deployed = server::SnmpConfig {
            defaults: server::Configuration {
                version: Some("v2c".into()),
                port: Some(161),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!unchanged(&desired, &deployed));
    }
}
