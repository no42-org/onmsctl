/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! REST wrappers.
//!
//! [`GraphmlApi`] writes and reads uploads at `rest/graphml/{name}`: `GET`
//! returns the stored XML (G2), `POST` creates (`201`, G1), `DELETE` removes
//! (G5). There is no list endpoint and no PUT. [`GraphApi`] reads the v2
//! `api/v2/graphs` API: containers, views, search, plus the node lookup used to
//! warn about unresolved vertex references (D4).

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use onmsctl_core::{Error, OnmsClient, Result};

const GRAPHML: &str = "rest/graphml";
const GRAPHS: &str = "api/v2/graphs";

/// Percent-encode characters unsafe in a single path segment, leaving `.`, `-`
/// and `:` intact (namespaces such as `wan:sites`, `fs:fid` node criteria).
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

/// `404`: the upload (or node, or container) does not exist (G5).
pub fn is_not_found(e: &Error) -> bool {
    matches!(e, Error::HttpStatus { status: 404, .. })
}

/// `POST` to a name that already has an upload: `500` "already exists" (G4).
pub fn is_already_exists(e: &Error) -> bool {
    matches!(e, Error::HttpStatus { status: 500, body, .. } if body.contains("already exists"))
}

/// `rest/graphml/{name}`.
pub struct GraphmlApi<'a> {
    client: &'a OnmsClient,
}

impl<'a> GraphmlApi<'a> {
    pub fn new(client: &'a OnmsClient) -> Self {
        Self { client }
    }

    fn path(name: &str) -> String {
        format!("{GRAPHML}/{}", enc(name))
    }

    /// The stored GraphML, or `None` when there is no upload under `name`.
    pub async fn get_xml(&self, name: &str) -> Result<Option<String>> {
        match self.client.get_bytes(&Self::path(name)).await {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|e| {
                Error::Config(format!(
                    "GET {}: response is not UTF-8: {e}",
                    Self::path(name)
                ))
            }),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Upload `xml` under `name`. Fails with [`is_already_exists`] if taken.
    pub async fn create(&self, name: &str, xml: String) -> Result<()> {
        self.client.post_xml(&Self::path(name), xml).await
    }

    /// Remove the upload. Fails with [`is_not_found`] if there is none.
    pub async fn delete(&self, name: &str) -> Result<()> {
        self.client.delete::<()>(&Self::path(name), None).await
    }
}

/// One container of `GET api/v2/graphs`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContainerInfo {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub graphs: Vec<GraphInfo>,
}

/// One graph of a container listing.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GraphInfo {
    pub namespace: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// Body of the v2 view `POST`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_zoom_level: Option<u32>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    vertices_in_focus: &'a [String],
}

/// `api/v2/graphs` and the node lookup.
pub struct GraphApi<'a> {
    client: &'a OnmsClient,
}

impl<'a> GraphApi<'a> {
    pub fn new(client: &'a OnmsClient) -> Self {
        Self { client }
    }

    /// Every registered container with its graph namespaces.
    pub async fn containers(&self) -> Result<Vec<ContainerInfo>> {
        self.client.get(GRAPHS, &[]).await
    }

    /// One container with its graphs, vertices and edges, or `None` on 404.
    pub async fn container(&self, id: &str) -> Result<Option<serde_json::Value>> {
        match self.client.get(&format!("{GRAPHS}/{}", enc(id)), &[]).await {
            Ok(v) => Ok(Some(v)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// A graph view for a focus and semantic zoom level. Both are optional;
    /// the server then uses the graph's default focus and level 1.
    pub async fn view(
        &self,
        container: &str,
        namespace: &str,
        focus: &[String],
        semantic_zoom_level: Option<u32>,
    ) -> Result<serde_json::Value> {
        let body = ViewRequest {
            semantic_zoom_level,
            vertices_in_focus: focus,
        };
        self.client
            .post(
                &format!("{GRAPHS}/{}/{}", enc(container), enc(namespace)),
                &body,
            )
            .await
    }

    /// Search suggestions within one namespace.
    pub async fn search(&self, namespace: &str, text: &str) -> Result<serde_json::Value> {
        self.client
            .get(
                &format!("{GRAPHS}/search/suggestions/{}", enc(namespace)),
                &[("s", text)],
            )
            .await
    }

    /// Whether `GET rest/nodes/{foreignSource}:{foreignId}` finds a node.
    pub async fn node_exists(&self, foreign_source: &str, foreign_id: &str) -> Result<bool> {
        let path = format!("rest/nodes/{}:{}", enc(foreign_source), enc(foreign_id));
        match self.client.get::<serde_json::Value>(&path, &[]).await {
            Ok(_) => Ok(true),
            Err(e) if is_not_found(&e) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::{AuthCreds, Url};
    use wiremock::matchers::{body_json, body_string, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> OnmsClient {
        OnmsClient::from_parts(
            Url::parse(&format!("{}/", server.uri())).unwrap(),
            AuthCreds::bearer("t"),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn get_xml_returns_body_or_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/graphml/wan"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<graphml/>"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/graphml/nope"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let c = client(&server);
        let api = GraphmlApi::new(&c);
        assert_eq!(
            api.get_xml("wan").await.unwrap().as_deref(),
            Some("<graphml/>")
        );
        assert_eq!(api.get_xml("nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn create_posts_xml_and_maps_already_exists() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/rest/graphml/wan"))
            .and(header("content-type", "application/xml"))
            .and(body_string("<graphml/>"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/graphml/taken"))
            .respond_with(
                ResponseTemplate::new(500).set_body_string("Graph with name taken already exists"),
            )
            .mount(&server)
            .await;
        let c = client(&server);
        let api = GraphmlApi::new(&c);
        api.create("wan", "<graphml/>".into()).await.unwrap();
        let err = api.create("taken", "<graphml/>".into()).await.unwrap_err();
        assert!(is_already_exists(&err), "{err:?}");
        assert!(!is_not_found(&err));
    }

    #[tokio::test]
    async fn delete_maps_not_found_and_other_errors() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/rest/graphml/wan"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/rest/graphml/gone"))
            .respond_with(
                ResponseTemplate::new(404).set_body_string("No GraphML file found with name  gone"),
            )
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/rest/graphml/boom"))
            .respond_with(ResponseTemplate::new(500).set_body_string("kaput"))
            .mount(&server)
            .await;
        let c = client(&server);
        let api = GraphmlApi::new(&c);
        api.delete("wan").await.unwrap();
        assert!(is_not_found(&api.delete("gone").await.unwrap_err()));
        let err = api.delete("boom").await.unwrap_err();
        assert!(!is_not_found(&err) && !is_already_exists(&err));
        assert!(err.to_string().contains("500"), "{err}");
    }

    #[tokio::test]
    async fn names_are_encoded_as_one_path_segment() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/graphml/a%2Fb"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server);
        assert_eq!(GraphmlApi::new(&c).get_xml("a/b").await.unwrap(), None);
    }

    #[tokio::test]
    async fn containers_and_container() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": "wan", "label": "WAN", "description": "d",
                 "graphs": [{"namespace": "wan:regions", "label": "Regions"}]},
                {"id": "bsm", "label": "BSM", "description": null, "graphs": []}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs/wan"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "wan"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs/nope"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let c = client(&server);
        let api = GraphApi::new(&c);
        let list = api.containers().await.unwrap();
        assert_eq!(list[0].graphs[0].namespace, "wan:regions");
        assert_eq!(list[1].description, None);
        assert!(api.container("wan").await.unwrap().is_some());
        assert!(api.container("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn view_posts_focus_and_level() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/graphs/wan/wan:regions"))
            .and(body_json(serde_json::json!({
                "semanticZoomLevel": 2, "verticesInFocus": ["eu"]
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"vertices": []})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/graphs/wan/wan:sites"))
            .and(body_json(serde_json::json!({})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server);
        let api = GraphApi::new(&c);
        api.view("wan", "wan:regions", &["eu".into()], Some(2))
            .await
            .unwrap();
        api.view("wan", "wan:sites", &[], None).await.unwrap();
    }

    #[tokio::test]
    async fn search_sends_the_text_as_s() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs/search/suggestions/wan:sites"))
            .and(query_param("s", "fra"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server);
        GraphApi::new(&c).search("wan:sites", "fra").await.unwrap();
    }

    #[tokio::test]
    async fn node_exists_maps_404_to_false() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/nodes/dc:fra-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "3"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/nodes/dc:ghost"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let c = client(&server);
        let api = GraphApi::new(&c);
        assert!(api.node_exists("dc", "fra-01").await.unwrap());
        assert!(!api.node_exists("dc", "ghost").await.unwrap());
    }
}
