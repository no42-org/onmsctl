/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed wrapper around Horizon's v1 `/rest/sched-outages` surface plus the
//! `/rest/nodes` read used to resolve node foreign references.
//!
//! Two layers (see the change's design D4): the outage **definition**
//! (`list`/`get`/`upsert`/`delete`, readable) and the **attachment**
//! (`attach`, ensure-present — no read endpoint exists). `get` maps a `404` to
//! `None` (create path). The check endpoints return `text/plain` `true`/`false`.

use onmsctl_core::{Error, OnmsClient, Result};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::Deserialize;

use crate::diff::AttachTarget;
use crate::model::Devices;
use crate::server::{NodeList, Outage, Outages};

const BASE: &str = "rest/sched-outages";

/// Percent-encode characters unsafe in a single path segment while leaving the
/// common literals (`.`, `-`, `:`) intact — IP addresses, package names, and the
/// `foreignSource:foreignId` criteria must not have their dots/colons mangled.
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

/// Typed view over the scheduled-outages endpoints. Borrows the shared client.
pub struct MaintenanceApi<'a> {
    client: &'a OnmsClient,
}

impl<'a> MaintenanceApi<'a> {
    pub fn new(client: &'a OnmsClient) -> Self {
        Self { client }
    }

    /// `GET /rest/sched-outages` — every defined outage.
    pub async fn list(&self) -> Result<Outages> {
        self.client.get(BASE, &[]).await
    }

    /// `GET /rest/sched-outages/{name}` — one outage definition, or `None` on 404.
    pub async fn get(&self, name: &str) -> Result<Option<Outage>> {
        match self
            .client
            .get::<Outage>(&format!("{BASE}/{}", enc(name)), &[])
            .await
        {
            Ok(o) => Ok(Some(o)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST /rest/sched-outages` — create or update the definition (JSON body).
    pub async fn upsert(&self, outage: &Outage) -> Result<()> {
        self.client.post_drain(BASE, outage).await
    }

    /// `DELETE /rest/sched-outages/{name}` — remove the outage from every daemon
    /// and delete the definition (the server's full teardown).
    pub async fn delete(&self, name: &str) -> Result<()> {
        self.client
            .delete::<()>(&format!("{BASE}/{}", enc(name)), None)
            .await
    }

    /// Attach the outage to one daemon (ensure-present; bodyless `PUT`). For the
    /// per-package daemons the target carries a package; `notifd` is global.
    pub async fn attach(&self, name: &str, target: &AttachTarget) -> Result<()> {
        let seg = target.daemon.segment();
        let path = match &target.package {
            Some(pkg) => format!("{BASE}/{}/{seg}/{}", enc(name), enc(pkg)),
            None => format!("{BASE}/{}/{seg}", enc(name)),
        };
        self.client.put_empty(&path).await
    }

    /// Resolve a node foreign reference to its server nodeId via
    /// `GET /rest/nodes/{foreignSource}:{foreignId}`. `None` if the node is not
    /// (yet) imported.
    pub async fn resolve_node(
        &self,
        foreign_source: &str,
        foreign_id: &str,
    ) -> Result<Option<i64>> {
        let criteria = format!("{}:{}", enc(foreign_source), enc(foreign_id));
        match self
            .client
            .get::<NodeIdResp>(&format!("rest/nodes/{criteria}"), &[])
            .await
        {
            Ok(n) => Ok(Some(n.id()?)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Resolve a set of dynamic node selectors (categories / locations / asset) to server
    /// nodeIds via one v2 search: `GET api/v2/nodes?_s=<fiql>&limit=0`. `limit=0`
    /// returns all matches; a truncated response (`count < totalCount`) is an
    /// error rather than a silent subset.
    ///
    /// A search that matches **nothing** returns HTTP `204 No Content` (verified
    /// against a live Horizon) — which the JSON client surfaces as a 204 error;
    /// we map it to an empty result (no matches), not a failure.
    pub async fn search_node_ids(&self, fiql: &str) -> Result<Vec<i64>> {
        let list: NodeList = match self
            .client
            .get("api/v2/nodes", &[("_s", fiql), ("limit", "0")])
            .await
        {
            Ok(list) => list,
            Err(Error::HttpStatus { status: 204, .. }) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let ids: Vec<i64> = list
            .node
            .iter()
            .map(|n| node_id_from_value(&n.id))
            .collect::<Result<_>>()?;
        // Guard truncation against the ids we actually received (not the server's
        // self-reported `count`, which could equal `totalCount` while the array
        // is short). When `totalCount` is present and exceeds what we got, fail
        // rather than silently cover a subset.
        if let Some(total) = list.total_count
            && (ids.len() as i64) < total
        {
            return Err(Error::Config(format!(
                "node search matched {total} nodes but only {} were returned (truncated) — \
                 refusing to cover a subset",
                ids.len()
            )));
        }
        Ok(ids)
    }

    /// `GET /rest/sched-outages/interfaceInOutage/{ip}` — is the interface
    /// currently in any scheduled outage? (`text/plain` `true`/`false`).
    pub async fn interface_in_outage(&self, ip: &str) -> Result<bool> {
        self.check(&format!("{BASE}/interfaceInOutage/{}", enc(ip)))
            .await
    }

    /// `GET /rest/sched-outages/nodeInOutage/{nodeId}` — is the node currently in
    /// any scheduled outage?
    pub async fn node_in_outage(&self, node_id: i64) -> Result<bool> {
        self.check(&format!("{BASE}/nodeInOutage/{node_id}")).await
    }

    async fn check(&self, path: &str) -> Result<bool> {
        let body = self.client.get_bytes(path).await?;
        let text = String::from_utf8_lossy(&body);
        match text.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(Error::Config(format!(
                "unexpected response from {path}: {other:?} (expected true/false)"
            ))),
        }
    }
}

/// Minimal view of a `/rest/nodes/{criteria}` response — just the id, which the
/// v1 API may serialize as a string or a number.
#[derive(Debug, Deserialize)]
struct NodeIdResp {
    id: serde_json::Value,
}

impl NodeIdResp {
    fn id(&self) -> Result<i64> {
        node_id_from_value(&self.id)
    }
}

/// Parse an OpenNMS node `id` that the REST API may serialize as a JSON string
/// or number into an `i64`.
fn node_id_from_value(v: &serde_json::Value) -> Result<i64> {
    match v {
        serde_json::Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| Error::Config(format!("node id is not an integer: {n}"))),
        serde_json::Value::String(s) => s
            .parse::<i64>()
            .map_err(|_| Error::Config(format!("node id {s:?} is not an integer"))),
        other => Err(Error::Config(format!("unexpected node id shape: {other}"))),
    }
}

/// Build the FIQL `_s` value for the dynamic node selectors, or `None` when no
/// `categories`/`locations`/`asset` are set. All clauses are OR-joined (`,`) so
/// the search returns the **union** of every selector
/// (`category.name==A,location.locationName==L,assetRecord.<field>==<value>`).
pub fn build_node_fiql(devices: &Devices) -> Option<String> {
    let mut clauses: Vec<String> = devices
        .categories
        .iter()
        .map(|c| format!("category.name=={c}"))
        .collect();
    clauses.extend(
        devices
            .locations
            .iter()
            .map(|l| format!("location.locationName=={l}")),
    );
    if let Some(a) = &devices.asset {
        clauses.push(format!("assetRecord.{}=={}", a.field, a.value));
    }
    (!clauses.is_empty()).then(|| clauses.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::Daemon;
    use crate::model::{AssetSelector, Devices, NodeRef};
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

    #[test]
    fn build_node_fiql_or_joins_categories_and_asset() {
        let d = Devices {
            categories: vec!["Routers".into(), "Core".into()],
            asset: Some(AssetSelector {
                field: "city".into(),
                value: "Berlin".into(),
            }),
            ..Default::default()
        };
        assert_eq!(
            build_node_fiql(&d).as_deref(),
            Some("category.name==Routers,category.name==Core,assetRecord.city==Berlin")
        );
        // Locations OR-join in alongside categories/asset.
        let with_loc = Devices {
            categories: vec!["Routers".into()],
            locations: vec!["Berlin".into(), "Default".into()],
            ..Default::default()
        };
        assert_eq!(
            build_node_fiql(&with_loc).as_deref(),
            Some(
                "category.name==Routers,location.locationName==Berlin,location.locationName==Default"
            )
        );

        // No dynamic selectors → None (only explicit nodes / interfaces).
        let none = Devices {
            nodes: vec![NodeRef {
                foreign_source: "hq".into(),
                foreign_id: "web01".into(),
            }],
            ..Default::default()
        };
        assert!(build_node_fiql(&none).is_none());
    }

    #[tokio::test]
    async fn search_node_ids_sends_query_and_parses_ids() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/nodes"))
            .and(query_param("_s", "category.name==Routers"))
            .and(query_param("limit", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 2, "totalCount": 2,
                "node": [ { "id": 1 }, { "id": "2" } ]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let ids = MaintenanceApi::new(&client)
            .search_node_ids("category.name==Routers")
            .await
            .unwrap();
        assert_eq!(ids, vec![1, 2]);
    }

    #[tokio::test]
    async fn search_node_ids_errors_on_truncation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/nodes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 10, "totalCount": 250, "node": []
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = MaintenanceApi::new(&client)
            .search_node_ids("category.name==Big")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("truncated"), "got: {err}");
    }

    #[tokio::test]
    async fn search_node_ids_treats_204_as_no_matches() {
        // A zero-match search returns 204 No Content on a live Horizon — must be
        // an empty result, not an error.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/nodes"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let ids = MaintenanceApi::new(&client)
            .search_node_ids("category.name==Empty")
            .await
            .expect("204 → empty, not an error");
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn get_404_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/sched-outages/win"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;
        let client = client_for(&server);
        assert!(
            MaintenanceApi::new(&client)
                .get("win")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn attach_per_package_and_global() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/rest/sched-outages/win/pollerd/prod"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/sched-outages/win/notifd"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let api = MaintenanceApi::new(&client);
        api.attach(
            "win",
            &AttachTarget {
                daemon: Daemon::Pollerd,
                package: Some("prod".into()),
            },
        )
        .await
        .unwrap();
        api.attach(
            "win",
            &AttachTarget {
                daemon: Daemon::Notifd,
                package: None,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn resolve_node_handles_string_and_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/nodes/lab:web01"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": "42", "label": "web01" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/nodes/lab:ghost"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let api = MaintenanceApi::new(&client);
        assert_eq!(api.resolve_node("lab", "web01").await.unwrap(), Some(42));
        assert_eq!(api.resolve_node("lab", "ghost").await.unwrap(), None);
    }

    #[tokio::test]
    async fn interface_in_outage_parses_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/sched-outages/interfaceInOutage/192.168.8.8"))
            .respond_with(ResponseTemplate::new(200).set_body_string("true"))
            .mount(&server)
            .await;
        let client = client_for(&server);
        assert!(
            MaintenanceApi::new(&client)
                .interface_in_outage("192.168.8.8")
                .await
                .unwrap()
        );
    }
}
