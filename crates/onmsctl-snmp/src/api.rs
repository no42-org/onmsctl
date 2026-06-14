/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed wrapper around Horizon's `/api/v2/snmp-config` REST surface.
//!
//! `apply` needs exactly two operations: read the deployed config (to diff
//! against the desired one) and replace it wholesale. Both are absorbed here so
//! the handler never touches the HTTP transport.
//!
//! NOTE: the endpoint paths and the multipart upload shape are derived from the
//! `SnmpConfigRestService` v2 source, not yet a captured live exchange — confirm
//! `GET /api/v2/snmp-config` and the `POST …/upload` multipart part against a
//! real Horizon (see the change's task 9.2).

use onmsctl_core::client::MultipartPart;
use onmsctl_core::{Error, OnmsClient, Result};
use serde::Deserialize;

use crate::server::{SnmpAgentConfig, SnmpConfig, TrapdConfig};

/// Tolerant view of an upload response body used only to detect a rejection.
/// Horizon's CXF multipart `/upload` handlers report per-file failures in an
/// `errors` array even on HTTP 200 (the eventconf sibling's `UploadResult` has
/// the same `{ success, errors }` shape). We don't model success entries —
/// only enough to fail loudly when the server rejected the config. `#[serde(default)]`
/// keeps an empty or partial body from breaking the parse; a body that isn't
/// this shape at all simply yields no `errors` and is treated as success.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UploadResponse {
    errors: Vec<serde_json::Value>,
}

/// REST base for the v2 snmp-config singleton. No leading slash: the client
/// joins it onto the context URL (which ends in `/`), matching the eventconf
/// capability's `api/v2/eventconf`.
const BASE: &str = "api/v2/snmp-config";

/// Typed view over the snmp-config endpoints. Borrows the shared client.
pub struct SnmpConfigApi<'a> {
    client: &'a OnmsClient,
}

impl<'a> SnmpConfigApi<'a> {
    pub fn new(client: &'a OnmsClient) -> Self {
        Self { client }
    }

    /// `GET /api/v2/snmp-config` — the deployed three-tier config (inline
    /// defaults + definitions + profiles).
    pub async fn get_config(&self) -> Result<SnmpConfig> {
        self.client.get(BASE, &[]).await
    }

    /// `GET /api/v2/snmp-config/lookup?ipAddress=&location=` — the effective
    /// (merged) agent config OpenNMS would use for `ip` at `location`.
    pub async fn lookup_for_ip(&self, ip: &str, location: &str) -> Result<SnmpAgentConfig> {
        self.client
            .get(
                &format!("{BASE}/lookup"),
                &[("ipAddress", ip), ("location", location)],
            )
            .await
    }

    /// `POST /api/v2/snmp-config/upload` — whole-config replace. The serialized
    /// `SnmpConfig` JSON is wrapped as a multipart `upload` part (the endpoint
    /// is a CXF multipart `Attachment`). Success is an empty (or non-error)
    /// body; a non-2xx is surfaced by the client, and a 2xx body carrying a
    /// non-empty `errors` envelope is treated as a rejection — so a
    /// content-rejected upload is not mistaken for success.
    pub async fn upload_config(&self, cfg: &SnmpConfig) -> Result<()> {
        let body = serde_json::to_vec(cfg)
            .map_err(|e| Error::Config(format!("serialize snmp-config for upload: {e}")))?;
        let part = MultipartPart::json("snmp-config.json", body);
        let resp = self
            .client
            .multipart_text(&format!("{BASE}/upload"), &[part])
            .await?;
        let trimmed = resp.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        // A recognizable error envelope on a 2xx means the server rejected the
        // config; anything else (an empty/benign body) is success.
        if let Ok(parsed) = serde_json::from_str::<UploadResponse>(trimmed)
            && !parsed.errors.is_empty()
        {
            let detail = parsed
                .errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(Error::Config(format!(
                "snmp-config upload rejected by the server: {detail}"
            )));
        }
        Ok(())
    }
}

/// REST base for the v2 trap-daemon (Trapd) singleton config. Introduced by
/// NMS-19128 (the Horizon `37.x`/`develop` line); absent on older servers.
const TRAPD_BASE: &str = "api/v2/trapd/config";

/// A clear message when the server has no Trapd REST surface (it predates
/// NMS-19128). Surfaced for a `404`/`405` on the write path so an operator on an
/// older Horizon gets a version hint, not a bare not-found.
const TRAPD_UNSUPPORTED: &str = "this OpenNMS server does not expose the Trapd config REST API \
     (requires the NMS-19128 build, Horizon 37.x/develop); remove `spec.trapd` \
     or upgrade the server";

/// Typed view over the trap-daemon config endpoint. Borrows the shared client.
pub struct TrapdConfigApi<'a> {
    client: &'a OnmsClient,
}

impl<'a> TrapdConfigApi<'a> {
    pub fn new(client: &'a OnmsClient) -> Self {
        Self { client }
    }

    /// `GET /api/v2/trapd/config` — the deployed trap-daemon config.
    ///
    /// Returns `Ok(None)` on a `404`: on a *supported* server a 404 means "no
    /// trap config persisted yet" (a legitimate first-run state, reconciled as a
    /// create), and on an *unsupported* server it means the route is absent. We
    /// do not abort the plan on either — the distinction is made on the write
    /// path, where an unsupported server returns `404`/`405` and
    /// [`Self::update_config`] surfaces a version hint. Other statuses (e.g.
    /// `401`/`403` permission, `500`) propagate unchanged so they are not
    /// mistaken for "no config".
    pub async fn get_config(&self) -> Result<Option<TrapdConfig>> {
        match self.client.get::<TrapdConfig>(TRAPD_BASE, &[]).await {
            Ok(cfg) => Ok(Some(cfg)),
            Err(Error::HttpStatus { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `PUT /api/v2/trapd/config` — replace the trap-daemon config from JSON.
    /// The body is discarded (success is any 2xx). A `404`/`405` here means the
    /// route is absent (the server predates NMS-19128) and is rewritten to a
    /// clear version error; any other non-2xx keeps the server's (plain-text)
    /// body verbatim via [`onmsctl_core::Error::HttpStatus`].
    pub async fn update_config(&self, cfg: &TrapdConfig) -> Result<()> {
        match self.client.put_drain(TRAPD_BASE, cfg).await {
            Ok(()) => Ok(()),
            Err(Error::HttpStatus {
                status: 404 | 405, ..
            }) => Err(Error::Config(TRAPD_UNSUPPORTED.into())),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::{AuthCreds, Url};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> OnmsClient {
        OnmsClient::from_parts(
            Url::parse(&format!("{}/", server.uri())).unwrap(),
            AuthCreds::basic("admin", "secret"),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn trapd_get_returns_config() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "snmpTrapPort": 162, "newSuspectOnTrap": false, "snmpv3User": []
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let cfg = TrapdConfigApi::new(&client).get_config().await.unwrap();
        assert_eq!(cfg.unwrap().snmp_trap_port, Some(162));
    }

    #[tokio::test]
    async fn trapd_get_404_is_none_not_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(
                ResponseTemplate::new(404).set_body_string("Trapd configuration not found."),
            )
            .mount(&server)
            .await;
        let client = client_for(&server);
        let cfg = TrapdConfigApi::new(&client).get_config().await.unwrap();
        assert!(cfg.is_none(), "404 ⇒ None (no config / create path)");
    }

    #[tokio::test]
    async fn trapd_get_403_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = TrapdConfigApi::new(&client).get_config().await.unwrap_err();
        assert!(
            matches!(err, Error::HttpStatus { status: 403, .. }),
            "permission errors must not be swallowed as None: {err:?}"
        );
    }

    #[tokio::test]
    async fn trapd_put_success() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = client_for(&server);
        TrapdConfigApi::new(&client)
            .update_config(&TrapdConfig {
                snmp_trap_port: Some(162),
                new_suspect_on_trap: Some(false),
                ..Default::default()
            })
            .await
            .expect("put succeeds");
    }

    #[tokio::test]
    async fn trapd_put_404_becomes_version_error() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = TrapdConfigApi::new(&client)
            .update_config(&TrapdConfig::default())
            .await
            .unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("NMS-19128"), "version hint: {m}"),
            other => panic!("expected a version Config error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn trapd_put_400_keeps_server_message() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string("snmpTrapPort is required and must be between 1 and 65535."),
            )
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = TrapdConfigApi::new(&client)
            .update_config(&TrapdConfig::default())
            .await
            .unwrap_err();
        // The server's plain-text validation message is surfaced verbatim.
        match err {
            Error::HttpStatus {
                status: 400, body, ..
            } => {
                assert!(body.contains("snmpTrapPort is required"), "got: {body}");
            }
            other => panic!("expected HttpStatus 400, got {other:?}"),
        }
    }
}
