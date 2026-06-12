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

use crate::server::SnmpConfig;

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

    /// `POST /api/v2/snmp-config/upload` — whole-config replace. The serialized
    /// `SnmpConfig` JSON is wrapped as a multipart `upload` part (the endpoint
    /// is a CXF multipart `Attachment`); the response carries no
    /// caller-actionable body, so it is drained.
    pub async fn upload_config(&self, cfg: &SnmpConfig) -> Result<()> {
        let body = serde_json::to_vec(cfg)
            .map_err(|e| Error::Config(format!("serialize snmp-config for upload: {e}")))?;
        let part = MultipartPart::json("snmp-config.json", body);
        self.client
            .multipart_drain(&format!("{BASE}/upload"), &[part])
            .await
    }
}
