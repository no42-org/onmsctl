/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed wrapper around the v2 `/api/v2/business-services` surface plus the
//! adjacent reads used to resolve names → numeric ids (BSM is ID-centric).
//!
//! BSM CRUD: `list` (URIs only), `get` (404 → None), `create` (POST, 201 +
//! Location — body discarded; the id is recovered by re-listing), `replace`
//! (PUT, destructive full-replace), `delete`, and `reload` (the bsmd
//! `daemon/reload` trigger). Resolution: child services via the BSM list,
//! applications via `/api/v2/applications`, nodes via `/api/v2/nodes`
//! (label+location) or `/rest/nodes/{fs}:{fid}` (foreign), and the monitored
//! service's id (ifserviceid) via the node/interface/service path — NOT the
//! service-type id.

use onmsctl_core::{Error, OnmsClient, Result};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::Deserialize;

use crate::model::NodeRefForm;
use crate::server::{
    self, ApplicationList, BusinessServiceList, BusinessServiceRequest, BusinessServiceResponse,
    NodeList,
};

const BASE: &str = "api/v2/business-services";

/// Percent-encode characters unsafe in a single path segment, leaving common
/// literals (`.`, `-`, `:`) intact (IP addresses, `fs:fid` criteria).
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

fn is_not_found(e: &Error) -> bool {
    matches!(e, Error::HttpStatus { status: 404, .. })
}

/// Typed view over the BSM endpoints and the resolution reads. Borrows the
/// shared client.
pub struct BusinessServiceApi<'a> {
    client: &'a OnmsClient,
}

impl<'a> BusinessServiceApi<'a> {
    pub fn new(client: &'a OnmsClient) -> Self {
        Self { client }
    }

    /// `GET /api/v2/business-services` → the numeric ids of every service
    /// (the list returns resource URIs only). `204` → empty.
    pub async fn list_ids(&self) -> Result<Vec<i64>> {
        let list: BusinessServiceList = match self.client.get(BASE, &[]).await {
            Ok(l) => l,
            Err(Error::HttpStatus { status: 204, .. }) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        // Error on an unparseable URI rather than silently dropping a service
        // (a dropped service would look absent and take the Create path).
        list.business_services
            .iter()
            .map(|u| {
                server::id_from_uri(u).ok_or_else(|| {
                    Error::Config(format!(
                        "business-services list returned an unparseable resource URI: {u:?}"
                    ))
                })
            })
            .collect()
    }

    /// `GET /api/v2/business-services/{id}` → one service, or `None` on 404.
    pub async fn get(&self, id: i64) -> Result<Option<BusinessServiceResponse>> {
        match self
            .client
            .get::<BusinessServiceResponse>(&format!("{BASE}/{id}"), &[])
            .await
        {
            Ok(r) => Ok(Some(r)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Fetch every service (list ids, then GET each). Returns `(id, response)`
    /// pairs — the basis for both the current-state diff and child-by-name
    /// resolution.
    pub async fn fetch_all(&self) -> Result<Vec<(i64, BusinessServiceResponse)>> {
        let mut out = Vec::new();
        for id in self.list_ids().await? {
            if let Some(r) = self.get(id).await? {
                out.push((id, r));
            }
        }
        Ok(out)
    }

    /// `POST /api/v2/business-services` — create (201 + Location, body discarded).
    pub async fn create(&self, req: &BusinessServiceRequest) -> Result<()> {
        self.client.post_drain(BASE, req).await
    }

    /// `PUT /api/v2/business-services/{id}` — destructive full-replace.
    pub async fn replace(&self, id: i64, req: &BusinessServiceRequest) -> Result<()> {
        self.client.put_drain(&format!("{BASE}/{id}"), req).await
    }

    /// `DELETE /api/v2/business-services/{id}`.
    pub async fn delete(&self, id: i64) -> Result<()> {
        self.client
            .delete::<()>(&format!("{BASE}/{id}"), None)
            .await
    }

    /// `POST /api/v2/business-services/daemon/reload` — trigger the bsmd reload
    /// that activates BSM changes.
    pub async fn reload(&self) -> Result<()> {
        self.client
            .post_drain(&format!("{BASE}/daemon/reload"), &serde_json::json!({}))
            .await
    }

    /// Resolve an application name → its numeric id via `GET /api/v2/applications`
    /// (fetched whole and matched client-side to avoid FIQL quoting pitfalls).
    /// `None` if no application has that name.
    pub async fn resolve_application(&self, name: &str) -> Result<Option<i64>> {
        let list: ApplicationList = match self
            .client
            .get("api/v2/applications", &[("limit", "0")])
            .await
        {
            Ok(l) => l,
            Err(Error::HttpStatus { status: 204, .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        // Guard against a truncated/paginated response: if the server reports
        // more applications than it returned, a "not found" could be a false
        // negative — fail loudly rather than silently aborting the apply.
        if let Some(total) = list.total_count
            && (list.application.len() as i64) < total
        {
            return Err(Error::Config(format!(
                "applications list returned {} of {total} entries (truncated) — cannot reliably \
                 resolve {name:?}",
                list.application.len()
            )));
        }
        Ok(list
            .application
            .iter()
            .find(|a| a.name == name)
            .and_then(|a| server::as_i64(&a.id)))
    }

    /// Resolve a node reference → its numeric nodeId. Label+location matches
    /// are not unique, so 0 matches and >1 matches are both plan errors (the
    /// latter directs the user to the foreignSource/foreignId form).
    pub async fn resolve_node(&self, form: &NodeRefForm<'_>) -> Result<i64> {
        match form {
            NodeRefForm::Foreign {
                foreign_source,
                foreign_id,
            } => {
                let criteria = format!("{}:{}", enc(foreign_source), enc(foreign_id));
                match self
                    .client
                    .get::<NodeIdResp>(&format!("rest/nodes/{criteria}"), &[])
                    .await
                {
                    Ok(n) => server::as_i64(&n.id).ok_or_else(|| {
                        Error::Config(format!(
                            "node {foreign_source}:{foreign_id} has a non-integer id"
                        ))
                    }),
                    Err(e) if is_not_found(&e) => Err(Error::Config(format!(
                        "node {foreign_source}:{foreign_id} not found"
                    ))),
                    Err(e) => Err(e),
                }
            }
            NodeRefForm::LabelLocation { label, location } => {
                let fiql = format!("label=={label};location.locationName=={location}");
                let list: NodeList = match self
                    .client
                    .get("api/v2/nodes", &[("_s", fiql.as_str()), ("limit", "0")])
                    .await
                {
                    Ok(l) => l,
                    Err(Error::HttpStatus { status: 204, .. }) => NodeList::default(),
                    Err(e) => return Err(e),
                };
                let ids: Vec<i64> = list
                    .node
                    .iter()
                    .filter_map(|n| server::as_i64(&n.id))
                    .collect();
                match ids.len() {
                    1 => Ok(ids[0]),
                    0 => Err(Error::Config(format!(
                        "no node matches label {label:?} in location {location:?}"
                    ))),
                    n => Err(Error::Config(format!(
                        "node label {label:?} in location {location:?} is ambiguous ({n} matches) — \
                         disambiguate with {{foreignSource, foreignId}}"
                    ))),
                }
            }
        }
    }

    /// Resolve a monitored service → its id (ifserviceid) via
    /// `GET /rest/nodes/{nodeId}/ipinterfaces/{ip}/services/{service}`. This is
    /// the OnmsMonitoredService primary key, NOT the service-type id. `None` on
    /// 404 (no such monitored service).
    pub async fn resolve_ifservice(
        &self,
        node_id: i64,
        ip: &str,
        service: &str,
    ) -> Result<Option<i64>> {
        let path = format!(
            "rest/nodes/{node_id}/ipinterfaces/{}/services/{}",
            enc(ip),
            enc(service)
        );
        match self.client.get::<NodeIdResp>(&path, &[]).await {
            Ok(s) => Ok(server::as_i64(&s.id)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Minimal view of any `{ "id": … }` response (node or monitored service); the
/// v1 API may serialize the id as a string or a number.
#[derive(Debug, Default, Deserialize)]
struct NodeIdResp {
    #[serde(default)]
    id: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeRefForm;
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
    async fn list_ids_parses_uris_and_204() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "business-services": [
                    "/api/v2/business-services/1",
                    "/api/v2/business-services/7"
                ]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let ids = BusinessServiceApi::new(&client).list_ids().await.unwrap();
        assert_eq!(ids, vec![1, 7]);
    }

    #[tokio::test]
    async fn resolve_application_matches_by_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/applications"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "application": [ { "id": 3, "name": "Webservers" }, { "id": 4, "name": "DB" } ]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let api = BusinessServiceApi::new(&client);
        assert_eq!(
            api.resolve_application("Webservers").await.unwrap(),
            Some(3)
        );
        assert_eq!(api.resolve_application("Nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn resolve_node_label_location_single_match() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/nodes"))
            .and(query_param(
                "_s",
                "label==webhost01;location.locationName==Default",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "node": [ { "id": "27" } ]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let id = BusinessServiceApi::new(&client)
            .resolve_node(&NodeRefForm::LabelLocation {
                label: "webhost01",
                location: "Default",
            })
            .await
            .unwrap();
        assert_eq!(id, 27);
    }

    #[tokio::test]
    async fn resolve_node_ambiguous_is_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/nodes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "node": [ { "id": 1 }, { "id": 2 } ]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = BusinessServiceApi::new(&client)
            .resolve_node(&NodeRefForm::LabelLocation {
                label: "edge-sw",
                location: "Default",
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "got: {err}");
        assert!(err.to_string().contains("foreignSource"));
    }

    #[tokio::test]
    async fn resolve_node_zero_match_is_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/nodes"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = BusinessServiceApi::new(&client)
            .resolve_node(&NodeRefForm::LabelLocation {
                label: "ghost",
                location: "Default",
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no node matches"), "got: {err}");
    }

    #[tokio::test]
    async fn resolve_ifservice_reads_monitored_service_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/nodes/27/ipinterfaces/10.0.0.10/services/HTTP"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 42, "serviceName": "HTTP" })),
            )
            .mount(&server)
            .await;
        let client = client_for(&server);
        let id = BusinessServiceApi::new(&client)
            .resolve_ifservice(27, "10.0.0.10", "HTTP")
            .await
            .unwrap();
        assert_eq!(id, Some(42));
    }

    #[tokio::test]
    async fn reload_posts_to_daemon_reload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services/daemon/reload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = client_for(&server);
        BusinessServiceApi::new(&client).reload().await.unwrap();
    }
}
