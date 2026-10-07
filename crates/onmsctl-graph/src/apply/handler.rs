/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `GraphHandler`: the graph capability's adapter into the core kind-router.
//!
//! `plan()` validates every document, gates on duplicate names and namespaces
//! within the apply, reads each stored upload, refuses namespaces owned by a
//! foreign container (D9), warns about unresolved node references (D4), and
//! compares canonical specs (D2). `execute()` creates with `POST`, updates with
//! `DELETE` then `POST`, rolls back from the stored XML when the `POST` fails
//! (D3), and waits for the container to appear in the v2 API (D10).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use async_trait::async_trait;
use similar::TextDiff;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OnmsClient, OutcomeStatus,
    Plan, RawDoc, Result,
};

use crate::api::{GraphApi, GraphmlApi, is_already_exists, is_not_found};
use crate::graphml;
use crate::model::{GraphLocal, KIND, NodeRef};
use crate::validate::validate;

/// Handler for `kind: Graph` documents.
pub struct GraphHandler {
    /// How long execute waits for a written graph to appear in `api/v2/graphs`.
    pub load_timeout: Duration,
    /// Poll interval for that wait.
    pub poll_interval: Duration,
}

impl Default for GraphHandler {
    fn default() -> Self {
        Self {
            load_timeout: Duration::from_secs(30),
            poll_interval: Duration::from_secs(2),
        }
    }
}

/// What `execute()` will do for one document.
struct DocPlan {
    name: String,
    action: Action,
    /// The rendered desired GraphML.
    xml: String,
    /// The stored GraphML read during plan; the rollback source.
    live_xml: Option<String>,
    /// The v2 container the stored upload produced. Differs from `name` for a
    /// legacy upload without `containerId` (G10).
    live_container: Option<String>,
}

struct GraphExecPayload {
    docs: Vec<DocPlan>,
}

#[async_trait]
impl KindHandler for GraphHandler {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        // -- Parse and validate offline; any failure aborts before HTTP. --
        let mut parsed: Vec<(String, GraphLocal)> = Vec::with_capacity(docs.len());
        let mut problems: Vec<String> = Vec::new();
        for d in docs {
            match GraphLocal::from_value(d.value.clone()) {
                Ok(local) => {
                    if let Err(e) = validate(&local) {
                        problems.push(format!("{}: {}", d.label(), config_msg(e)));
                    }
                    parsed.push((d.label(), local));
                }
                Err(e) => problems.push(format!("{}: {e}", d.label())),
            }
        }
        problems.extend(cross_document_problems(&parsed));
        if !problems.is_empty() {
            return Err(Error::Config(problems.join("\n")));
        }

        let client = OnmsClient::from_context(ctx)?;
        let graphml_api = GraphmlApi::new(&client);
        let graph_api = GraphApi::new(&client);

        // -- Stored uploads, and the containers they produced. --
        let mut live: Vec<Option<(String, Result<graphml::Parsed>)>> = Vec::new();
        for (_, local) in &parsed {
            let xml = graphml_api.get_xml(&local.metadata.name).await?;
            live.push(xml.map(|x| {
                let p = graphml::parse(&x);
                (x, p)
            }));
        }

        // -- Namespace ownership (D9). --
        let owners: HashMap<String, String> = graph_api
            .containers()
            .await?
            .into_iter()
            .flat_map(|c| {
                let id = c.id;
                c.graphs.into_iter().map(move |g| (g.namespace, id.clone()))
            })
            .collect();
        for ((_, local), stored) in parsed.iter().zip(&live) {
            // The container the document's own upload produced. An upload
            // onmsctl cannot parse is still the document's own, so fall back
            // to the name rather than blaming a foreign container.
            let own_stored = stored.as_ref().map(|(_, p)| {
                p.as_ref()
                    .ok()
                    .and_then(|p| p.container_name())
                    .unwrap_or(local.metadata.name.as_str())
            });
            for layer in &local.spec.layers {
                // Only the container the document's own upload produced is
                // its own. A same-named container without that upload belongs
                // to another upload or provider.
                if let Some(owner) = owners.get(&layer.namespace)
                    && Some(owner.as_str()) != own_stored
                {
                    problems.push(format!(
                        "Graph {}: namespace {} already belongs to container {owner}",
                        local.metadata.name, layer.namespace
                    ));
                }
            }
        }
        if !problems.is_empty() {
            return Err(Error::Config(problems.join("\n")));
        }

        // -- Compare and render. --
        let mut plans = Vec::with_capacity(parsed.len());
        let mut preview = Vec::with_capacity(parsed.len());
        let mut diffs = Vec::new();
        for ((_, local), stored) in parsed.iter().zip(live) {
            let name = local.metadata.name.clone();
            let desired = GraphLocal::new(&name, &local.spec);
            let xml = graphml::render(&name, &desired.spec);
            let live_container = stored
                .as_ref()
                .and_then(|(_, p)| p.as_ref().ok())
                .and_then(|p| p.container_name())
                .map(String::from);
            let (action, live_xml) = match stored {
                None => {
                    diffs.push(render_diff(&name, "create", "", &desired.to_yaml()));
                    (Action::Create, None)
                }
                Some((live_xml, Ok(p))) => {
                    if p.spec == desired.spec && p.container_id.as_deref() == Some(&name) {
                        (Action::None, Some(live_xml))
                    } else {
                        // The replacement drops whatever the parser dropped.
                        for w in &p.warnings {
                            eprintln!("warning: Graph {name}: stored upload: {w}");
                        }
                        let mut d = render_diff(
                            &name,
                            "update",
                            &GraphLocal::new(&name, &p.spec).to_yaml(),
                            &desired.to_yaml(),
                        );
                        if p.container_id.as_deref() != Some(&name) {
                            d.push_str(&format!(
                                "\n  container id: {} -> {name}",
                                p.container_name().unwrap_or("<none>")
                            ));
                        }
                        diffs.push(d);
                        (Action::Update, Some(live_xml))
                    }
                }
                Some((live_xml, Err(e))) => {
                    diffs.push(format!(
                        "Graph/{name}: update\n  stored upload could not be parsed ({}); it will be replaced",
                        config_msg(e)
                    ));
                    (Action::Update, Some(live_xml))
                }
            };
            preview.push(ApplyOutcome::would(KIND, name.clone(), action));
            plans.push(DocPlan {
                name,
                action,
                xml,
                live_xml,
                live_container,
            });
        }

        // -- Node references of graphs about to be written: warn, never fail (D4). --
        let changed: Vec<&GraphLocal> = parsed
            .iter()
            .zip(&plans)
            .filter(|(_, dp)| dp.action != Action::None)
            .map(|((_, local), _)| local)
            .collect();
        warn_missing_nodes(&graph_api, &changed).await;

        let diff = (!diffs.is_empty()).then(|| diffs.join("\n"));
        Ok(Plan::new(preview, Box::new(GraphExecPayload { docs: plans })).with_diff(diff))
    }

    async fn execute(
        &self,
        plan: Plan,
        params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let payload = plan
            .payload
            .downcast::<GraphExecPayload>()
            .map_err(|_| Error::Config("internal: GraphHandler payload type mismatch".into()))?;
        let client = OnmsClient::from_context(ctx)?;
        let graphml_api = GraphmlApi::new(&client);
        let graph_api = GraphApi::new(&client);

        let mut outcomes = Vec::with_capacity(payload.docs.len());
        let mut halted = false;
        for dp in payload.docs {
            if halted {
                outcomes.push(ApplyOutcome::skipped(
                    KIND,
                    &dp.name,
                    dp.action,
                    "not attempted (a prior graph failed and --continue-on-error is off)",
                ));
                continue;
            }
            let outcome = match dp.action {
                Action::Create => self.create(&graphml_api, &graph_api, &dp).await,
                Action::Update => self.update(&graphml_api, &graph_api, &dp).await,
                _ => ApplyOutcome::new(
                    KIND,
                    &dp.name,
                    Action::None,
                    OutcomeStatus::Unchanged,
                    "in sync",
                ),
            };
            if outcome.status.is_failure() && !params.continue_on_error {
                halted = true;
            }
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }
}

impl GraphHandler {
    async fn create(&self, api: &GraphmlApi<'_>, v2: &GraphApi<'_>, dp: &DocPlan) -> ApplyOutcome {
        match api.create(&dp.name, dp.xml.clone()).await {
            Ok(()) => {
                self.wait_until_loaded(v2, &dp.name).await;
                ApplyOutcome::new(
                    KIND,
                    &dp.name,
                    Action::Create,
                    OutcomeStatus::Created,
                    "created",
                )
            }
            Err(e) if is_already_exists(&e) => ApplyOutcome::failed(
                KIND,
                &dp.name,
                Action::Create,
                format!("graph {} appeared since plan; re-run apply", dp.name),
                "re-run `onmsctl apply -f` to plan against the current upload",
            ),
            Err(e) => ApplyOutcome::failed(
                KIND,
                &dp.name,
                Action::Create,
                e.to_string(),
                "fix the reported error and re-apply",
            ),
        }
    }

    async fn update(&self, api: &GraphmlApi<'_>, v2: &GraphApi<'_>, dp: &DocPlan) -> ApplyOutcome {
        let failed = |msg: String, remediation: &str| {
            ApplyOutcome::failed(KIND, &dp.name, Action::Update, msg, remediation)
        };
        // A 404 means someone deleted it since plan: carry on as a create.
        let existed = match api.delete(&dp.name).await {
            Ok(()) => {
                // A POST before the old container has left v2 is stored but
                // never served (G17), so wait for the removal first.
                let old = dp.live_container.as_deref().unwrap_or(&dp.name);
                self.wait_until_removed(v2, old).await;
                true
            }
            Err(e) if is_not_found(&e) => false,
            Err(e) => {
                return failed(
                    format!("{e}; the stored graph is unchanged"),
                    "fix the reported error and re-apply",
                );
            }
        };
        let err = match api.create(&dp.name, dp.xml.clone()).await {
            Ok(()) => {
                self.wait_until_loaded(v2, &dp.name).await;
                return ApplyOutcome::new(
                    KIND,
                    &dp.name,
                    Action::Update,
                    OutcomeStatus::Updated,
                    "updated",
                );
            }
            Err(e) => e,
        };
        let Some(live) = dp.live_xml.clone().filter(|_| existed) else {
            return failed(err.to_string(), "fix the reported error and re-apply");
        };
        match api.create(&dp.name, live).await {
            Ok(()) => {
                self.wait_until_loaded(v2, &dp.name).await;
                failed(
                    format!("{err}; rolled back to the previous version"),
                    "fix the reported error and re-apply",
                )
            }
            Err(rollback) => failed(
                format!(
                    "{err}; rollback failed: {rollback}; graph {} is now absent",
                    dp.name
                ),
                "fix the reported error and re-apply to recreate the graph",
            ),
        }
    }

    /// Poll the v2 API until the container appears (D10). Never fails: the
    /// upload is stored, so a slow or failed load is a warning.
    async fn wait_until_loaded(&self, v2: &GraphApi<'_>, name: &str) {
        if !self.poll_container(v2, name, true).await {
            eprintln!(
                "warning: Graph {name}: not visible in api/v2/graphs after {}s; \
                 the upload is stored, check karaf.log for load errors",
                self.load_timeout.as_secs()
            );
        }
    }

    /// Poll the v2 API until a deleted container is gone (G17). Never fails;
    /// on timeout the update goes ahead and says the view may be stale.
    async fn wait_until_removed(&self, v2: &GraphApi<'_>, name: &str) {
        if !self.poll_container(v2, name, false).await {
            eprintln!(
                "warning: Graph {name}: still listed in api/v2/graphs {}s after DELETE; \
                 the topology UI may keep showing the previous version",
                self.load_timeout.as_secs()
            );
        }
    }

    /// Poll the container list until `name`'s presence equals `present`, up
    /// to `load_timeout`. The list carries ids and namespaces only, unlike a
    /// container fetch with every vertex. A transport error counts as "not yet".
    async fn poll_container(&self, v2: &GraphApi<'_>, name: &str, present: bool) -> bool {
        let deadline = tokio::time::Instant::now() + self.load_timeout;
        loop {
            if let Ok(list) = v2.containers().await
                && list.iter().any(|c| c.id == name) == present
            {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

/// Duplicate names and namespaces across the documents of one apply.
fn cross_document_problems(parsed: &[(String, GraphLocal)]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut by_ns: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (label, local) in parsed {
        by_name.entry(&local.metadata.name).or_default().push(label);
        for layer in &local.spec.layers {
            if !layer.namespace.is_empty() {
                by_ns
                    .entry(&layer.namespace)
                    .or_default()
                    .insert(&local.metadata.name);
            }
        }
    }
    for (name, labels) in by_name.iter().filter(|(_, l)| l.len() > 1) {
        problems.push(format!(
            "duplicate Graph metadata.name {name} ({})",
            labels.join(", ")
        ));
    }
    for (ns, docs) in by_ns.iter().filter(|(_, d)| d.len() > 1) {
        problems.push(format!(
            "namespace {ns} is declared by more than one Graph ({})",
            docs.iter().copied().collect::<Vec<_>>().join(", ")
        ));
    }
    problems
}

/// Look up each distinct node reference once and warn for every vertex whose
/// node does not exist (yet). A failed lookup is a warning too: references
/// are advisory and must not abort the apply.
async fn warn_missing_nodes(api: &GraphApi<'_>, docs: &[&GraphLocal]) {
    let mut refs: BTreeMap<&NodeRef, Vec<(&str, &str)>> = BTreeMap::new();
    for local in docs {
        for v in local.spec.layers.iter().flat_map(|l| &l.vertices) {
            if let Some(node) = &v.node {
                refs.entry(node)
                    .or_default()
                    .push((&local.metadata.name, &v.id));
            }
        }
    }
    for (node, users) in refs {
        let problem = match api
            .node_exists(&node.foreign_source, &node.foreign_id)
            .await
        {
            Ok(true) => continue,
            Ok(false) => "not found".to_string(),
            Err(e) => format!("lookup failed: {e}"),
        };
        for (graph, vertex) in users {
            eprintln!(
                "warning: Graph {graph}: vertex {vertex}: node {}:{} {problem}",
                node.foreign_source, node.foreign_id
            );
        }
    }
}

/// A unified YAML diff under a `Graph/<name>: <verb>` header.
fn render_diff(name: &str, verb: &str, old: &str, new: &str) -> String {
    let body = TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .to_string();
    let body: Vec<String> = body.lines().map(|l| format!("  {l}")).collect();
    format!("Graph/{name}: {verb}\n{}", body.join("\n"))
}

/// The message of a config error without its variant prefix.
fn config_msg(e: Error) -> String {
    match e {
        Error::Config(m) => m,
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GraphSpec;
    use onmsctl_core::kind::parse_documents;
    use onmsctl_core::{AuthCreds, OutputFormat, Url};
    use wiremock::matchers::{body_string, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PROBE: &str = include_str!("../../tests/fixtures/probe-two-layer.yaml");

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

    fn handler() -> GraphHandler {
        GraphHandler {
            load_timeout: Duration::from_millis(50),
            poll_interval: Duration::from_millis(10),
        }
    }

    fn docs(yaml: &str) -> Vec<RawDoc> {
        parse_documents("g.yaml", yaml).unwrap()
    }

    fn probe_spec() -> GraphSpec {
        serde_norway::from_str::<GraphLocal>(PROBE).unwrap().spec
    }

    fn probe_xml() -> String {
        graphml::render("wan-overview", &probe_spec())
    }

    /// The reads every plan issues: containers, nodes, and the stored upload.
    async fn mount_reads(
        server: &MockServer,
        containers: serde_json::Value,
        stored: Option<String>,
    ) {
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(containers))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/nodes/dc-eu:fra-core-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "1"})))
            .mount(server)
            .await;
        let resp = match stored {
            Some(xml) => ResponseTemplate::new(200).set_body_string(xml),
            None => ResponseTemplate::new(404),
        };
        Mock::given(method("GET"))
            .and(path("/rest/graphml/wan-overview"))
            .respond_with(resp)
            .mount(server)
            .await;
    }

    async fn mount_loaded(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs/wan-overview"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "wan-overview"})),
            )
            .mount(server)
            .await;
    }

    async fn writes(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() != "GET")
            .map(|r| format!("{} {}", r.method, r.url.path()))
            .collect()
    }

    fn plan_err(r: Result<Plan>) -> String {
        match r {
            Ok(_) => panic!("expected a plan error"),
            Err(e) => e.to_string(),
        }
    }

    // -- plan ------------------------------------------------------------------

    #[tokio::test]
    async fn absent_upload_plans_a_create_with_a_diff() {
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), None).await;
        let plan = handler()
            .plan(&docs(PROBE), &ApplyParams::default(), &ctx_for(&server))
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Create);
        let diff = plan.diff.unwrap();
        assert!(diff.starts_with("Graph/wan-overview: create"), "{diff}");
        assert!(diff.contains("+  - namespace: wan:regions"), "{diff}");
    }

    #[tokio::test]
    async fn identical_upload_plans_unchanged() {
        let server = MockServer::start().await;
        mount_reads(
            &server,
            serde_json::json!([{"id": "wan-overview", "graphs": [
                {"namespace": "wan:regions"}, {"namespace": "wan:sites"}]}]),
            Some(probe_xml()),
        )
        .await;
        let plan = handler()
            .plan(&docs(PROBE), &ApplyParams::default(), &ctx_for(&server))
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::None);
        assert!(plan.diff.is_none());
    }

    #[tokio::test]
    async fn changed_label_plans_an_update_with_a_yaml_diff() {
        let server = MockServer::start().await;
        let mut live = probe_spec();
        live.layers[1].vertices[0].label = Some("Old Frankfurt".into());
        mount_reads(
            &server,
            serde_json::json!([]),
            Some(graphml::render("wan-overview", &live)),
        )
        .await;
        let plan = handler()
            .plan(&docs(PROBE), &ApplyParams::default(), &ctx_for(&server))
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Update);
        let diff = plan.diff.unwrap();
        assert!(diff.contains("-      label: Old Frankfurt"), "{diff}");
        assert!(diff.contains("+      label: Frankfurt"), "{diff}");
    }

    #[tokio::test]
    async fn foreign_namespace_owner_is_a_plan_error_without_writes() {
        let server = MockServer::start().await;
        mount_reads(
            &server,
            serde_json::json!([{"id": "other", "graphs": [{"namespace": "wan:sites"}]}]),
            None,
        )
        .await;
        let msg = plan_err(
            handler()
                .plan(&docs(PROBE), &ApplyParams::default(), &ctx_for(&server))
                .await,
        );
        assert!(
            msg.contains(
                "Graph wan-overview: namespace wan:sites already belongs to container other"
            ),
            "{msg}"
        );
        assert!(writes(&server).await.is_empty());
    }

    #[tokio::test]
    async fn container_named_like_the_document_without_its_upload_is_foreign() {
        // A legacy upload stored under another name produced container
        // "wan-overview"; this document has no upload of its own yet.
        let server = MockServer::start().await;
        mount_reads(
            &server,
            serde_json::json!([{"id": "wan-overview", "graphs": [{"namespace": "wan:regions"}]}]),
            None,
        )
        .await;
        let msg = plan_err(
            handler()
                .plan(&docs(PROBE), &ApplyParams::default(), &ctx_for(&server))
                .await,
        );
        assert!(
            msg.contains("namespace wan:regions already belongs to container wan-overview"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn legacy_upload_container_counts_as_own_and_plans_the_rename() {
        let server = MockServer::start().await;
        // Stored without containerId: the server named the container after
        // the first <graph id>, which the probe writer sets to the namespace.
        let legacy = probe_xml().replace("  <data key=\"containerId\">wan-overview</data>\n", "");
        mount_reads(
            &server,
            serde_json::json!([{"id": "wan:regions", "graphs": [
                {"namespace": "wan:regions"}, {"namespace": "wan:sites"}]}]),
            Some(legacy),
        )
        .await;
        let plan = handler()
            .plan(&docs(PROBE), &ApplyParams::default(), &ctx_for(&server))
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Update);
        assert!(
            plan.diff
                .unwrap()
                .contains("container id: wan:regions -> wan-overview")
        );
    }

    #[tokio::test]
    async fn same_namespace_in_two_documents_is_a_plan_error() {
        let server = MockServer::start().await;
        let two = "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {name: a}\nspec:\n  layers: [{namespace: shared}]\n---\n\
                   apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {name: b}\nspec:\n  layers: [{namespace: shared}]\n";
        let msg = plan_err(
            handler()
                .plan(&docs(two), &ApplyParams::default(), &ctx_for(&server))
                .await,
        );
        assert!(
            msg.contains("namespace shared is declared by more than one Graph (a, b)"),
            "{msg}"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn duplicate_names_and_validation_errors_gate_before_http() {
        let server = MockServer::start().await;
        let two = format!("{PROBE}---\n{PROBE}");
        let msg = plan_err(
            handler()
                .plan(&docs(&two), &ApplyParams::default(), &ctx_for(&server))
                .await,
        );
        assert!(
            msg.contains("duplicate Graph metadata.name wan-overview (g.yaml#0, g.yaml#1)"),
            "{msg}"
        );

        let bad = "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {name: g}\nspec:\n  layers: [{namespace: \"\"}]\n";
        let msg = plan_err(
            handler()
                .plan(&docs(bad), &ApplyParams::default(), &ctx_for(&server))
                .await,
        );
        assert!(
            msg.contains("g.yaml#0: Graph g: layer 0: namespace is required"),
            "{msg}"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_node_warns_and_each_reference_is_looked_up_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/graphml/g"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/nodes/fs:missing"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        let yaml = "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {name: g}\nspec:\n  layers:\n    - namespace: n\n      vertices:\n\
                    \x20       - {id: a, node: {foreignSource: fs, foreignId: missing}}\n\
                    \x20       - {id: b, node: {foreignSource: fs, foreignId: missing}}\n";
        let plan = handler()
            .plan(&docs(yaml), &ApplyParams::default(), &ctx_for(&server))
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Create);
    }

    // -- execute ---------------------------------------------------------------

    async fn plan_and_execute(server: &MockServer, params: &ApplyParams) -> Vec<ApplyOutcome> {
        let h = handler();
        let ctx = ctx_for(server);
        let plan = h.plan(&docs(PROBE), params, &ctx).await.unwrap();
        h.execute(plan, params, &ctx).await.unwrap()
    }

    fn changed_live() -> String {
        let mut live = probe_spec();
        live.label = Some("Old".into());
        graphml::render("wan-overview", &live)
    }

    #[tokio::test]
    async fn create_posts_the_rendered_xml() {
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), None).await;
        mount_loaded(&server).await;
        Mock::given(method("POST"))
            .and(path("/rest/graphml/wan-overview"))
            .and(body_string(probe_xml()))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Created);
    }

    #[tokio::test]
    async fn update_deletes_then_posts() {
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), Some(changed_live())).await;
        mount_loaded(&server).await;
        Mock::given(method("DELETE"))
            .and(path("/rest/graphml/wan-overview"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/graphml/wan-overview"))
            .and(body_string(probe_xml()))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Updated);
        assert_eq!(
            writes(&server).await,
            vec![
                "DELETE /rest/graphml/wan-overview",
                "POST /rest/graphml/wan-overview"
            ]
        );
    }

    #[tokio::test]
    async fn failed_update_rolls_back_to_the_stored_xml() {
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), Some(changed_live())).await;
        mount_loaded(&server).await;
        Mock::given(method("DELETE"))
            .and(path("/rest/graphml/wan-overview"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string(probe_xml()))
            .respond_with(ResponseTemplate::new(500).set_body_string("parse failure"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string(changed_live()))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Failed);
        assert!(
            out[0]
                .message
                .ends_with("; rolled back to the previous version"),
            "{}",
            out[0].message
        );
        assert!(
            out[0].message.contains("parse failure"),
            "{}",
            out[0].message
        );
    }

    #[tokio::test]
    async fn failed_rollback_says_the_graph_is_absent() {
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), Some(changed_live())).await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("down"))
            .expect(2)
            .mount(&server)
            .await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Failed);
        assert!(
            out[0].message.contains("; rollback failed: "),
            "{}",
            out[0].message
        );
        assert!(
            out[0].message.ends_with("graph wan-overview is now absent"),
            "{}",
            out[0].message
        );
    }

    #[tokio::test]
    async fn update_whose_delete_finds_nothing_continues_as_a_create() {
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), Some(changed_live())).await;
        mount_loaded(&server).await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(404).set_body_string("No GraphML file found"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Updated);
    }

    #[tokio::test]
    async fn create_race_reports_appeared_since_plan() {
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), None).await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_string("Graph with name wan-overview already exists"),
            )
            .mount(&server)
            .await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Failed);
        assert_eq!(
            out[0].message,
            "graph wan-overview appeared since plan; re-run apply"
        );
    }

    /// Mount `GET /api/v2/graphs` answering each body in `seq` once, in
    /// order, then `last` for every later call.
    async fn mount_list_sequence(
        server: &MockServer,
        seq: &[serde_json::Value],
        last: serde_json::Value,
    ) {
        for (i, body) in seq.iter().enumerate() {
            Mock::given(method("GET"))
                .and(path("/api/v2/graphs"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
                .up_to_n_times(1)
                .with_priority((i + 1) as u8)
                .mount(server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(last))
            .with_priority(100)
            .mount(server)
            .await;
    }

    /// Mount the stored upload and the node lookup, without the container list.
    async fn mount_stored(server: &MockServer, stored: Option<String>) {
        Mock::given(method("GET"))
            .and(path("/rest/nodes/dc-eu:fra-core-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "1"})))
            .mount(server)
            .await;
        let resp = match stored {
            Some(xml) => ResponseTemplate::new(200).set_body_string(xml),
            None => ResponseTemplate::new(404),
        };
        Mock::given(method("GET"))
            .and(path("/rest/graphml/wan-overview"))
            .respond_with(resp)
            .mount(server)
            .await;
    }

    /// Methods and paths of every request, except node lookups and uploads reads.
    async fn sequence(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() != "GET" || r.url.path() == "/api/v2/graphs")
            .map(|r| format!("{} {}", r.method, r.url.path()))
            .collect()
    }

    #[tokio::test]
    async fn update_waits_for_the_old_container_to_leave_v2_before_posting() {
        let server = MockServer::start().await;
        mount_stored(&server, Some(changed_live())).await;
        let listed = serde_json::json!([{"id": "wan-overview", "graphs": []}]);
        // plan: listed; after DELETE: still listed, then gone; after POST: loaded.
        mount_list_sequence(
            &server,
            &[listed.clone(), listed.clone(), serde_json::json!([])],
            listed,
        )
        .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Updated);
        assert_eq!(
            sequence(&server).await,
            [
                "GET /api/v2/graphs",
                "DELETE /rest/graphml/wan-overview",
                "GET /api/v2/graphs",
                "GET /api/v2/graphs",
                "POST /rest/graphml/wan-overview",
                "GET /api/v2/graphs",
            ]
        );
    }

    #[tokio::test]
    async fn renaming_update_waits_for_the_old_container_id() {
        let server = MockServer::start().await;
        let legacy = probe_xml().replace("  <data key=\"containerId\">wan-overview</data>\n", "");
        mount_stored(&server, Some(legacy)).await;
        let old = serde_json::json!([{"id": "wan:regions", "graphs": [
            {"namespace": "wan:regions"}, {"namespace": "wan:sites"}]}]);
        let new = serde_json::json!([{"id": "wan-overview", "graphs": []}]);
        // plan: old; after DELETE: old still there, then gone; after POST: new.
        mount_list_sequence(&server, &[old.clone(), old, serde_json::json!([])], new).await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Updated, "{out:?}");
        let seq = sequence(&server).await;
        assert_eq!(
            &seq[1..5],
            [
                "DELETE /rest/graphml/wan-overview",
                "GET /api/v2/graphs",
                "GET /api/v2/graphs",
                "POST /rest/graphml/wan-overview"
            ],
            "{seq:?}"
        );
    }

    #[tokio::test]
    async fn slow_load_is_a_warning_not_a_failure() {
        let server = MockServer::start().await;
        // The container never appears in the list.
        mount_reads(&server, serde_json::json!([]), None).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Created);
        let polls = sequence(&server)
            .await
            .iter()
            .filter(|r| r.ends_with("/api/v2/graphs"))
            .count();
        assert!(polls >= 3, "plan read plus at least two polls, got {polls}");
    }

    #[tokio::test]
    async fn unparseable_own_upload_plans_a_replacing_update() {
        let server = MockServer::start().await;
        // The stored upload loaded (it owns the namespaces) but onmsctl cannot parse it.
        mount_reads(
            &server,
            serde_json::json!([{"id": "wan-overview", "graphs": [
                {"namespace": "wan:regions"}, {"namespace": "wan:sites"}]}]),
            Some("<graphml><graph id=\"a\"></graphml>".into()),
        )
        .await;
        let plan = handler()
            .plan(&docs(PROBE), &ApplyParams::default(), &ctx_for(&server))
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Update);
        assert!(plan.diff.unwrap().contains("could not be parsed"));
    }

    #[tokio::test]
    async fn node_lookup_failure_warns_and_unchanged_graphs_skip_lookups() {
        // A 500 from the node lookup must not abort the plan.
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), None).await;
        Mock::given(method("GET"))
            .and(path("/rest/nodes/dc-eu:fra-core-01"))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(1)
            .mount(&server)
            .await;
        let plan = handler()
            .plan(&docs(PROBE), &ApplyParams::default(), &ctx_for(&server))
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Create);

        // An unchanged graph issues no node lookups at all.
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), Some(probe_xml())).await;
        let plan = handler()
            .plan(&docs(PROBE), &ApplyParams::default(), &ctx_for(&server))
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::None);
        assert!(
            !server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path().starts_with("/rest/nodes/")),
            "no node lookup for an unchanged graph"
        );
    }

    #[tokio::test]
    async fn unchanged_sends_no_writes() {
        let server = MockServer::start().await;
        mount_reads(&server, serde_json::json!([]), Some(probe_xml())).await;
        let out = plan_and_execute(&server, &ApplyParams::default()).await;
        assert_eq!(out[0].status, OutcomeStatus::Unchanged);
        assert!(writes(&server).await.is_empty());
    }

    #[tokio::test]
    async fn stop_on_error_skips_later_graphs_and_continue_on_error_runs_them() {
        let second = PROBE
            .replace("name: wan-overview", "name: wan-two")
            .replace("wan:regions", "two:regions")
            .replace("wan:sites", "two:sites");
        let input = format!("{PROBE}---\n{second}");
        for (continue_on_error, expected) in [
            (false, OutcomeStatus::Skipped),
            (true, OutcomeStatus::Created),
        ] {
            let server = MockServer::start().await;
            mount_reads(&server, serde_json::json!([]), None).await;
            Mock::given(method("GET"))
                .and(path("/rest/graphml/wan-two"))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/rest/graphml/wan-overview"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/rest/graphml/wan-two"))
                .respond_with(ResponseTemplate::new(201))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/api/v2/graphs/wan-two"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
                .mount(&server)
                .await;
            let params = ApplyParams {
                continue_on_error,
                ..ApplyParams::default()
            };
            let h = handler();
            let ctx = ctx_for(&server);
            let plan = h.plan(&docs(&input), &params, &ctx).await.unwrap();
            let out = h.execute(plan, &params, &ctx).await.unwrap();
            assert_eq!(out[0].status, OutcomeStatus::Failed);
            assert_eq!(
                out[1].status, expected,
                "continue_on_error={continue_on_error}"
            );
        }
    }
}
