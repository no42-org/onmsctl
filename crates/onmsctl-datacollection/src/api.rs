/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed wrapper around the v2 `datacollectionconf` REST surface
//! (`/api/v2/datacollectionconf`). Confirmed against a live `37.0.0-SNAPSHOT`.
//!
//! Sources are reconciled by whole-source **upload** (multipart
//! `<datacollection-group>` XML, which the server upserts AND prunes — OpenSpec
//! DC4) read back via the JSON **download**. Profiles (`/profiles`) carry the
//! RRD tuning and expose their `sourceNames`, so associations are a true
//! reconcile (attach + detach). The endpoint is absent on released Horizon
//! (≤ 37.0.0), so [`Self::preflight`] gates the whole apply.

use onmsctl_core::client::MultipartPart;
use onmsctl_core::{Error, OnmsClient, Result};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::server::{ProfileDto, SourceDownload, SourceSummary};

const BASE: &str = "api/v2/datacollectionconf";

/// Percent-encode characters unsafe in a single path segment, leaving the
/// common literals intact. Source/profile names can contain spaces (e.g.
/// `Cisco Nexus`, `AKCP sensorProbe`).
const PATH_SEG: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'?')
    .add(b'<')
    .add(b'>')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'|');

fn enc(s: &str) -> String {
    utf8_percent_encode(s, PATH_SEG).to_string()
}

/// True when an error means the data-collection REST endpoint is absent (server
/// predates the subsystem) — a `404`/`405`, distinct from auth/5xx.
pub fn is_endpoint_absent(e: &Error) -> bool {
    matches!(
        e,
        Error::HttpStatus {
            status: 404 | 405,
            ..
        }
    )
}

/// A profile create/update body. Mirrors the validated DTO fields (`rrdStep`,
/// `rrdRras`, `storageFlag`); `enabled` defaults true.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileWrite {
    pub name: String,
    pub rrd_step: u32,
    pub rrd_rras: Vec<String>,
    pub storage_flag: String,
    pub enabled: bool,
}

/// The `…/upload` response envelope: a non-empty `errors` on a 2xx means the
/// server rejected the upload (like the snmp-config upload).
#[derive(Debug, Default, Deserialize)]
struct UploadResponse {
    #[serde(default)]
    errors: Vec<serde_json::Value>,
    #[serde(default)]
    success: Vec<serde_json::Value>,
}

/// Typed view over the `datacollectionconf` endpoints. Borrows the shared client.
pub struct DataCollectionApi<'a> {
    client: &'a OnmsClient,
}

impl<'a> DataCollectionApi<'a> {
    pub fn new(client: &'a OnmsClient) -> Self {
        Self { client }
    }

    /// Probe the endpoint once before any write. A `404`/`405` is rewritten to a
    /// clear "server too old" error; auth/5xx propagate unchanged. Returns the
    /// `names-and-ids` list (reused for name→id resolution, so the probe is not
    /// a wasted round-trip).
    pub async fn preflight(&self) -> Result<Vec<SourceSummary>> {
        self.source_names_and_ids().await.map_err(|e| {
            if is_endpoint_absent(&e) {
                Error::Config(
                    "the data-collection REST endpoint (/api/v2/datacollectionconf) is not \
                     available on this server — it requires a Horizon build with the DB-backed \
                     data-collection subsystem (absent from released Horizon ≤ 37.0.0)"
                        .into(),
                )
            } else {
                e
            }
        })
    }

    /// `GET …/collectsources/names-and-ids` — every source's `{id, name}`.
    pub async fn source_names_and_ids(&self) -> Result<Vec<SourceSummary>> {
        self.client
            .get(&format!("{BASE}/collectsources/names-and-ids"), &[])
            .await
    }

    /// Resolve a source name to its id via `names-and-ids` (`None` if absent).
    pub async fn source_id(&self, name: &str) -> Result<Option<i64>> {
        Ok(self
            .source_names_and_ids()
            .await?
            .into_iter()
            .find(|s| s.name == name)
            .map(|s| s.id))
    }

    /// `GET …/collectsources/{id}` — a source's metadata (incl. `enabled`).
    pub async fn get_source(&self, id: i64) -> Result<SourceSummary> {
        self.client
            .get(&format!("{BASE}/collectsources/{id}"), &[])
            .await
    }

    /// `GET …/collectsources/{id}/download?format=json` — the source tree, or
    /// `None` on 404.
    pub async fn download_source(&self, id: i64) -> Result<Option<SourceDownload>> {
        match self
            .client
            .get::<SourceDownload>(
                &format!("{BASE}/collectsources/{id}/download"),
                &[("format", "json")],
            )
            .await
        {
            Ok(d) => Ok(Some(d)),
            Err(Error::HttpStatus { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST …/upload` — whole-source replace from `<datacollection-group>` XML
    /// (multipart part `"upload"`), attaching to each `profile_names` (the
    /// server requires ≥1 for a NEW source). A non-empty `errors` envelope on a
    /// 2xx is treated as a rejection.
    pub async fn upload_source(
        &self,
        name: &str,
        xml: String,
        profile_names: &[String],
    ) -> Result<()> {
        let mut parts = vec![MultipartPart::xml(format!("{name}.xml"), xml.into_bytes())];
        for p in profile_names {
            parts.push(MultipartPart {
                field_name: "profileNames".into(),
                filename: p.clone(),
                content_type: "text/plain".into(),
                body: p.clone().into_bytes(),
            });
        }
        let resp: UploadResponse = self
            .client
            .multipart(&format!("{BASE}/upload"), &parts)
            .await?;
        if !resp.errors.is_empty() {
            return Err(Error::Config(format!(
                "server rejected the upload of source {name:?}: {}",
                serde_json::to_string(&resp.errors).unwrap_or_default()
            )));
        }
        let _ = resp.success;
        Ok(())
    }

    /// `GET …/collectsources/{id}/download?format={format}` — the raw source
    /// document as text (`xml` or `json`), for the `export` verb.
    pub async fn download_raw(&self, id: i64, format: &str) -> Result<String> {
        let bytes = self
            .client
            .get_bytes(&format!(
                "{BASE}/collectsources/{id}/download?format={format}"
            ))
            .await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// `DELETE …/collectsources?id={id}` — delete the source and its children
    /// (the id is a repeated query param named `id`).
    pub async fn delete_source(&self, id: i64) -> Result<()> {
        self.client
            .delete::<()>(&format!("{BASE}/collectsources?id={id}"), None)
            .await
    }

    /// `PATCH …/collectsources/status/{enabled}` — enable/disable sources by id.
    pub async fn set_source_enabled(&self, ids: &[i64], enabled: bool) -> Result<()> {
        self.client
            .patch_drain(&format!("{BASE}/collectsources/status/{enabled}"), &ids)
            .await
    }

    /// `GET …/profiles` — every snmp-collection profile (with `sourceNames`).
    pub async fn list_profiles(&self) -> Result<Vec<ProfileDto>> {
        self.client.get(&format!("{BASE}/profiles"), &[]).await
    }

    /// `POST …/profiles` — create a profile, returning its new id.
    pub async fn create_profile(&self, p: &ProfileWrite) -> Result<i64> {
        self.client.post(&format!("{BASE}/profiles"), p).await
    }

    /// `PUT …/profiles/{id}` — update a profile's tuning.
    pub async fn update_profile(&self, id: i64, p: &ProfileWrite) -> Result<()> {
        self.client
            .put_drain(&format!("{BASE}/profiles/{id}"), p)
            .await
    }

    /// `POST …/profiles/{id}/sources` — attach a source to a profile by name
    /// (idempotent server-side). The endpoint reads the request body verbatim as
    /// the source name despite `@Consumes(application/json)`, so the bare name is
    /// sent raw — a JSON-quoted `"name"` is rejected as "Unknown source name"
    /// (verified live against 37.0.0-SNAPSHOT).
    pub async fn attach_source(&self, profile_id: i64, source_name: &str) -> Result<()> {
        self.client
            .post_text(
                &format!("{BASE}/profiles/{profile_id}/sources"),
                source_name.to_string(),
                "application/json",
            )
            .await
    }

    /// `DELETE …/profiles/{id}/sources/{name}` — detach a source from a profile.
    pub async fn detach_source(&self, profile_id: i64, source_name: &str) -> Result<()> {
        self.client
            .delete::<()>(
                &format!("{BASE}/profiles/{profile_id}/sources/{}", enc(source_name)),
                None,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::{AuthCreds, Url};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> OnmsClient {
        OnmsClient::from_parts(
            Url::parse(&format!("{}/", server.uri())).unwrap(),
            AuthCreds::basic("admin", "secret"),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn preflight_maps_404_to_too_old() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v2/datacollectionconf/collectsources/names-and-ids",
            ))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = DataCollectionApi::new(&client)
            .preflight()
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not"), "got: {err}");
        assert!(
            err.to_string().contains("data-collection REST"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn source_id_resolves_and_misses() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v2/datacollectionconf/collectsources/names-and-ids",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": 15, "name": "Cisco"}, {"id": 48, "name": "MIB2"}
            ])))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let api = DataCollectionApi::new(&client);
        assert_eq!(api.source_id("Cisco").await.unwrap(), Some(15));
        assert_eq!(api.source_id("Nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn download_404_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/datacollectionconf/collectsources/9/download"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = client_for(&server);
        assert!(
            DataCollectionApi::new(&client)
                .download_source(9)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn upload_rejects_on_error_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/datacollectionconf/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [ {"file": "bad", "reason": "boom"} ], "success": []
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = DataCollectionApi::new(&client)
            .upload_source("bad", "<datacollection-group/>".into(), &["default".into()])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rejected"), "got: {err}");
    }

    #[tokio::test]
    async fn upload_success_is_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/datacollectionconf/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "errors": [], "success": [ {"file": "acme"} ]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        DataCollectionApi::new(&client)
            .upload_source(
                "acme",
                "<datacollection-group/>".into(),
                &["default".into()],
            )
            .await
            .expect("clean upload is ok");
    }

    #[tokio::test]
    async fn delete_uses_id_query_param() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/api/v2/datacollectionconf/collectsources"))
            .and(query_param("id", "7"))
            .respond_with(ResponseTemplate::new(200).set_body_string("deleted"))
            .mount(&server)
            .await;
        let client = client_for(&server);
        DataCollectionApi::new(&client)
            .delete_source(7)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn attach_and_detach_source() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/datacollectionconf/profiles/3/sources"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(
                "/api/v2/datacollectionconf/profiles/3/sources/Cisco%20Nexus",
            ))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let api = DataCollectionApi::new(&client);
        api.attach_source(3, "acme").await.unwrap();
        api.detach_source(3, "Cisco Nexus").await.unwrap();
    }

    #[tokio::test]
    async fn create_profile_returns_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/datacollectionconf/profiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(42)))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let id = DataCollectionApi::new(&client)
            .create_profile(&ProfileWrite {
                name: "p".into(),
                rrd_step: 300,
                rrd_rras: vec!["RRA:AVERAGE:0.5:1:2016".into()],
                storage_flag: "select".into(),
                enabled: true,
            })
            .await
            .unwrap();
        assert_eq!(id, 42);
    }
}
