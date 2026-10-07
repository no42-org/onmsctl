/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl graph …` verbs.
//!
//! `get`, `export` and `delete` work on uploads (`rest/graphml/{name}`);
//! `list`, `view` and `search` read the v2 Graph API. `convert` is a pure local
//! transform of a GraphML file and issues no HTTP.

use std::path::PathBuf;

use clap::Subcommand;
use serde::Serialize;

use onmsctl_core::{
    Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result, TableRow, render_list,
};

use crate::api::{GraphApi, GraphmlApi, is_not_found};
use crate::graphml;
use crate::model::GraphLocal;

/// `onmsctl graph …` verbs.
#[derive(Subcommand, Debug, Clone)]
pub enum GraphCmd {
    /// List graph containers, marking those backed by a GraphML upload.
    List,
    /// Show one GraphML upload, parsed into the Graph model.
    Get {
        /// The upload name (`metadata.name`).
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Print one GraphML upload as an apply-ready `kind: Graph` document.
    Export {
        /// The upload name (`metadata.name`).
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Convert a local GraphML file into a `kind: Graph` document. No server access.
    Convert {
        /// The GraphML file to convert.
        #[arg(short = 'f', long = "file", value_name = "FILE")]
        file: PathBuf,
    },
    /// Show a graph view (focus and semantic zoom level) from the v2 Graph API.
    View {
        /// The container id.
        #[arg(value_name = "CONTAINER")]
        container: String,
        /// The graph namespace.
        #[arg(value_name = "NAMESPACE")]
        namespace: String,
        /// A vertex id of NAMESPACE to focus. Repeat for several.
        #[arg(long, value_name = "ID")]
        focus: Vec<String>,
        /// Semantic zoom level (server default 1).
        #[arg(long, value_name = "N")]
        szl: Option<u32>,
    },
    /// Search one graph namespace for vertices.
    Search {
        /// The graph namespace.
        #[arg(value_name = "NAMESPACE")]
        namespace: String,
        /// The search text.
        #[arg(value_name = "TEXT")]
        text: String,
    },
    /// Delete a GraphML upload.
    Delete {
        /// The upload name (`metadata.name`).
        #[arg(value_name = "NAME")]
        name: String,
    },
}

impl Classify for GraphCmd {
    fn kind(&self) -> CmdKind {
        match self {
            GraphCmd::Delete { .. } => CmdKind::Write,
            GraphCmd::List
            | GraphCmd::Get { .. }
            | GraphCmd::Export { .. }
            | GraphCmd::Convert { .. }
            | GraphCmd::View { .. }
            | GraphCmd::Search { .. } => CmdKind::Read,
        }
    }
}

impl GraphCmd {
    /// True for verbs that need no server (`convert`).
    pub fn is_local_only(&self) -> bool {
        matches!(self, GraphCmd::Convert { .. })
    }

    /// Run a pure-local verb without a [`Context`]. The binary checks
    /// [`is_local_only`](Self::is_local_only) first.
    pub async fn run_local(self) -> Result<()> {
        match self {
            GraphCmd::Convert { file } => {
                let (yaml, warnings) = convert_file(&file)?;
                print_warnings(&warnings);
                print!("{yaml}");
                Ok(())
            }
            other => Err(Error::Config(format!(
                "internal: {other:?} needs a server context"
            ))),
        }
    }

    pub async fn run(self, ctx: &Context) -> Result<()> {
        if self.is_local_only() {
            return self.run_local().await;
        }
        let client = OnmsClient::from_context(ctx)?;
        let uploads = GraphmlApi::new(&client);
        let v2 = GraphApi::new(&client);
        match self {
            GraphCmd::List => {
                let rows = list_rows(&uploads, &v2).await?;
                print!("{}", render_list(&rows, ctx.output_format)?);
            }
            GraphCmd::Get { name } => {
                let doc = fetch_doc(&uploads, &name).await?;
                print!("{}", render_value(&doc, ctx.output_format)?);
            }
            GraphCmd::Export { name } => {
                print!("{}", fetch_doc(&uploads, &name).await?.to_yaml());
            }
            GraphCmd::View {
                container,
                namespace,
                focus,
                szl,
            } => {
                let view = v2.view(&container, &namespace, &focus, szl).await?;
                print!("{}", render_value(&view, ctx.output_format)?);
            }
            GraphCmd::Search { namespace, text } => {
                let hits = v2.search(&namespace, &text).await?;
                print!("{}", render_value(&hits, ctx.output_format)?);
            }
            GraphCmd::Delete { name } => match uploads.delete(&name).await {
                Ok(()) => eprintln!("Deleted graph {name}"),
                Err(e) if is_not_found(&e) => {
                    return Err(Error::Config(format!("graph {name} not found")));
                }
                Err(e) => return Err(e),
            },
            GraphCmd::Convert { .. } => unreachable!("handled by run_local"),
        }
        Ok(())
    }
}

/// One row of `graph list`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContainerRow {
    pub id: String,
    pub label: String,
    pub layers: usize,
    /// Whether a GraphML upload of the same name exists.
    pub managed: bool,
}

impl TableRow for ContainerRow {
    fn headers() -> Vec<&'static str> {
        vec!["ID", "LABEL", "LAYERS", "MANAGED"]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.id.clone(),
            self.label.clone(),
            self.layers.to_string(),
            if self.managed { "yes" } else { "no" }.into(),
        ]
    }
}

/// Every v2 container. There is no upload list endpoint (G3), so `managed`
/// costs one `GET rest/graphml/{id}` per container.
async fn list_rows(uploads: &GraphmlApi<'_>, v2: &GraphApi<'_>) -> Result<Vec<ContainerRow>> {
    let mut rows = Vec::new();
    for c in v2.containers().await? {
        let managed = uploads.get_xml(&c.id).await?.is_some();
        rows.push(ContainerRow {
            label: c.label.unwrap_or_default(),
            layers: c.graphs.len(),
            managed,
            id: c.id,
        });
    }
    Ok(rows)
}

/// The stored upload as a `kind: Graph` document, warnings on stderr.
async fn fetch_doc(uploads: &GraphmlApi<'_>, name: &str) -> Result<GraphLocal> {
    let xml = uploads
        .get_xml(name)
        .await?
        .ok_or_else(|| Error::Config(format!("graph {name} not found")))?;
    let parsed = graphml::parse(&xml)?;
    print_warnings(&parsed.warnings);
    Ok(GraphLocal::new(name, &parsed.spec))
}

/// Convert a GraphML file: the YAML document and the parser's warnings. The
/// document is named after the container the server would create (G10).
pub fn convert_file(file: &std::path::Path) -> Result<(String, Vec<String>)> {
    let xml = std::fs::read_to_string(file)
        .map_err(|e| Error::Config(format!("reading {}: {e}", file.display())))?;
    let parsed = graphml::parse(&xml)?;
    let name = parsed.container_name().map(String::from).ok_or_else(|| {
        Error::Config(format!(
            "{}: no containerId and no <graph id> to name the document",
            file.display()
        ))
    })?;
    let mut warnings = parsed.warnings;
    if !crate::validate::is_path_safe(&name) {
        warnings.push(format!(
            "derived metadata.name {name:?} is not a valid upload name; set metadata.name before applying"
        ));
    }
    Ok((GraphLocal::new(&name, &parsed.spec).to_yaml(), warnings))
}

/// JSON for `-o json`, YAML otherwise. Graphs have no useful table form.
fn render_value<T: Serialize>(value: &T, format: OutputFormat) -> Result<String> {
    match format {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(value)?)),
        _ => Ok(serde_norway::to_string(value)?),
    }
}

fn print_warnings(warnings: &[String]) {
    for w in warnings {
        eprintln!("warning: {w}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::{AuthCreds, Url};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx_for(server: &MockServer) -> Context {
        Context {
            name: "test".into(),
            url: Url::parse(&format!("{}/", server.uri())).unwrap(),
            creds: AuthCreds::bearer("t"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        }
    }

    #[test]
    fn only_delete_is_a_write() {
        let name = || "g".to_string();
        assert_eq!(GraphCmd::Delete { name: name() }.kind(), CmdKind::Write);
        for read in [
            GraphCmd::List,
            GraphCmd::Get { name: name() },
            GraphCmd::Export { name: name() },
            GraphCmd::Convert {
                file: "x.xml".into(),
            },
            GraphCmd::View {
                container: name(),
                namespace: name(),
                focus: vec![],
                szl: None,
            },
            GraphCmd::Search {
                namespace: name(),
                text: name(),
            },
        ] {
            assert_eq!(read.kind(), CmdKind::Read, "{read:?}");
        }
        assert!(
            GraphCmd::Convert {
                file: "x.xml".into()
            }
            .is_local_only()
        );
        assert!(!GraphCmd::List.is_local_only());
    }

    #[test]
    fn convert_names_a_legacy_file_after_its_first_graph() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/legacy-campus.xml");
        let (yaml, warnings) = convert_file(&file).unwrap();
        let doc: GraphLocal = serde_norway::from_str(&yaml).unwrap();
        assert_eq!(doc.metadata.name, "campus");
        assert_eq!(doc.spec.layers[0].vertices.len(), 7);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn convert_warns_when_the_derived_name_is_not_path_safe() {
        let dir =
            std::env::temp_dir().join(format!("onmsctl-graph-convert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("colon.xml");
        std::fs::write(
            &file,
            r#"<graphml xmlns="http://graphml.graphdrawing.org/xmlns">
               <key id="namespace" for="graph" attr.name="namespace" attr.type="string"/>
               <graph id="acme:regions"><data key="namespace">acme:regions</data></graph></graphml>"#,
        )
        .unwrap();
        let (yaml, warnings) = convert_file(&file).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(yaml.contains("name: acme:regions"), "{yaml}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("not a valid upload name")),
            "{warnings:?}"
        );
    }

    #[tokio::test]
    async fn list_marks_containers_backed_by_an_upload() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": "wan", "label": "WAN", "graphs": [{"namespace": "a"}, {"namespace": "b"}]},
                {"id": "bsm", "label": "Business Service Graph", "graphs": [{"namespace": "bsm"}]}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/graphml/wan"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<graphml/>"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/graphml/bsm"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = OnmsClient::from_context(&ctx_for(&server)).unwrap();
        let rows = list_rows(&GraphmlApi::new(&client), &GraphApi::new(&client))
            .await
            .unwrap();
        assert_eq!(rows[0].row(), vec!["wan", "WAN", "2", "yes"]);
        assert_eq!(
            rows[1].row(),
            vec!["bsm", "Business Service Graph", "1", "no"]
        );
    }

    #[tokio::test]
    async fn delete_of_a_missing_graph_fails_with_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/rest/graphml/gone"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let err = GraphCmd::Delete {
            name: "gone".into(),
        }
        .run(&ctx_for(&server))
        .await
        .unwrap_err();
        assert!(err.to_string().ends_with("graph gone not found"), "{err}");
    }

    #[tokio::test]
    async fn get_of_a_missing_graph_fails_with_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/graphml/gone"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let err = GraphCmd::Get {
            name: "gone".into(),
        }
        .run(&ctx_for(&server))
        .await
        .unwrap_err();
        assert!(err.to_string().ends_with("graph gone not found"), "{err}");
    }
}
