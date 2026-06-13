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

use crate::server::{SnmpAgentConfig, SnmpConfig};

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
