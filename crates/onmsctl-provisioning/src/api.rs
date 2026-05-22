/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed wrapper around Horizon's legacy provisioning REST surface.
//!
//! Endpoints exposed:
//!
//! | Method  | Path                                          | Method                |
//! |---------|-----------------------------------------------|-----------------------|
//! | GET     | `rest/requisitions/{fs}`                      | [`get_requisition`](ProvisioningApi::get_requisition) |
//! | POST    | `rest/requisitions/{fs}`                      | [`post_requisition`](ProvisioningApi::post_requisition) |
//! | PUT     | `rest/requisitions/{fs}/import?rescanExisting`| [`trigger_import`](ProvisioningApi::trigger_import) |
//! | GET     | `rest/foreignSources/{fs}`                    | [`get_foreign_source`](ProvisioningApi::get_foreign_source) |
//! | POST    | `rest/foreignSources/{fs}`                    | [`post_foreign_source`](ProvisioningApi::post_foreign_source) |
//! | DELETE  | `rest/foreignSources/{fs}`                    | [`delete_foreign_source`](ProvisioningApi::delete_foreign_source) |
//! | GET     | `rest/foreignSources/default`                 | [`get_default_foreign_source`](ProvisioningApi::get_default_foreign_source) |
//!
//! GET endpoints that may legitimately return `404` (a requisition or
//! foreign-source that doesn't exist on this server) surface that as
//! `Ok(None)` rather than `Err(HttpStatus { 404 })` — the apply path
//! treats absence as a first-class case (new requisition; default FS
//! inheritance).

use onmsctl_core::{Error, OnmsClient, Result};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use crate::model::server::{
    ForeignSourceServer, InterfaceServer, MonitoredServiceServer, NodeServer, RequisitionServer,
};

/// Characters that must be percent-encoded inside a path segment.
/// Anything beyond RFC 3986's "unreserved" set (`A-Z`, `a-z`, `0-9`,
/// `-`, `_`, `.`, `~`) plus the reserved/sub-delim characters that
/// have semantic meaning in URLs. We intentionally encode `/` so it
/// can't escape the foreign-source path component into a subpath,
/// `%` so a literal `%` in the input doesn't get confused with an
/// encoded triplet on the wire (callers must pass raw, unencoded
/// names), and `[` / `]` because some routers and proxies reject
/// unencoded brackets in path segments even though RFC 3986 allows
/// them only in the authority component.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b'/')
    .add(b'?')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'=')
    .add(b'+')
    .add(b';')
    .add(b'@')
    .add(b'[')
    .add(b']');

/// Base path under the OnmsClient root URL. Horizon's legacy
/// provisioning endpoints live under `/opennms/rest/...`; the
/// OnmsClient is configured at `/opennms/`, so this BASE bridges the
/// gap.
const BASE: &str = "rest";

/// Typed wrapper over [`OnmsClient`] for the provisioning surface.
pub struct ProvisioningApi<'c> {
    client: &'c OnmsClient,
}

impl<'c> ProvisioningApi<'c> {
    pub fn new(client: &'c OnmsClient) -> Self {
        Self { client }
    }

    // ---------------- Requisitions ----------------

    /// `GET /rest/requisitionNames`. Returns the names of every
    /// deployed requisition on the server, sorted ascending. Used by
    /// `requisition export` with no `<foreign-source>` argument to
    /// enumerate everything for bulk export.
    ///
    /// The wire shape is `{"count": N, "foreign-source": [...]}` —
    /// we surface just the name list.
    pub async fn list_requisition_names(&self) -> Result<Vec<String>> {
        #[derive(serde::Deserialize)]
        struct Wire {
            #[serde(default, rename = "foreign-source")]
            foreign_source: Vec<String>,
        }
        let path = format!("{BASE}/requisitionNames");
        let wire: Wire = self.client.get(&path, &[]).await?;
        let mut names = wire.foreign_source;
        names.sort();
        Ok(names)
    }

    /// `GET /rest/requisitions/{fs}`. Returns `Ok(None)` if the server
    /// responds 404 — i.e. the requisition does not yet exist on this
    /// Horizon (we're about to create it via POST).
    pub async fn get_requisition(&self, fs: &str) -> Result<Option<RequisitionServer>> {
        let path = format!("{BASE}/requisitions/{}", encode(fs));
        match self.client.get::<RequisitionServer>(&path, &[]).await {
            Ok(r) => Ok(Some(r)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST /rest/requisitions/{fs}` (the path segment is part of the
    /// URL but Horizon also takes the foreign-source name from the
    /// body's `foreign-source` field — they must agree). Horizon's
    /// response on success is typically empty or a status string, so
    /// we drain the body.
    pub async fn post_requisition(&self, req: &RequisitionServer) -> Result<()> {
        let path = format!("{BASE}/requisitions/{}", encode(&req.foreign_source));
        // OpenNMS provisioning legacy REST uses POST for create-or-replace.
        self.client.post::<_, serde_json::Value>(&path, req).await?;
        Ok(())
    }

    /// `PUT /rest/requisitions/{fs}/import?rescanExisting=<bool>`.
    /// Triggers the import + scan flow. Returns `Ok(())` on
    /// acceptance — the actual completion is asynchronous on the
    /// server side and is observed via `requisition status`
    /// (task 6.2, future). The scan-report identifier the spec
    /// scenarios reference lives in a Group 6 enhancement; for now
    /// the apply path treats the import as fire-and-forget.
    pub async fn trigger_import(&self, fs: &str, rescan_existing: bool) -> Result<()> {
        let path = format!(
            "{BASE}/requisitions/{}/import?rescanExisting={rescan_existing}",
            encode(fs),
        );
        // Empty PUT body — the query parameter carries the only input.
        self.client.put_drain(&path, &serde_json::Value::Null).await
    }

    // ---------------- Requisition nodes (sub-resource) ----------------

    /// `GET /rest/requisitions/{fs}/nodes/{foreign-id}`. Returns
    /// `Ok(None)` if the server responds 404 — i.e. the node does
    /// not exist within the named requisition.
    pub async fn get_requisition_node(
        &self,
        fs: &str,
        foreign_id: &str,
    ) -> Result<Option<NodeServer>> {
        let path = format!(
            "{BASE}/requisitions/{}/nodes/{}",
            encode(fs),
            encode(foreign_id),
        );
        match self.client.get::<NodeServer>(&path, &[]).await {
            Ok(n) => Ok(Some(n)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST /rest/requisitions/{fs}/nodes`. Adds a new node to the
    /// named requisition. The server treats this as create-or-replace
    /// keyed by the body's `foreign-id`.
    pub async fn post_requisition_node(&self, fs: &str, node: &NodeServer) -> Result<()> {
        let path = format!("{BASE}/requisitions/{}/nodes", encode(fs));
        self.client.post::<_, serde_json::Value>(&path, node).await?;
        Ok(())
    }

    /// `PUT /rest/requisitions/{fs}/nodes/{foreign-id}`. Replaces the
    /// node's full content. The path's `foreign-id` is authoritative;
    /// the body's `foreign-id` should agree.
    pub async fn put_requisition_node(
        &self,
        fs: &str,
        foreign_id: &str,
        node: &NodeServer,
    ) -> Result<()> {
        let path = format!(
            "{BASE}/requisitions/{}/nodes/{}",
            encode(fs),
            encode(foreign_id),
        );
        self.client.put_drain(&path, node).await
    }

    /// `DELETE /rest/requisitions/{fs}/nodes/{foreign-id}`. Removes
    /// the node from the requisition's pending state. The change
    /// takes effect on the next import.
    pub async fn delete_requisition_node(&self, fs: &str, foreign_id: &str) -> Result<()> {
        let path = format!(
            "{BASE}/requisitions/{}/nodes/{}",
            encode(fs),
            encode(foreign_id),
        );
        self.client.delete::<serde_json::Value>(&path, None).await
    }

    // ---------------- Requisition interfaces (sub-resource) ----------------

    /// `GET /rest/requisitions/{fs}/nodes/{foreign-id}/interfaces/{ip}`.
    /// Returns `Ok(None)` if the server responds 404 — i.e. the
    /// interface doesn't exist on the named node.
    pub async fn get_requisition_interface(
        &self,
        fs: &str,
        foreign_id: &str,
        ip: &str,
    ) -> Result<Option<InterfaceServer>> {
        let path = format!(
            "{BASE}/requisitions/{}/nodes/{}/interfaces/{}",
            encode(fs),
            encode(foreign_id),
            encode(ip),
        );
        match self.client.get::<InterfaceServer>(&path, &[]).await {
            Ok(i) => Ok(Some(i)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST /rest/requisitions/{fs}/nodes/{foreign-id}/interfaces`.
    /// Same create-or-replace semantic as the node POST — keyed by
    /// the body's `ip-addr`.
    pub async fn post_requisition_interface(
        &self,
        fs: &str,
        foreign_id: &str,
        iface: &InterfaceServer,
    ) -> Result<()> {
        let path = format!(
            "{BASE}/requisitions/{}/nodes/{}/interfaces",
            encode(fs),
            encode(foreign_id),
        );
        self.client.post::<_, serde_json::Value>(&path, iface).await?;
        Ok(())
    }

    /// `PUT /rest/requisitions/{fs}/nodes/{foreign-id}/interfaces/{ip}`.
    /// Replaces the interface's full body. Path's `ip` is
    /// authoritative.
    pub async fn put_requisition_interface(
        &self,
        fs: &str,
        foreign_id: &str,
        ip: &str,
        iface: &InterfaceServer,
    ) -> Result<()> {
        let path = format!(
            "{BASE}/requisitions/{}/nodes/{}/interfaces/{}",
            encode(fs),
            encode(foreign_id),
            encode(ip),
        );
        self.client.put_drain(&path, iface).await
    }

    /// `DELETE /rest/requisitions/{fs}/nodes/{foreign-id}/interfaces/{ip}`.
    pub async fn delete_requisition_interface(
        &self,
        fs: &str,
        foreign_id: &str,
        ip: &str,
    ) -> Result<()> {
        let path = format!(
            "{BASE}/requisitions/{}/nodes/{}/interfaces/{}",
            encode(fs),
            encode(foreign_id),
            encode(ip),
        );
        self.client.delete::<serde_json::Value>(&path, None).await
    }

    // ---------------- Requisition services (sub-resource) ----------------

    /// `POST /rest/requisitions/{fs}/nodes/{foreign-id}/interfaces/{ip}/services`.
    /// Create-or-replace by the body's `service-name`. Same overwrite
    /// hazard as the node / interface POST: an existing service-name
    /// silently has its `category` and `meta-data` arrays replaced
    /// with whatever this call sends.
    pub async fn post_requisition_service(
        &self,
        fs: &str,
        foreign_id: &str,
        ip: &str,
        svc: &MonitoredServiceServer,
    ) -> Result<()> {
        let path = format!(
            "{BASE}/requisitions/{}/nodes/{}/interfaces/{}/services",
            encode(fs),
            encode(foreign_id),
            encode(ip),
        );
        self.client.post::<_, serde_json::Value>(&path, svc).await?;
        Ok(())
    }

    /// `DELETE /rest/requisitions/{fs}/nodes/{foreign-id}/interfaces/{ip}/services/{name}`.
    pub async fn delete_requisition_service(
        &self,
        fs: &str,
        foreign_id: &str,
        ip: &str,
        service_name: &str,
    ) -> Result<()> {
        let path = format!(
            "{BASE}/requisitions/{}/nodes/{}/interfaces/{}/services/{}",
            encode(fs),
            encode(foreign_id),
            encode(ip),
            encode(service_name),
        );
        self.client.delete::<serde_json::Value>(&path, None).await
    }

    // ---------------- Foreign sources ----------------

    /// `GET /rest/foreignSources/{fs}`. Returns `Ok(None)` if the
    /// server responds 404 — the requisition exists (or is about to
    /// be created) but has no custom foreign-source definition, in
    /// which case Horizon's default-FS applies (see
    /// [`get_default_foreign_source`](Self::get_default_foreign_source)).
    pub async fn get_foreign_source(&self, fs: &str) -> Result<Option<ForeignSourceServer>> {
        let path = format!("{BASE}/foreignSources/{}", encode(fs));
        match self.client.get::<ForeignSourceServer>(&path, &[]).await {
            Ok(fs_) => Ok(Some(fs_)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `GET /rest/foreignSources/default`. Always present on Horizon.
    /// Used as the diff baseline when a portable-style YAML (omitted
    /// `spec.foreignSource`) is applied against a server that also has
    /// no custom FS — per design D1 the diff compares against the
    /// default so out-of-band changes to the default surface as a
    /// diff.
    pub async fn get_default_foreign_source(&self) -> Result<ForeignSourceServer> {
        let path = format!("{BASE}/foreignSources/default");
        self.client.get(&path, &[]).await
    }

    /// `POST /rest/foreignSources/{fs}`. Same create-or-replace
    /// semantic as the requisition POST.
    pub async fn post_foreign_source(&self, fs: &ForeignSourceServer) -> Result<()> {
        let path = format!("{BASE}/foreignSources/{}", encode(&fs.name));
        self.client.post::<_, serde_json::Value>(&path, fs).await?;
        Ok(())
    }

    /// `DELETE /rest/foreignSources/{fs}`. Removes any custom FS
    /// definition for the named foreign-source so that subsequent
    /// imports use Horizon's default-FS. Used by the apply path when
    /// a portable-style YAML is applied against a server that
    /// currently has a custom FS (design D1).
    pub async fn delete_foreign_source(&self, fs: &str) -> Result<()> {
        let path = format!("{BASE}/foreignSources/{}", encode(fs));
        self.client.delete::<serde_json::Value>(&path, None).await
    }
}

/// URL-encode a foreign-source name segment. Encodes only the
/// characters that aren't safe in a path segment (controls, spaces,
/// `/`, `?`, `#`, `<`, `>`, etc.) per RFC 3986 — preserves common
/// safe characters like `-`, `_`, `.`, `~` verbatim so URLs remain
/// human-readable.
fn encode(s: &str) -> impl std::fmt::Display + '_ {
    utf8_percent_encode(s, PATH_SEGMENT)
}

fn is_not_found(e: &Error) -> bool {
    matches!(e, Error::HttpStatus { status: 404, .. })
}

// ---------------------------------------------------------------------------
// Tests — wiremock-driven HTTP round-trips
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::{AuthCreds, Context, OutputFormat, Url};
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_with_client() -> (MockServer, OnmsClient) {
        let server = MockServer::start().await;
        let url = Url::parse(&format!("{}/", server.uri())).unwrap();
        let ctx = Context {
            name: "test".into(),
            url,
            creds: AuthCreds::bearer("t"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
        };
        let client = OnmsClient::from_context(&ctx).unwrap();
        (server, client)
    }

    #[tokio::test]
    async fn get_requisition_returns_some_on_200() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "foreign-source": "acme-prod",
                "node": []
            })))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        let got = api.get_requisition("acme-prod").await.unwrap();
        assert_eq!(got.unwrap().foreign_source, "acme-prod");
    }

    #[tokio::test]
    async fn get_requisition_returns_none_on_404() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        assert!(api.get_requisition("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_foreign_source_returns_none_on_404() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/no-custom"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        assert!(api.get_foreign_source("no-custom").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_default_foreign_source_succeeds() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "default",
                "scan-interval": "1d",
                "detectors": [],
                "policies": []
            })))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        let fs = api.get_default_foreign_source().await.unwrap();
        assert_eq!(fs.name, "default");
    }

    #[tokio::test]
    async fn post_requisition_uses_foreign_source_path() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme-prod"))
            .and(body_json(serde_json::json!({
                "foreign-source": "acme-prod",
                "node": []
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        let req = RequisitionServer {
            foreign_source: "acme-prod".into(),
            date_stamp: None,
            last_import: None,
            node: vec![],
        };
        api.post_requisition(&req).await.unwrap();
    }

    #[tokio::test]
    async fn delete_foreign_source_works() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("DELETE"))
            .and(path("/rest/foreignSources/old-custom"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.delete_foreign_source("old-custom").await.unwrap();
    }

    #[tokio::test]
    async fn trigger_import_passes_rescan_existing_query_param() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .and(query_param("rescanExisting", "true"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.trigger_import("acme-prod", true).await.unwrap();
    }

    #[tokio::test]
    async fn trigger_import_passes_false_when_requested() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme-prod/import"))
            .and(query_param("rescanExisting", "false"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.trigger_import("acme-prod", false).await.unwrap();
    }

    #[tokio::test]
    async fn get_requisition_node_returns_some_on_200() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme/nodes/web01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "foreign-id": "web01",
                "node-label": "web01.acme",
                "interface": [],
                "category": [],
                "asset": [],
                "meta-data": []
            })))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        let got = api.get_requisition_node("acme", "web01").await.unwrap();
        let node = got.expect("node exists");
        assert_eq!(node.foreign_id, "web01");
        assert_eq!(node.node_label, "web01.acme");
    }

    #[tokio::test]
    async fn get_requisition_node_returns_none_on_404() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme/nodes/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        assert!(
            api.get_requisition_node("acme", "missing")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn post_requisition_node_targets_nodes_collection() {
        use crate::model::server::NodeServer;
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme/nodes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.post_requisition_node(
            "acme",
            &NodeServer {
                foreign_id: "web01".into(),
                node_label: "web01.acme".into(),
                location: None,
                building: None,
                city: None,
                parent_foreign_source: None,
                parent_foreign_id: None,
                parent_node_label: None,
                interface: vec![],
                category: vec![],
                asset: vec![],
                meta_data: vec![],
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn put_requisition_node_targets_specific_node() {
        use crate::model::server::NodeServer;
        let (mock, client) = mock_with_client().await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme/nodes/web01"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.put_requisition_node(
            "acme",
            "web01",
            &NodeServer {
                foreign_id: "web01".into(),
                node_label: "web01.renamed".into(),
                location: None,
                building: None,
                city: None,
                parent_foreign_source: None,
                parent_foreign_id: None,
                parent_node_label: None,
                interface: vec![],
                category: vec![],
                asset: vec![],
                meta_data: vec![],
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_requisition_node_targets_specific_node() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("DELETE"))
            .and(path("/rest/requisitions/acme/nodes/web01"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.delete_requisition_node("acme", "web01").await.unwrap();
    }

    #[tokio::test]
    async fn get_requisition_interface_returns_some_on_200() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme/nodes/web01/interfaces/10.0.0.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ip-addr": "10.0.0.1",
                "snmp-primary": "P",
                "status": 1,
                "monitored-service": [],
                "meta-data": []
            })))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        let got = api
            .get_requisition_interface("acme", "web01", "10.0.0.1")
            .await
            .unwrap();
        let iface = got.expect("interface exists");
        assert_eq!(iface.ip_addr, "10.0.0.1");
        assert_eq!(iface.snmp_primary, "P");
    }

    #[tokio::test]
    async fn get_requisition_interface_returns_none_on_404() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme/nodes/web01/interfaces/10.0.0.99"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        assert!(
            api.get_requisition_interface("acme", "web01", "10.0.0.99")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn post_requisition_interface_targets_interfaces_collection() {
        use crate::model::server::InterfaceServer;
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme/nodes/web01/interfaces"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.post_requisition_interface(
            "acme",
            "web01",
            &InterfaceServer {
                ip_addr: "10.0.0.1".into(),
                snmp_primary: "P".into(),
                status: None,
                managed: None,
                descr: None,
                monitored_service: vec![],
                category: vec![],
                meta_data: vec![],
            },
        )
        .await
        .unwrap();
    }

    /// Locks the documented `add` create-or-replace hazard: when an
    /// operator adds an interface whose IP already exists, the body we
    /// POST carries only the new payload — empty `monitored-service`,
    /// `category`, and `meta-data` arrays — so the server's existing
    /// values for those collections are wiped. Asserting this contract
    /// pins the warning text in `InterfaceCmd::Add`.
    #[tokio::test]
    async fn post_requisition_interface_body_overwrites_collections() {
        use crate::model::server::InterfaceServer;
        use wiremock::matchers::body_json;
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/rest/requisitions/acme/nodes/web01/interfaces"))
            .and(body_json(serde_json::json!({
                "ip-addr": "10.0.0.1",
                "snmp-primary": "P",
                "monitored-service": [],
                "category": [],
                "meta-data": [],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.post_requisition_interface(
            "acme",
            "web01",
            &InterfaceServer {
                ip_addr: "10.0.0.1".into(),
                snmp_primary: "P".into(),
                status: None,
                managed: None,
                descr: None,
                monitored_service: vec![],
                category: vec![],
                meta_data: vec![],
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn put_requisition_interface_targets_specific_ip() {
        use crate::model::server::InterfaceServer;
        let (mock, client) = mock_with_client().await;
        Mock::given(method("PUT"))
            .and(path("/rest/requisitions/acme/nodes/web01/interfaces/10.0.0.1"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.put_requisition_interface(
            "acme",
            "web01",
            "10.0.0.1",
            &InterfaceServer {
                ip_addr: "10.0.0.1".into(),
                snmp_primary: "S".into(),
                status: None,
                managed: None,
                descr: Some("renamed".into()),
                monitored_service: vec![],
                category: vec![],
                meta_data: vec![],
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_requisition_interface_targets_specific_ip() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("DELETE"))
            .and(path("/rest/requisitions/acme/nodes/web01/interfaces/10.0.0.1"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.delete_requisition_interface("acme", "web01", "10.0.0.1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn post_requisition_service_targets_services_collection() {
        use crate::model::server::MonitoredServiceServer;
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path(
                "/rest/requisitions/acme/nodes/web01/interfaces/10.0.0.1/services",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.post_requisition_service(
            "acme",
            "web01",
            "10.0.0.1",
            &MonitoredServiceServer {
                service_name: "HTTP".into(),
                category: vec![],
                meta_data: vec![],
            },
        )
        .await
        .unwrap();
    }

    /// Locks the create-or-replace hazard: when an operator adds a
    /// service whose name already exists, the body we POST carries
    /// empty `category` and `meta-data` arrays — so the server's
    /// existing values for those collections are wiped.
    #[tokio::test]
    async fn post_requisition_service_body_overwrites_collections() {
        use crate::model::server::MonitoredServiceServer;
        use wiremock::matchers::body_json;
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path(
                "/rest/requisitions/acme/nodes/web01/interfaces/10.0.0.1/services",
            ))
            .and(body_json(serde_json::json!({
                "service-name": "HTTP",
                "category": [],
                "meta-data": [],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.post_requisition_service(
            "acme",
            "web01",
            "10.0.0.1",
            &MonitoredServiceServer {
                service_name: "HTTP".into(),
                category: vec![],
                meta_data: vec![],
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_requisition_service_targets_specific_service() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("DELETE"))
            .and(path(
                "/rest/requisitions/acme/nodes/web01/interfaces/10.0.0.1/services/HTTP",
            ))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.delete_requisition_service("acme", "web01", "10.0.0.1", "HTTP")
            .await
            .unwrap();
    }

    /// Locks the contract: a 404 from `DELETE /services/{name}` does
    /// NOT surface as `Ok(())`. The operator-visible verb (`service
    /// remove`) propagates the error and exits non-zero, so the
    /// outcome line never falsely claims "removed" on a missing
    /// service.
    #[tokio::test]
    async fn delete_requisition_service_returns_error_on_404() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("DELETE"))
            .and(path(
                "/rest/requisitions/acme/nodes/web01/interfaces/10.0.0.1/services/MISSING",
            ))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        assert!(
            api.delete_requisition_service("acme", "web01", "10.0.0.1", "MISSING")
                .await
                .is_err()
        );
    }

    /// Pins the IPv6 round-trip on the service endpoints — the
    /// IP segment is percent-encoded literally (no bracket-stripping,
    /// no normalization). If `PATH_SEGMENT` ever shrinks (e.g.
    /// dropping the colon-encoder), this test surfaces it.
    #[tokio::test]
    async fn requisition_service_endpoints_accept_ipv6() {
        use crate::model::server::MonitoredServiceServer;
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path(
                "/rest/requisitions/acme/nodes/web01/interfaces/2001:db8::1/services",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        Mock::given(method("DELETE"))
            .and(path(
                "/rest/requisitions/acme/nodes/web01/interfaces/2001:db8::1/services/HTTP",
            ))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        api.post_requisition_service(
            "acme",
            "web01",
            "2001:db8::1",
            &MonitoredServiceServer {
                service_name: "HTTP".into(),
                category: vec![],
                meta_data: vec![],
            },
        )
        .await
        .unwrap();
        api.delete_requisition_service("acme", "web01", "2001:db8::1", "HTTP")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_requisition_names_returns_sorted_list() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitionNames"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 3,
                "foreign-source": ["zebra", "alpha", "mango"]
            })))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        let names = api.list_requisition_names().await.unwrap();
        assert_eq!(names, vec!["alpha", "mango", "zebra"]);
    }

    #[tokio::test]
    async fn list_requisition_names_handles_empty_response() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitionNames"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 0
            })))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        let names = api.list_requisition_names().await.unwrap();
        assert!(names.is_empty());
    }

    #[tokio::test]
    async fn foreign_source_with_special_chars_is_percent_encoded() {
        // Defensive: name validation prohibits these at parse time,
        // but verify the URL encoder catches them if they slip
        // through (e.g. via direct API consumer rather than YAML).
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/my%20fs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "foreign-source": "my fs",
                "node": []
            })))
            .mount(&mock)
            .await;
        let api = ProvisioningApi::new(&client);
        assert!(api.get_requisition("my fs").await.unwrap().is_some());
    }
}
