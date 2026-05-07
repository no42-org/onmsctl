/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resolved authentication credentials.
//!
//! v0.1 supports HTTP Basic and Bearer only per `cli-core` spec
//! "Basic and Bearer authentication" requirement. OAuth2/OIDC and mTLS are
//! deferred (see proposal.md non-goals).
//!
//! Defense-in-depth: this type implements custom `Debug`, `Display`, and
//! `Serialize` impls that all redact the secret material. The `Deserialize`
//! derive is intentionally absent so credentials cannot be reconstructed
//! from arbitrary input — they are built imperatively from
//! [`crate::config::AuthSpec`] in [`crate::context`].

use serde::Serialize;
use serde::ser::{SerializeStructVariant, Serializer};

/// The credentials that will be sent on each outbound HTTP request.
///
/// Constructed from a [`crate::config::AuthSpec`] via the resolution flow
/// in [`crate::context`]. The variants represent what hits the wire.
///
/// `Debug`, `Display`, and `Serialize` all redact the secret. Tests verify
/// that none of the standard formatting paths leak credentials.
#[derive(Clone, PartialEq, Eq)]
pub enum AuthCreds {
    Basic { username: String, password: String },
    Bearer { token: String },
}

impl AuthCreds {
    pub fn basic(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self::Basic {
            username: username.into(),
            password: password.into(),
        }
    }

    pub fn bearer(token: impl Into<String>) -> Self {
        Self::Bearer {
            token: token.into(),
        }
    }

    /// Tag for diagnostics — never includes the secret material.
    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Basic { .. } => "Basic",
            Self::Bearer { .. } => "Bearer",
        }
    }
}

const REDACTED: &str = "<redacted>";

/// Custom Display: usernames are visible (they are not secrets), passwords
/// and tokens are redacted.
impl std::fmt::Display for AuthCreds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Basic { username, .. } => write!(f, "Basic({username}, {REDACTED})"),
            Self::Bearer { .. } => write!(f, "Bearer({REDACTED})"),
        }
    }
}

/// Custom Debug that does NOT print the secret. The default-derived `Debug`
/// would print struct fields verbatim, leaking passwords and tokens via
/// `dbg!(creds)` or `tracing::error!(?creds)`. This impl mirrors the Display
/// redaction so accidental Debug-formatting is safe.
impl std::fmt::Debug for AuthCreds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Basic { username, .. } => f
                .debug_struct("Basic")
                .field("username", username)
                .field("password", &REDACTED)
                .finish(),
            Self::Bearer { .. } => f.debug_struct("Bearer").field("token", &REDACTED).finish(),
        }
    }
}

/// Custom Serialize that emits the redacted form. A `serde_json::to_string`
/// call on a `Context` (which contains `AuthCreds`) will produce
/// `"password": "<redacted>"` instead of the cleartext. The
/// `Deserialize` impl is deliberately not provided so credentials cannot be
/// reconstructed from arbitrary serialized input.
impl Serialize for AuthCreds {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Basic { username, .. } => {
                let mut s = serializer.serialize_struct_variant("AuthCreds", 0, "Basic", 2)?;
                s.serialize_field("username", username)?;
                s.serialize_field("password", REDACTED)?;
                s.end()
            }
            Self::Bearer { .. } => {
                let mut s = serializer.serialize_struct_variant("AuthCreds", 1, "Bearer", 1)?;
                s.serialize_field("token", REDACTED)?;
                s.end()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_tag_is_human_readable() {
        assert_eq!(AuthCreds::basic("u", "p").scheme(), "Basic");
        assert_eq!(AuthCreds::bearer("t").scheme(), "Bearer");
    }

    #[test]
    fn display_redacts_password() {
        let c = AuthCreds::basic("admin", "supersecret");
        let s = format!("{c}");
        assert!(s.contains("admin"));
        assert!(!s.contains("supersecret"));
        assert!(s.contains(REDACTED));
    }

    #[test]
    fn display_redacts_token() {
        let c = AuthCreds::bearer("eyJraWQ.token");
        let s = format!("{c}");
        assert!(!s.contains("eyJraWQ"));
        assert!(s.contains(REDACTED));
    }

    #[test]
    fn debug_redacts_password() {
        let c = AuthCreds::basic("admin", "supersecret");
        let s = format!("{c:?}");
        assert!(s.contains("admin"));
        assert!(!s.contains("supersecret"), "Debug leaked password: {s}");
        assert!(s.contains(REDACTED));
    }

    #[test]
    fn debug_redacts_token() {
        let c = AuthCreds::bearer("eyJraWQ.secret-token-content");
        let s = format!("{c:?}");
        assert!(
            !s.contains("secret-token-content"),
            "Debug leaked token: {s}"
        );
        assert!(s.contains(REDACTED));
    }

    #[test]
    fn serialize_to_json_redacts_password() {
        let c = AuthCreds::basic("admin", "supersecret");
        let json = serde_json::to_string(&c).unwrap();
        assert!(
            !json.contains("supersecret"),
            "JSON leaked password: {json}"
        );
        assert!(json.contains(REDACTED));
        assert!(json.contains("admin"));
    }

    #[test]
    fn serialize_to_json_redacts_token() {
        let c = AuthCreds::bearer("real-token");
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("real-token"), "JSON leaked token: {json}");
        assert!(json.contains(REDACTED));
    }

    #[test]
    fn equality_by_value() {
        assert_eq!(AuthCreds::basic("u", "p"), AuthCreds::basic("u", "p"));
        assert_ne!(AuthCreds::basic("u", "p"), AuthCreds::basic("u", "q"));
    }
}
