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

/// Hard cap on the body excerpt included in [`Error::HttpStatus`]. The wire
/// body may be much larger; we truncate to this length and append a marker
/// so error chains do not balloon stderr or log output.
pub const HTTP_BODY_EXCERPT_BYTES: usize = 4096;

/// Why a post-upload lookup failed. Lets ops scripts distinguish "the
/// source was deleted by another actor" from "two sources now share the
/// name" from a transient transport failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostUploadLookupKind {
    /// The source named in the upload no longer exists on the server.
    Absent,
    /// More than one source on the server matches the uploaded name.
    Ambiguous,
    /// The lookup itself failed (transport / HTTP error).
    Transport,
}

/// Truncate `body` to [`HTTP_BODY_EXCERPT_BYTES`] characters (not bytes —
/// honors UTF-8 char boundaries to avoid panics) and append a marker noting
/// the original length.
pub fn excerpt_body(body: &str) -> String {
    let total = body.len();
    if total <= HTTP_BODY_EXCERPT_BYTES {
        return body.to_string();
    }
    // Trim at a UTF-8 char boundary at or below the cap.
    let mut end = HTTP_BODY_EXCERPT_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [… truncated, {} bytes total]", &body[..end], total)
}

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
    /// HTTP non-success response. The `body` field is capped at
    /// [`HTTP_BODY_EXCERPT_BYTES`] to avoid dumping multi-MB error pages
    /// (e.g. Tomcat HTML stack traces) into stderr / log output.
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

    /// A multi-item batch operation completed with at least one failed
    /// item. Distinct from `Error::Config` (misuse) and `Error::HttpStatus`
    /// (single HTTP failure) so ops scripts can branch on it via exit
    /// code 1 — same as HTTP-status failures, but semantically "the
    /// request was understood; some items did not succeed".
    #[error("partial success: {failed} item(s) failed")]
    PartialSuccess { failed: usize },

    /// Apply succeeded with the upload, but the post-upload re-lookup
    /// needed to perform follow-up steps (e.g. PATCH the enabled flag)
    /// did not return exactly one match. The upload IS persisted
    /// server-side; the follow-up state-sync did not happen. See
    /// design.md §7 "find_source_by_name race" for the race conditions
    /// that produce this.
    #[error(
        "apply: {name} uploaded, but post-upload lookup {kind:?} blocked the follow-up state sync"
    )]
    PostUploadLookupFailed {
        name: String,
        kind: PostUploadLookupKind,
    },

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
            Error::PartialSuccess { .. } => 1,
            Error::PostUploadLookupFailed { .. } => 1,
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
        assert_eq!(Error::PartialSuccess { failed: 3 }.exit_code(), 1);
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

    #[test]
    fn excerpt_body_passes_short_input_through() {
        let s = "short body";
        assert_eq!(excerpt_body(s), s);
    }

    #[test]
    fn excerpt_body_truncates_long_input_and_marks_total_length() {
        let big = "x".repeat(HTTP_BODY_EXCERPT_BYTES * 2);
        let out = excerpt_body(&big);
        assert!(out.len() < big.len());
        assert!(out.contains("truncated"));
        assert!(out.contains(&format!("{} bytes total", big.len())));
    }

    #[test]
    fn excerpt_body_respects_utf8_boundary() {
        // A multi-byte char that straddles the cap must not split a code point.
        let prefix = "x".repeat(HTTP_BODY_EXCERPT_BYTES - 1);
        let big = format!("{prefix}€€€"); // each € is 3 bytes
        let _ = excerpt_body(&big); // just must not panic
    }
}
