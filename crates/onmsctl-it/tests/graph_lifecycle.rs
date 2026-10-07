/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Live-Horizon lifecycle for `kind: Graph`: create, re-apply unchanged,
//! update (and see it served by the v2 API), export round trip, view, delete.
//! `#[ignore]`d so `make test` is unaffected; run via `make integration`.

use std::time::Duration;

use onmsctl_core::kind::parse_documents;
use onmsctl_core::kind::precedence::RANK_GRAPH;
use onmsctl_core::{ApplyOutcome, ApplyParams, OutcomeStatus, Registry, apply_documents};

use onmsctl_graph::api::{GraphApi, GraphmlApi, is_not_found};
use onmsctl_graph::apply::GraphHandler;
use onmsctl_graph::graphml;
use onmsctl_graph::model::GraphLocal;

use onmsctl_it::{Harness, harness_or_skip};

fn registry() -> Registry {
    let mut reg = Registry::new();
    reg.register(RANK_GRAPH, Box::new(GraphHandler::default()));
    reg
}

fn graph_doc(name: &str, label: &str) -> String {
    format!(
        "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata:\n  name: {name}\n\
         spec:\n  label: IT graph\n  layers:\n    - namespace: {name}:top\n      \
         focus: {{strategy: ALL}}\n      vertices:\n        - {{id: a, label: {label}}}\n        \
         - {{id: b, label: Beta}}\n      edges:\n        - {{source: a, target: b, tooltip: link}}\n"
    )
}

async fn apply(h: &Harness, yaml: &str) -> Vec<ApplyOutcome> {
    apply_documents(
        &registry(),
        parse_documents("it.yaml", yaml).unwrap(),
        &ApplyParams::default(),
        &h.context(false),
    )
    .await
    .expect("apply must pass the plan gate")
}

/// The label the v2 API serves for vertex `id` of `namespace`.
async fn served_label(v2: &GraphApi<'_>, name: &str, namespace: &str, id: &str) -> Option<String> {
    let view = v2.view(name, namespace, &[], None).await.ok()?;
    view["vertices"]
        .as_array()?
        .iter()
        .find(|v| v["id"] == id)
        .and_then(|v| v["label"].as_str().map(String::from))
}

#[tokio::test]
#[ignore = "live Horizon required (run via `make integration`)"]
async fn graph_create_update_export_view_delete() {
    let h = harness_or_skip!();
    h.cleanup_graphs().await.expect("pre-test cleanup");
    let v2 = GraphApi::new(h.client());
    let uploads = GraphmlApi::new(h.client());

    let name = h.unique_name("graph");
    let ns = format!("{name}:top");

    // -- create, then nothing to do --
    let out = apply(&h, &graph_doc(&name, "Alpha")).await;
    assert_eq!(out[0].status, OutcomeStatus::Created, "{out:?}");
    assert!(
        v2.container(&name).await.unwrap().is_some(),
        "container loaded"
    );
    let out = apply(&h, &graph_doc(&name, "Alpha")).await;
    assert_eq!(out[0].status, OutcomeStatus::Unchanged, "{out:?}");

    // -- update: stored and served (G17) --
    let out = apply(&h, &graph_doc(&name, "Alpha Two")).await;
    assert_eq!(out[0].status, OutcomeStatus::Updated, "{out:?}");
    let stored = graphml::parse(&uploads.get_xml(&name).await.unwrap().unwrap()).unwrap();
    assert_eq!(
        stored.spec.layers[0].vertices[0].label.as_deref(),
        Some("Alpha Two")
    );
    let mut served = None;
    for _ in 0..15 {
        served = served_label(&v2, &name, &ns, "a").await;
        if served.as_deref() == Some("Alpha Two") {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert_eq!(served.as_deref(), Some("Alpha Two"), "v2 serves the update");

    // -- export round trip --
    let exported = GraphLocal::new(&name, &stored.spec).to_yaml();
    let out = apply(&h, &exported).await;
    assert_eq!(out[0].status, OutcomeStatus::Unchanged, "{out:?}");

    // -- view with a bare-id focus (G15) --
    let view = v2.view(&name, &ns, &["a".into()], Some(0)).await.unwrap();
    let ids: Vec<&str> = view["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["id"].as_str())
        .collect();
    assert_eq!(ids, ["a"], "{view}");

    // -- delete, twice --
    uploads.delete(&name).await.expect("delete");
    assert!(is_not_found(&uploads.delete(&name).await.unwrap_err()));

    let _ = h.cleanup_graphs().await;
}
