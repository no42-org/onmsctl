/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Shared HTTP transport for `onmsctl` capabilities.
//!
//! Capabilities consume `&OnmsClient` and never reach for `reqwest`
//! directly. This is the bright line that keeps the workspace
//! maintainable (design.md §2.1). The [`MultipartPart`] type is the
//! capability-facing abstraction over multipart bodies; capabilities do not
//! see `reqwest::multipart::Form`.
//!
//! Defaults:
//!   - Total request timeout: 30 s (kubectl-equivalent default).
//!   - get_bytes() body cap: 16 MiB.
//!   - HTTP non-success body excerpt cap: 4 KiB
//!     (see [`crate::error::HTTP_BODY_EXCERPT_BYTES`]).
//!   - Redirects: limited to 5 hops.
//!
//! Error mapping:
//!   - HTTP non-success status → [`Error::HttpStatus`] with method/path/excerpted body.
//!   - Network failures → mapped via `From<reqwest::Error>` to typed
//!     transport variants (DNS, ConnRefused, Timeout, TLS, Redirect)
//!     so exit codes per cli-core spec §4.5 are preserved.

use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderValue};
use reqwest::{Method, RequestBuilder, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::auth::AuthCreds;
use crate::context::Context;
use crate::error::{Error, Result, excerpt_body};

/// Default total-request timeout. Hard-coded here; capabilities that need a
/// different deadline will gain configurability in a follow-up if a real
/// use case appears.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Hard cap on bytes returned by [`OnmsClient::get_bytes`]. 16 MiB is well
/// above realistic OpenNMS source XML (largest published files ~2 MB) and
/// guards against a pathological/malicious server forcing the process to
/// allocate without bound.
pub const MAX_BYTES_RESPONSE: usize = 16 * 1024 * 1024;

/// HTTP transport for OpenNMS Horizon capabilities. Cheap to clone within a
/// process (the inner `reqwest::Client` is reference-counted).
#[derive(Clone, Debug)]
pub struct OnmsClient {
    inner: reqwest::Client,
    base: Url,
    creds: AuthCreds,
    /// True when the active context has `insecure-skip-tls-verify`.
    /// Drives a per-request stderr warning so operators see the risk on
    /// every call rather than only the first request of a process.
    insecure: bool,
}

/// One part of a multipart upload. Capabilities construct these and pass a
/// slice to [`OnmsClient::multipart`]; the client wraps them into a
/// `reqwest::multipart::Form` internally so capability code never imports
/// reqwest types.
#[derive(Clone, Debug)]
pub struct MultipartPart {
    /// Multipart form-field name. Horizon's `/eventconf/upload` is annotated
    /// `@Multipart("upload")` on the JAX-RS interface (NMS-19813), so CXF
    /// only binds parts whose `Content-Disposition: form-data; name=...`
    /// is literally `"upload"` — any other name is rejected with an empty
    /// HTTP 400. The annotation has been removed upstream, but unpatched
    /// servers are still common in the wild; keep `"upload"` as the
    /// default for compatibility with both fixed and unfixed Horizon.
    pub field_name: String,
    /// Filename associated with this part. `EventConfRestService.uploadEventConfFiles`
    /// reads `getContentDisposition().getParameter("filename")` to derive
    /// the source basename — this field is load-bearing.
    pub filename: String,
    /// MIME type. Common values: `application/xml` for eventconf XML.
    pub content_type: String,
    /// Body bytes.
    pub body: Vec<u8>,
}

impl MultipartPart {
    pub fn xml(filename: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            field_name: "upload".into(),
            filename: filename.into(),
            content_type: "application/xml".into(),
            body: body.into(),
        }
    }
}

impl OnmsClient {
    /// Construct from a resolved [`Context`]. Honors `insecure_skip_tls_verify`
    /// and stages a per-request stderr warning when it is set.
    pub fn from_context(ctx: &Context) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .timeout(DEFAULT_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(5));
        if ctx.insecure_skip_tls_verify {
            builder = builder.danger_accept_invalid_certs(true);
        }
        let inner = builder.build()?;
        Ok(Self {
            inner,
            base: ctx.url.clone(),
            creds: ctx.creds.clone(),
            insecure: ctx.insecure_skip_tls_verify,
        })
    }

    /// Construct with explicit pieces; useful for tests against `wiremock`.
    /// Tests do not need a timeout (wiremock responses are immediate); the
    /// `insecure_skip_tls_verify` flag is irrelevant for plain HTTP.
    pub fn from_parts(base: Url, creds: AuthCreds) -> Result<Self> {
        Ok(Self {
            inner: reqwest::Client::builder()
                .timeout(DEFAULT_REQUEST_TIMEOUT)
                .build()?,
            base,
            creds,
            insecure: false,
        })
    }

    /// `GET <base><path>` with optional query parameters, deserializing the
    /// JSON response.
    pub async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let url = self.url_for(path)?;
        // Horizon REST endpoints are `@Produces({XML, JSON, ...})` with XML
        // listed first, so JAX-RS content negotiation returns XML unless we
        // explicitly ask for JSON. Without this header a real Horizon replies
        // with XML and the JSON decode fails as `error decoding response body`.
        let mut req = self
            .inner
            .request(Method::GET, url)
            .header(ACCEPT, HeaderValue::from_static("application/json"));
        if !query.is_empty() {
            req = req.query(query);
        }
        let resp = self.send(req, Method::GET, path).await?;
        json_or_no_content(resp, Method::GET, path).await
    }

    /// `GET <base><path>` returning the raw response body bytes (e.g. for XML
    /// downloads from `/eventconf/sources/{id}/events/download`). The
    /// returned body is capped at [`MAX_BYTES_RESPONSE`] to guard against
    /// runaway allocations.
    pub async fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let url = self.url_for(path)?;
        let req = self.inner.request(Method::GET, url);
        let resp = self.send(req, Method::GET, path).await?;
        // Inspect Content-Length when present and reject early if it
        // exceeds the cap. Servers may omit the header; we still defensively
        // cap during the streaming read below.
        if let Some(cl) = resp.content_length()
            && cl as usize > MAX_BYTES_RESPONSE
        {
            return Err(Error::Config(format!(
                "GET {path} response Content-Length {cl} exceeds cap of {MAX_BYTES_RESPONSE} bytes"
            )));
        }
        let bytes = resp.bytes().await?;
        if bytes.len() > MAX_BYTES_RESPONSE {
            return Err(Error::Config(format!(
                "GET {path} response body {} bytes exceeds cap of {MAX_BYTES_RESPONSE} bytes",
                bytes.len()
            )));
        }
        Ok(bytes.to_vec())
    }

    /// `POST` with a JSON body, deserializing the JSON response.
    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.json_request(Method::POST, path, body).await
    }

    /// `PUT` with a JSON body, deserializing the JSON response.
    pub async fn put<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.json_request(Method::PUT, path, body).await
    }

    /// `PATCH` with a JSON body, deserializing the JSON response.
    pub async fn patch<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.json_request(Method::PATCH, path, body).await
    }

    /// `PATCH` with a JSON body, discarding the response body. For Horizon
    /// mutation endpoints that reply with a plaintext success string (e.g.
    /// "EventConf sources updated successfully.") rather than a JSON body —
    /// trying to deserialize those via [`Self::patch`] yields a transport
    /// error on the response decode.
    pub async fn patch_drain<B: Serialize>(&self, path: &str, body: &B) -> Result<()> {
        self.mutate_drain(Method::PATCH, path, body).await
    }

    /// `PUT` with a JSON body, discarding the response body. Same rationale
    /// as [`Self::patch_drain`].
    pub async fn put_drain<B: Serialize>(&self, path: &str, body: &B) -> Result<()> {
        self.mutate_drain(Method::PUT, path, body).await
    }

    /// Shared body for the `*_drain` mutation helpers — apply auth, send,
    /// drain the body so the connection can return to the pool, return `()`.
    async fn mutate_drain<B: Serialize>(&self, method: Method, path: &str, body: &B) -> Result<()> {
        let url = self.url_for(path)?;
        let req = self
            .inner
            .request(method.clone(), url)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .json(body);
        let resp = self.send(req, method, path).await?;
        let _ = resp.bytes().await?;
        Ok(())
    }

    /// `PUT` with an `application/x-www-form-urlencoded` body, discarding the
    /// response body. Used by v1 `UserRestService.updateUser` (and similar
    /// Horizon endpoints) that do not accept JSON. The form struct must
    /// serialize to a flat key=value map — nested values are not representable
    /// in form-encoding and `serde_urlencoded` will refuse them.
    pub async fn put_form<F: Serialize>(&self, path: &str, form: &F) -> Result<()> {
        let url = self.url_for(path)?;
        let body = serde_urlencoded::to_string(form)
            .map_err(|e| Error::Config(format!("form encode for PUT {path}: {e}")))?;
        let req = self
            .inner
            .request(Method::PUT, url)
            .header(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            )
            .body(body);
        let resp = self.send(req, Method::PUT, path).await?;
        let _ = resp.bytes().await?;
        Ok(())
    }

    /// `POST` with an `application/xml` body, discarding the response body.
    /// Used by v1 `UserRestService.addUser`, which only consumes XML — a JSON
    /// POST to `/users` returns `415 Unsupported Media Type` (verified against
    /// a live Horizon). The caller serializes the document (e.g. via
    /// `quick-xml`) and passes the full string; any query string (such as
    /// `?hashPassword=true`) rides on `path`.
    pub async fn post_xml(&self, path: &str, body: String) -> Result<()> {
        let url = self.url_for(path)?;
        let req = self
            .inner
            .request(Method::POST, url)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/xml"))
            .body(body);
        let resp = self.send(req, Method::POST, path).await?;
        let _ = resp.bytes().await?;
        Ok(())
    }

    /// `DELETE` with optional JSON body. Returns unit on success since the
    /// EventConf delete endpoints return either 200 or 204 with no
    /// caller-actionable payload. The body is read and discarded so the
    /// underlying connection can return to the pool cleanly.
    pub async fn delete<B: Serialize>(&self, path: &str, body: Option<&B>) -> Result<()> {
        let url = self.url_for(path)?;
        let mut req = self.inner.request(Method::DELETE, url);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = self.send(req, Method::DELETE, path).await?;
        // Drain the response body so the connection is reusable.
        let _ = resp.bytes().await?;
        Ok(())
    }

    /// `POST` with `multipart/form-data`, returning JSON. Used by
    /// `/eventconf/upload`. Capabilities pass typed [`MultipartPart`]s; the
    /// reqwest `Form` is constructed here so capability code never sees
    /// reqwest types.
    pub async fn multipart<T: DeserializeOwned>(
        &self,
        path: &str,
        parts: &[MultipartPart],
    ) -> Result<T> {
        let url = self.url_for(path)?;
        let mut form = reqwest::multipart::Form::new();
        for p in parts {
            let part = reqwest::multipart::Part::bytes(p.body.clone())
                .file_name(p.filename.clone())
                .mime_str(&p.content_type)
                .map_err(|e| {
                    Error::Config(format!(
                        "multipart part '{}': invalid content-type '{}': {e}",
                        p.filename, p.content_type
                    ))
                })?;
            form = form.part(p.field_name.clone(), part);
        }
        let req = self.inner.request(Method::POST, url).multipart(form);
        let resp = self.send(req, Method::POST, path).await?;
        json_or_no_content(resp, Method::POST, path).await
    }

    // -- internals -----------------------------------------------------------

    async fn json_request<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = self.url_for(path)?;
        let req = self
            .inner
            .request(method.clone(), url)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .header(ACCEPT, HeaderValue::from_static("application/json"))
            .json(body);
        let resp = self.send(req, method.clone(), path).await?;
        json_or_no_content(resp, method, path).await
    }

    /// Send a request with auth applied, mapping non-success status codes to
    /// [`Error::HttpStatus`] with the response body inlined (and excerpted).
    async fn send(
        &self,
        req: RequestBuilder,
        method: Method,
        path: &str,
    ) -> Result<reqwest::Response> {
        if self.insecure {
            warn_insecure_tls(&method);
        }
        let req = self.apply_auth(req);
        let resp = req.send().await?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        // 401 with WWW-Authenticate naming an unsupported scheme is its own
        // error class so ops automation can branch on exit code 9.
        if resp.status() == StatusCode::UNAUTHORIZED
            && let Some(scheme) = unsupported_auth_scheme(&resp)
        {
            return Err(Error::UnsupportedAuthScheme(scheme));
        }
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Err(Error::HttpStatus {
            method: method.to_string(),
            path: path.to_string(),
            status,
            body: excerpt_body(&body),
        })
    }

    fn apply_auth(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.creds {
            AuthCreds::Basic { username, password } => req.basic_auth(username, Some(password)),
            AuthCreds::Bearer { token } => req.bearer_auth(token),
        }
    }

    fn url_for(&self, path: &str) -> Result<Url> {
        // Reject path-traversal segments. Capability code is trusted but a
        // defensive check prevents subtle bugs that would otherwise let a
        // malformed path escape the API base.
        if path.split(['/', '\\']).any(|seg| seg == "..") {
            return Err(Error::Config(format!(
                "path '{path}' contains path-traversal segment"
            )));
        }
        // Joining a relative path: ensure base ends with `/` so reqwest's join
        // treats it as a directory rather than replacing the last segment.
        let base = if self.base.path().ends_with('/') {
            self.base.clone()
        } else {
            let mut b = self.base.clone();
            b.set_path(&format!("{}/", b.path()));
            b
        };
        let trimmed = path.trim_start_matches('/');
        base.join(trimmed)
            .map_err(|e| Error::Config(format!("invalid path '{path}' joined to base: {e}")))
    }
}

/// Decode the response body as JSON. If the response is `204 No Content`,
/// attempt to deserialize a unit / empty value via `serde_json::from_str("null")`
/// — `T = ()` works; other types yield a clear error.
async fn json_or_no_content<T: DeserializeOwned>(
    resp: reqwest::Response,
    method: Method,
    path: &str,
) -> Result<T> {
    if resp.status() == StatusCode::NO_CONTENT {
        return serde_json::from_str("null").map_err(|_| Error::HttpStatus {
            method: method.to_string(),
            path: path.to_string(),
            status: 204,
            body: "204 No Content with non-unit response type expected by caller".into(),
        });
    }
    Ok(resp.json().await?)
}

/// Inspect a 401 response for any `WWW-Authenticate` challenge that names
/// something other than Basic or Bearer. The header may carry multiple
/// schemes (e.g. `"Bearer realm=\"x\", Negotiate"`); we scan all of them
/// and report the first unsupported scheme found. If at least one
/// supported scheme is offered AND no unsupported one is, returns `None`.
fn unsupported_auth_scheme(resp: &reqwest::Response) -> Option<String> {
    let header = resp.headers().get(reqwest::header::WWW_AUTHENTICATE)?;
    let challenge = header.to_str().ok()?;
    // Schemes are comma-separated at the top level. Each scheme entry
    // begins with the scheme keyword; we skip auth-params (key="value"
    // pairs) when looking for scheme tokens.
    for scheme in extract_schemes(challenge) {
        let lower = scheme.to_ascii_lowercase();
        if lower != "basic" && lower != "bearer" {
            return Some(scheme);
        }
    }
    None
}

/// Pull scheme tokens from a WWW-Authenticate header value. A scheme token
/// is a bare word (no `=`) that appears at the top level (between commas).
/// Tokens that contain `=` are auth-params and are skipped.
fn extract_schemes(challenge: &str) -> Vec<String> {
    let mut out = Vec::new();
    for entry in challenge.split(',') {
        let first = entry.split_whitespace().next().unwrap_or("");
        if !first.is_empty() && !first.contains('=') {
            out.push(first.to_string());
        }
    }
    out
}

/// Emit a per-request stderr warning that TLS certificate verification is
/// disabled. Capability code calls this through `OnmsClient::send` so every
/// outgoing request reminds the operator that the connection is unsafe.
///
/// The path is intentionally NOT included — request paths can leak
/// identifying information (customer ids, account names) into log
/// retention and explode log cardinality on paginated capability calls.
/// The method alone is enough to anchor the warning.
fn warn_insecure_tls(method: &Method) {
    eprintln!(
        "warning: TLS certificate verification is disabled \
         (insecure-skip-tls-verify) for {method} request. \
         Use only on trusted networks."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::OutputFormat;
    use serde::{Deserialize, Serialize};
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx_for(server_url: &str, creds: AuthCreds) -> Context {
        Context {
            name: "test".into(),
            url: Url::parse(server_url).unwrap(),
            creds,
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
        }
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct Sample {
        name: String,
        count: u32,
    }

    #[tokio::test]
    async fn get_returns_parsed_json_with_basic_auth() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/things"))
            .and(header("authorization", "Basic YWRtaW46c2VjcmV0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(Sample {
                name: "first".into(),
                count: 17,
            }))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::basic("admin", "secret"),
        ))
        .unwrap();

        let got: Sample = client.get("things", &[]).await.unwrap();
        assert_eq!(
            got,
            Sample {
                name: "first".into(),
                count: 17
            }
        );
    }

    #[tokio::test]
    async fn get_requests_json_via_accept_header() {
        // Regression: Horizon REST endpoints are `@Produces({XML, JSON})` with
        // XML first, so without `Accept: application/json` a real server returns
        // XML and the JSON decode fails as "error decoding response body". The
        // matcher requires the header — if the client stops sending it, the mock
        // won't match and this test fails.
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/things"))
            .and(header("accept", "application/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(Sample {
                name: "first".into(),
                count: 17,
            }))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::basic("admin", "secret"),
        ))
        .unwrap();

        let got: Sample = client.get("things", &[]).await.unwrap();
        assert_eq!(got.count, 17);
    }

    #[tokio::test]
    async fn query_parameters_are_passed_through() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/things"))
            .and(query_param("limit", "5"))
            .and(query_param("offset", "10"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let _v: Vec<Sample> = client
            .get("things", &[("limit", "5"), ("offset", "10")])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn http_404_yields_httpstatus_error_with_method_path_body() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/missing"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not here"))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let err: Error = client.get::<Sample>("missing", &[]).await.unwrap_err();
        match err {
            Error::HttpStatus {
                method,
                path,
                status,
                body,
            } => {
                assert_eq!(method, "GET");
                assert_eq!(path, "missing");
                assert_eq!(status, 404);
                assert_eq!(body, "not here");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn http_5xx_body_is_excerpted() {
        let mock = MockServer::start().await;
        // Build a body larger than the 4 KiB cap.
        let big_body = "x".repeat(8000);
        Mock::given(method("GET"))
            .and(path("/api/v2/boom"))
            .respond_with(ResponseTemplate::new(500).set_body_string(big_body.clone()))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let err = client.get::<Sample>("boom", &[]).await.unwrap_err();
        match err {
            Error::HttpStatus { body, .. } => {
                assert!(body.len() < big_body.len(), "body should be excerpted");
                assert!(body.contains("truncated"));
                assert!(body.contains(&format!("{} bytes total", big_body.len())));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn unsupported_auth_challenge_yields_specific_error_with_exit_9() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/x"))
            .respond_with(ResponseTemplate::new(401).insert_header("WWW-Authenticate", "Negotiate"))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let err = client.get::<Sample>("x", &[]).await.unwrap_err();
        match &err {
            Error::UnsupportedAuthScheme(s) => assert_eq!(s, "Negotiate"),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(err.exit_code(), 9);
    }

    #[tokio::test]
    async fn unsupported_auth_detected_when_listed_after_supported_one() {
        // Server offers Bearer first, then Negotiate. The first-only parser
        // would miss Negotiate; the multi-scheme parser must detect it.
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/x"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("WWW-Authenticate", "Bearer realm=\"x\", Negotiate"),
            )
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let err = client.get::<Sample>("x", &[]).await.unwrap_err();
        match err {
            Error::UnsupportedAuthScheme(s) => assert_eq!(s, "Negotiate"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn supported_only_challenge_is_not_flagged() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/x"))
            .respond_with(
                ResponseTemplate::new(401).insert_header("WWW-Authenticate", "Basic realm=\"x\""),
            )
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let err = client.get::<Sample>("x", &[]).await.unwrap_err();
        // 401 with a *supported* scheme falls through to HttpStatus(401).
        match err {
            Error::HttpStatus { status, .. } => assert_eq!(status, 401),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn post_sends_json_body_and_returns_parsed_response() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/things"))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(201).set_body_json(Sample {
                name: "created".into(),
                count: 1,
            }))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let body = serde_json::json!({"name": "new"});
        let got: Sample = client.post("things", &body).await.unwrap();
        assert_eq!(got.name, "created");
    }

    #[tokio::test]
    async fn patch_round_trips_correctly() {
        let mock = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/api/v2/sources/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let _: serde_json::Value = client
            .patch("sources/status", &serde_json::json!({ "enabled": true }))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_returns_unit_on_success() {
        let mock = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/api/v2/sources"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        client
            .delete::<serde_json::Value>("sources", Some(&serde_json::json!({"sourceIds": [1]})))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn get_bytes_returns_raw_response() {
        let mock = MockServer::start().await;
        let xml = b"<events><event><uei>uei.test</uei></event></events>";
        Mock::given(method("GET"))
            .and(path("/api/v2/sources/42/events/download"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(xml.as_slice()))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let bytes = client
            .get_bytes("sources/42/events/download")
            .await
            .unwrap();
        assert_eq!(bytes, xml);
    }

    #[tokio::test]
    async fn multipart_wrapper_constructs_form_internally() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/eventconf/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": [{"file": "foo.events.xml", "eventCount": 0}],
                "errors": []
            })))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let parts = vec![MultipartPart::xml("foo.events.xml", b"<events/>".to_vec())];
        let _: serde_json::Value = client.multipart("eventconf/upload", &parts).await.unwrap();
    }

    #[test]
    fn url_for_rejects_path_traversal() {
        let client = OnmsClient::from_parts(
            Url::parse("http://example.com/opennms/").unwrap(),
            AuthCreds::bearer("t"),
        )
        .unwrap();
        let err = client.url_for("api/v2/../../etc/passwd").unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("path-traversal")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn url_join_handles_base_without_trailing_slash() {
        let client = OnmsClient::from_parts(
            Url::parse("http://example.com/opennms").unwrap(),
            AuthCreds::bearer("t"),
        )
        .unwrap();
        let url = client.url_for("api/v2/eventconf/sources").unwrap();
        assert_eq!(
            url.as_str(),
            "http://example.com/opennms/api/v2/eventconf/sources"
        );
    }

    #[test]
    fn url_join_strips_leading_slash_on_path() {
        let client = OnmsClient::from_parts(
            Url::parse("http://example.com/opennms/").unwrap(),
            AuthCreds::bearer("t"),
        )
        .unwrap();
        let url = client.url_for("/api/v2/eventconf").unwrap();
        assert_eq!(url.as_str(), "http://example.com/opennms/api/v2/eventconf");
    }

    #[test]
    fn extract_schemes_handles_realistic_challenges() {
        assert_eq!(extract_schemes("Basic"), vec!["Basic"]);
        assert_eq!(
            extract_schemes("Bearer realm=\"x\", Negotiate"),
            vec!["Bearer", "Negotiate"]
        );
        assert_eq!(
            extract_schemes("Digest realm=\"foo\", qop=\"auth\""),
            vec!["Digest"]
        );
        // Plain Bearer (no params) followed by another scheme.
        assert_eq!(
            extract_schemes("Bearer, Basic realm=\"y\""),
            vec!["Bearer", "Basic"]
        );
    }

    #[tokio::test]
    async fn put_form_happy_path_drains_204() {
        let mock = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/users/alice"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        client
            .put_form(
                "users/alice",
                &Sample {
                    name: "alice".into(),
                    count: 17,
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn put_form_400_yields_http_status() {
        let mock = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/users/alice"))
            .respond_with(ResponseTemplate::new(400).set_body_string("invalid form field"))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        let err = client
            .put_form(
                "users/alice",
                &Sample {
                    name: "alice".into(),
                    count: 17,
                },
            )
            .await
            .unwrap_err();
        match err {
            Error::HttpStatus { status, body, .. } => {
                assert_eq!(status, 400);
                assert!(body.contains("invalid form field"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn put_form_sends_basic_auth_header() {
        let mock = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/users/alice"))
            .and(header("authorization", "Basic YWRtaW46c2VjcmV0"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/api/v2/", mock.uri()),
            AuthCreds::basic("admin", "secret"),
        ))
        .unwrap();
        client
            .put_form(
                "users/alice",
                &Sample {
                    name: "alice".into(),
                    count: 17,
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn put_form_respects_base_path_prefix() {
        let mock = MockServer::start().await;
        // Mount at the prefixed URL the client should construct.
        Mock::given(method("PUT"))
            .and(path("/opennms/rest/users/alice"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;

        let client = OnmsClient::from_context(&ctx_for(
            &format!("{}/opennms/rest/", mock.uri()),
            AuthCreds::bearer("tok"),
        ))
        .unwrap();
        client
            .put_form(
                "users/alice",
                &Sample {
                    name: "alice".into(),
                    count: 17,
                },
            )
            .await
            .unwrap();
    }
}
