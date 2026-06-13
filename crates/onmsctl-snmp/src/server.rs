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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definition: Vec<Definition>,
    #[serde(default, skip_serializing_if = "SnmpProfiles::is_empty")]
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
    /// Getter is `getTTL`, so the JSON key keeps both capitals.
    #[serde(rename = "TTL", skip_serializing_if = "Option::is_none")]
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
}
