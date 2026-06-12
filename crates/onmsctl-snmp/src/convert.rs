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
    DefinitionLocal, Params, ProfileLocal, RangeLocal, SecurityLevel, SnmpConfigLocal, Spec,
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

fn config_to_params(c: &server::Configuration) -> Params {
    Params {
        version: c.version.clone(),
        port: c.port,
        timeout: c.timeout,
        retries: c.retry,
        ttl: c.ttl,
        proxy_host: c.proxy_host.clone(),
        read_community: placeholder("SNMP_READ_COMMUNITY", &c.read_community),
        write_community: placeholder("SNMP_WRITE_COMMUNITY", &c.write_community),
        max_repetitions: c.max_repetitions,
        max_vars_per_pdu: c.max_vars_per_pdu,
        max_request_size: c.max_request_size,
        security_name: c.security_name.clone(),
        security_level: c.security_level.and_then(level_from_wire),
        auth_protocol: c.auth_protocol.clone(),
        auth_passphrase: placeholder("SNMP_AUTH_PASSPHRASE", &c.auth_passphrase),
        privacy_protocol: c.privacy_protocol.clone(),
        privacy_passphrase: placeholder("SNMP_PRIVACY_PASSPHRASE", &c.privacy_passphrase),
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
            defaults: Some(config_to_params(&wire.defaults)),
            profiles: wire
                .profiles
                .profile
                .iter()
                .map(|p| ProfileLocal {
                    params: config_to_params(&p.config),
                    label: p.label.clone().unwrap_or_default(),
                    filter_expression: p.filter.clone(),
                })
                .collect(),
            definitions: wire
                .definition
                .iter()
                .map(|d| DefinitionLocal {
                    params: config_to_params(&d.config),
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
                })
                .collect(),
        },
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
}
