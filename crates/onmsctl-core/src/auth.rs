/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resolved authentication credentials.
//!
//! v0.1 supports HTTP Basic and Bearer only per `cli-core` spec
//! "Basic and Bearer authentication" requirement. OAuth2/OIDC and mTLS are
//! deferred (see proposal.md non-goals).

use serde::{Deserialize, Serialize};

/// The credentials that will be sent on each outbound HTTP request.
///
/// Constructed from a [`crate::config::AuthSpec`] via the resolution flow in
/// [`crate::context`]. The variants here represent what hits the wire — not
/// the "where to find the secret" hints stored in the config file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// Custom Debug formatting redacts the secret material so accidental
/// `dbg!`/`println!` invocations do not leak credentials into logs.
impl std::fmt::Display for AuthCreds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Basic { username, .. } => write!(f, "Basic({username}, <redacted>)"),
            Self::Bearer { .. } => write!(f, "Bearer(<redacted>)"),
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
        assert!(s.contains("<redacted>"));
    }

    #[test]
    fn display_redacts_token() {
        let c = AuthCreds::bearer("eyJraWQ.token");
        let s = format!("{c}");
        assert!(!s.contains("eyJraWQ"));
        assert!(s.contains("<redacted>"));
    }

    #[test]
    fn equality_by_value() {
        assert_eq!(AuthCreds::basic("u", "p"), AuthCreds::basic("u", "p"));
        assert_ne!(AuthCreds::basic("u", "p"), AuthCreds::basic("u", "q"));
    }
}
