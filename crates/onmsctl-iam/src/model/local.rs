/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local YAML model for the IAM capability — the typed shape operators
//! write under `kind: User` and submit through `iam apply -f`.
//!
//! See `openspec/changes/add-iam-capability/design.md` §D3 (declarative
//! YAML), §D5 (passwordRef policy), §D7 (form-encoded PUT), §D9
//! (unmodeled passthrough), §D11.5 (dutySchedule create-only),
//! §D13 (known-roles validation).
//!
//! The wire DTO (`OnmsUserWire`) is intentionally absent from this module
//! — it lives under `model::wire` and depends on spike 0.1's verdict on
//! POST body serialization (XML vs JSON). The local model is decoupled
//! from that decision; it round-trips through `export` and the planner
//! produces wire bodies in `model::convert` (added when the wire DTO
//! lands).

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::de::{Deserializer, Error as DeError};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// API version literal for IAM documents.
pub const API_VERSION: &str = "onmsctl.no42.org/v1alpha1";

/// Kind literal for user documents.
pub const KIND_USER: &str = "User";

/// Upstream's canonical role set, verified against `Authentication.s_availableRoles`
/// on the `develop` branch (2026-05-27 via spike 0.2). Operators MAY extend
/// the upstream set server-side via `etc/security-roles.properties`; this
/// list is the **default** for soft validation only — unknown roles emit a
/// warning, never a refusal. Per-context override lives at `iam.known-roles`.
pub const KNOWN_ROLES: &[&str] = &[
    "ROLE_USER",
    "ROLE_ADMIN",
    "ROLE_READONLY",
    "ROLE_DASHBOARD",
    "ROLE_DELEGATE",
    "ROLE_RTC",
    "ROLE_PROVISION",
    "ROLE_REST",
    "ROLE_ASSET_EDITOR",
    "ROLE_FILESYSTEM_EDITOR",
    "ROLE_MOBILE",
    "ROLE_JMX",
    "ROLE_MINION",
    "ROLE_REPORT_DESIGNER",
    "ROLE_FLOW_MANAGER",
    "ROLE_DEVICE_CONFIG_BACKUP",
];

// ---------------------------------------------------------------------------
// ApiVersion / Kind newtypes — validated literals, same pattern as
// provisioning's RequisitionLocal (commit 9695c90).
// ---------------------------------------------------------------------------

/// Newtype wrapping the `apiVersion` literal. Deserialization rejects any
/// value other than [`API_VERSION`] so parse fails fast for typos or
/// wrong-version documents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiVersion;

impl Serialize for ApiVersion {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(API_VERSION)
    }
}

impl<'de> Deserialize<'de> for ApiVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s != API_VERSION {
            return Err(DeError::custom(format!(
                "unsupported apiVersion {s:?}; expected {API_VERSION:?}"
            )));
        }
        Ok(ApiVersion)
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(API_VERSION)
    }
}

impl JsonSchema for ApiVersion {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ApiVersion".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "const": API_VERSION,
            "description": "API version literal. Must equal 'onmsctl.no42.org/v1alpha1'."
        })
    }
}

/// Newtype wrapping the `kind` literal. Rejects anything other than
/// [`KIND_USER`] at parse time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindUser;

impl Serialize for KindUser {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(KIND_USER)
    }
}

impl<'de> Deserialize<'de> for KindUser {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s != KIND_USER {
            return Err(DeError::custom(format!(
                "unsupported kind {s:?}; expected {KIND_USER:?}"
            )));
        }
        Ok(KindUser)
    }
}

impl fmt::Display for KindUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(KIND_USER)
    }
}

impl JsonSchema for KindUser {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "KindUser".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "const": KIND_USER,
            "description": "Kind literal. Must equal 'User'."
        })
    }
}

// ---------------------------------------------------------------------------
// Metadata — name + unmodeled passthrough
// ---------------------------------------------------------------------------

/// Document metadata: foreign-key username + optional unmodeled passthrough.
///
/// The `unmodeled` field carries fields Horizon exposes on a user that this
/// DTO does not model — same convention as provisioning's `RequisitionLocal::Metadata`
/// (commit 9695c90). The annotation round-trips through `export` and is
/// stripped before the wire body reaches Horizon and before `l1_compare`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Metadata {
    /// Username. Must be non-empty and NOT match `^\d+$` (numeric-only):
    /// upstream's `{userCriteria}` path parameter resolves either by
    /// username OR by numeric DB ID, and a numeric-string username is
    /// routed ambiguously. Enforced at parse time as finding `PR-IAM-003`.
    #[serde(deserialize_with = "deserialize_user_name")]
    pub name: String,

    /// Fields Horizon exposes that the typed DTO doesn't model. See the
    /// type-level doc comment for shape. `Option<Mapping>` so an empty
    /// annotation serializes as absent rather than as an empty map.
    /// `serde_norway::Mapping` lacks `JsonSchema`; the schemars workaround
    /// advertises a generic JSON object at this key, mirroring the
    /// established pattern in `RequisitionLocal::Metadata`.
    #[serde(
        rename = "x-onmsctl-unmodeled",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(
        with = "Option<std::collections::BTreeMap<String, serde_json::Value>>",
        description = "Server-side fields the DTO does not model. Stripped before apply; ignored by Horizon."
    )]
    pub unmodeled: Option<serde_norway::Mapping>,
}

fn deserialize_user_name<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Err(DeError::custom("metadata.name must not be empty"));
    }
    // PR-IAM-003: numeric-only metadata.name is ambiguous against upstream
    // `{userCriteria}` resolution (username vs DB id). Refuse at parse time.
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DeError::custom(format!(
            "PR-IAM-003: metadata.name {s:?} is numeric-only; upstream {{userCriteria}} \
             resolves either by username OR by numeric DB ID, so this would be routed \
             ambiguously. Rename the user upstream."
        )));
    }
    Ok(s)
}

// ---------------------------------------------------------------------------
// PasswordRef — secret indirection
// ---------------------------------------------------------------------------

/// Reference to where to load the plaintext password from. The YAML shape
/// per design.md §D5 is a single-key mapping naming the source:
///
/// ```yaml
/// passwordRef: { fromFile: /run/secrets/alice.pw }
/// passwordRef: { fromEnv: ALICE_PW }
/// passwordRef: { fromKeyring: { service: onmsctl, account: alice } }
/// ```
///
/// Resolution happens at apply time in Group 5; `Create` plans embed the
/// resolved value, `Update` plans ignore the ref.
///
/// The enum is **untagged with each variant wrapping a strict
/// `deny_unknown_fields` struct**. Serde does not accept
/// `deny_unknown_fields` as a per-variant attribute on enum struct
/// variants, so each variant takes a one-field wrapper struct that
/// carries the strict-field attribute itself. This is what gives us
/// "exactly one of the three" structurally — a mapping carrying two
/// source keys is unknown to every wrapper, so every variant fails.
/// serde_norway's externally-tagged YAML representation uses tag syntax
/// (`!FromFile /path`) which would diverge from the spec shape, so
/// untagged is the right discriminator strategy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PasswordRef {
    /// Read the password from a local file. World-writable mode is refused;
    /// world-readable triggers a warning. Group 5 implements the mode checks.
    FromFile(FromFileRef),
    /// Read the password from an environment variable.
    FromEnv(FromEnvRef),
    /// Read the password from the OS keyring under the given service+account.
    FromKeyring(FromKeyringRef),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FromFileRef {
    #[serde(rename = "fromFile")]
    pub from_file: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FromEnvRef {
    #[serde(rename = "fromEnv")]
    pub from_env: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FromKeyringRef {
    #[serde(rename = "fromKeyring")]
    pub from_keyring: KeyringRef,
}

/// Service + account tuple identifying a keyring entry. Mirrors
/// `onmsctl_core::config::KeyringRef` field-for-field; defined locally so
/// onmsctl-core does not have to depend on `schemars` just to advertise
/// this struct in the IAM JSON schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyringRef {
    pub service: String,
    pub account: String,
}

// ---------------------------------------------------------------------------
// UserSpec — the body of a User document
// ---------------------------------------------------------------------------

/// The mutable IAM body of a User document.
///
/// `password: <literal>` is explicitly rejected by [`UserSpec::deserialize`]
/// with finding `PR-IAM-001`; the only password channel is `passwordRef`.
///
/// `dutySchedule` is modeled so it round-trips through `export`, but is
/// **create-only** on the apply path: an Update plan that sees a diff on
/// this field emits warning `PR-IAM-004` rather than mutating, because the
/// form-encoded PUT cannot reliably round-trip a `List<String>` through
/// Spring's `BeanWrapper` (`params.getFirst(key)` discards multi-valued
/// entries). See design.md §D11.5.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct UserSpec {
    #[serde(rename = "fullName", default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<String>,
    /// `List<String>` on the wire side. Create-only via the POST XML body;
    /// updates emit a warning instead of mutating. See type-level doc.
    #[serde(
        rename = "dutySchedule",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub duty_schedule: Option<String>,
    /// Closed-set of role strings. Duplicate entries in the input list are
    /// rejected at parse time (BTreeSet alone would silently merge them).
    /// Soft validation against [`KNOWN_ROLES`] warns on unknowns; never refuses.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub roles: BTreeSet<String>,
    /// Secret indirection — never a literal password. See design.md §D5.
    #[serde(
        rename = "passwordRef",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub password_ref: Option<PasswordRef>,
}

// Custom Deserialize for UserSpec: catches a literal `password:` field via
// a honeypot and emits the specific PR-IAM-001 finding (the derived
// `deny_unknown_fields` error would say "unknown field" instead). Also
// detects duplicate role entries which a plain `BTreeSet` would silently
// dedupe.
impl<'de> Deserialize<'de> for UserSpec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            #[serde(rename = "fullName", default)]
            full_name: Option<String>,
            #[serde(default)]
            email: Option<String>,
            #[serde(default)]
            comments: Option<String>,
            #[serde(rename = "dutySchedule", default)]
            duty_schedule: Option<String>,
            #[serde(default, deserialize_with = "deserialize_unique_roles")]
            roles: BTreeSet<String>,
            #[serde(rename = "passwordRef", default)]
            password_ref: Option<PasswordRef>,
            // Honeypot: serde populates this when `password:` appears in
            // the input map. The catch is intentional so the error message
            // can name the policy (PR-IAM-001) rather than the generic
            // "unknown field" serde produces.
            #[serde(default)]
            password: Option<serde::de::IgnoredAny>,
        }

        let raw = Raw::deserialize(d)?;
        if raw.password.is_some() {
            return Err(DeError::custom(
                "PR-IAM-001: never commit plaintext passwords; use `passwordRef: { fromFile | fromEnv | fromKeyring }` instead",
            ));
        }
        Ok(UserSpec {
            full_name: raw.full_name,
            email: raw.email,
            comments: raw.comments,
            duty_schedule: raw.duty_schedule,
            roles: raw.roles,
            password_ref: raw.password_ref,
        })
    }
}

fn deserialize_unique_roles<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeSet<String>, D::Error> {
    let vec: Vec<String> = Vec::deserialize(d)?;
    let mut set = BTreeSet::new();
    for r in vec {
        if !set.insert(r.clone()) {
            return Err(DeError::custom(format!(
                "duplicate role {r:?} in spec.roles; declare each role at most once"
            )));
        }
    }
    Ok(set)
}

// ---------------------------------------------------------------------------
// UserLocal — top-level document
// ---------------------------------------------------------------------------

/// One `kind: User` document, as parsed from local YAML.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserLocal {
    #[serde(rename = "apiVersion")]
    pub api_version: ApiVersion,
    pub kind: KindUser,
    pub metadata: Metadata,
    pub spec: UserSpec,
}

impl UserLocal {
    /// Canonical JSON value of this document with the local-only
    /// `metadata.x-onmsctl-unmodeled` annotation stripped. Used by the
    /// planner's `l1_compare` so apply outcome is identical with or
    /// without the annotation. Mirrors `provisioning::diff::canonical_value`.
    pub fn canonicalize(&self) -> serde_json::Value {
        let mut value: serde_json::Value =
            serde_json::to_value(self).expect("UserLocal serializes as JSON");
        if let Some(metadata) = value
            .get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
        {
            metadata.remove("x-onmsctl-unmodeled");
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_user(yaml: &str) -> Result<UserLocal, serde_norway::Error> {
        serde_norway::from_str::<UserLocal>(yaml)
    }

    #[test]
    fn parses_minimal_user() {
        let yaml = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: alice
spec:
  fullName: Alice Example
"#;
        let u = parse_user(yaml).unwrap();
        assert_eq!(u.metadata.name, "alice");
        assert_eq!(u.spec.full_name.as_deref(), Some("Alice Example"));
        assert!(u.spec.roles.is_empty());
        assert!(u.spec.password_ref.is_none());
        assert!(u.spec.duty_schedule.is_none());
    }

    #[test]
    fn rejects_wrong_api_version() {
        let yaml = r#"
apiVersion: provisioning.opennms.org/v1
kind: User
metadata:
  name: alice
spec: {}
"#;
        let err = parse_user(yaml).unwrap_err().to_string();
        assert!(err.contains("apiVersion"));
    }

    #[test]
    fn rejects_wrong_kind() {
        let yaml = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: Requisition
metadata:
  name: alice
spec: {}
"#;
        let err = parse_user(yaml).unwrap_err().to_string();
        assert!(err.contains("kind"));
    }

    #[test]
    fn rejects_literal_password_with_pr_iam_001() {
        let yaml = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: alice
spec:
  password: hunter2
"#;
        let err = parse_user(yaml).unwrap_err().to_string();
        assert!(
            err.contains("PR-IAM-001"),
            "expected PR-IAM-001 finding, got: {err}"
        );
        assert!(err.contains("passwordRef"));
    }

    #[test]
    fn rejects_numeric_only_username_with_pr_iam_003() {
        let yaml = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: "12345"
spec: {}
"#;
        let err = parse_user(yaml).unwrap_err().to_string();
        assert!(
            err.contains("PR-IAM-003"),
            "expected PR-IAM-003 finding, got: {err}"
        );
    }

    #[test]
    fn rejects_empty_username() {
        let yaml = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: ""
spec: {}
"#;
        let err = parse_user(yaml).unwrap_err().to_string();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn rejects_duplicate_roles_at_parse() {
        let yaml = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: alice
spec:
  roles:
    - ROLE_USER
    - ROLE_REST
    - ROLE_USER
"#;
        let err = parse_user(yaml).unwrap_err().to_string();
        assert!(err.contains("duplicate role"));
        assert!(err.contains("ROLE_USER"));
    }

    #[test]
    fn password_ref_round_trips_each_variant() {
        for (yaml_value, expected) in [
            (
                "fromFile: /run/secrets/alice.pw",
                PasswordRef::FromFile(FromFileRef {
                    from_file: PathBuf::from("/run/secrets/alice.pw"),
                }),
            ),
            (
                "fromEnv: ALICE_PW",
                PasswordRef::FromEnv(FromEnvRef {
                    from_env: "ALICE_PW".into(),
                }),
            ),
            (
                "fromKeyring:\n  service: onmsctl\n  account: alice",
                PasswordRef::FromKeyring(FromKeyringRef {
                    from_keyring: KeyringRef {
                        service: "onmsctl".into(),
                        account: "alice".into(),
                    },
                }),
            ),
        ] {
            let parsed: PasswordRef = serde_norway::from_str(yaml_value).unwrap();
            assert_eq!(parsed, expected, "input: {yaml_value}");
            // Round-trip: serialize and re-parse.
            let re = serde_norway::to_string(&parsed).unwrap();
            let again: PasswordRef = serde_norway::from_str(&re).unwrap();
            assert_eq!(again, expected);
        }
    }

    #[test]
    fn password_ref_rejects_two_keys_present() {
        // "Exactly one of the three" enforcement: a passwordRef that lists
        // both fromFile and fromEnv has to fail. Untagged + per-variant
        // deny_unknown_fields gives us this structurally — every variant
        // sees the other variant's key as unknown.
        let bad = "fromFile: /a\nfromEnv: B";
        let err = serde_norway::from_str::<PasswordRef>(bad).unwrap_err();
        let _ = err; // any error is fine; the point is parse must fail.
        assert!(serde_norway::from_str::<PasswordRef>(bad).is_err());
    }

    #[test]
    fn unmodeled_annotation_round_trips_and_canonicalize_strips_it() {
        let yaml = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: alice
  x-onmsctl-unmodeled:
    legacyField: gone-in-39
spec:
  fullName: Alice
"#;
        let u = parse_user(yaml).unwrap();
        assert!(u.metadata.unmodeled.is_some());

        // Round-trip: serialize back to YAML; annotation survives.
        let dumped = serde_norway::to_string(&u).unwrap();
        assert!(
            dumped.contains("x-onmsctl-unmodeled"),
            "annotation should survive YAML round-trip: {dumped}"
        );

        // Canonicalize: annotation stripped from the JSON value.
        let canon = u.canonicalize();
        let meta = canon
            .get("metadata")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        assert!(
            !meta.contains_key("x-onmsctl-unmodeled"),
            "canonicalize must strip the annotation"
        );
        assert_eq!(meta.get("name").and_then(|v| v.as_str()), Some("alice"));
    }

    #[test]
    fn canonicalize_outcome_identical_with_or_without_annotation() {
        let base = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: alice
spec:
  fullName: Alice
  roles: [ROLE_USER]
"#;
        let with_anno = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: alice
  x-onmsctl-unmodeled:
    extraField: from-server
spec:
  fullName: Alice
  roles: [ROLE_USER]
"#;
        let a = parse_user(base).unwrap().canonicalize();
        let b = parse_user(with_anno).unwrap().canonicalize();
        assert_eq!(
            a, b,
            "canonicalized values must match regardless of annotation"
        );
    }

    #[test]
    fn duty_schedule_round_trips() {
        // §D11.5: dutySchedule is modeled so it survives YAML round-trips,
        // even though the apply path treats it as create-only and warns on
        // Update diffs (PR-IAM-004, enforced in Group 6).
        let yaml = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: alice
spec:
  dutySchedule: "MoTuWeThFr0800-1700"
"#;
        let u = parse_user(yaml).unwrap();
        assert_eq!(u.spec.duty_schedule.as_deref(), Some("MoTuWeThFr0800-1700"));
        let dumped = serde_norway::to_string(&u).unwrap();
        assert!(dumped.contains("dutySchedule"));
        assert!(dumped.contains("MoTuWeThFr0800-1700"));
    }

    #[test]
    fn rejects_unknown_top_level_spec_field() {
        let yaml = r#"
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: alice
spec:
  fullName: Alice
  bogus: should-be-rejected
"#;
        let err = parse_user(yaml).unwrap_err().to_string();
        assert!(err.contains("bogus") || err.contains("unknown field"));
    }

    #[test]
    fn known_roles_includes_upstream_set() {
        // Spike 0.2 verified this list against Authentication.s_availableRoles
        // on `develop` HEAD; this test locks it so a careless edit gets caught.
        assert!(KNOWN_ROLES.contains(&"ROLE_ADMIN"));
        assert!(KNOWN_ROLES.contains(&"ROLE_PROVISION"));
        assert!(KNOWN_ROLES.contains(&"ROLE_JMX"));
        assert!(KNOWN_ROLES.contains(&"ROLE_DEVICE_CONFIG_BACKUP"));
        // Ensure the deferred-from-design "MEASUREMENTS / OPERATOR" mistake
        // does not creep back in.
        assert!(!KNOWN_ROLES.contains(&"ROLE_MEASUREMENTS"));
        assert!(!KNOWN_ROLES.contains(&"ROLE_OPERATOR"));
        assert_eq!(KNOWN_ROLES.len(), 16);
    }
}
