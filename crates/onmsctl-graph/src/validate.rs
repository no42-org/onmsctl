/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Offline validation of one `kind: Graph` document, run before any HTTP
//! request. Most rules catch uploads the server accepts with `201` but never
//! loads (design G8, G9) or rejects with an opaque `500` (G7).

use std::collections::{BTreeMap, HashMap, HashSet};

use onmsctl_core::{Error, Result};

use crate::model::{API_VERSION, FocusStrategy, GraphLocal, Layer};

/// Validate `doc`, reporting every problem at once. Each problem is prefixed
/// with `Graph <name>:`; problems are joined with `; `.
pub fn validate(doc: &GraphLocal) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    let spec = &doc.spec;

    if doc.api_version != API_VERSION {
        problems.push(format!("apiVersion must be {API_VERSION}"));
    }
    if !is_path_safe(&doc.metadata.name) {
        problems.push("name must match [A-Za-z0-9._-]+".into());
    }
    if spec.layers.is_empty() {
        problems.push("spec.layers must not be empty".into());
    }

    // Namespaces: required and unique within the document.
    let mut ns_first: HashMap<&str, usize> = HashMap::new();
    for (i, layer) in spec.layers.iter().enumerate() {
        if layer.namespace.is_empty() {
            problems.push(format!("layer {i}: namespace is required"));
        } else if let Some(first) = ns_first.insert(&layer.namespace, i) {
            ns_first.insert(&layer.namespace, first);
            problems.push(format!(
                "namespace {} is declared twice (layers {first}, {i})",
                layer.namespace
            ));
        }
    }

    // Vertex ids are document-wide in GraphML.
    let mut vertex_layer: BTreeMap<&str, &str> = BTreeMap::new();
    for layer in &spec.layers {
        let ns = layer_name(layer);
        for v in &layer.vertices {
            if v.id.is_empty() {
                problems.push(format!("layer {ns}: vertex id must not be empty"));
                continue;
            }
            if let Some(first) = vertex_layer.get(v.id.as_str()) {
                problems.push(format!(
                    "vertex id {} is declared twice (layers {first}, {ns})",
                    v.id
                ));
            } else {
                vertex_layer.insert(&v.id, ns);
            }
            if let Some(node) = &v.node
                && (node.foreign_source.is_empty() || node.foreign_id.is_empty())
            {
                problems.push(format!(
                    "vertex {}: foreignSource and foreignId must both be non-empty",
                    v.id
                ));
            }
        }
    }

    let mut edge_ids: HashSet<String> = HashSet::new();
    for layer in &spec.layers {
        let ns = layer_name(layer);
        let own: HashSet<&str> = layer.vertices.iter().map(|v| v.id.as_str()).collect();

        for e in &layer.edges {
            let id = e.effective_id();
            if !own.contains(e.source.as_str()) {
                problems.push(format!(
                    "edge {id}: source {} is not a vertex of layer {ns}",
                    e.source
                ));
            }
            if !vertex_layer.contains_key(e.target.as_str()) {
                problems.push(format!("edge {id}: unknown target {}", e.target));
            }
            if !edge_ids.insert(id.clone()) {
                problems.push(format!(
                    "edge id {id} is declared twice; set an explicit id"
                ));
            }
        }

        if let Some(focus) = &layer.focus {
            match (focus.strategy, focus.vertices.is_empty()) {
                (FocusStrategy::Specific, true) => problems.push(format!(
                    "layer {ns}: strategy SPECIFIC requires focus.vertices"
                )),
                (FocusStrategy::Specific, false) => {}
                (_, false) => problems.push(format!(
                    "layer {ns}: focus.vertices requires strategy SPECIFIC"
                )),
                (_, true) => {}
            }
            for v in &focus.vertices {
                // `focus-ids` is a comma-joined list on the wire.
                if v.contains(',') {
                    problems.push(format!("focus vertex {v} must not contain a comma"));
                }
                if !own.contains(v.as_str()) {
                    problems.push(format!("focus vertex {v} is not in layer {ns}"));
                }
            }
        }
    }

    if problems.is_empty() {
        return Ok(());
    }
    let prefix = format!("Graph {}: ", doc.metadata.name);
    Err(Error::Config(
        problems
            .iter()
            .map(|p| format!("{prefix}{p}"))
            .collect::<Vec<_>>()
            .join("; "),
    ))
}

/// `[A-Za-z0-9._-]+`, excluding `.` and `..`: the name is a URL path segment.
pub fn is_path_safe(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// How a layer is named in messages: its namespace, or `<unnamed>`.
fn layer_name(layer: &Layer) -> &str {
    if layer.namespace.is_empty() {
        "<unnamed>"
    } else {
        &layer.namespace
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(spec: &str) -> GraphLocal {
        let yaml = format!(
            "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {{name: g}}\nspec:\n{spec}"
        );
        serde_norway::from_str(&yaml).unwrap()
    }

    fn err(d: &GraphLocal) -> String {
        match validate(d) {
            Ok(()) => panic!("expected a validation error"),
            Err(Error::Config(m)) => m,
            Err(e) => panic!("unexpected error type {e:?}"),
        }
    }

    const TWO_LAYERS: &str = "  layers:
    - namespace: wan:regions
      focus: {strategy: SPECIFIC, vertices: [eu]}
      vertices: [{id: eu}, {id: us}]
      edges:
        - {source: eu, target: us}
        - {source: eu, target: fra}
    - namespace: wan:sites
      vertices:
        - {id: fra, node: {foreignSource: dc, foreignId: fra-01}}
";

    #[test]
    fn valid_multi_layer_document_passes() {
        validate(&doc(TWO_LAYERS)).unwrap();
        let fixture: GraphLocal =
            serde_norway::from_str(include_str!("../tests/fixtures/probe-two-layer.yaml")).unwrap();
        validate(&fixture).unwrap();
    }

    #[test]
    fn wrong_api_version_is_rejected() {
        let mut d = doc(TWO_LAYERS);
        d.api_version = "v1".into();
        assert!(err(&d).contains("apiVersion must be graphml.opennms.org/v1"));
    }

    #[test]
    fn name_must_be_path_safe() {
        for bad in ["", "a/b", "a b", "..", "."] {
            let mut d = doc(TWO_LAYERS);
            d.metadata.name = bad.into();
            assert!(
                err(&d).contains("name must match [A-Za-z0-9._-]+"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn messages_are_prefixed_with_the_document() {
        assert!(err(&doc("  layers: []\n")).starts_with("Graph g: "));
    }

    #[test]
    fn empty_layers_are_rejected() {
        assert!(err(&doc("  layers: []\n")).contains("spec.layers must not be empty"));
    }

    #[test]
    fn missing_namespace_is_rejected() {
        let m = err(&doc("  layers:\n    - namespace: \"\"\n"));
        assert!(m.contains("Graph g: layer 0: namespace is required"), "{m}");
    }

    #[test]
    fn duplicate_namespace_is_rejected() {
        let m = err(&doc("  layers:\n    - namespace: n\n    - namespace: n\n"));
        assert!(
            m.contains("namespace n is declared twice (layers 0, 1)"),
            "{m}"
        );
    }

    #[test]
    fn duplicate_vertex_across_layers_is_rejected() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      vertices: [{id: v}]\n    - namespace: b\n      vertices: [{id: v}]\n",
        ));
        assert!(
            m.contains("vertex id v is declared twice (layers a, b)"),
            "{m}"
        );
    }

    #[test]
    fn empty_vertex_id_is_rejected() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      vertices: [{id: \"\"}]\n",
        ));
        assert!(m.contains("layer a: vertex id must not be empty"), "{m}");
    }

    #[test]
    fn edge_source_must_be_in_the_declaring_layer() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      vertices: [{id: x}]\n\
             \x20   - namespace: b\n      vertices: [{id: y}]\n      edges: [{source: x, target: y}]\n",
        ));
        assert!(
            m.contains("edge x-y: source x is not a vertex of layer b"),
            "{m}"
        );
    }

    #[test]
    fn dangling_edge_target_is_rejected() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      vertices: [{id: x}]\n      edges: [{source: x, target: ghost}]\n",
        ));
        assert!(m.contains("edge x-ghost: unknown target ghost"), "{m}");
    }

    #[test]
    fn colliding_defaulted_edge_ids_are_rejected() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      vertices: [{id: a}, {id: b}]\n\
             \x20     edges: [{source: a, target: b}, {source: a, target: b}]\n",
        ));
        assert!(
            m.contains("edge id a-b is declared twice; set an explicit id"),
            "{m}"
        );
    }

    #[test]
    fn explicit_edge_ids_resolve_the_collision() {
        validate(&doc(
            "  layers:\n    - namespace: a\n      vertices: [{id: a}, {id: b}]\n\
             \x20     edges: [{source: a, target: b}, {id: a-b-2, source: a, target: b}]\n",
        ))
        .unwrap();
    }

    #[test]
    fn focus_vertices_require_specific() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      focus: {strategy: ALL, vertices: [x]}\n      vertices: [{id: x}]\n",
        ));
        assert!(
            m.contains("layer a: focus.vertices requires strategy SPECIFIC"),
            "{m}"
        );
    }

    #[test]
    fn specific_requires_focus_vertices() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      focus: {strategy: SPECIFIC}\n      vertices: [{id: x}]\n",
        ));
        assert!(
            m.contains("layer a: strategy SPECIFIC requires focus.vertices"),
            "{m}"
        );
    }

    #[test]
    fn focus_vertex_must_be_in_its_layer() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      focus: {strategy: SPECIFIC, vertices: [y]}\n      vertices: [{id: x}]\n\
             \x20   - namespace: b\n      vertices: [{id: y}]\n",
        ));
        assert!(m.contains("focus vertex y is not in layer a"), "{m}");
    }

    #[test]
    fn focus_vertex_must_not_contain_a_comma() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      focus: {strategy: SPECIFIC, vertices: [\"x,y\"]}\n      vertices: [{id: \"x,y\"}]\n",
        ));
        assert!(
            m.contains("focus vertex x,y must not contain a comma"),
            "{m}"
        );
    }

    #[test]
    fn incomplete_node_reference_is_rejected() {
        let m = err(&doc(
            "  layers:\n    - namespace: a\n      vertices: [{id: x, node: {foreignSource: fs, foreignId: \"\"}}]\n",
        ));
        assert!(
            m.contains("vertex x: foreignSource and foreignId must both be non-empty"),
            "{m}"
        );
    }

    #[test]
    fn every_problem_is_reported() {
        let m = err(&doc(
            "  layers:\n    - namespace: \"\"\n    - namespace: b\n      focus: {strategy: SPECIFIC}\n",
        ));
        assert!(
            m.contains("namespace is required") && m.contains("requires focus.vertices"),
            "{m}"
        );
    }
}
