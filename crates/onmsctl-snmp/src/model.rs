/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local (YAML) model for `kind: SnmpConfig` — the operator-facing document
//! that `onmsctl apply -f` reconciles against `/api/v2/snmp-config`.
//!
//! Singleton: there is one snmp-config per Horizon, so `metadata.name` is fixed
//! to the literal `default`. The `spec` carries `defaults` (the fallback
//! parameters), `profiles` (named templates), and `definitions` (per-target
//! overrides). The SNMP parameter set ([`Params`]) is shared across all three
//! tiers via `#[serde(flatten)]`, so YAML keeps the wire's flat shape.
//!
//! Strictness: the document *structure* is strict (`deny_unknown_fields` on the
//! root, `metadata`, and `spec`). The parameter blocks are **permissive** — a
//! serde limitation: `flatten` is incompatible with `deny_unknown_fields`, so
//! an unknown key inside a `defaults`/`profile`/`definition` block is ignored
//! rather than rejected. Semantic errors are still caught by [`validate`].
//!
//! Operator-friendly local names map to the wire on conversion (a later
//! increment): `securityLevel` accepts `noAuthNoPriv`/`authNoPriv`/`authPriv`
//! (→ wire int 1/2/3), selectors are plural (`specifics`/`ranges`/`ipMatches`
//! → wire `specific`/`range`/`ipMatch`), `filterExpression` → wire `filter`,
//! `retries` → wire `retry`. Secret fields hold a [`SecretRef`], resolved on
//! apply (write-only).

use crate::secret::SecretRef;
use onmsctl_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only accepted `apiVersion`.
pub const API_VERSION: &str = "snmp.opennms.org/v1";
/// The only accepted `kind`.
pub const KIND: &str = "SnmpConfig";
/// The only accepted `metadata.name` (singleton).
pub const SINGLETON_NAME: &str = "default";

/// A `kind: SnmpConfig` document.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnmpConfigLocal {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: Spec,
}

/// Document metadata. `name` must equal [`SINGLETON_NAME`].
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
}

/// The SNMP configuration body.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Fallback parameters applied when no definition matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<Params>,
    /// Named, reusable parameter templates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ProfileLocal>,
    /// Per-target overrides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<DefinitionLocal>,
    /// SNMP **trap daemon** (Trapd) configuration. Optional and additive: when
    /// absent, `apply`/`export` touch only `/api/v2/snmp-config` and never reach
    /// the Trapd endpoint (so older Horizon is unaffected). When present, the
    /// same document also reconciles `/api/v2/trapd/config`.
    ///
    /// REQUIRES a Horizon build with the Trapd REST service (NMS-19128, the
    /// `37.x`/`develop` line); against older servers a present `trapd` block
    /// fails the trap-daemon reconcile with a clear version message while the
    /// snmp-config half is unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trapd: Option<Trapd>,
}

/// The SNMP trap daemon (Trapd) configuration — the local model for
/// `/api/v2/trapd/config` (`TrapdConfigDto`). A flat singleton: there is one
/// trap-daemon config per Horizon.
///
/// `snmpTrapPort` and `newSuspectOnTrap` are **required** (the server rejects an
/// update missing either); the remaining tuning fields are optional and inherit
/// the server's defaults when omitted (the diff treats an omitted field as
/// "don't care", matching the snmp-config tiers). Passphrases are
/// [`SecretRef`]s, resolved write-only on apply.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Trapd {
    /// Listen address; `*` (the default) binds all interfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snmp_trap_address: Option<String>,
    /// UDP listen port (1–65535). Required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snmp_trap_port: Option<i32>,
    /// Create a new suspect node on a trap from an unknown source. Required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_suspect_on_trap: Option<bool>,
    /// Persist the raw trap PDU on the resulting event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_raw_message: Option<bool>,
    /// Trap-processing thread count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<i32>,
    /// Bounded processing queue size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_size: Option<i32>,
    /// Max traps coalesced per batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<i32>,
    /// Max milliseconds to hold a partial batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_interval: Option<i32>,
    /// Honor `snmpTrapAddress` from the trap varbinds when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_address_from_varbind: Option<bool>,
    /// SNMPv3 trap users (full-replace list: the desired set is sent verbatim,
    /// so omitting a previously-present user removes it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snmpv3_users: Vec<TrapdV3User>,
}

/// An SNMPv3 user the trap daemon accepts (`Snmpv3UserDto`). `securityName` is
/// required; the auth/privacy fields follow the chosen `securityLevel`.
/// Passphrases are write-only [`SecretRef`]s.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrapdV3User {
    /// The v3 security (user) name. Required, unique within the list.
    pub security_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    /// `noAuthNoPriv` / `authNoPriv` / `authPriv` (→ wire int 1/2/3), shared
    /// with the snmp-config [`SecurityLevel`] mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_level: Option<SecurityLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_passphrase: Option<SecretRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_passphrase: Option<SecretRef>,
}

/// SNMPv3 security level (maps to the wire int 1/2/3).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub enum SecurityLevel {
    #[serde(rename = "noAuthNoPriv")]
    NoAuthNoPriv,
    #[serde(rename = "authNoPriv")]
    AuthNoPriv,
    #[serde(rename = "authPriv")]
    AuthPriv,
}

/// The SNMP parameter set shared by `defaults`, each `profile`, and each
/// `definition`. Every field is optional; absent fields inherit the server's
/// schema defaults (or, on a definition/profile, the `defaults` tier).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    /// `v1` / `v2c` / `v3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i32>,
    /// Friendly alias for the wire's `retry`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_community: Option<SecretRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_community: Option<SecretRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_repetitions: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_vars_per_pdu: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_request_size: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_level: Option<SecurityLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_passphrase: Option<SecretRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_passphrase: Option<SecretRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_engine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enterprise_id: Option<String>,
}

/// A named parameter template. `label` is required.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileLocal {
    #[serde(flatten)]
    pub params: Params,
    pub label: String,
    /// OpenNMS filter expression (wire `filter`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_expression: Option<String>,
}

/// A per-target override. At least one selector is required (see [`validate`]).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionLocal {
    #[serde(flatten)]
    pub params: Params,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specifics: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<RangeLocal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ip_matches: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Assign a named profile (must match a `spec.profiles[].label`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_label: Option<String>,
}

/// An inclusive IP range.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RangeLocal {
    pub begin: String,
    pub end: String,
}

impl SnmpConfigLocal {
    /// Validate the document. Returns a single user-actionable `Config` error
    /// on the first problem. Covers the API literals, the singleton name, and
    /// the structural rules the server enforces on definitions.
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
                "metadata.name must be {SINGLETON_NAME:?} (snmp-config is a singleton), got {:?}",
                self.metadata.name
            )));
        }

        let labels: std::collections::HashSet<&str> = self
            .spec
            .profiles
            .iter()
            .map(|p| p.label.as_str())
            .collect();

        for (i, d) in self.spec.definitions.iter().enumerate() {
            let has_specifics = !d.specifics.is_empty();
            let has_ranges = !d.ranges.is_empty();
            let has_ip_matches = !d.ip_matches.is_empty();
            if !has_specifics && !has_ranges && !has_ip_matches {
                return Err(Error::Config(format!(
                    "spec.definitions[{i}] has no selector; declare at least one of \
                     `specifics`, `ranges`, or `ipMatches`"
                )));
            }
            // The server rejects mixing IP-match expressions with explicit
            // specifics/ranges in one definition.
            if has_ip_matches && (has_specifics || has_ranges) {
                return Err(Error::Config(format!(
                    "spec.definitions[{i}]: `ipMatches` cannot be combined with \
                     `specifics` or `ranges` in the same definition"
                )));
            }
            for ip in &d.specifics {
                if ip.parse::<std::net::IpAddr>().is_err() {
                    return Err(Error::Config(format!(
                        "spec.definitions[{i}]: invalid specific IP {ip:?}"
                    )));
                }
            }
            for r in &d.ranges {
                if r.begin.parse::<std::net::IpAddr>().is_err() {
                    return Err(Error::Config(format!(
                        "spec.definitions[{i}]: invalid range begin {:?}",
                        r.begin
                    )));
                }
                if r.end.parse::<std::net::IpAddr>().is_err() {
                    return Err(Error::Config(format!(
                        "spec.definitions[{i}]: invalid range end {:?}",
                        r.end
                    )));
                }
            }
            if let Some(label) = &d.profile_label
                && !labels.contains(label.as_str())
            {
                return Err(Error::Config(format!(
                    "spec.definitions[{i}]: profileLabel {label:?} names no declared \
                     spec.profiles[].label"
                )));
            }
        }

        if let Some(trapd) = &self.spec.trapd {
            trapd.validate()?;
        }
        Ok(())
    }
}

impl Trapd {
    /// Validate the trap-daemon block. The server **requires** `snmpTrapPort`
    /// (1–65535) and `newSuspectOnTrap` on every update, so a present-but-empty
    /// `trapd: {}` is rejected here (rather than silently uploading an
    /// all-defaults config that could clobber a configured daemon). Each v3 user
    /// needs a non-empty `securityName`, unique within the list.
    pub fn validate(&self) -> Result<()> {
        match self.snmp_trap_port {
            None => {
                return Err(Error::Config(
                    "spec.trapd.snmpTrapPort is required (the server rejects a trap-daemon \
                     update without it)"
                        .into(),
                ));
            }
            Some(p) if !(1..=65535).contains(&p) => {
                return Err(Error::Config(format!(
                    "spec.trapd.snmpTrapPort must be between 1 and 65535, got {p}"
                )));
            }
            Some(_) => {}
        }
        if self.new_suspect_on_trap.is_none() {
            return Err(Error::Config(
                "spec.trapd.newSuspectOnTrap is required (the server rejects a trap-daemon \
                 update without it)"
                    .into(),
            ));
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (i, u) in self.snmpv3_users.iter().enumerate() {
            if u.security_name.trim().is_empty() {
                return Err(Error::Config(format!(
                    "spec.trapd.snmpv3Users[{i}]: securityName must not be empty"
                )));
            }
            if !seen.insert(u.security_name.as_str()) {
                return Err(Error::Config(format!(
                    "spec.trapd.snmpv3Users[{i}]: duplicate securityName {:?}",
                    u.security_name
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> std::result::Result<SnmpConfigLocal, serde_norway::Error> {
        serde_norway::from_str(yaml)
    }

    const FULL: &str = r#"
apiVersion: snmp.opennms.org/v1
kind: SnmpConfig
metadata:
  name: default
spec:
  defaults:
    version: v2c
    port: 161
    retries: 1
    readCommunity: { fromEnv: ONMS_SNMP_RO }
  profiles:
    - label: core-v3
      version: v3
      securityName: monitor
      securityLevel: authPriv
      authProtocol: SHA
      authPassphrase: { fromKeyring: { service: onmsctl, account: snmp-auth } }
      privacyProtocol: AES
      privacyPassphrase: { fromFile: /run/secrets/snmp-priv }
      filterExpression: "categoryName == 'Routers'"
  definitions:
    - location: labmonkeys-hq
      specifics: [192.168.8.8]
      ranges:
        - { begin: 10.0.0.1, end: 10.0.0.254 }
      profileLabel: core-v3
"#;

    #[test]
    fn full_document_parses_and_validates() {
        let doc = parse(FULL).expect("parses");
        doc.validate().expect("valid");
        assert_eq!(doc.spec.profiles.len(), 1);
        let p = &doc.spec.profiles[0];
        assert_eq!(p.label, "core-v3");
        assert_eq!(p.params.security_level, Some(SecurityLevel::AuthPriv));
        assert!(matches!(
            p.params.auth_passphrase,
            Some(SecretRef::FromKeyring(_))
        ));
        let d = &doc.spec.definitions[0];
        assert_eq!(d.specifics, vec!["192.168.8.8"]);
        assert_eq!(d.profile_label.as_deref(), Some("core-v3"));
    }

    #[test]
    fn wrong_singleton_name_is_rejected() {
        let doc = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: prod }\nspec: {}\n",
        )
        .unwrap();
        let err = doc.validate().unwrap_err().to_string();
        assert!(err.contains("singleton") && err.contains("default"));
    }

    #[test]
    fn definition_without_selector_is_rejected() {
        let doc = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\n\
             spec:\n  definitions:\n    - location: x\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("no selector")
        );
    }

    #[test]
    fn ipmatches_with_ranges_is_rejected() {
        let doc = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\n\
             spec:\n  definitions:\n    - ipMatches: ['10.*.*.*']\n      ranges:\n        - { begin: 10.0.0.1, end: 10.0.0.2 }\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("cannot be combined")
        );
    }

    #[test]
    fn invalid_specific_ip_is_rejected() {
        let doc = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\n\
             spec:\n  definitions:\n    - specifics: ['not-an-ip']\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("invalid specific IP")
        );
    }

    #[test]
    fn unknown_profile_label_is_rejected() {
        let doc = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\n\
             spec:\n  definitions:\n    - specifics: ['10.0.0.1']\n      profileLabel: ghost\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("names no declared")
        );
    }

    #[test]
    fn inline_secret_literal_is_rejected_at_parse() {
        // readCommunity must be a SecretRef, not a bare string.
        let err = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\n\
             spec:\n  defaults:\n    readCommunity: public\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn unknown_top_level_field_is_rejected() {
        let err = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\nspec: {}\nbogus: 1\n",
        );
        assert!(err.is_err());
    }

    const WITH_TRAPD: &str = r#"
apiVersion: snmp.opennms.org/v1
kind: SnmpConfig
metadata: { name: default }
spec:
  trapd:
    snmpTrapAddress: "*"
    snmpTrapPort: 162
    newSuspectOnTrap: false
    threads: 4
    snmpv3Users:
      - securityName: monitor
        securityLevel: authPriv
        authProtocol: SHA
        authPassphrase: { fromEnv: ONMS_TRAPD_AUTH }
        privacyProtocol: AES
        privacyPassphrase: { fromKeyring: { service: onmsctl, account: trapd-priv } }
"#;

    #[test]
    fn trapd_block_parses_and_validates() {
        let doc = parse(WITH_TRAPD).expect("parses");
        doc.validate().expect("valid");
        let t = doc.spec.trapd.as_ref().expect("trapd present");
        assert_eq!(t.snmp_trap_port, Some(162));
        assert_eq!(t.new_suspect_on_trap, Some(false));
        assert_eq!(t.threads, Some(4));
        assert_eq!(t.snmpv3_users.len(), 1);
        let u = &t.snmpv3_users[0];
        assert_eq!(u.security_name, "monitor");
        assert_eq!(u.security_level, Some(SecurityLevel::AuthPriv));
        assert!(matches!(u.auth_passphrase, Some(SecretRef::FromEnv(_))));
    }

    #[test]
    fn trapd_without_required_fields_is_rejected() {
        // Present-but-empty `trapd: {}` must fail validation, not silently
        // upload an all-defaults config.
        let doc = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\nspec:\n  trapd: {}\n",
        )
        .unwrap();
        let err = doc.validate().unwrap_err().to_string();
        assert!(err.contains("snmpTrapPort is required"), "got: {err}");
    }

    #[test]
    fn trapd_missing_new_suspect_is_rejected() {
        let doc = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\nspec:\n  trapd:\n    snmpTrapPort: 162\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("newSuspectOnTrap is required")
        );
    }

    #[test]
    fn trapd_port_out_of_range_is_rejected() {
        let doc = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\nspec:\n  trapd:\n    snmpTrapPort: 70000\n    newSuspectOnTrap: false\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("between 1 and 65535")
        );
    }

    #[test]
    fn trapd_duplicate_v3_user_is_rejected() {
        let doc = parse(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata: { name: default }\nspec:\n  trapd:\n    snmpTrapPort: 162\n    newSuspectOnTrap: false\n    snmpv3Users:\n      - securityName: dup\n      - securityName: dup\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate securityName")
        );
    }

    #[test]
    fn absent_trapd_is_none() {
        let doc = parse(FULL).expect("parses");
        assert!(doc.spec.trapd.is_none(), "no trapd block ⇒ None");
    }
}
