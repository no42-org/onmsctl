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

/// Reduce a Horizon error-body envelope to the message inside, when the
/// envelope is one of the common shapes:
///
///   * `<error>message</error>` / `<message>message</message>` — XML
///   * `{"error":"message"}` / `{"message":"message"}` — JSON
///
/// Falls back to the raw input when no envelope matches. Used purely for
/// the user-facing [`Error::HttpStatus`] message; the stored `body` field
/// is unchanged so log analyzers still see the wire form.
pub fn prettify_body(body: &str) -> String {
    let trimmed = body.trim();

    // XML-ish single-tag envelope.
    for tag in ["error", "message"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if let Some(rest) = trimmed.strip_prefix(&open)
            && let Some(inner) = rest.strip_suffix(&close)
        {
            return inner.trim().to_string();
        }
    }

    // JSON object with a single string-valued `error` or `message` key.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(obj) = v.as_object()
        && obj.len() == 1
    {
        for key in ["error", "message"] {
            if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
                return s.to_string();
            }
        }
    }

    trimmed.to_string()
}

/// Map common HTTP status codes to their canonical reason phrases. Returns
/// an empty string for codes we don't recognize, so the formatter doesn't
/// emit a misleading phrase.
pub fn status_reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
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
    ///
    /// The Display form prettifies common envelope shapes (XML `<error>…`,
    /// JSON `{"error":"…"}`) so the user sees the message, not the wrapper.
    /// The stored `body` is the raw wire form — analytics / log scrapers
    /// see the original.
    #[error("HTTP {status} {} ({method} {path}): {}", status_reason(*status), prettify_body(body))]
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

    /// A classified `WriteCmd` invocation was refused locally because the
    /// active context is `read-only`. Distinct from [`Error::Config`] so
    /// ops scripts can branch on it via its dedicated exit code (12).
    /// No HTTP request was issued.
    #[error(
        "active context '{context}' is read-only; this is a Write command and \
         was refused locally without issuing any HTTP request. Remove \
         `read-only: true` from the context or switch to a writable context \
         to proceed."
    )]
    ReadOnlyRefused { context: String },

    /// `--wait` was requested but the async server-side operation did not
    /// reach a terminal state within the configured `--timeout`. The
    /// `handle` (e.g. scan-report id) is included so the operator can
    /// resume waiting via the relevant `status` subcommand. Dedicated
    /// exit code 10 per the cli-core spec.
    #[error(
        "timed out waiting for async operation to complete after {timeout}; \
         the server-side operation may still be running. Handle: {handle}. \
         Resume via the relevant `status` subcommand."
    )]
    WaitTimeout { handle: String, timeout: String },

    /// `--wait` observed the async server-side operation transition to a
    /// terminal failure state. The `reason` is the server's reported
    /// failure message. Dedicated exit code 11 per the cli-core spec.
    #[error("async operation failed: {reason} (handle: {handle})")]
    AsyncOpFailed { handle: String, reason: String },

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

    /// A IAM operation targeted a user that does not exist on the server.
    /// Raised by `iam user set-password` (and similar update flows) after a
    /// pre-flight `GET /users/{name}` returns 404, so the operator gets a
    /// clear "no such user" rather than an ambiguous 404 from a form-encoded
    /// PUT. Shares exit code 1 with [`Error::HttpStatus`] — the request was
    /// understood; the target is absent.
    #[error("user '{name}' does not exist on the server")]
    UserNotFound { name: String },

    /// **IAM-001** — an `iam apply` would leave a protected role (default
    /// `ROLE_ADMIN`) with zero holders on the server. Refused before any
    /// write. Overridable with `--allow-admin-lockout --yes`. Dedicated exit
    /// code 13 so ops automation can branch on "admin lockout averted".
    #[error(
        "IAM-001: this apply would remove the last holder of protected role(s) [{roles}]; \
         refusing to avoid locking everyone out. Re-run with `--allow-admin-lockout --yes` \
         if this is intentional."
    )]
    IamLockout { roles: String },

    /// **IAM-002** — an `iam apply` would strip the **calling** user's own
    /// protected role (or delete their account). Refused with **no override**
    /// — switch to another context (`--context`) to proceed. Exit code 14.
    #[error(
        "IAM-002: this apply would strip your own protected role or delete your account \
         ('{user}'); refusing (no override). Switch contexts with `--context` to proceed."
    )]
    IamSelfLockout { user: String },

    /// The self-lockout invariant (IAM-002) could not be evaluated because
    /// `GET /users/whoami` did not yield a usable caller identity (non-2xx or
    /// empty body — e.g. anonymous-token auth). The apply refuses rather than
    /// skip the check silently. Operators in this situation are limited to
    /// `--dry-run` / read-only workflows. Exit code 15.
    #[error(
        "could not determine the calling user via GET /users/whoami (non-2xx or empty body); \
         refusing the apply because the self-lockout invariant (IAM-002) cannot be evaluated. \
         Use a basic-auth context, or restrict to --dry-run / read-only workflows."
    )]
    IamWhoamiUnavailable,

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
            Error::UserNotFound { .. } => 1,
            Error::Dns(_) => 4,
            Error::ConnRefused(_) => 5,
            Error::Timeout(_) => 6,
            Error::TlsHandshake(_) => 7,
            Error::Redirect(_) => 8,
            Error::UnsupportedAuthScheme(_) => 9,
            // Async waiting outcomes (cli-core spec): timeout and
            // server-reported failure get distinct codes so ops scripts
            // can branch on "still running, gave up" vs "actually failed".
            Error::WaitTimeout { .. } => 10,
            Error::AsyncOpFailed { .. } => 11,
            // Read-only refusal: distinct so ops scripts can branch on
            // "policy refused" vs config misuse.
            Error::ReadOnlyRefused { .. } => 12,
            // IAM lockout invariants: distinct codes so automation can tell
            // "admin lockout averted" / "self lockout averted" / "couldn't
            // identify caller" apart.
            Error::IamLockout { .. } => 13,
            Error::IamSelfLockout { .. } => 14,
            Error::IamWhoamiUnavailable => 15,
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
        // Async wait outcomes — cli-core spec scenarios for exit 10/11.
        assert_eq!(
            Error::WaitTimeout {
                handle: "acme-prod".into(),
                timeout: "30s".into(),
            }
            .exit_code(),
            10
        );
        assert_eq!(
            Error::AsyncOpFailed {
                handle: "acme-prod".into(),
                reason: "vanished".into(),
            }
            .exit_code(),
            11
        );
        assert_eq!(Error::PartialSuccess { failed: 3 }.exit_code(), 1);
        assert_eq!(Error::Config("x".into()).exit_code(), 2);
        assert_eq!(Error::NoContext.exit_code(), 2);
        // IAM lockout invariants — stable codes 13/14/15.
        assert_eq!(Error::UserNotFound { name: "x".into() }.exit_code(), 1);
        assert_eq!(
            Error::IamLockout {
                roles: "ROLE_ADMIN".into()
            }
            .exit_code(),
            13
        );
        assert_eq!(
            Error::IamSelfLockout {
                user: "admin".into()
            }
            .exit_code(),
            14
        );
        assert_eq!(Error::IamWhoamiUnavailable.exit_code(), 15);
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
        assert!(msg.contains("Not Found"));
    }

    #[test]
    fn http_status_strips_xml_error_envelope() {
        let e = Error::HttpStatus {
            method: "GET".into(),
            path: "/x".into(),
            status: 404,
            body: "<error>No events found for source ID: 24</error>".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("No events found for source ID: 24"));
        assert!(!msg.contains("<error>"));
        assert!(!msg.contains("</error>"));
    }

    #[test]
    fn http_status_strips_json_error_envelope() {
        let e = Error::HttpStatus {
            method: "GET".into(),
            path: "/x".into(),
            status: 400,
            body: r#"{"error":"Invalid offset/limit values"}"#.into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("Invalid offset/limit values"));
        assert!(!msg.contains("\"error\""));
        assert!(msg.contains("Bad Request"));
    }

    #[test]
    fn prettify_body_passes_through_unrecognized_shape() {
        assert_eq!(prettify_body("plain text"), "plain text");
        // Multi-key JSON: not a single-error envelope, leave intact.
        assert_eq!(
            prettify_body(r#"{"error":"a","detail":"b"}"#),
            r#"{"error":"a","detail":"b"}"#
        );
    }

    #[test]
    fn status_reason_returns_empty_for_unknown_codes() {
        assert_eq!(status_reason(404), "Not Found");
        assert_eq!(status_reason(418), "");
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
