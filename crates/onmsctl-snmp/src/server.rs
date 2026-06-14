/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Wire-format DTOs for `/api/v2/snmp-config`.
//!
//! Mirrored from the OpenNMS JAXB/Jackson types `SnmpConfig`, `Configuration`,
//! `Definition`, `SnmpProfile`, and `Range`. The v2 JSON is **camelCase** (the
//! `@JsonProperty` names), so the structs use `#[serde(rename_all =
//! "camelCase")]`; the few fields whose camelCase differs from the Rust
//! snake_case are covered by that rule (`read-community` → `readCommunity`,
//! `max-vars-per-pdu` → `maxVarsPerPdu`, `ip-match` → `ipMatch`, …).
//!
//! Key structural facts (from source):
//! - `SnmpConfig` **extends** `Configuration`, so the default parameters are
//!   inline at the top level — modeled here with `#[serde(flatten)]`.
//! - `Definition` and `SnmpProfile` also extend `Configuration` (flattened).
//! - definitions are a flat `"definition": [ … ]` list; profiles are wrapped:
//!   `"profiles": { "profile": [ … ] }`.
//!
//! These DTOs are **permissive** on deserialize (no `deny_unknown_fields`) so a
//! future Horizon field doesn't break parse, and they omit `None`/empty on
//! serialize so an uploaded config stays minimal.
//!
//! NOTE: the wire shape is derived from the DTO source, not yet a captured live
//! payload — the `profiles` wrapper key and secret-field population should be
//! confirmed against a real Horizon (see the change's task 9.2).

use serde::{Deserialize, Serialize};

/// Deserialize helper: treat an explicit JSON `null` as `T::default()`.
/// `#[serde(default)]` alone only covers a *missing* field — but the live v2
/// `GET /api/v2/snmp-config` sends `"profiles": null` (and may send `null` for
/// an empty list), which would otherwise fail to deserialize into a struct/Vec.
fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// The SNMP parameter set shared by defaults, definitions, and profiles
/// (`Configuration` in OpenNMS). Every field is optional; absent fields take
/// the server's schema defaults.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Configuration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_host: Option<String>,
    /// `v1` / `v2c` / `v3` (a string on the wire).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_community: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_community: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_vars_per_pdu: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_repetitions: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_request_size: Option<i32>,
    /// Server-managed: whether stored secrets are encrypted (SCV at rest).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_name: Option<String>,
    /// `1` = noAuthNoPriv, `2` = authNoPriv, `3` = authPriv (an int on the wire).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy_passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_engine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enterprise_id: Option<String>,
}

/// An IP range selector (`begin`..`end`, inclusive).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Range {
    pub begin: String,
    pub end: String,
}

/// A per-target override (`Definition`): a `Configuration` (flattened) plus the
/// selectors and the location it applies at.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    #[serde(flatten)]
    pub config: Configuration,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specific: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub range: Vec<Range>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ip_match: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_label: Option<String>,
}

/// A named, reusable parameter template (`SnmpProfile`): a `Configuration`
/// (flattened) plus a `label` and an optional `filter` expression.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnmpProfile {
    #[serde(flatten)]
    pub config: Configuration,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

/// The `profiles` wrapper: `{ "profile": [ … ] }`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SnmpProfiles {
    #[serde(
        default,
        deserialize_with = "null_to_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub profile: Vec<SnmpProfile>,
}

/// The whole SNMP configuration (`SnmpConfig`, root of `/api/v2/snmp-config`):
/// the default `Configuration` (flattened/inline) plus the definition list and
/// the profiles wrapper.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnmpConfig {
    #[serde(flatten)]
    pub defaults: Configuration,
    #[serde(
        default,
        deserialize_with = "null_to_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub definition: Vec<Definition>,
    #[serde(
        default,
        deserialize_with = "null_to_default",
        skip_serializing_if = "SnmpProfiles::is_empty"
    )]
    pub profiles: SnmpProfiles,
}

impl SnmpProfiles {
    fn is_empty(&self) -> bool {
        self.profile.is_empty()
    }
}

/// The effective agent configuration OpenNMS would use for one IP at one
/// location — the response of `GET /api/v2/snmp-config/lookup` (the `lookup`
/// verb). This is the *merged* result (defaults ⊕ matching definition ⊕
/// profile), not a tier of the stored config, so it is its own type.
///
/// Field names are the camelCase bean-property names Jackson emits from
/// `SnmpAgentConfig`'s getters (e.g. `versionAsString`, `authPassPhrase`); the
/// one exception is `TTL` (the getter is `getTTL`, so the property keeps both
/// capitals). Permissive on deserialize.
///
/// NOTE: derived from the DTO + the v2 IT test getters, not a captured live
/// response — confirm the exact key casing against a real Horizon (task 9.2).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SnmpAgentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_for: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<i32>,
    /// SNMP version as the friendly string (`v1` / `v2c` / `v3`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_as_string: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_vars_per_pdu: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_repetitions: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_request_size: Option<i32>,
    /// The live lookup response serializes this lowercase as `ttl`; accept
    /// `TTL` too in case a different code path emits the getter-cased name.
    #[serde(alias = "TTL", skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_protocol: Option<String>,
    /// The Jackson getter is `getAuthPassPhrase` → `authPassPhrase`, but the
    /// stored-config DTO uses `authPassphrase` (lowercase p); accept both so a
    /// live response in either casing can't silently drop the (masked) secret.
    #[serde(alias = "authPassphrase", skip_serializing_if = "Option::is_none")]
    pub auth_pass_phrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priv_protocol: Option<String>,
    #[serde(alias = "privPassphrase", skip_serializing_if = "Option::is_none")]
    pub priv_pass_phrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_community: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_community: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_engine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enterprise_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_label: Option<String>,
}

// ---------------------------------------------------------------------------
// Trap daemon (Trapd) — `/api/v2/trapd/config`
// ---------------------------------------------------------------------------

/// Wire DTO for the trap daemon config (`TrapdConfigDto`). Flat singleton; the
/// v2 JSON is camelCase. Every field is optional on the wire (the server has no
/// field-level validation annotations — it validates imperatively), so this is
/// permissive on deserialize and omits `None` on serialize.
///
/// NOTE: derived from the `TrapdConfigDto` source (NMS-19128), not yet a
/// captured live exchange — confirm field casing against a real Horizon.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct TrapdConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snmp_trap_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snmp_trap_port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_suspect_on_trap: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_raw_message: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_size: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_interval: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_address_from_varbind: Option<bool>,
    /// SNMPv3 trap users. The JSON key is the singular `snmpv3User` (the DTO's
    /// field name); the list may arrive as `null` on an empty config.
    #[serde(
        rename = "snmpv3User",
        deserialize_with = "null_to_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub snmpv3_user: Vec<Snmpv3User>,
}

/// Wire DTO for an SNMPv3 trap user (`Snmpv3UserDto`). camelCase, permissive.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Snmpv3User {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_name: Option<String>,
    /// `1` = noAuthNoPriv, `2` = authNoPriv, `3` = authPriv (an int on the wire).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy_passphrase: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Representative payload DERIVED from the DTO source (not a captured live
    // response). Exercises inline defaults + a v2c definition with a range +
    // a v3 profile. Confirm against a real Horizon per the change's task 9.2.
    const FIXTURE: &str = r#"{
      "version": "v2c",
      "port": 161,
      "retry": 1,
      "timeout": 1800,
      "readCommunity": "public",
      "maxRepetitions": 10,
      "definition": [
        {
          "version": "v2c",
          "readCommunity": "secret-ro",
          "location": "labmonkeys-hq",
          "specific": ["192.168.8.8"],
          "range": [{ "begin": "10.0.0.1", "end": "10.0.0.254" }]
        }
      ],
      "profiles": {
        "profile": [
          {
            "label": "core-v3",
            "filter": "categoryName == 'Routers'",
            "version": "v3",
            "securityName": "monitor",
            "securityLevel": 3,
            "authProtocol": "SHA",
            "authPassphrase": "auth-pass",
            "privacyProtocol": "AES",
            "privacyPassphrase": "priv-pass"
          }
        ]
      }
    }"#;

    #[test]
    fn fixture_deserializes_with_inline_defaults() {
        let cfg: SnmpConfig = serde_json::from_str(FIXTURE).expect("snmp-config parses");
        // Inline defaults (flattened Configuration).
        assert_eq!(cfg.defaults.version.as_deref(), Some("v2c"));
        assert_eq!(cfg.defaults.port, Some(161));
        assert_eq!(cfg.defaults.retry, Some(1));
        assert_eq!(cfg.defaults.read_community.as_deref(), Some("public"));

        // One definition with both a specific and a range, at a location.
        assert_eq!(cfg.definition.len(), 1);
        let d = &cfg.definition[0];
        assert_eq!(d.location.as_deref(), Some("labmonkeys-hq"));
        assert_eq!(d.specific, vec!["192.168.8.8"]);
        assert_eq!(d.range.len(), 1);
        assert_eq!(d.range[0].begin, "10.0.0.1");
        assert_eq!(d.range[0].end, "10.0.0.254");
        assert_eq!(d.config.read_community.as_deref(), Some("secret-ro"));

        // One v3 profile under the `profiles.profile` wrapper.
        assert_eq!(cfg.profiles.profile.len(), 1);
        let p = &cfg.profiles.profile[0];
        assert_eq!(p.label.as_deref(), Some("core-v3"));
        assert_eq!(p.filter.as_deref(), Some("categoryName == 'Routers'"));
        assert_eq!(p.config.version.as_deref(), Some("v3"));
        assert_eq!(p.config.security_level, Some(3));
        assert_eq!(p.config.privacy_protocol.as_deref(), Some("AES"));
    }

    #[test]
    fn serialize_uses_camelcase_keys_and_round_trips() {
        let cfg: SnmpConfig = serde_json::from_str(FIXTURE).unwrap();
        let json = serde_json::to_string(&cfg).unwrap();
        // camelCase wire keys (not snake_case) for the renamed fields.
        for key in [
            "readCommunity",
            "maxRepetitions",
            "securityLevel",
            "privacyPassphrase",
            "profiles",
            "profile",
        ] {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "expected camelCase key {key:?} in {json}"
            );
        }
        // Genuine round-trip: serialize → deserialize → equal. This proves the
        // flattened `Configuration` and the `profiles` wrapper preserve every
        // value at the right nesting, which substring checks cannot.
        let reparsed: SnmpConfig = serde_json::from_str(&json).expect("re-parses");
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn ip_match_renames_to_camelcase_when_populated() {
        // The fixture never sets ip_match, so exercise the rename on the
        // populated path explicitly.
        let cfg = SnmpConfig {
            definition: vec![Definition {
                ip_match: vec!["10.*.*.*".to_string()],
                location: Some("Default".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"ipMatch\""));
        assert!(!json.contains("ip_match"));
        let reparsed: SnmpConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.definition[0].ip_match, vec!["10.*.*.*"]);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // Permissive deserialize — a future server field must not break parse.
        let j = r#"{ "version": "v1", "totallyNewField": 42, "definition": [] }"#;
        let cfg: SnmpConfig = serde_json::from_str(j).expect("forward-compatible");
        assert_eq!(cfg.defaults.version.as_deref(), Some("v1"));
    }

    #[test]
    fn explicit_null_profiles_deserialize_as_empty() {
        // The live v2 GET sends `"profiles": null` (and an empty config could
        // send `null` lists). `#[serde(default)]` alone does NOT cover an
        // explicit null — the `null_to_default` deserializer must.
        let j = r#"{ "version": "v2c", "definition": null, "profiles": null }"#;
        let cfg: SnmpConfig = serde_json::from_str(j).expect("null collections tolerated");
        assert_eq!(cfg.defaults.version.as_deref(), Some("v2c"));
        assert!(cfg.definition.is_empty());
        assert!(cfg.profiles.profile.is_empty());
    }

    #[test]
    fn trapd_config_deserializes_and_round_trips_camelcase() {
        let j = r#"{
            "snmpTrapAddress": "*", "snmpTrapPort": 162, "newSuspectOnTrap": false,
            "includeRawMessage": true, "threads": 4, "queueSize": 1000,
            "useAddressFromVarbind": true,
            "snmpv3User": [
                { "securityName": "monitor", "securityLevel": 3,
                  "authProtocol": "SHA", "authPassphrase": "scrubbed",
                  "privacyProtocol": "AES", "privacyPassphrase": "scrubbed" }
            ]
        }"#;
        let cfg: TrapdConfig = serde_json::from_str(j).expect("trapd config parses");
        assert_eq!(cfg.snmp_trap_port, Some(162));
        assert_eq!(cfg.new_suspect_on_trap, Some(false));
        assert_eq!(cfg.snmpv3_user.len(), 1);
        assert_eq!(cfg.snmpv3_user[0].security_level, Some(3));

        let out = serde_json::to_string(&cfg).unwrap();
        for key in [
            "snmpTrapPort",
            "newSuspectOnTrap",
            "snmpv3User",
            "securityLevel",
        ] {
            assert!(
                out.contains(&format!("\"{key}\"")),
                "expected {key} in {out}"
            );
        }
        let reparsed: TrapdConfig = serde_json::from_str(&out).expect("re-parses");
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn trapd_null_user_list_is_tolerated() {
        // A fresh server may send `"snmpv3User": null`.
        let j = r#"{ "snmpTrapPort": 162, "newSuspectOnTrap": false, "snmpv3User": null }"#;
        let cfg: TrapdConfig = serde_json::from_str(j).expect("null user list tolerated");
        assert!(cfg.snmpv3_user.is_empty());
    }

    #[test]
    fn agent_config_lookup_shape_deserializes() {
        // Shape captured from a live `GET /api/v2/snmp-config/lookup` (secret
        // values scrubbed): `ttl` is lowercase, `version` is numeric (ignored),
        // `version3` is an unmodeled bool, and the v3 passphrases are capital-P.
        let j = r#"{
            "address": "192.168.8.8", "version3": false, "port": 161,
            "version": 2, "versionAsString": "v2c", "timeout": 1800, "retries": 1,
            "securityName": "snmpUser", "readCommunity": "scrubbed",
            "writeCommunity": "scrubbed", "authPassPhrase": null,
            "privPassPhrase": null, "securityLevel": 1, "ttl": 7000,
            "maxRepetitions": 2, "maxVarsPerPdu": 10, "maxRequestSize": 65535
        }"#;
        let a: SnmpAgentConfig = serde_json::from_str(j).expect("lookup shape parses");
        assert_eq!(a.version_as_string.as_deref(), Some("v2c"));
        assert_eq!(a.ttl, Some(7000)); // lowercase `ttl` populates
        assert_eq!(a.port, Some(161));
        assert_eq!(a.read_community.as_deref(), Some("scrubbed"));
    }
}
