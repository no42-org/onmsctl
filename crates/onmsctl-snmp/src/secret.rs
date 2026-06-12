/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Client-side secret references for SNMP credentials (community strings and
//! v3 passphrases).
//!
//! Mirrors the IAM `passwordRef` shape (decision: kept capability-local rather
//! than hoisted into `onmsctl-core`, which deliberately avoids a `schemars`
//! dependency — IAM already duplicates its secret-ref types for the same
//! reason). A secret field names exactly one source:
//!
//! ```yaml
//! readCommunity:     { fromFile: /run/secrets/snmp-ro }
//! readCommunity:     { fromEnv: ONMS_SNMP_RO }
//! authPassphrase:    { fromKeyring: { service: onmsctl, account: snmp-auth } }
//! ```
//!
//! A bare inline cleartext literal is **not** accepted — `serde` deserializes a
//! secret field only as one of the three reference shapes, so
//! `readCommunity: public` fails to parse (the SNMP analogue of IAM's
//! `PR-IAM-001`). Secrets are **write-only**: resolved at apply and written
//! through to the wire, never read back, compared, or exported as cleartext.

use serde::{Deserialize, Serialize};

/// Reference to where a secret value is loaded from at apply time.
///
/// Untagged enum, each variant a strict `deny_unknown_fields` wrapper — so a
/// mapping carrying two source keys is unknown to every wrapper and parse
/// fails, giving "exactly one of the three" structurally (same strategy as
/// IAM's `PasswordRef`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum SecretRef {
    /// Read the secret from a local file (mode-checked at resolve time).
    FromFile(FromFileRef),
    /// Read the secret from an environment variable.
    FromEnv(FromEnvRef),
    /// Read the secret from the OS keyring under `service`/`account`.
    FromKeyring(FromKeyringRef),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FromFileRef {
    #[serde(rename = "fromFile")]
    pub from_file: std::path::PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FromEnvRef {
    #[serde(rename = "fromEnv")]
    pub from_env: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FromKeyringRef {
    #[serde(rename = "fromKeyring")]
    pub from_keyring: KeyringRef,
}

/// Service + account tuple identifying a keyring entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyringRef {
    pub service: String,
    pub account: String,
}

/// A resolved secret, zeroized on drop.
pub type SecretString = zeroize::Zeroizing<String>;

/// Resolve a [`SecretRef`] to its plaintext at apply time. Write-only: the
/// caller injects the value into the wire payload and never reads it back.
///
/// Mirrors IAM's `resolve_password_ref`; keyring resolution reuses the shared
/// `onmsctl_core::auth::read_keyring_secret`. An empty resolved value is an
/// error (an empty community/passphrase is almost always a misconfiguration).
///
/// NOTE: file resolution reads + trims a single trailing newline but does not
/// yet enforce file-mode hardening (IAM's `resolve_from_file` refuses
/// world-writable / warns on world-readable). That check should be hoisted to
/// a shared helper and reused here — tracked for a follow-up.
pub fn resolve_secret_ref(r: &SecretRef) -> onmsctl_core::Result<SecretString> {
    use onmsctl_core::Error;
    let value = match r {
        SecretRef::FromEnv(s) => std::env::var(&s.from_env).map_err(|_| {
            Error::Config(format!(
                "secret fromEnv: environment variable {:?} is not set",
                s.from_env
            ))
        })?,
        SecretRef::FromFile(s) => {
            let raw = std::fs::read_to_string(&s.from_file).map_err(|e| {
                Error::Config(format!("secret fromFile {}: {e}", s.from_file.display()))
            })?;
            // Trim a single trailing newline (handles `\n` and `\r\n`).
            let trimmed = raw.strip_suffix('\n').unwrap_or(&raw);
            let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
            trimmed.to_string()
        }
        SecretRef::FromKeyring(s) => onmsctl_core::auth::read_keyring_secret(
            &s.from_keyring.service,
            &s.from_keyring.account,
        )
        .map_err(|e| {
            Error::Config(format!(
                "secret fromKeyring (service={:?}, account={:?}): {e}",
                s.from_keyring.service, s.from_keyring.account
            ))
        })?,
    };
    if value.is_empty() {
        return Err(Error::Config(
            "resolved SNMP secret is empty; refusing to send an empty credential".into(),
        ));
    }
    Ok(SecretString::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Result<SecretRef, serde_norway::Error> {
        serde_norway::from_str(yaml)
    }

    #[test]
    fn each_reference_variant_parses() {
        assert!(matches!(
            parse("fromFile: /run/secrets/snmp-ro").unwrap(),
            SecretRef::FromFile(_)
        ));
        assert!(matches!(
            parse("fromEnv: ONMS_SNMP_RO").unwrap(),
            SecretRef::FromEnv(_)
        ));
        assert!(matches!(
            parse("fromKeyring:\n  service: onmsctl\n  account: snmp-auth").unwrap(),
            SecretRef::FromKeyring(_)
        ));
    }

    #[test]
    fn inline_cleartext_is_rejected() {
        // A bare scalar is not any of the three reference shapes.
        assert!(parse("public").is_err());
    }

    #[test]
    fn two_sources_are_rejected() {
        // Carries both fromEnv and fromFile → unknown to every strict wrapper.
        assert!(parse("fromEnv: X\nfromFile: /x").is_err());
    }
}
