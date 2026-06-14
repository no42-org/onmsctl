/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Conversions between the local model ([`crate::model`]) and the wire DTOs
//! ([`crate::server`]).
//!
//! - [`to_wire`] maps the local document onto the wire shape **without**
//!   resolving secrets (secret fields are left `None`) — used for the diff.
//! - [`to_wire_resolved`] is [`to_wire`] plus secret resolution+injection —
//!   used for the actual upload. Resolution is write-only (the values are
//!   serialized into the POST body and never read back).
//! - [`from_wire`] maps a server response back to the local model for
//!   `export`, emitting secret fields as **reference placeholders** (never
//!   cleartext) since secrets are write-only.
//!
//! Name/representation mapping (local ⇆ wire): `securityLevel`
//! enum ⇆ int 1/2/3; `retries` ⇆ `retry`; `specifics`/`ranges`/`ipMatches`
//! ⇆ `specific`/`range`/`ipMatch`; `filterExpression` ⇆ `filter`.

use crate::model::{
    DefinitionLocal, Params, ProfileLocal, RangeLocal, SecurityLevel, SnmpConfigLocal, Spec, Trapd,
    TrapdV3User,
};
use crate::secret::{FromEnvRef, SecretRef, resolve_secret_ref};
use crate::server;
use onmsctl_core::Result;

fn level_to_wire(l: SecurityLevel) -> i32 {
    match l {
        SecurityLevel::NoAuthNoPriv => 1,
        SecurityLevel::AuthNoPriv => 2,
        SecurityLevel::AuthPriv => 3,
    }
}

fn level_from_wire(n: i32) -> Option<SecurityLevel> {
    match n {
        1 => Some(SecurityLevel::NoAuthNoPriv),
        2 => Some(SecurityLevel::AuthNoPriv),
        3 => Some(SecurityLevel::AuthPriv),
        _ => None,
    }
}

/// Map local params → wire `Configuration`, leaving secret fields `None`.
fn params_to_config(p: &Params) -> server::Configuration {
    server::Configuration {
        version: p.version.clone(),
        port: p.port,
        timeout: p.timeout,
        retry: p.retries,
        ttl: p.ttl,
        proxy_host: p.proxy_host.clone(),
        read_community: None,
        write_community: None,
        max_repetitions: p.max_repetitions,
        max_vars_per_pdu: p.max_vars_per_pdu,
        max_request_size: p.max_request_size,
        encrypted: None,
        security_name: p.security_name.clone(),
        security_level: p.security_level.map(level_to_wire),
        auth_protocol: p.auth_protocol.clone(),
        auth_passphrase: None,
        privacy_protocol: p.privacy_protocol.clone(),
        privacy_passphrase: None,
        context_name: p.context_name.clone(),
        engine_id: p.engine_id.clone(),
        context_engine_id: p.context_engine_id.clone(),
        enterprise_id: p.enterprise_id.clone(),
    }
}

fn definition_to_wire(d: &DefinitionLocal) -> server::Definition {
    server::Definition {
        config: params_to_config(&d.params),
        specific: d.specifics.clone(),
        range: d
            .ranges
            .iter()
            .map(|r| server::Range {
                begin: r.begin.clone(),
                end: r.end.clone(),
            })
            .collect(),
        ip_match: d.ip_matches.clone(),
        location: d.location.clone(),
        profile_label: d.profile_label.clone(),
    }
}

fn profile_to_wire(p: &ProfileLocal) -> server::SnmpProfile {
    server::SnmpProfile {
        config: params_to_config(&p.params),
        label: Some(p.label.clone()),
        filter: p.filter_expression.clone(),
    }
}

/// Project the local document onto the wire shape. Secret fields are left
/// `None` (see [`to_wire_resolved`] for the upload path). Pure — no I/O.
pub fn to_wire(local: &SnmpConfigLocal) -> server::SnmpConfig {
    server::SnmpConfig {
        defaults: local
            .spec
            .defaults
            .as_ref()
            .map(params_to_config)
            .unwrap_or_default(),
        definition: local
            .spec
            .definitions
            .iter()
            .map(definition_to_wire)
            .collect(),
        profiles: server::SnmpProfiles {
            profile: local.spec.profiles.iter().map(profile_to_wire).collect(),
        },
    }
}

/// Resolve a params block's secret refs into a wire `Configuration`.
fn inject_secrets(p: &Params, c: &mut server::Configuration) -> Result<()> {
    if let Some(r) = &p.read_community {
        c.read_community = Some(resolve_secret_ref(r)?.to_string());
    }
    if let Some(r) = &p.write_community {
        c.write_community = Some(resolve_secret_ref(r)?.to_string());
    }
    if let Some(r) = &p.auth_passphrase {
        c.auth_passphrase = Some(resolve_secret_ref(r)?.to_string());
    }
    if let Some(r) = &p.privacy_passphrase {
        c.privacy_passphrase = Some(resolve_secret_ref(r)?.to_string());
    }
    Ok(())
}

/// [`to_wire`] plus secret resolution+injection — the payload to upload.
/// Index alignment with [`to_wire`] is guaranteed (same iteration order).
pub fn to_wire_resolved(local: &SnmpConfigLocal) -> Result<server::SnmpConfig> {
    let mut wire = to_wire(local);
    if let Some(p) = &local.spec.defaults {
        inject_secrets(p, &mut wire.defaults)?;
    }
    for (d, wd) in local
        .spec
        .definitions
        .iter()
        .zip(wire.definition.iter_mut())
    {
        inject_secrets(&d.params, &mut wd.config)?;
    }
    for (p, wp) in local
        .spec
        .profiles
        .iter()
        .zip(wire.profiles.profile.iter_mut())
    {
        inject_secrets(&p.params, &mut wp.config)?;
    }
    Ok(wire)
}

// ---------------------------------------------------------------------------
// Server → local (for `export`)
// ---------------------------------------------------------------------------

/// Emit a placeholder secret reference when the wire carries a value, so the
/// exported YAML never contains cleartext.
fn placeholder(env_name: &str, wire_value: &Option<String>) -> Option<SecretRef> {
    wire_value.as_ref().map(|_| {
        SecretRef::FromEnv(FromEnvRef {
            from_env: env_name.to_string(),
        })
    })
}

/// Uppercase a tier label into a shell-identifier-safe fragment (non-alphanumeric
/// → `_`), used to build a per-occurrence env placeholder name.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// The env-var name for a secret `field` at a given `scope` (tier). Per
/// occurrence — distinct tiers get distinct names — so an exported config whose
/// `defaults` and a `definition` carry *different* communities round-trips
/// faithfully: the operator sets each env var independently. (A single fixed
/// name per field would collapse them on re-apply.)
fn secret_env(field: &str, scope: &str) -> String {
    format!("SNMP_{field}_{scope}")
}

fn config_to_params(c: &server::Configuration, scope: &str) -> Params {
    Params {
        version: c.version.clone(),
        port: c.port,
        timeout: c.timeout,
        retries: c.retry,
        ttl: c.ttl,
        proxy_host: c.proxy_host.clone(),
        read_community: placeholder(&secret_env("READ_COMMUNITY", scope), &c.read_community),
        write_community: placeholder(&secret_env("WRITE_COMMUNITY", scope), &c.write_community),
        max_repetitions: c.max_repetitions,
        max_vars_per_pdu: c.max_vars_per_pdu,
        max_request_size: c.max_request_size,
        security_name: c.security_name.clone(),
        security_level: c.security_level.and_then(level_from_wire),
        auth_protocol: c.auth_protocol.clone(),
        auth_passphrase: placeholder(&secret_env("AUTH_PASSPHRASE", scope), &c.auth_passphrase),
        privacy_protocol: c.privacy_protocol.clone(),
        privacy_passphrase: placeholder(
            &secret_env("PRIVACY_PASSPHRASE", scope),
            &c.privacy_passphrase,
        ),
        context_name: c.context_name.clone(),
        engine_id: c.engine_id.clone(),
        context_engine_id: c.context_engine_id.clone(),
        enterprise_id: c.enterprise_id.clone(),
    }
}

/// Map a server response back to the local model for `export`. Secret fields
/// become reference placeholders; the document is safe to commit but needs the
/// operator to wire up the real refs before re-apply.
pub fn from_wire(wire: &server::SnmpConfig) -> SnmpConfigLocal {
    SnmpConfigLocal {
        api_version: crate::model::API_VERSION.to_string(),
        kind: crate::model::KIND.to_string(),
        metadata: crate::model::Metadata {
            name: crate::model::SINGLETON_NAME.to_string(),
        },
        spec: Spec {
            defaults: Some(config_to_params(&wire.defaults, "DEFAULTS")),
            profiles: wire
                .profiles
                .profile
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let label = p.label.clone().unwrap_or_default();
                    // Profile labels are the unique reference key; fall back to
                    // the index for an (unexpected) unlabeled profile.
                    let scope = if label.is_empty() {
                        format!("PROFILE_{i}")
                    } else {
                        format!("PROFILE_{}", sanitize(&label))
                    };
                    ProfileLocal {
                        params: config_to_params(&p.config, &scope),
                        label,
                        filter_expression: p.filter.clone(),
                    }
                })
                .collect(),
            definitions: wire
                .definition
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    // The index guarantees uniqueness (two definitions can share
                    // a location); the location adds readability.
                    let scope = match d.location.as_deref() {
                        Some(loc) if !loc.is_empty() => format!("DEF_{i}_{}", sanitize(loc)),
                        _ => format!("DEF_{i}"),
                    };
                    DefinitionLocal {
                        params: config_to_params(&d.config, &scope),
                        specifics: d.specific.clone(),
                        ranges: d
                            .range
                            .iter()
                            .map(|r| RangeLocal {
                                begin: r.begin.clone(),
                                end: r.end.clone(),
                            })
                            .collect(),
                        ip_matches: d.ip_match.clone(),
                        location: d.location.clone(),
                        profile_label: d.profile_label.clone(),
                    }
                })
                .collect(),
            // The trap-daemon block is populated separately by the export
            // command (it reads a different endpoint); from a snmp-config
            // response alone there is nothing to fill here.
            trapd: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Trap daemon (Trapd) conversions
// ---------------------------------------------------------------------------

/// Map a local v3 trap user → wire, leaving passphrase secrets `None`.
fn v3_user_to_wire(u: &TrapdV3User) -> server::Snmpv3User {
    server::Snmpv3User {
        engine_id: u.engine_id.clone(),
        security_name: Some(u.security_name.clone()),
        security_level: u.security_level.map(level_to_wire),
        auth_protocol: u.auth_protocol.clone(),
        auth_passphrase: None,
        privacy_protocol: u.privacy_protocol.clone(),
        privacy_passphrase: None,
    }
}

/// Project the local trap-daemon block onto the wire shape. Passphrase secrets
/// are left `None` (see [`trapd_to_wire_resolved`]). Pure — no I/O.
pub fn trapd_to_wire(t: &Trapd) -> server::TrapdConfig {
    server::TrapdConfig {
        snmp_trap_address: t.snmp_trap_address.clone(),
        snmp_trap_port: t.snmp_trap_port,
        new_suspect_on_trap: t.new_suspect_on_trap,
        include_raw_message: t.include_raw_message,
        threads: t.threads,
        queue_size: t.queue_size,
        batch_size: t.batch_size,
        batch_interval: t.batch_interval,
        use_address_from_varbind: t.use_address_from_varbind,
        snmpv3_user: t.snmpv3_users.iter().map(v3_user_to_wire).collect(),
    }
}

/// [`trapd_to_wire`] plus passphrase resolution+injection — the PUT payload.
/// Index alignment with [`trapd_to_wire`] is guaranteed (same iteration order).
pub fn trapd_to_wire_resolved(t: &Trapd) -> Result<server::TrapdConfig> {
    let mut wire = trapd_to_wire(t);
    for (u, wu) in t.snmpv3_users.iter().zip(wire.snmpv3_user.iter_mut()) {
        if let Some(r) = &u.auth_passphrase {
            wu.auth_passphrase = Some(resolve_secret_ref(r)?.to_string());
        }
        if let Some(r) = &u.privacy_passphrase {
            wu.privacy_passphrase = Some(resolve_secret_ref(r)?.to_string());
        }
    }
    Ok(wire)
}

/// Map a server trap-daemon response back to the local model for `export`.
/// Passphrases become per-user `fromEnv` placeholders (never cleartext).
pub fn trapd_from_wire(t: &server::TrapdConfig) -> Trapd {
    Trapd {
        snmp_trap_address: t.snmp_trap_address.clone(),
        snmp_trap_port: t.snmp_trap_port,
        new_suspect_on_trap: t.new_suspect_on_trap,
        include_raw_message: t.include_raw_message,
        threads: t.threads,
        queue_size: t.queue_size,
        batch_size: t.batch_size,
        batch_interval: t.batch_interval,
        use_address_from_varbind: t.use_address_from_varbind,
        snmpv3_users: t
            .snmpv3_user
            .iter()
            .enumerate()
            .map(|(i, u)| {
                // The security name is the unique reference key; fall back to the
                // index for an (unexpected) unnamed user so placeholders stay
                // distinct and the export round-trips.
                let scope = match u.security_name.as_deref() {
                    Some(n) if !n.is_empty() => format!("TRAPD_{}", sanitize(n)),
                    _ => format!("TRAPD_USER_{i}"),
                };
                TrapdV3User {
                    security_name: u.security_name.clone().unwrap_or_default(),
                    engine_id: u.engine_id.clone(),
                    security_level: u.security_level.and_then(level_from_wire),
                    auth_protocol: u.auth_protocol.clone(),
                    auth_passphrase: placeholder(
                        &secret_env("AUTH_PASSPHRASE", &scope),
                        &u.auth_passphrase,
                    ),
                    privacy_protocol: u.privacy_protocol.clone(),
                    privacy_passphrase: placeholder(
                        &secret_env("PRIVACY_PASSPHRASE", &scope),
                        &u.privacy_passphrase,
                    ),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> SnmpConfigLocal {
        serde_norway::from_str(
            r#"
apiVersion: snmp.opennms.org/v1
kind: SnmpConfig
metadata: { name: default }
spec:
  defaults:
    version: v2c
    retries: 2
    readCommunity: { fromEnv: SNMP_RO_TEST }
  profiles:
    - label: core-v3
      securityLevel: authPriv
      authPassphrase: { fromEnv: SNMP_AUTH_TEST }
  definitions:
    - location: hq
      specifics: [192.168.8.8]
"#,
        )
        .unwrap()
    }

    #[test]
    fn to_wire_maps_names_and_leaves_secrets_none() {
        let w = to_wire(&local());
        assert_eq!(w.defaults.version.as_deref(), Some("v2c"));
        assert_eq!(w.defaults.retry, Some(2)); // retries → retry
        assert!(w.defaults.read_community.is_none()); // secret not resolved by to_wire
        assert_eq!(w.profiles.profile[0].config.security_level, Some(3)); // authPriv → 3
        assert_eq!(w.profiles.profile[0].label.as_deref(), Some("core-v3"));
        assert_eq!(w.definition[0].location.as_deref(), Some("hq"));
        assert_eq!(w.definition[0].specific, vec!["192.168.8.8"]);
    }

    #[test]
    fn to_wire_resolved_injects_secret_values() {
        // SAFETY: test-only env mutation.
        unsafe {
            std::env::set_var("SNMP_RO_TEST", "ro-secret");
            std::env::set_var("SNMP_AUTH_TEST", "auth-secret");
        }
        let w = to_wire_resolved(&local()).expect("resolves");
        assert_eq!(w.defaults.read_community.as_deref(), Some("ro-secret"));
        assert_eq!(
            w.profiles.profile[0].config.auth_passphrase.as_deref(),
            Some("auth-secret")
        );
        unsafe {
            std::env::remove_var("SNMP_RO_TEST");
            std::env::remove_var("SNMP_AUTH_TEST");
        }
    }

    #[test]
    fn from_wire_emits_secret_placeholders_not_cleartext() {
        let wire = server::SnmpConfig {
            defaults: server::Configuration {
                version: Some("v2c".into()),
                retry: Some(1),
                read_community: Some("public-cleartext".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let local = from_wire(&wire);
        let yaml = serde_norway::to_string(&local).unwrap();
        // The cleartext is NOT carried into the local model.
        assert!(!yaml.contains("public-cleartext"));
        let p = local.spec.defaults.as_ref().unwrap();
        assert_eq!(p.version.as_deref(), Some("v2c"));
        assert_eq!(p.retries, Some(1)); // retry → retries
        // A placeholder reference is emitted instead.
        assert!(matches!(&p.read_community, Some(SecretRef::FromEnv(_))));
    }

    #[test]
    fn from_wire_gives_each_tier_a_distinct_placeholder() {
        // defaults and a definition carry DIFFERENT communities — the exported
        // placeholders must differ so the operator can set them independently
        // (a single fixed name would collapse them on re-apply).
        let wire = server::SnmpConfig {
            defaults: server::Configuration {
                read_community: Some("default-ro".into()),
                ..Default::default()
            },
            definition: vec![server::Definition {
                config: server::Configuration {
                    read_community: Some("def-ro".into()),
                    ..Default::default()
                },
                location: Some("nyc-dc".into()),
                specific: vec!["10.0.0.1".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let local = from_wire(&wire);
        let env_of = |r: &Option<SecretRef>| match r {
            Some(SecretRef::FromEnv(e)) => e.from_env.clone(),
            _ => panic!("expected a fromEnv placeholder"),
        };
        let defaults_env = env_of(&local.spec.defaults.as_ref().unwrap().read_community);
        let def_env = env_of(&local.spec.definitions[0].params.read_community);
        assert_eq!(defaults_env, "SNMP_READ_COMMUNITY_DEFAULTS");
        assert_eq!(def_env, "SNMP_READ_COMMUNITY_DEF_0_NYC_DC");
        assert_ne!(
            defaults_env, def_env,
            "distinct tiers must export distinct placeholders"
        );
    }

    fn trapd_local() -> Trapd {
        serde_norway::from_str(
            r#"
snmpTrapPort: 162
newSuspectOnTrap: false
threads: 4
snmpv3Users:
  - securityName: monitor
    securityLevel: authPriv
    authProtocol: SHA
    authPassphrase: { fromEnv: TRAPD_AUTH_TEST }
    privacyProtocol: AES
    privacyPassphrase: { fromEnv: TRAPD_PRIV_TEST }
"#,
        )
        .unwrap()
    }

    #[test]
    fn trapd_to_wire_maps_and_leaves_secrets_none() {
        let w = trapd_to_wire(&trapd_local());
        assert_eq!(w.snmp_trap_port, Some(162));
        assert_eq!(w.new_suspect_on_trap, Some(false));
        assert_eq!(w.threads, Some(4));
        assert_eq!(w.snmpv3_user.len(), 1);
        let u = &w.snmpv3_user[0];
        assert_eq!(u.security_name.as_deref(), Some("monitor"));
        assert_eq!(u.security_level, Some(3)); // authPriv → 3
        assert!(
            u.auth_passphrase.is_none(),
            "secret not resolved by to_wire"
        );
        assert!(u.privacy_passphrase.is_none());
    }

    #[test]
    fn trapd_to_wire_resolved_injects_passphrases() {
        // SAFETY: test-only env mutation.
        unsafe {
            std::env::set_var("TRAPD_AUTH_TEST", "auth-secret");
            std::env::set_var("TRAPD_PRIV_TEST", "priv-secret");
        }
        let w = trapd_to_wire_resolved(&trapd_local()).expect("resolves");
        let u = &w.snmpv3_user[0];
        assert_eq!(u.auth_passphrase.as_deref(), Some("auth-secret"));
        assert_eq!(u.privacy_passphrase.as_deref(), Some("priv-secret"));
        unsafe {
            std::env::remove_var("TRAPD_AUTH_TEST");
            std::env::remove_var("TRAPD_PRIV_TEST");
        }
    }

    #[test]
    fn trapd_from_wire_emits_per_user_placeholders_not_cleartext() {
        let wire = server::TrapdConfig {
            snmp_trap_port: Some(162),
            new_suspect_on_trap: Some(true),
            snmpv3_user: vec![server::Snmpv3User {
                security_name: Some("monitor".into()),
                security_level: Some(3),
                auth_passphrase: Some("auth-cleartext".into()),
                privacy_passphrase: Some("priv-cleartext".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let t = trapd_from_wire(&wire);
        let yaml = serde_norway::to_string(&t).unwrap();
        assert!(!yaml.contains("auth-cleartext"));
        assert!(!yaml.contains("priv-cleartext"));
        let u = &t.snmpv3_users[0];
        assert_eq!(u.security_level, Some(SecurityLevel::AuthPriv));
        let auth_env = match &u.auth_passphrase {
            Some(SecretRef::FromEnv(e)) => e.from_env.clone(),
            _ => panic!("expected a fromEnv placeholder"),
        };
        assert_eq!(auth_env, "SNMP_AUTH_PASSPHRASE_TRAPD_MONITOR");
    }
}
