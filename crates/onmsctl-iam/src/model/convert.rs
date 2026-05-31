/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Conversions between the server wire DTOs ([`super::wire`]) and the local
//! YAML model ([`super::local`]).
//!
//! Only the **server → local** direction lives here. The local → server
//! directions are split by verb and live in [`super::wire`]: create bodies
//! go through [`super::wire::user_create_xml`] (XML), updates through
//! [`super::wire::UpdateForm`] (form-encoded). There is no single
//! `local_to_wire` because the three verbs use three different formats.
//!
//! `wire_to_local` is intentionally **lossy** in a controlled way:
//!
//! - `password` / `passwordSalt` are dropped — the local model never
//!   carries a password (only a `passwordRef`, which is a create-time
//!   input, not a server-readable value).
//! - Unmodeled server fields (anything in [`OnmsUserWire::extras`]) are
//!   folded into `metadata.x-onmsctl-unmodeled` so they survive a
//!   `server → local → server` round-trip.
//! - Empty-string scalars (`""`) normalize to `None`. Horizon returns
//!   `"email":""` for users without an email; mapping that to `Some("")`
//!   would produce a spurious diff against a local document that simply
//!   omits the field.
//! - `duty-schedule` is an array on the wire but `Option<String>` in the
//!   local model (§D11.5, create-only). v1 carries only the **first**
//!   entry into the local model; see [`wire_to_local`] for the limitation.

use crate::model::local::{ApiVersion, KindUser, Metadata, UserLocal, UserSpec};
use crate::model::wire::OnmsUserWire;

/// Build a local [`UserLocal`] from a server user response. Used as the
/// diff baseline (canonicalize remote state into the local shape before
/// comparing against the operator's YAML).
///
/// **Limitation (v1):** a user with more than one `duty-schedule` entry on
/// the server collapses to its first entry in the local model, because the
/// local `dutySchedule` is `Option<String>` (§D11.5). This only affects
/// `export` fidelity for pre-existing multi-block schedules — an uncommon
/// shape — and never the apply path, where `dutySchedule` is create-only
/// and update diffs emit `PR-IAM-004` rather than comparing field values.
pub fn wire_to_local(wire: &OnmsUserWire) -> UserLocal {
    let unmodeled = if wire.extras.is_empty() {
        None
    } else {
        Some(json_map_to_yaml_mapping(&wire.extras))
    };

    UserLocal {
        api_version: ApiVersion,
        kind: KindUser,
        metadata: Metadata {
            name: wire.user_id.clone(),
            unmodeled,
        },
        spec: UserSpec {
            full_name: empty_to_none(wire.full_name.as_deref()),
            email: empty_to_none(wire.email.as_deref()),
            comments: empty_to_none(wire.user_comments.as_deref()),
            duty_schedule: wire.duty_schedule.first().cloned(),
            roles: wire.roles.iter().cloned().collect(),
            password_ref: None,
        },
    }
}

/// `Some("")` → `None`; `Some(non-empty)` and `None` pass through. Keeps the
/// diff baseline aligned with local documents that omit empty fields.
fn empty_to_none(s: Option<&str>) -> Option<String> {
    match s {
        Some(v) if !v.is_empty() => Some(v.to_owned()),
        _ => None,
    }
}

/// Convert a JSON object (the wire `extras` catch-all) into a
/// `serde_norway::Mapping` for the local unmodeled annotation. JSON is a
/// subset of YAML, so the JSON text parses directly as a YAML mapping.
fn json_map_to_yaml_mapping(
    map: &serde_json::Map<String, serde_json::Value>,
) -> serde_norway::Mapping {
    let json = serde_json::Value::Object(map.clone()).to_string();
    serde_norway::from_str(&json).expect("a JSON object is always a valid YAML mapping")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::wire::OnmsUserWire;
    use std::collections::BTreeSet;

    fn wire(json: &str) -> OnmsUserWire {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn maps_core_fields_and_drops_password() {
        let u = wire(
            r#"{"user-id":"alice","full-name":"Alice","user-comments":"hi",
                "email":"a@x.io","password":"HASH","passwordSalt":true,
                "duty-schedule":["MoTuWeThFr800-1700"],"role":["ROLE_USER","ROLE_REST"]}"#,
        );
        let local = wire_to_local(&u);
        assert_eq!(local.metadata.name, "alice");
        assert_eq!(local.spec.full_name.as_deref(), Some("Alice"));
        assert_eq!(local.spec.comments.as_deref(), Some("hi"));
        assert_eq!(local.spec.email.as_deref(), Some("a@x.io"));
        assert_eq!(
            local.spec.duty_schedule.as_deref(),
            Some("MoTuWeThFr800-1700")
        );
        assert_eq!(
            local.spec.roles,
            BTreeSet::from(["ROLE_USER".to_string(), "ROLE_REST".to_string()])
        );
        // Password hash never crosses into the local model.
        assert!(local.spec.password_ref.is_none());
        assert!(local.metadata.unmodeled.is_none());
    }

    #[test]
    fn empty_email_normalizes_to_none() {
        // Horizon returns "" for the admin user's email; must not surface
        // as Some("") or it would diff against a local doc that omits email.
        let u = wire(r#"{"user-id":"admin","full-name":"Administrator","email":""}"#);
        let local = wire_to_local(&u);
        assert!(local.spec.email.is_none());
    }

    #[test]
    fn no_roles_yields_empty_set() {
        let u = wire(r#"{"user-id":"bob"}"#);
        let local = wire_to_local(&u);
        assert!(local.spec.roles.is_empty());
        assert!(local.spec.duty_schedule.is_none());
    }

    #[test]
    fn unmodeled_fields_fold_into_annotation() {
        let u = wire(r#"{"user-id":"carol","x-future":"keep","numeric":7}"#);
        let local = wire_to_local(&u);
        let anno = local.metadata.unmodeled.expect("annotation present");
        assert_eq!(
            anno.get(serde_norway::Value::from("x-future"))
                .and_then(|v| v.as_str()),
            Some("keep")
        );
        assert_eq!(
            anno.get(serde_norway::Value::from("numeric"))
                .and_then(|v| v.as_i64()),
            Some(7)
        );
    }

    #[test]
    fn multi_entry_duty_schedule_keeps_first_only() {
        // Documented v1 limitation: local dutySchedule is Option<String>.
        let u = wire(r#"{"user-id":"oncall","duty-schedule":["Mo800-1700","Tu800-1700"]}"#);
        let local = wire_to_local(&u);
        assert_eq!(local.spec.duty_schedule.as_deref(), Some("Mo800-1700"));
    }

    #[test]
    fn roundtrip_local_canonicalize_is_stable_across_unmodeled() {
        // wire_to_local feeds the diff baseline; canonicalize must strip the
        // unmodeled annotation so a server-only field can't trip a false diff.
        let bare = wire(r#"{"user-id":"dave","full-name":"Dave"}"#);
        let with_extra = wire(r#"{"user-id":"dave","full-name":"Dave","srv-only":"x"}"#);
        assert_eq!(
            wire_to_local(&bare).canonicalize(),
            wire_to_local(&with_extra).canonicalize()
        );
    }
}
