/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local (YAML) model for `kind: Graph`: one GraphML upload.
//!
//! `metadata.name` is both the `rest/graphml/{name}` key and the v2
//! `containerId` (design D1, D6). `spec.layers` are the GraphML `<graph>`
//! elements in order. The same [`GraphSpec`] is produced from YAML and from
//! stored GraphML ([`crate::graphml::parse`]), so plan compares structs (D2).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only accepted `apiVersion`.
pub const API_VERSION: &str = "graphml.opennms.org/v1";
/// The only accepted `kind`.
pub const KIND: &str = "Graph";

/// A `kind: Graph` document.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GraphLocal {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: GraphSpec,
}

/// Document metadata. `name` is the upload name and the v2 container id.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
}

/// The GraphML document body: container-level attributes and the layers.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphSpec {
    /// Container label, shown in the topology menu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Breadcrumb navigation between layers. Written at document level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breadcrumb_strategy: Option<BreadcrumbStrategy>,
    /// The graphs of this container, in order. Layer 0 is the top.
    #[serde(default)]
    pub layers: Vec<Layer>,
}

/// One GraphML `<graph>`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Layer {
    /// Required, and unique across the server. A graph without one, or with
    /// one another upload already uses, is stored but never loads.
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Layout the topology UI starts with (server default `D3`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_layout: Option<String>,
    /// Default semantic zoom level (server default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_zoom_level: Option<u32>,
    /// Default focus. Without one the server reports `EMPTY`, so the layer's
    /// default view shows no vertices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<Focus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertex_status_provider: Option<VertexStatusProvider>,
    /// Spacing between parallel edges (server default 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_path_offset: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vertices: Vec<Vertex>,
    /// Edges whose source vertex is in this layer. The target may be in any layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
}

/// A layer's default focus.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Focus {
    pub strategy: FocusStrategy,
    /// Vertex ids in this layer. Required for, and only allowed with, `SPECIFIC`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vertices: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum FocusStrategy {
    Empty,
    All,
    First,
    Specific,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VertexStatusProvider {
    /// Worst unacknowledged alarm of the vertex's node.
    Default,
    /// Groovy scripts in `etc/graphml-vertex-status`.
    Script,
    /// Worst status of connected vertices.
    Propagate,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BreadcrumbStrategy {
    None,
    ShortestPathToRoot,
}

/// A GraphML `<node>`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Vertex {
    /// Unique across all layers of the document.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Topology icon (server default `generic`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
    /// Layout level (server default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    /// The OpenNMS node this vertex shows status for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeRef>,
}

/// A node by foreign reference, passed through to GraphML unchanged.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeRef {
    pub foreign_source: String,
    pub foreign_id: String,
}

/// A GraphML `<edge>`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    /// Defaults to `<source>-<target>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// A vertex of the layer that declares the edge.
    pub source: String,
    /// A vertex of any layer.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
}

impl Edge {
    /// The id written to GraphML: the explicit one, or `<source>-<target>`.
    pub fn effective_id(&self) -> String {
        self.id
            .clone()
            .unwrap_or_else(|| format!("{}-{}", self.source, self.target))
    }
}

impl GraphSpec {
    /// The form plan compares and `export` prints: an edge id equal to its
    /// `<source>-<target>` default is dropped, so a spelled-out default and an
    /// omitted id are the same graph.
    pub fn canonical(&self) -> GraphSpec {
        let mut spec = self.clone();
        for edge in spec.layers.iter_mut().flat_map(|l| l.edges.iter_mut()) {
            if edge.id.as_deref() == Some(&format!("{}-{}", edge.source, edge.target)) {
                edge.id = None;
            }
        }
        spec
    }
}

impl GraphLocal {
    /// Deserialize one already-split YAML document.
    pub fn from_value(value: serde_norway::Value) -> Result<Self, serde_norway::Error> {
        serde_norway::from_value(value)
    }

    /// Wrap a spec in a document envelope (for `export`/`convert`). The spec is
    /// stored in canonical form, so default edge ids are omitted.
    pub fn new(name: impl Into<String>, spec: &GraphSpec) -> Self {
        Self {
            api_version: API_VERSION.into(),
            kind: KIND.into(),
            metadata: Metadata { name: name.into() },
            spec: spec.canonical(),
        }
    }

    /// The document as apply-ready YAML.
    pub fn to_yaml(&self) -> String {
        serde_norway::to_string(self).expect("the Graph model always serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Result<GraphLocal, serde_norway::Error> {
        serde_norway::from_str(yaml)
    }

    #[test]
    fn probe_fixture_parses() {
        let doc = parse(include_str!("../tests/fixtures/probe-two-layer.yaml")).unwrap();
        assert_eq!(doc.metadata.name, "wan-overview");
        assert_eq!(
            doc.spec.breadcrumb_strategy,
            Some(BreadcrumbStrategy::ShortestPathToRoot)
        );
        let regions = &doc.spec.layers[0];
        assert_eq!(regions.semantic_zoom_level, Some(2));
        assert_eq!(
            regions.focus.as_ref().unwrap().strategy,
            FocusStrategy::Specific
        );
        assert_eq!(
            regions.vertex_status_provider,
            Some(VertexStatusProvider::Propagate)
        );
        assert_eq!(regions.edges[0].effective_id(), "eu-us");
        assert_eq!(regions.edges[1].effective_id(), "eu-to-fra");
        let fra = &doc.spec.layers[1].vertices[0];
        assert_eq!(fra.node.as_ref().unwrap().foreign_id, "fra-core-01");
    }

    #[test]
    fn unknown_vertex_key_is_rejected() {
        let err = parse(
            "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {name: g}\n\
             spec:\n  layers:\n    - namespace: n\n      vertices:\n        - {id: a, nodeID: 3}\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("nodeID"), "{err}");
    }

    #[test]
    fn negative_numbers_are_rejected() {
        for field in ["semanticZoomLevel", "edgePathOffset"] {
            let yaml = format!(
                "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {{name: g}}\n\
                 spec:\n  layers:\n    - namespace: n\n      {field}: -1\n"
            );
            assert!(parse(&yaml).is_err(), "{field} -1 must be rejected");
        }
    }

    #[test]
    fn unknown_enum_value_is_rejected() {
        let err = parse(
            "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {name: g}\n\
             spec:\n  layers:\n    - namespace: n\n      vertexStatusProvider: \"true\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("true"), "{err}");
    }

    #[test]
    fn export_omits_default_edge_ids_and_reparses() {
        let parsed =
            crate::graphml::parse(include_str!("../tests/fixtures/legacy-campus.xml")).unwrap();
        let yaml = GraphLocal::new("campus", &parsed.spec).to_yaml();
        let back = parse(&yaml).unwrap();
        crate::validate::validate(&back).unwrap();
        let ids: Vec<&str> = back.spec.layers[0]
            .edges
            .iter()
            .filter_map(|e| e.id.as_deref())
            .collect();
        assert_eq!(ids, vec!["uplink-a", "dmz-web"], "{yaml}");
        assert!(
            yaml.starts_with("apiVersion: graphml.opennms.org/v1\nkind: Graph\n"),
            "{yaml}"
        );
    }

    #[test]
    fn canonical_drops_spelled_out_default_ids() {
        let edge = |id: &str| Edge {
            id: Some(id.into()),
            source: "a".into(),
            target: "b".into(),
            tooltip: None,
        };
        let spec = GraphSpec {
            layers: vec![Layer {
                namespace: "n".into(),
                edges: vec![edge("a-b"), edge("custom")],
                ..Layer::default()
            }],
            ..GraphSpec::default()
        };
        let c = spec.canonical();
        assert_eq!(c.layers[0].edges[0].id, None);
        assert_eq!(c.layers[0].edges[1].id.as_deref(), Some("custom"));
    }

    #[test]
    fn published_example_parses_and_validates() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/graph.yaml");
        let doc = parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        crate::validate::validate(&doc).unwrap();
        assert_eq!(doc.spec.layers.len(), 2);
    }

    #[test]
    fn missing_namespace_is_a_parse_error_naming_the_layer() {
        let err = parse(
            "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {name: g}\n\
             spec:\n  layers:\n    - label: no namespace\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("layers[0]") && err.contains("namespace"),
            "{err}"
        );
    }
}
