/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Shared HTTP transport for `onmsctl` capabilities.
//!
//! Capabilities consume `&OnmsClient` and never reach for `reqwest`
//! directly. This is the bright line that keeps the workspace
//! maintainable (design.md §2.1).
//!
//! Error mapping:
//!   - HTTP non-success status → [`Error::HttpStatus`] with method/path/body
//!   - Network failures → mapped via `From<reqwest::Error>` to typed
//!     transport variants (DNS, ConnRefused, Timeout, TLS, Redirect)
//!     so exit codes per cli-core spec §4.5 are preserved.

use reqwest::header::{CONTENT_TYPE, HeaderValue};
use reqwest::{Method, RequestBuilder, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::auth::AuthCreds;
use crate::context::Context;
use crate::error::{Error, Result};

/// HTTP transport for OpenNMS Horizon capabilities. Cheap to clone.
#[derive(Clone, Debug)]
pub struct OnmsClient {
    inner: reqwest::Client,
    base: Url,
    creds: AuthCreds,
}

impl OnmsClient {
    /// Construct from a resolved [`Context`]. Honors `insecure_skip_tls_verify`
    /// and emits a single stderr warning per process when it is set.
    pub fn from_context(ctx: &Context) -> Result<Self> {
        let mut builder =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::limited(5));
        if ctx.insecure_skip_tls_verify {
            warn_insecure_tls_once();
            builder = builder.danger_accept_invalid_certs(true);
        }
        let inner = builder.build()?;
        Ok(Self {
            inner,
            base: ctx.url.clone(),
            creds: ctx.creds.clone(),
        })
    }

    /// Construct with explicit pieces; useful for tests against `wiremock`.
    pub fn from_parts(base: Url, creds: AuthCreds) -> Result<Self> {
        Ok(Self {
            inner: reqwest::Client::new(),
            base,
            creds,
        })
    }

    /// `GET <base><path>` with optional query parameters, deserializing the
    /// JSON response.
    pub async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let url = self.url_for(path)?;
        let mut req = self.inner.request(Method::GET, url);
        if !query.is_empty() {
            req = req.query(query);
        }
        let resp = self.send(req, Method::GET, path).await?;
        Ok(resp.json().await?)
    }

    /// `GET <base><path>` returning the raw response body bytes (e.g. for XML
    /// downloads from `/eventconf/sources/{id}/events/download`).
    pub async fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let url = self.url_for(path)?;
        let req = self.inner.request(Method::GET, url);
        let resp = self.send(req, Method::GET, path).await?;
        Ok(resp.bytes().await?.to_vec())
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

    /// `DELETE` with optional JSON body. Returns unit on success since the
    /// EventConf delete endpoints return either 200 or 204 with no
    /// caller-actionable payload.
    pub async fn delete<B: Serialize>(&self, path: &str, body: Option<&B>) -> Result<()> {
        let url = self.url_for(path)?;
        let mut req = self.inner.request(Method::DELETE, url);
        if let Some(b) = body {
            req = req.json(b);
        }
        let _ = self.send(req, Method::DELETE, path).await?;
        Ok(())
    }

    /// `POST` with `multipart/form-data`, returning JSON. Used by
    /// `/eventconf/upload`.
    pub async fn multipart<T: DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<T> {
        let url = self.url_for(path)?;
        let req = self.inner.request(Method::POST, url).multipart(form);
        let resp = self.send(req, Method::POST, path).await?;
        Ok(resp.json().await?)
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
            .json(body);
        let resp = self.send(req, method, path).await?;
        Ok(resp.json().await?)
    }

    /// Send a request with auth applied, mapping non-success status codes to
    /// [`Error::HttpStatus`] with the response body inlined.
    async fn send(
        &self,
        req: RequestBuilder,
        method: Method,
        path: &str,
    ) -> Result<reqwest::Response> {
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
            body,
        })
    }

    fn apply_auth(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.creds {
            AuthCreds::Basic { username, password } => req.basic_auth(username, Some(password)),
            AuthCreds::Bearer { token } => req.bearer_auth(token),
        }
    }

    fn url_for(&self, path: &str) -> Result<Url> {
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

/// Inspect a 401 response for a `WWW-Authenticate` challenge that names
/// something other than Basic or Bearer.
fn unsupported_auth_scheme(resp: &reqwest::Response) -> Option<String> {
    let challenge = resp
        .headers()
        .get("www-authenticate")?
        .to_str()
        .ok()?
        .trim();
    let scheme = challenge.split_whitespace().next()?.trim_end_matches(',');
    let lower = scheme.to_ascii_lowercase();
    if lower == "basic" || lower == "bearer" {
        return None;
    }
    Some(scheme.to_string())
}

static INSECURE_TLS_WARNING_EMITTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn warn_insecure_tls_once() {
    use std::sync::atomic::Ordering;
    if !INSECURE_TLS_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "warning: TLS certificate verification is disabled (insecure-skip-tls-verify). \
             Use only on trusted networks."
        );
    }
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
            url: Url::parse(server_url).unwrap(),
            creds,
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
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
    async fn unsupported_auth_challenge_yields_specific_error_with_exit_9() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/x"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("WWW-Authenticate", "Negotiate, Bearer realm=fallback"),
            )
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
}
