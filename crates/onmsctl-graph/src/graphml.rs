/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! GraphML writer and parser for the `Graph` model.
//!
//! [`render`] turns a [`GraphSpec`] into the GraphML that `rest/graphml/{name}`
//! stores. It always writes `containerId` (design D6), `breadcrumb-strategy` at
//! document level (D8), one `<graph>` per layer with `edgedefault="undirected"`
//! and the namespace as its id, and only the `<key>` elements in use.
//!
//! [`parse`] is its inverse for anything [`render`] wrote, and also reads
//! GraphML written by hand (D5, D7, D8): edges move to their source's layer, a
//! graph-level `breadcrumb-strategy` lifts to the spec, the legacy
//! `vertex-status-provider` value `true` maps to `default`, and keys the model
//! does not know are dropped. Each such step adds a warning.

use std::collections::HashMap;

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesDecl, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::{Writer, XmlVersion};

use onmsctl_core::{Error, Result};

use crate::model::{
    BreadcrumbStrategy, Edge, Focus, FocusStrategy, GraphSpec, Layer, NodeRef, Vertex,
    VertexStatusProvider,
};

const GRAPHML_NS: &str = "http://graphml.graphdrawing.org/xmlns";

/// Every key the writer can emit, in declaration order: (name, for, type).
const KEYS: &[(&str, &str, &str)] = &[
    ("containerId", "graphml", "string"),
    ("label", "all", "string"),
    ("description", "all", "string"),
    ("breadcrumb-strategy", "graphml", "string"),
    ("namespace", "graph", "string"),
    ("preferred-layout", "graph", "string"),
    ("semantic-zoom-level", "graph", "int"),
    ("focus-strategy", "graph", "string"),
    ("focus-ids", "graph", "string"),
    ("vertex-status-provider", "graph", "string"),
    ("edge-path-offset", "graph", "int"),
    ("iconKey", "node", "string"),
    ("tooltipText", "all", "string"),
    ("level", "node", "int"),
    ("foreignSource", "node", "string"),
    ("foreignID", "node", "string"),
];

// -- writer -------------------------------------------------------------------

/// `<data>` entries of one element, in output order.
type DataList = Vec<(&'static str, String)>;

fn push(d: &mut DataList, key: &'static str, v: Option<impl ToString>) {
    if let Some(v) = v {
        d.push((key, v.to_string()));
    }
}

fn doc_data(name: &str, spec: &GraphSpec) -> DataList {
    let mut d = vec![("containerId", name.to_string())];
    push(&mut d, "label", spec.label.as_ref());
    push(&mut d, "description", spec.description.as_ref());
    push(
        &mut d,
        "breadcrumb-strategy",
        spec.breadcrumb_strategy.map(breadcrumb_str),
    );
    d
}

fn layer_data(l: &Layer) -> DataList {
    let mut d = vec![("namespace", l.namespace.clone())];
    push(&mut d, "label", l.label.as_ref());
    push(&mut d, "description", l.description.as_ref());
    push(&mut d, "preferred-layout", l.preferred_layout.as_ref());
    push(&mut d, "semantic-zoom-level", l.semantic_zoom_level);
    if let Some(f) = &l.focus {
        d.push(("focus-strategy", focus_str(f.strategy).into()));
        if !f.vertices.is_empty() {
            d.push(("focus-ids", f.vertices.join(",")));
        }
    }
    push(
        &mut d,
        "vertex-status-provider",
        l.vertex_status_provider.map(status_provider_str),
    );
    push(&mut d, "edge-path-offset", l.edge_path_offset);
    d
}

fn vertex_data(v: &Vertex) -> DataList {
    let mut d = Vec::new();
    push(&mut d, "label", v.label.as_ref());
    push(&mut d, "iconKey", v.icon_key.as_ref());
    push(&mut d, "tooltipText", v.tooltip.as_ref());
    push(&mut d, "level", v.level);
    if let Some(n) = &v.node {
        d.push(("foreignSource", n.foreign_source.clone()));
        d.push(("foreignID", n.foreign_id.clone()));
    }
    d
}

fn edge_data(e: &Edge) -> DataList {
    let mut d = Vec::new();
    push(&mut d, "tooltipText", e.tooltip.as_ref());
    d
}

/// Render `spec` as the GraphML document stored under `name`.
pub fn render(name: &str, spec: &GraphSpec) -> String {
    let doc = doc_data(name, spec);
    let layers: Vec<(DataList, Vec<DataList>, Vec<DataList>)> = spec
        .layers
        .iter()
        .map(|l| {
            (
                layer_data(l),
                l.vertices.iter().map(vertex_data).collect(),
                l.edges.iter().map(edge_data).collect(),
            )
        })
        .collect();

    let mut used: Vec<&str> = doc.iter().map(|(k, _)| *k).collect();
    for (ld, vs, es) in &layers {
        used.extend(ld.iter().map(|(k, _)| *k));
        used.extend(vs.iter().flatten().map(|(k, _)| *k));
        used.extend(es.iter().flatten().map(|(k, _)| *k));
    }

    let mut w = Writer::new_with_indent(Vec::new(), b' ', 2);
    // Writing into a Vec cannot fail; the io::Result plumbing is quick-xml's.
    let res: std::io::Result<()> = (|| {
        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
        w.create_element("graphml")
            .with_attribute(("xmlns", GRAPHML_NS))
            .write_inner_content(|w| {
                for (key, for_, ty) in KEYS.iter().filter(|(k, _, _)| used.contains(k)) {
                    w.create_element("key")
                        .with_attribute(("id", *key))
                        .with_attribute(("for", *for_))
                        .with_attribute(("attr.name", *key))
                        .with_attribute(("attr.type", *ty))
                        .write_empty()?;
                }
                write_data(w, &doc)?;
                for (layer, (ld, vds, eds)) in spec.layers.iter().zip(&layers) {
                    w.create_element("graph")
                        .with_attribute(("id", layer.namespace.as_str()))
                        .with_attribute(("edgedefault", "undirected"))
                        .write_inner_content(|w| {
                            write_data(w, ld)?;
                            for (v, vd) in layer.vertices.iter().zip(vds) {
                                let el = w
                                    .create_element("node")
                                    .with_attribute(("id", v.id.as_str()));
                                if vd.is_empty() {
                                    el.write_empty()?;
                                } else {
                                    el.write_inner_content(|w| write_data(w, vd))?;
                                }
                            }
                            for (e, ed) in layer.edges.iter().zip(eds) {
                                let id = e.effective_id();
                                let el = w
                                    .create_element("edge")
                                    .with_attribute(("id", id.as_str()))
                                    .with_attribute(("source", e.source.as_str()))
                                    .with_attribute(("target", e.target.as_str()));
                                if ed.is_empty() {
                                    el.write_empty()?;
                                } else {
                                    el.write_inner_content(|w| write_data(w, ed))?;
                                }
                            }
                            Ok(())
                        })?;
                }
                Ok(())
            })?;
        Ok(())
    })();
    res.expect("writing GraphML into memory cannot fail");

    let mut out = String::from_utf8(w.into_inner()).expect("quick-xml writes UTF-8");
    out.push('\n');
    out
}

fn write_data(w: &mut Writer<Vec<u8>>, data: &DataList) -> std::io::Result<()> {
    for (key, value) in data {
        w.create_element("data")
            .with_attribute(("key", *key))
            .write_text_content(BytesText::new(value))?;
    }
    Ok(())
}

fn focus_str(s: FocusStrategy) -> &'static str {
    match s {
        FocusStrategy::Empty => "EMPTY",
        FocusStrategy::All => "ALL",
        FocusStrategy::First => "FIRST",
        FocusStrategy::Specific => "SPECIFIC",
    }
}

fn status_provider_str(p: VertexStatusProvider) -> &'static str {
    match p {
        VertexStatusProvider::Default => "default",
        VertexStatusProvider::Script => "script",
        VertexStatusProvider::Propagate => "propagate",
    }
}

fn breadcrumb_str(b: BreadcrumbStrategy) -> &'static str {
    match b {
        BreadcrumbStrategy::None => "NONE",
        BreadcrumbStrategy::ShortestPathToRoot => "SHORTEST_PATH_TO_ROOT",
    }
}

// -- parser -------------------------------------------------------------------

/// A parsed GraphML document.
#[derive(Debug)]
pub struct Parsed {
    /// The document-level `containerId`, if present.
    pub container_id: Option<String>,
    /// The first `<graph id>`: the server's container id without `containerId` (G10).
    pub first_graph_id: Option<String>,
    /// The model, in canonical form ([`GraphSpec::canonical`]).
    pub spec: GraphSpec,
    /// What the parser dropped or rewrote, one human-readable line each.
    pub warnings: Vec<String>,
}

impl Parsed {
    /// The v2 container id the server derives for this document.
    pub fn container_name(&self) -> Option<&str> {
        self.container_id
            .as_deref()
            .or(self.first_graph_id.as_deref())
    }
}

/// Where a `<data>` element attaches.
#[derive(Clone, Copy)]
enum Ctx {
    Doc,
    Graph(usize),
    Node(usize, usize),
    Edge(usize),
}

struct RawGraph {
    id: Option<String>,
    data: Vec<(String, String)>,
    nodes: Vec<(String, Vec<(String, String)>)>,
}

struct RawEdge {
    layer: usize,
    id: Option<String>,
    source: String,
    target: String,
    data: Vec<(String, String)>,
}

fn invalid(msg: impl std::fmt::Display) -> Error {
    Error::Config(format!("invalid GraphML: {msg}"))
}

fn attr(e: &BytesStart, name: &str) -> Result<Option<String>> {
    match e.try_get_attribute(name).map_err(invalid)? {
        Some(a) => Ok(Some(
            a.normalized_value(XmlVersion::default())
                .map_err(invalid)?
                .into_owned(),
        )),
        None => Ok(None),
    }
}

/// Parse a GraphML document into the model.
pub fn parse(xml: &str) -> Result<Parsed> {
    let mut reader = Reader::from_str(xml);

    let mut keys: HashMap<String, String> = HashMap::new();
    let mut doc_data: Vec<(String, String)> = Vec::new();
    let mut graphs: Vec<RawGraph> = Vec::new();
    let mut edges: Vec<RawEdge> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut stack: Vec<Ctx> = Vec::new();
    let mut saw_root = false;

    loop {
        let (e, empty) = match reader.read_event().map_err(invalid)? {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(_) => {
                stack.pop();
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let parent = stack.last().copied();
        let local = e.local_name();
        let ctx = match (local.as_ref(), parent) {
            ("graphml", None) => {
                saw_root = true;
                Ctx::Doc
            }
            ("key", Some(Ctx::Doc)) => {
                if let Some(id) = attr(&e, "id")? {
                    let name = attr(&e, "attr.name")?.unwrap_or_else(|| id.clone());
                    keys.insert(id, name);
                }
                if !empty {
                    // A key may carry a <default>; the model does not use it.
                    reader.read_to_end(e.name()).map_err(invalid)?;
                }
                continue;
            }
            ("data", Some(target)) => {
                let key = attr(&e, "key")?.ok_or_else(|| invalid("<data> without key"))?;
                let value = if empty {
                    String::new()
                } else {
                    read_data_text(&mut reader, e.name().as_ref())?
                };
                let key = keys.get(&key).cloned().unwrap_or(key);
                match target {
                    Ctx::Doc => doc_data.push((key, value)),
                    Ctx::Graph(g) => graphs[g].data.push((key, value)),
                    Ctx::Node(g, n) => graphs[g].nodes[n].1.push((key, value)),
                    Ctx::Edge(i) => edges[i].data.push((key, value)),
                }
                continue;
            }
            ("graph", Some(Ctx::Doc)) => {
                graphs.push(RawGraph {
                    id: attr(&e, "id")?,
                    data: Vec::new(),
                    nodes: Vec::new(),
                });
                Ctx::Graph(graphs.len() - 1)
            }
            ("graph", Some(_)) => return Err(invalid("nested graphs are not supported")),
            ("node", Some(Ctx::Graph(g))) => {
                let id = attr(&e, "id")?.ok_or_else(|| invalid("<node> without id"))?;
                graphs[g].nodes.push((id, Vec::new()));
                Ctx::Node(g, graphs[g].nodes.len() - 1)
            }
            ("edge", Some(Ctx::Graph(g))) => {
                let source = attr(&e, "source")?.ok_or_else(|| invalid("<edge> without source"))?;
                let target = attr(&e, "target")?.ok_or_else(|| invalid("<edge> without target"))?;
                edges.push(RawEdge {
                    layer: g,
                    id: attr(&e, "id")?,
                    source,
                    target,
                    data: Vec::new(),
                });
                Ctx::Edge(edges.len() - 1)
            }
            (other, _) => {
                warnings.push(format!(
                    "element <{other}> is not supported and was dropped"
                ));
                if !empty {
                    reader.read_to_end(e.name()).map_err(invalid)?;
                }
                continue;
            }
        };
        if !empty {
            stack.push(ctx);
        }
    }
    if !saw_root {
        return Err(invalid("no <graphml> root element"));
    }

    let mut spec = GraphSpec::default();
    let mut container_id = None;
    for (key, value) in doc_data {
        match key.as_str() {
            "containerId" => container_id = Some(value),
            "label" => spec.label = Some(value),
            "description" => spec.description = Some(value),
            "breadcrumb-strategy" => {
                spec.breadcrumb_strategy = parse_breadcrumb(&value, "document", &mut warnings)
            }
            _ => warnings.push(format!("unknown document key {key} was dropped")),
        }
    }

    let first_graph_id = graphs.first().and_then(|g| g.id.clone());
    for g in &graphs {
        spec.layers
            .push(build_layer(g, &mut spec.breadcrumb_strategy, &mut warnings));
    }

    // Edges belong to their source vertex's layer (G11, D7).
    let vertex_layer: HashMap<&str, usize> = graphs
        .iter()
        .enumerate()
        .flat_map(|(i, g)| g.nodes.iter().map(move |(id, _)| (id.as_str(), i)))
        .collect();
    for raw in &edges {
        let mut edge = Edge {
            id: raw.id.clone(),
            source: raw.source.clone(),
            target: raw.target.clone(),
            tooltip: None,
        };
        let what = format!("edge {}", edge.effective_id());
        for (key, value) in &raw.data {
            match key.as_str() {
                "tooltipText" => edge.tooltip = Some(value.clone()),
                _ => warnings.push(format!("{what}: unknown key {key} was dropped")),
            }
        }
        let layer = vertex_layer
            .get(raw.source.as_str())
            .copied()
            .unwrap_or(raw.layer);
        spec.layers[layer].edges.push(edge);
    }

    Ok(Parsed {
        container_id,
        first_graph_id,
        spec: spec.canonical(),
        warnings,
    })
}

/// The character content of a `<data>` element up to its end tag: text,
/// CDATA sections and entity references, in document order. Comments are
/// skipped; child elements are an error.
fn read_data_text(reader: &mut Reader<&[u8]>, end: &str) -> Result<String> {
    let mut out = String::new();
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Text(t) => out.push_str(&t.xml10_content()),
            Event::CData(c) => out.push_str(&c.xml10_content()),
            Event::GeneralRef(r) => {
                if let Some(ch) = r.resolve_char_ref().map_err(invalid)? {
                    out.push(ch);
                } else {
                    let name = r.xml10_content();
                    let s = resolve_predefined_entity(&name)
                        .ok_or_else(|| invalid(format!("unknown entity &{name};")))?;
                    out.push_str(s);
                }
            }
            Event::End(e) if e.name().as_ref() == end => return Ok(out),
            Event::Start(_) | Event::Empty(_) => {
                return Err(invalid("<data> must not contain elements"));
            }
            Event::Eof => return Err(invalid("unexpected end of document inside <data>")),
            _ => {}
        }
    }
}

fn build_layer(
    g: &RawGraph,
    breadcrumb: &mut Option<BreadcrumbStrategy>,
    warnings: &mut Vec<String>,
) -> Layer {
    let mut layer = Layer::default();
    let mut focus_strategy = None;
    let mut focus_ids: Option<String> = None;
    let mut graph_breadcrumb = None;
    for (key, value) in &g.data {
        match key.as_str() {
            "namespace" => layer.namespace = value.clone(),
            "label" => layer.label = Some(value.clone()),
            "description" => layer.description = Some(value.clone()),
            "preferred-layout" => layer.preferred_layout = Some(value.clone()),
            "semantic-zoom-level" => layer.semantic_zoom_level = parse_num(key, value, warnings),
            "edge-path-offset" => layer.edge_path_offset = parse_num(key, value, warnings),
            "focus-strategy" => focus_strategy = parse_focus(value, warnings),
            "focus-ids" => focus_ids = Some(value.clone()),
            "vertex-status-provider" => {
                layer.vertex_status_provider = parse_status_provider(value, warnings)
            }
            "breadcrumb-strategy" => graph_breadcrumb = Some(value.clone()),
            _ => warnings.push(format!("unknown graph key {key} was dropped")),
        }
    }
    let ns = if layer.namespace.is_empty() {
        g.id.clone().unwrap_or_default()
    } else {
        layer.namespace.clone()
    };
    if let Some(value) = graph_breadcrumb {
        if breadcrumb.is_none() {
            *breadcrumb = parse_breadcrumb(&value, &format!("graph {ns}"), warnings);
            warnings.push(format!(
                "graph {ns}: breadcrumb-strategy moved to document level"
            ));
        } else {
            warnings.push(format!(
                "graph {ns}: breadcrumb-strategy dropped; the document already has one"
            ));
        }
    }
    layer.focus = match (focus_strategy, focus_ids) {
        (Some(FocusStrategy::Specific), ids) => Some(Focus {
            strategy: FocusStrategy::Specific,
            vertices: ids
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
        }),
        (strategy, ids) => {
            if ids.is_some() {
                warnings.push(format!(
                    "graph {ns}: focus-ids dropped; it needs focus-strategy SPECIFIC"
                ));
            }
            strategy.map(|strategy| Focus {
                strategy,
                vertices: Vec::new(),
            })
        }
    };

    for (id, data) in &g.nodes {
        layer.vertices.push(build_vertex(id, data, warnings));
    }
    layer
}

fn build_vertex(id: &str, data: &[(String, String)], warnings: &mut Vec<String>) -> Vertex {
    let mut v = Vertex {
        id: id.to_string(),
        label: None,
        icon_key: None,
        tooltip: None,
        level: None,
        node: None,
    };
    let mut fs = None;
    let mut fid = None;
    for (key, value) in data {
        match key.as_str() {
            "label" => v.label = Some(value.clone()),
            "iconKey" => v.icon_key = Some(value.clone()),
            "tooltipText" => v.tooltip = Some(value.clone()),
            "level" => v.level = parse_num(key, value, warnings),
            "foreignSource" => fs = Some(value.clone()),
            "foreignID" => fid = Some(value.clone()),
            "nodeID" => warnings.push(format!(
                "vertex {id}: nodeID {value} was dropped; use node: {{foreignSource, foreignId}}"
            )),
            _ => warnings.push(format!("vertex {id}: unknown key {key} was dropped")),
        }
    }
    match (fs, fid) {
        (Some(foreign_source), Some(foreign_id)) => {
            v.node = Some(NodeRef {
                foreign_source,
                foreign_id,
            })
        }
        (None, None) => {}
        _ => warnings.push(format!(
            "vertex {id}: incomplete foreignSource/foreignID was dropped"
        )),
    }
    v
}

fn parse_num<T: std::str::FromStr>(
    key: &str,
    value: &str,
    warnings: &mut Vec<String>,
) -> Option<T> {
    match value.trim().parse() {
        Ok(n) => Some(n),
        Err(_) => {
            warnings.push(format!(
                "{key} {value:?} is not a valid number and was dropped"
            ));
            None
        }
    }
}

fn parse_focus(value: &str, warnings: &mut Vec<String>) -> Option<FocusStrategy> {
    match value.trim().to_ascii_uppercase().as_str() {
        "EMPTY" => Some(FocusStrategy::Empty),
        "ALL" => Some(FocusStrategy::All),
        "FIRST" => Some(FocusStrategy::First),
        "SPECIFIC" => Some(FocusStrategy::Specific),
        _ => {
            warnings.push(format!(
                "focus-strategy {value:?} is unknown and was dropped"
            ));
            None
        }
    }
}

fn parse_status_provider(value: &str, warnings: &mut Vec<String>) -> Option<VertexStatusProvider> {
    match value.trim() {
        "default" => Some(VertexStatusProvider::Default),
        "script" => Some(VertexStatusProvider::Script),
        "propagate" => Some(VertexStatusProvider::Propagate),
        // D8: the legacy typed-boolean form. An assumption, so say so.
        "true" => {
            warnings.push("legacy vertex-status-provider \"true\" was read as \"default\"".into());
            Some(VertexStatusProvider::Default)
        }
        _ => {
            warnings.push(format!(
                "vertex-status-provider {value:?} is unknown and was dropped"
            ));
            None
        }
    }
}

fn parse_breadcrumb(
    value: &str,
    scope: &str,
    warnings: &mut Vec<String>,
) -> Option<BreadcrumbStrategy> {
    match value.trim() {
        "NONE" => Some(BreadcrumbStrategy::None),
        "SHORTEST_PATH_TO_ROOT" => Some(BreadcrumbStrategy::ShortestPathToRoot),
        _ => {
            warnings.push(format!(
                "{scope}: breadcrumb-strategy {value:?} is unknown and was dropped"
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GraphLocal;

    fn graphml(body: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><graphml xmlns="http://graphml.graphdrawing.org/xmlns">
            <key id="namespace" for="graph" attr.name="namespace" attr.type="string"/>
            <key id="label" for="all" attr.name="label" attr.type="string"/>
            {body}</graphml>"#
        )
    }

    fn vertex(id: &str) -> Vertex {
        Vertex {
            id: id.into(),
            label: None,
            icon_key: None,
            tooltip: None,
            level: None,
            node: None,
        }
    }

    #[test]
    fn render_then_parse_is_identity_for_the_probe_document() {
        let doc = fixture_doc();
        let parsed = parse(&render(&doc.metadata.name, &doc.spec)).unwrap();
        assert_eq!(parsed.container_id.as_deref(), Some("wan-overview"));
        assert_eq!(parsed.spec, doc.spec.canonical());
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    }

    #[test]
    fn render_then_parse_is_identity_for_hand_built_specs() {
        let mut minimal = GraphSpec::default();
        minimal.layers.push(Layer {
            namespace: "only".into(),
            ..Layer::default()
        });

        let mut focused = GraphSpec {
            label: Some(String::new()),
            breadcrumb_strategy: Some(BreadcrumbStrategy::None),
            ..GraphSpec::default()
        };
        focused.layers.push(Layer {
            namespace: "a".into(),
            focus: Some(Focus {
                strategy: FocusStrategy::Specific,
                vertices: vec!["x".into(), "y".into()],
            }),
            vertices: vec![
                Vertex {
                    level: Some(-2),
                    tooltip: Some("quote \" and apostrophe '".into()),
                    ..vertex("x")
                },
                vertex("y"),
            ],
            edges: vec![
                Edge {
                    id: None,
                    source: "x".into(),
                    target: "y".into(),
                    tooltip: None,
                },
                Edge {
                    id: Some("x-y-2".into()),
                    source: "x".into(),
                    target: "y".into(),
                    tooltip: Some(String::new()),
                },
            ],
            ..Layer::default()
        });
        focused.layers.push(Layer {
            namespace: "b".into(),
            focus: Some(Focus {
                strategy: FocusStrategy::Empty,
                vertices: vec![],
            }),
            vertex_status_provider: Some(VertexStatusProvider::Script),
            ..Layer::default()
        });

        for spec in [minimal, focused] {
            let parsed = parse(&render("g", &spec)).unwrap();
            assert_eq!(parsed.spec, spec.canonical());
            assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        }
    }

    #[test]
    fn data_text_keeps_padding_and_newlines() {
        let mut spec = GraphSpec {
            label: Some("  padded  ".into()),
            description: Some("line one\nline two".into()),
            ..GraphSpec::default()
        };
        spec.layers.push(Layer {
            namespace: "n".into(),
            ..Layer::default()
        });
        let parsed = parse(&render("g", &spec)).unwrap();
        assert_eq!(parsed.spec, spec);
    }

    #[test]
    fn edge_moves_to_its_source_layer() {
        let p = parse(&graphml(
            r#"<graph id="a"><data key="namespace">a</data><node id="x"/></graph>
               <graph id="b"><data key="namespace">b</data><node id="y"/>
                 <edge id="e" source="x" target="y"/></graph>"#,
        ))
        .unwrap();
        assert_eq!(p.spec.layers[0].edges.len(), 1);
        assert!(p.spec.layers[1].edges.is_empty());
    }

    #[test]
    fn graph_level_breadcrumb_lifts_to_the_document() {
        let p = parse(&graphml(
            r#"<key id="breadcrumb-strategy" for="graph" attr.name="breadcrumb-strategy" attr.type="string"/>
               <graph id="a"><data key="namespace">a</data>
               <data key="breadcrumb-strategy">SHORTEST_PATH_TO_ROOT</data></graph>"#,
        ))
        .unwrap();
        assert_eq!(
            p.spec.breadcrumb_strategy,
            Some(BreadcrumbStrategy::ShortestPathToRoot)
        );
        assert!(
            p.warnings
                .iter()
                .any(|w| w.contains("moved to document level"))
        );
    }

    #[test]
    fn unknown_keys_and_node_id_are_dropped_with_warnings() {
        let p = parse(&graphml(
            r#"<key id="nodeID" for="node" attr.name="nodeID" attr.type="int"/>
               <key id="color" for="node" attr.name="color" attr.type="string"/>
               <graph id="a"><data key="namespace">a</data>
               <node id="x"><data key="nodeID">7</data><data key="color">red</data></node></graph>"#,
        ))
        .unwrap();
        assert_eq!(p.spec.layers[0].vertices[0], vertex("x"));
        assert!(
            p.warnings
                .iter()
                .any(|w| w.contains("nodeID 7 was dropped")),
            "{:?}",
            p.warnings
        );
        assert!(
            p.warnings.iter().any(|w| w.contains("unknown key color")),
            "{:?}",
            p.warnings
        );
    }

    #[test]
    fn data_content_accepts_cdata_entities_and_comments() {
        let p = parse(&graphml(
            r#"<key id="tooltipText" for="node" attr.name="tooltipText" attr.type="string"/>
               <graph id="a"><data key="namespace">a</data>
               <node id="x"><data key="tooltipText"><![CDATA[<b>core</b> & more]]> &amp; &#x21;<!-- c --></data>
               <data key="label"><![CDATA[]]></data></node></graph>"#,
        ))
        .unwrap();
        let v = &p.spec.layers[0].vertices[0];
        assert_eq!(v.tooltip.as_deref(), Some("<b>core</b> & more & !"));
        assert_eq!(v.label.as_deref(), Some(""));
        assert!(
            parse(&graphml(
                r#"<graph id="a"><data key="namespace"><b/></data></graph>"#
            ))
            .is_err()
        );
        assert!(
            parse(&graphml(
                r#"<graph id="a"><data key="namespace">&nope;</data></graph>"#
            ))
            .is_err()
        );
    }

    #[test]
    fn data_keys_resolve_through_attr_name() {
        let p = parse(
            r#"<graphml xmlns="http://graphml.graphdrawing.org/xmlns">
               <key id="d0" for="graph" attr.name="namespace" attr.type="string"/>
               <graph id="g"><data key="d0">ns</data></graph></graphml>"#,
        )
        .unwrap();
        assert_eq!(p.spec.layers[0].namespace, "ns");
    }

    #[test]
    fn container_name_falls_back_to_the_first_graph_id() {
        let p = parse(&graphml(
            r#"<graph id="first"><data key="namespace">a</data></graph>"#,
        ))
        .unwrap();
        assert_eq!(p.container_id, None);
        assert_eq!(p.container_name(), Some("first"));
    }

    #[test]
    fn malformed_xml_is_an_error() {
        assert!(parse("not xml at all").is_err());
        assert!(parse("<graphml><graph id=\"a\"></graphml>").is_err());
    }

    #[test]
    fn nested_graphs_are_an_error() {
        let err = parse(&graphml(
            r#"<graph id="a"><node id="x"><graph id="inner"/></node></graph>"#,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("nested graphs"), "{err}");
    }

    #[test]
    fn legacy_campus_fixture() {
        let p = parse(include_str!("../tests/fixtures/legacy-campus.xml")).unwrap();
        assert_eq!(p.container_id, None);
        assert_eq!(p.container_name(), Some("campus"));
        assert_eq!(p.spec.label.as_deref(), Some("Campus Network"));
        assert_eq!(p.spec.breadcrumb_strategy, Some(BreadcrumbStrategy::None));
        assert_eq!(p.spec.layers.len(), 1);
        let layer = &p.spec.layers[0];
        assert_eq!(layer.namespace, "campus");
        assert_eq!(layer.vertices.len(), 7);
        assert_eq!(layer.edges.len(), 6);
        assert_eq!(
            layer.vertex_status_provider,
            Some(VertexStatusProvider::Default)
        );
        assert_eq!(layer.focus.as_ref().unwrap().strategy, FocusStrategy::All);
        let fw = layer.vertices.iter().find(|v| v.id == "fw").unwrap();
        assert_eq!(fw.label.as_deref(), Some("firewall"));
        assert_eq!(fw.node.as_ref().unwrap().foreign_id, "fw-01");
        assert_eq!(
            p.warnings,
            vec!["legacy vertex-status-provider \"true\" was read as \"default\"".to_string()]
        );
        let explicit: Vec<&str> = layer.edges.iter().filter_map(|e| e.id.as_deref()).collect();
        assert_eq!(explicit, vec!["uplink-a", "dmz-web"]);
    }

    fn fixture_doc() -> GraphLocal {
        serde_norway::from_str(include_str!("../tests/fixtures/probe-two-layer.yaml")).unwrap()
    }

    #[test]
    fn probe_document_renders_to_golden_graphml() {
        let doc = fixture_doc();
        let xml = render(&doc.metadata.name, &doc.spec);
        assert_eq!(xml, include_str!("../tests/fixtures/probe-two-layer.xml"));
    }

    #[test]
    fn only_used_keys_are_declared() {
        let mut spec = GraphSpec::default();
        spec.layers.push(Layer {
            namespace: "n".into(),
            vertices: vec![Vertex {
                id: "a".into(),
                label: None,
                icon_key: None,
                tooltip: None,
                level: None,
                node: None,
            }],
            ..Layer::default()
        });
        let xml = render("g", &spec);
        assert!(xml.contains(r#"<key id="containerId""#), "{xml}");
        assert!(xml.contains(r#"<key id="namespace""#), "{xml}");
        assert!(!xml.contains(r#"<key id="label""#), "{xml}");
        assert!(xml.contains(r#"<node id="a"/>"#), "{xml}");
    }
}
