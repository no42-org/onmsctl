/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Error types for `onmsctl-core`.
//!
//! Exit codes are stable and observable per `cli-core` spec
//! (transport-layer-failures requirement).

use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    // -- Config / context resolution --
    #[error("config error: {0}")]
    Config(String),

    #[error(
        "no context resolved (no --context flag, no ONMSCTL_CONTEXT, no current-context in config)"
    )]
    NoContext,

    #[error("context '{0}' not found in config")]
    UnknownContext(String),

    #[error("auth error: {0}")]
    Auth(String),

    // -- HTTP layer --
    #[error("{method} {path} returned {status}: {body}")]
    HttpStatus {
        method: String,
        path: String,
        status: u16,
        body: String,
    },

    // -- Transport layer (distinct exit codes per cli-core spec §4.5) --
    #[error("could not resolve host: {0}")]
    Dns(String),

    #[error("connection refused: {0}")]
    ConnRefused(String),

    #[error("timed out connecting to / reading from {0}")]
    Timeout(String),

    #[error("TLS handshake failed: {0}")]
    TlsHandshake(String),

    #[error("too many redirects from {0}")]
    Redirect(String),

    #[error(
        "server requires authentication scheme '{0}'; v0.1 supports HTTP Basic and Bearer only"
    )]
    UnsupportedAuthScheme(String),

    // -- Wrapped foreign errors --
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("yaml parse error: {0}")]
    Yaml(#[from] serde_norway::Error),

    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("http transport error: {0}")]
    Transport(#[source] reqwest::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// Stable exit code per `cli-core` spec.
    ///
    /// Ops automation may rely on these codes; do not change without a spec amendment.
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::HttpStatus { .. } => 1,
            Error::Dns(_) => 4,
            Error::ConnRefused(_) => 5,
            Error::Timeout(_) => 6,
            Error::TlsHandshake(_) => 7,
            Error::Redirect(_) => 8,
            Error::UnsupportedAuthScheme(_) => 9,
            // Generic/internal errors collapse to 2 (analogous to misuse).
            _ => 2,
        }
    }
}

/// Map a `reqwest::Error` to a transport-class variant, falling back to
/// `Error::Transport` when reqwest's classification is ambiguous.
impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        let msg = e
            .url()
            .map(|u| u.to_string())
            .unwrap_or_else(|| e.to_string());
        if e.is_connect() {
            // `is_connect` covers DNS + TCP refusal in reqwest's API.
            // Distinguish via the error chain when possible.
            let chain = format!("{e}").to_lowercase();
            if chain.contains("dns") || chain.contains("name resolution") {
                return Error::Dns(msg);
            }
            if chain.contains("refused") {
                return Error::ConnRefused(msg);
            }
            return Error::ConnRefused(msg);
        }
        if e.is_timeout() {
            return Error::Timeout(msg);
        }
        if e.is_redirect() {
            return Error::Redirect(msg);
        }
        // TLS errors surface as `is_request` with chain mentioning rustls.
        let chain = format!("{e}").to_lowercase();
        if chain.contains("tls") || chain.contains("certificate") {
            return Error::TlsHandshake(msg);
        }
        Error::Transport(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(
            Error::HttpStatus {
                method: "GET".into(),
                path: "/".into(),
                status: 404,
                body: String::new(),
            }
            .exit_code(),
            1
        );
        assert_eq!(Error::Dns("h".into()).exit_code(), 4);
        assert_eq!(Error::ConnRefused("h".into()).exit_code(), 5);
        assert_eq!(Error::Timeout("h".into()).exit_code(), 6);
        assert_eq!(Error::TlsHandshake("x".into()).exit_code(), 7);
        assert_eq!(Error::Redirect("h".into()).exit_code(), 8);
        assert_eq!(
            Error::UnsupportedAuthScheme("Negotiate".into()).exit_code(),
            9
        );
        assert_eq!(Error::Config("x".into()).exit_code(), 2);
        assert_eq!(Error::NoContext.exit_code(), 2);
    }

    #[test]
    fn http_status_message_contains_method_path_and_status() {
        let e = Error::HttpStatus {
            method: "GET".into(),
            path: "/eventconf/sources/9999".into(),
            status: 404,
            body: "not found".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("404"));
        assert!(msg.contains("GET"));
        assert!(msg.contains("/eventconf/sources/9999"));
    }

    #[test]
    fn unknown_context_names_the_context() {
        let e = Error::UnknownContext("staging".into());
        assert!(e.to_string().contains("staging"));
    }
}
