/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Wire-format DTOs for Horizon's v1 `UserRestService`
//! (`/rest/users`, `/rest/users/{name}`, `/rest/users/whoami`).
//!
//! ## Serialization split (spike 0.1, verified 2026-05-29 against the dev lab)
//!
//! The endpoint is asymmetric — three different body formats on three
//! verbs, all confirmed by curl against a live Horizon:
//!
//! - **GET / list / whoami → JSON.** `Accept: application/json` yields a
//!   clean JSON body. Field names are hyphenated (`user-id`, `full-name`,
//!   `user-comments`, `duty-schedule`) with the role array under the
//!   **singular** key `role`, plus the camelCase `passwordSalt` boolean.
//!   [`OnmsUserWire`] / [`OnmsUserListWire`] model this.
//! - **POST `/users` → XML only.** A JSON POST returns `415 Unsupported
//!   Media Type`; XML (root `<user>`) returns `201`. [`user_create_xml`]
//!   builds that body via `quick-xml`. The plaintext password rides in
//!   the body and `?hashPassword=true` rides on the URL (Group 4).
//! - **PUT `/users/{name}` → form-encoded** with **bean-property** keys
//!   (`fullName`, `email`, `comments`), not hyphenated. [`UpdateForm`]
//!   models this; it is serialized with `serde_urlencoded` in the api
//!   layer (Group 4).
//!
//! Conversions between these wire shapes and the local YAML model live in
//! [`crate::model::convert`].
//!
//! Like provisioning's `server` DTOs these are **permissive** on
//! deserialize (no `deny_unknown_fields`); unknown server fields are
//! captured in [`OnmsUserWire::extras`] so a `wire → local → wire`
//! round-trip preserves data this DTO does not model (folded into the
//! local `metadata.x-onmsctl-unmodeled` annotation).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// JSON response DTOs — GET /users/{name}, GET /users/whoami, GET /users
// ---------------------------------------------------------------------------

/// A single user as returned by `GET /users/{name}` (and as an element of
/// the `GET /users` list). JSON, hyphenated keys.
///
/// `password` / `password_salt` are the **server-side** hash and salt
/// flag; they are read here so the DTO round-trips a full server response,
/// but the conversion to the local model deliberately drops them — the
/// local YAML never carries a password (only a `passwordRef`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct OnmsUserWire {
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_comments: Option<String>,
    /// Repeated on the wire; `[]` when unset. Modeled create-only on the
    /// local side (§D11.5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub duty_schedule: Vec<String>,
    /// Role array under the **singular** wire key `role`.
    #[serde(rename = "role", default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    /// Server-side password hash. Present on list/get responses; never
    /// mapped to the local model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// camelCase on the wire (`passwordSalt`), not hyphenated — so it needs
    /// an explicit rename that overrides the struct-level `kebab-case`.
    #[serde(
        rename = "passwordSalt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub password_salt: Option<bool>,
    /// Catch-all for server fields this DTO does not model, so they survive
    /// a `wire → local → wire` round-trip via the local unmodeled
    /// annotation. `serde(flatten)` collects every otherwise-unmatched key.
    #[serde(flatten)]
    pub extras: serde_json::Map<String, serde_json::Value>,
}

/// `GET /users` list wrapper. Verified shape (spike 0.4):
/// `{"offset":0,"count":N,"totalCount":N,"user":[...]}`. A single
/// unbounded GET returns the full set — `count == totalCount`, no real
/// paging on this endpoint.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct OnmsUserListWire {
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub count: i64,
    /// Note: `totalCount`, **not** `totalRecords`.
    #[serde(rename = "totalCount", default)]
    pub total_count: i64,
    /// Singular wire key `user`, always an array.
    #[serde(rename = "user", default)]
    pub users: Vec<OnmsUserWire>,
}

// ---------------------------------------------------------------------------
// Form-encoded PUT body — PUT /users/{name}
// ---------------------------------------------------------------------------

/// Body for the form-encoded `PUT /users/{name}` update. Keys are Java
/// **bean-property** names (`fullName`, `email`, `comments`) — distinct
/// from the hyphenated JSON response keys — because `updateUser` dispatches
/// through Spring's `BeanWrapper.setPropertyValue` (§D7). `None` fields are
/// omitted, so the planner can send a narrow update touching only the
/// changed scalars.
///
/// There is intentionally **no `#[serde(flatten)] extras`** and **no
/// `dutySchedule` / `roles`** here: `serde_urlencoded` cannot encode nested
/// or repeated values, and `dutySchedule` is create-only (§D11.5). Roles
/// are mutated through the dedicated `/users/{name}/roles/{role}`
/// endpoints, not this form.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct UpdateForm {
    #[serde(rename = "fullName", skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<String>,
}

impl UpdateForm {
    /// `true` when no field is set — the planner uses this to avoid issuing
    /// an empty PUT.
    pub fn is_empty(&self) -> bool {
        self.full_name.is_none() && self.email.is_none() && self.comments.is_none()
    }
}

/// Body for the form-encoded `PUT /users/{name}` set-password call (task 5.8).
/// Emits `password=<plaintext>&hashPassword=true` so the server hashes it and
/// sets `passwordSalt=true` internally (verified live). The client never
/// sends `passwordSalt` or a precomputed hash. `hash_password` is always
/// `true` — the field exists so the wire shape is explicit, not configurable.
#[derive(Clone, Debug, Serialize)]
pub struct SetPasswordForm {
    pub password: String,
    #[serde(rename = "hashPassword")]
    pub hash_password: bool,
}

impl SetPasswordForm {
    /// Build a set-password body for `plaintext`, always requesting
    /// server-side hashing.
    pub fn new(plaintext: &str) -> Self {
        Self {
            password: plaintext.to_owned(),
            hash_password: true,
        }
    }
}

// ---------------------------------------------------------------------------
// XML create body — POST /users
// ---------------------------------------------------------------------------

/// Serializable view of a user for the XML `POST /users` body. quick-xml
/// serializes `Vec` fields as repeated elements (`<role>…</role>` once per
/// entry), matching JAXB's `OnmsUser` shape. `None` scalars are skipped.
///
/// Element names are the JAXB element names (`user-id`, `full-name`,
/// `user-comments`, `duty-schedule`), which differ from both the JSON
/// response keys (same here, coincidentally) and the form-PUT bean keys.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename = "user")]
struct UserCreateXml {
    #[serde(rename = "user-id")]
    user_id: String,
    #[serde(rename = "full-name", skip_serializing_if = "Option::is_none")]
    full_name: Option<String>,
    #[serde(rename = "user-comments", skip_serializing_if = "Option::is_none")]
    user_comments: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    /// Plaintext on the wire; the server hashes it when the URL carries
    /// `?hashPassword=true`. Skipped when no password is being set.
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    /// Repeated `<duty-schedule>` elements.
    #[serde(rename = "duty-schedule", skip_serializing_if = "Vec::is_empty")]
    duty_schedule: Vec<String>,
    /// Repeated `<role>` elements.
    #[serde(rename = "role", skip_serializing_if = "Vec::is_empty")]
    role: Vec<String>,
}

/// Reject text that cannot be represented in XML 1.0. quick-xml escapes the
/// markup-significant characters (`< > & " '`) but passes C0 control bytes
/// through verbatim, so a value carrying e.g. a NUL would serialize to a
/// document the server rejects with an opaque 4xx/5xx. The only C0 controls
/// legal in XML 1.0 text are tab/LF/CR; everything else below `U+0020` is
/// refused here with a clear client-side error. (Passwords already have
/// internal newlines rejected upstream in Group 5; this is the belt for the
/// remaining control bytes and for the other fields.)
fn reject_xml_illegal_controls(field: &str, value: &str) -> Result<(), quick_xml::SeError> {
    use serde::ser::Error as _;
    if let Some(c) = value
        .chars()
        .find(|&c| (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r')
    {
        return Err(quick_xml::SeError::custom(format!(
            "{field} contains control character U+{:04X}, which is not representable in XML 1.0; \
             refusing to build a malformed POST /users body",
            c as u32
        )));
    }
    Ok(())
}

/// Build the XML `POST /users` body from typed parts. `password` is the
/// already-resolved plaintext (from a `passwordRef`, Group 5); pass `None`
/// to create a user without a password. The caller appends
/// `?hashPassword=true` to the URL when a password is present.
///
/// Every text field is checked for XML-1.0-illegal control characters
/// before serialization (see [`reject_xml_illegal_controls`]) so a control
/// byte in a password or comment fails fast client-side instead of emitting
/// a malformed body the server rejects opaquely.
pub fn user_create_xml(
    user_id: &str,
    full_name: Option<&str>,
    comments: Option<&str>,
    email: Option<&str>,
    password: Option<&str>,
    duty_schedule: &[String],
    roles: &[String],
) -> Result<String, quick_xml::SeError> {
    reject_xml_illegal_controls("user-id", user_id)?;
    for (field, value) in [
        ("full-name", full_name),
        ("user-comments", comments),
        ("email", email),
        ("password", password),
    ] {
        if let Some(v) = value {
            reject_xml_illegal_controls(field, v)?;
        }
    }
    for d in duty_schedule {
        reject_xml_illegal_controls("duty-schedule", d)?;
    }
    for r in roles {
        reject_xml_illegal_controls("role", r)?;
    }
    let doc = UserCreateXml {
        user_id: user_id.to_owned(),
        full_name: full_name.map(str::to_owned),
        user_comments: comments.map(str::to_owned),
        email: email.map(str::to_owned),
        password: password.map(str::to_owned),
        duty_schedule: duty_schedule.to_vec(),
        role: roles.to_vec(),
    };
    quick_xml::se::to_string(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured verbatim from the dev lab (2026-05-29), GET /users/{n}
    // with Accept: application/json.
    const USER_JSON: &str = r#"{
        "user-id":"onmsctl-it-spike0",
        "full-name":"Spike Zero",
        "user-comments":"spike",
        "email":"spike0@example.com",
        "password":"y9iw2GacA95B7KBauvywbRJkvIneFuU0HmNmpNblDC50Sl/UPQKJ2OaYDoUnwAmC",
        "passwordSalt":true,
        "duty-schedule":["MoTuWeThFr800-1700"],
        "role":["ROLE_USER"]
    }"#;

    // Captured from GET /users?limit=10000&offset=0.
    const USER_LIST_JSON: &str = r#"{
        "offset":0,"count":2,"totalCount":2,
        "user":[
            {"user-id":"admin","full-name":"Administrator","user-comments":"Default administrator, do not delete","email":"","password":"gU2w","passwordSalt":true,"duty-schedule":[],"role":["ROLE_ADMIN","ROLE_FILESYSTEM_EDITOR"]},
            {"user-id":"rtc","full-name":"RTC","user-comments":"RTC user, do not delete","email":"","password":"sHMy","passwordSalt":true,"duty-schedule":[],"role":["ROLE_RTC"]}
        ]
    }"#;

    #[test]
    fn user_wire_deserializes_lab_shape() {
        let u: OnmsUserWire = serde_json::from_str(USER_JSON).unwrap();
        assert_eq!(u.user_id, "onmsctl-it-spike0");
        assert_eq!(u.full_name.as_deref(), Some("Spike Zero"));
        assert_eq!(u.user_comments.as_deref(), Some("spike"));
        assert_eq!(u.email.as_deref(), Some("spike0@example.com"));
        assert_eq!(u.duty_schedule, vec!["MoTuWeThFr800-1700"]);
        assert_eq!(u.roles, vec!["ROLE_USER"]);
        assert_eq!(u.password_salt, Some(true));
        assert!(u.password.is_some());
        // Every key was modeled — nothing leaked into extras.
        assert!(u.extras.is_empty(), "unexpected extras: {:?}", u.extras);
    }

    #[test]
    fn user_list_wire_deserializes_lab_shape() {
        let list: OnmsUserListWire = serde_json::from_str(USER_LIST_JSON).unwrap();
        assert_eq!(list.offset, 0);
        assert_eq!(list.count, 2);
        assert_eq!(list.total_count, 2);
        assert_eq!(list.users.len(), 2);
        assert_eq!(list.users[0].user_id, "admin");
        assert_eq!(
            list.users[0].roles,
            vec!["ROLE_ADMIN", "ROLE_FILESYSTEM_EDITOR"]
        );
        assert_eq!(list.users[1].user_id, "rtc");
    }

    #[test]
    fn unknown_server_fields_land_in_extras() {
        // Forward-compat: a future Horizon field must not break parse, and
        // must be preserved (not silently dropped) so the unmodeled
        // annotation can carry it back.
        let json = r#"{"user-id":"x","totally-new-field":"keep-me","another":42}"#;
        let u: OnmsUserWire = serde_json::from_str(json).unwrap();
        assert_eq!(u.user_id, "x");
        assert_eq!(
            u.extras.get("totally-new-field").and_then(|v| v.as_str()),
            Some("keep-me")
        );
        assert_eq!(u.extras.get("another").and_then(|v| v.as_i64()), Some(42));
    }

    #[test]
    fn update_form_omits_none_fields() {
        let form = UpdateForm {
            full_name: Some("Alice Renamed".into()),
            email: None,
            comments: None,
        };
        let encoded = serde_urlencoded::to_string(&form).unwrap();
        assert_eq!(encoded, "fullName=Alice+Renamed");
        assert!(!form.is_empty());
    }

    #[test]
    fn update_form_encodes_all_fields_with_bean_keys() {
        let form = UpdateForm {
            full_name: Some("Alice".into()),
            email: Some("alice@example.com".into()),
            comments: Some("hi".into()),
        };
        let encoded = serde_urlencoded::to_string(&form).unwrap();
        // Bean keys, not hyphenated; '@' percent-encoded.
        assert_eq!(
            encoded,
            "fullName=Alice&email=alice%40example.com&comments=hi"
        );
    }

    #[test]
    fn empty_update_form_is_empty() {
        assert!(UpdateForm::default().is_empty());
    }

    #[test]
    fn create_xml_matches_accepted_post_shape() {
        // This is the exact body shape the lab accepted with 201 on
        // POST /users?hashPassword=true (spike 0.1b).
        let xml = user_create_xml(
            "onmsctl-it-spike0",
            Some("Spike Zero"),
            Some("spike"),
            Some("spike0@example.com"),
            Some("s3cr3t-pw"),
            &["MoTuWeThFr800-1700".to_string()],
            &["ROLE_USER".to_string()],
        )
        .unwrap();
        assert!(xml.starts_with("<user>"), "root element: {xml}");
        assert!(xml.contains("<user-id>onmsctl-it-spike0</user-id>"));
        assert!(xml.contains("<full-name>Spike Zero</full-name>"));
        assert!(xml.contains("<user-comments>spike</user-comments>"));
        assert!(xml.contains("<email>spike0@example.com</email>"));
        assert!(xml.contains("<password>s3cr3t-pw</password>"));
        assert!(xml.contains("<duty-schedule>MoTuWeThFr800-1700</duty-schedule>"));
        assert!(xml.contains("<role>ROLE_USER</role>"));
        assert!(xml.ends_with("</user>"));
    }

    #[test]
    fn create_xml_skips_absent_scalars_and_empty_lists() {
        let xml = user_create_xml("bob", None, None, None, None, &[], &[]).unwrap();
        assert_eq!(xml, "<user><user-id>bob</user-id></user>");
    }

    #[test]
    fn create_xml_rejects_control_characters() {
        // A NUL (or other C0 control) in any text field must fail fast
        // client-side rather than emit a body the server rejects opaquely.
        let err =
            user_create_xml("bob", None, None, None, Some("pass\u{0}word"), &[], &[]).unwrap_err();
        assert!(err.to_string().contains("password"), "{err}");
        assert!(err.to_string().contains("U+0000"), "{err}");

        // tab / LF / CR are legal XML 1.0 text and must still pass.
        assert!(
            user_create_xml("bob", Some("line1\nline2\tend"), None, None, None, &[], &[]).is_ok()
        );
    }

    #[test]
    fn create_xml_emits_repeated_role_elements() {
        let xml = user_create_xml(
            "multi",
            None,
            None,
            None,
            None,
            &[],
            &["ROLE_USER".to_string(), "ROLE_REST".to_string()],
        )
        .unwrap();
        assert!(
            xml.contains("<role>ROLE_USER</role><role>ROLE_REST</role>"),
            "{xml}"
        );
    }
}
