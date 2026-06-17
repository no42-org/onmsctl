/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `BusinessServiceHandler` — the BSM capability's adapter into the kind-router.
//!
//! `kind: BusinessService` is named and multi-instance (one document per
//! service). `plan()` parses the bucket, gates on duplicate `metadata.name`,
//! validates each document, rejects child-reference cycles, fetches current
//! state, and resolves every name reference to a numeric id (applications, IP
//! services, nodes, reduction-key `{{nodeId}}` templates) — an unresolvable or
//! ambiguous reference aborts the whole apply before any write. Child-service
//! references to services *also created in this apply* are deferred to execute.
//!
//! `execute()` is the **two-pass** reconcile (DD6): pass 1 POSTs a minimal body
//! (name + attributes + reduce function, no edges) for each new service to
//! obtain its id; pass 2 PUTs the complete body — including all four edge
//! collections with resolved ids — for every created or changed service. A
//! single bsmd `daemon/reload` runs after any successful write.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OnmsClient, OutcomeStatus,
    Plan, RawDoc, Result,
};

use crate::api::BusinessServiceApi;
use crate::convert;
use crate::model::{BusinessServiceLocal, KIND, NodeRef, template_tokens};
use crate::server::{
    ApplicationEdgeRequest, BusinessServiceRequest, BusinessServiceResponse, ChildEdgeRequest,
    FunctionDto, IpServiceEdgeRequest, ReductionKeyEdgeRequest,
};

/// Handler for `kind: BusinessService` documents.
#[derive(Default)]
pub struct BusinessServiceHandler;

/// A child edge whose `child-id` resolves only after pass 1 (the referenced
/// service is created in the same apply).
struct PendingChild {
    name: String,
    weight: i64,
    map_function: Option<FunctionDto>,
}

/// One service whose read-only plan succeeded.
struct PlannedService {
    name: String,
    /// `Some` if the service already exists on the server.
    existing_id: Option<i64>,
    /// The desired request with everything resolved except `pending_children`.
    base_request: BusinessServiceRequest,
    /// Child edges resolved at execute (referenced service created this apply).
    pending_children: Vec<PendingChild>,
    action: Action,
}

struct ExecPayload {
    services: Vec<PlannedService>,
}

#[async_trait]
impl KindHandler for BusinessServiceHandler {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        // Parse the whole bucket (gate on parse failure).
        let mut locals: Vec<BusinessServiceLocal> = Vec::with_capacity(docs.len());
        for d in docs {
            let local: BusinessServiceLocal =
                serde_norway::from_value(d.value.clone()).map_err(|e| {
                    Error::Config(format!(
                        "{}: invalid `kind: BusinessService` document: {e}",
                        d.label()
                    ))
                })?;
            locals.push(local);
        }

        // Gate: duplicate metadata.name within the bucket.
        if let Some(dup) = first_duplicate(locals.iter().map(|l| l.metadata.name.as_str())) {
            return Err(Error::Config(format!(
                "kind: BusinessService names must be unique within an apply — duplicate metadata.name {dup:?}"
            )));
        }

        // Gate: validate each document before any HTTP.
        for local in &locals {
            local.validate()?;
        }

        // Gate: reject child-reference cycles among the applied set.
        detect_cycle(&locals)?;

        let applied_names: HashSet<&str> =
            locals.iter().map(|l| l.metadata.name.as_str()).collect();

        let client = OnmsClient::from_context(ctx)?;
        let api = BusinessServiceApi::new(&client);

        // Current state: name → (id, response).
        let current: HashMap<String, (i64, BusinessServiceResponse)> = api
            .fetch_all()
            .await?
            .into_iter()
            .map(|(id, r)| (r.name.clone(), (id, r)))
            .collect();

        let mut previews = Vec::with_capacity(locals.len());
        let mut services = Vec::with_capacity(locals.len());
        for local in &locals {
            let planned = plan_service(local, &api, &applied_names, &current).await?;
            previews.push(ApplyOutcome::would(
                KIND,
                planned.name.clone(),
                planned.action,
            ));
            services.push(planned);
        }

        Ok(Plan::new(previews, Box::new(ExecPayload { services })).with_diff(Some(
            "business-service: each service is reconciled as a whole object (create / full PUT). \
             Edges omitted from a document are pruned; a service present on the server but absent \
             from this apply is NOT deleted (use `onmsctl business-service delete <name>`)."
                .to_string(),
        )))
    }

    async fn execute(
        &self,
        plan: Plan,
        _params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let payload = plan.payload.downcast::<ExecPayload>().map_err(|_| {
            Error::Config("internal: BusinessServiceHandler payload type mismatch".into())
        })?;
        let client = OnmsClient::from_context(ctx)?;
        let api = BusinessServiceApi::new(&client);

        // -- Pass 1: create each new service with a minimal body to obtain its id. --
        let mut failed_create: HashMap<String, String> = HashMap::new();
        for ps in &payload.services {
            if ps.action == Action::Create {
                let minimal = BusinessServiceRequest {
                    name: ps.name.clone(),
                    attributes: ps.base_request.attributes.clone(),
                    reduce_function: ps.base_request.reduce_function.clone(),
                    ..Default::default()
                };
                if let Err(e) = api.create(&minimal).await {
                    failed_create.insert(ps.name.clone(), e.to_string());
                }
            }
        }

        // Map every applied service name → its server id (existing + newly created).
        let name_to_id: HashMap<String, i64> = api
            .fetch_all()
            .await?
            .into_iter()
            .map(|(id, r)| (r.name, id))
            .collect();

        // -- Pass 2: PUT the full body for every created/changed service. --
        let mut outcomes = Vec::with_capacity(payload.services.len());
        let mut wrote = false;
        for ps in &payload.services {
            if ps.action == Action::None {
                outcomes.push(ApplyOutcome::new(
                    KIND,
                    ps.name.clone(),
                    Action::None,
                    OutcomeStatus::Unchanged,
                    "in sync",
                ));
                continue;
            }
            if let Some(err) = failed_create.get(&ps.name) {
                outcomes.push(ApplyOutcome::failed(
                    KIND,
                    ps.name.clone(),
                    Action::Create,
                    format!("create failed: {err}"),
                    "verify connectivity and re-apply",
                ));
                continue;
            }

            // Resolve child id for each pending child via the post-pass-1 map.
            let mut req = ps.base_request.clone();
            let mut missing: Option<String> = None;
            for pc in &ps.pending_children {
                match name_to_id.get(&pc.name) {
                    Some(id) => req.child_edges.push(ChildEdgeRequest {
                        child_id: *id,
                        weight: pc.weight,
                        map_function: pc.map_function.clone(),
                    }),
                    None => {
                        missing = Some(pc.name.clone());
                        break;
                    }
                }
            }
            if let Some(name) = missing {
                outcomes.push(ApplyOutcome::failed(
                    KIND,
                    ps.name.clone(),
                    ps.action,
                    format!("child service {name:?} was not created/found"),
                    "ensure the referenced service is in this apply and re-apply",
                ));
                continue;
            }

            let id = match ps.existing_id.or_else(|| name_to_id.get(&ps.name).copied()) {
                Some(id) => id,
                None => {
                    outcomes.push(ApplyOutcome::failed(
                        KIND,
                        ps.name.clone(),
                        ps.action,
                        "could not resolve the service id after create",
                        "re-apply",
                    ));
                    continue;
                }
            };

            match api.replace(id, &req).await {
                Ok(()) => {
                    wrote = true;
                    let (status, msg) = match ps.action {
                        Action::Create => (OutcomeStatus::Created, "created"),
                        _ => (OutcomeStatus::Updated, "updated"),
                    };
                    outcomes.push(ApplyOutcome::new(
                        KIND,
                        ps.name.clone(),
                        ps.action,
                        status,
                        msg,
                    ));
                }
                Err(e) => outcomes.push(ApplyOutcome::failed(
                    KIND,
                    ps.name.clone(),
                    ps.action,
                    format!("write failed: {e}"),
                    "verify connectivity and re-apply",
                )),
            }
        }

        // -- Single bsmd reload after any successful write. --
        if wrote && let Err(e) = api.reload().await {
            eprintln!(
                "warning: business-service writes succeeded but bsmd daemon/reload failed ({e}); \
                 changes will not be active until bsmd is reloaded (POST .../business-services/daemon/reload)"
            );
        }

        Ok(outcomes)
    }
}

/// Read-only plan for one service: resolve references and decide the action.
/// Returns `Err` to abort the whole apply on an unresolvable/ambiguous reference
/// (the spec's "abort before any mutating HTTP").
async fn plan_service(
    local: &BusinessServiceLocal,
    api: &BusinessServiceApi<'_>,
    applied_names: &HashSet<&str>,
    current: &HashMap<String, (i64, BusinessServiceResponse)>,
) -> Result<PlannedService> {
    let name = local.metadata.name.clone();
    let spec = &local.spec;

    let mut req = BusinessServiceRequest {
        name: name.clone(),
        attributes: crate::server::AttributeList::from_map(&spec.attributes),
        reduce_function: Some(convert::reduce_dto(&spec.reduce_function)),
        ..Default::default()
    };

    // Applications → application-id.
    for e in &spec.applications {
        let id = api.resolve_application(&e.name).await?.ok_or_else(|| {
            Error::Config(format!(
                "{name}: application {:?} not found (resolve to an id failed)",
                e.name
            ))
        })?;
        req.application_edges.push(ApplicationEdgeRequest {
            application_id: id,
            weight: e.weight,
            map_function: Some(convert::map_dto(&e.map_function)),
        });
    }

    // IP services → ip-service-id (ifserviceid).
    for e in &spec.ip_services {
        let node_id = api.resolve_node(&e.node.form()?).await?;
        let svc_id = api
            .resolve_ifservice(node_id, &e.ip_address, &e.service)
            .await?
            .ok_or_else(|| {
                Error::Config(format!(
                    "{name}: monitored service {}/{} on node {node_id} not found",
                    e.ip_address, e.service
                ))
            })?;
        req.ip_service_edges.push(IpServiceEdgeRequest {
            ip_service_id: svc_id,
            weight: e.weight,
            friendly_name: e.friendly_name.clone(),
            map_function: Some(convert::map_dto(&e.map_function)),
        });
    }

    // Reduction keys → expand any {{token}} from the node reference.
    for e in &spec.reduction_keys {
        let expanded = expand_reduction_key(&e.reduction_key, e.node.as_ref(), api)
            .await
            .map_err(|m| Error::Config(format!("{name}: {m}")))?;
        req.reduction_key_edges.push(ReductionKeyEdgeRequest {
            reduction_key: expanded,
            weight: e.weight,
            friendly_name: e.friendly_name.clone(),
            map_function: Some(convert::map_dto(&e.map_function)),
        });
    }

    // Child services → resolve now if the child already exists; otherwise defer
    // to execute (created in this apply). A child neither on the server nor in
    // this apply is a plan error.
    let mut pending_children = Vec::new();
    for e in &spec.child_services {
        if let Some((id, _)) = current.get(&e.name) {
            req.child_edges.push(ChildEdgeRequest {
                child_id: *id,
                weight: e.weight,
                map_function: Some(convert::map_dto(&e.map_function)),
            });
        } else if applied_names.contains(e.name.as_str()) {
            pending_children.push(PendingChild {
                name: e.name.clone(),
                weight: e.weight,
                map_function: Some(convert::map_dto(&e.map_function)),
            });
        } else {
            return Err(Error::Config(format!(
                "{name}: child service {:?} not found (not on the server and not in this apply)",
                e.name
            )));
        }
    }

    let existing = current.get(&name);
    let action = match existing {
        None => Action::Create,
        Some((_, resp)) => {
            if !pending_children.is_empty() {
                // A new child edge will be added → not in sync.
                Action::Update
            } else if convert::unchanged(&req, resp) {
                Action::None
            } else {
                Action::Update
            }
        }
    };

    Ok(PlannedService {
        name,
        existing_id: existing.map(|(id, _)| *id),
        base_request: req,
        pending_children,
        action,
    })
}

/// Expand node-derived `{{token}}`s in a reduction-key string. Resolves the node
/// id via the API only when `{{nodeId}}` is present. Returns a message fragment
/// on any expansion failure (token/form mismatch, unresolved node).
async fn expand_reduction_key(
    key: &str,
    node: Option<&NodeRef>,
    api: &BusinessServiceApi<'_>,
) -> std::result::Result<String, String> {
    let tokens = template_tokens(key);
    if tokens.is_empty() {
        return Ok(key.to_string());
    }
    let node = node.ok_or("reductionKey has a {{token}} but no node reference")?;
    let form = node.form().map_err(|e| e.to_string())?;

    // Resolve the numeric id only if {{nodeId}} is actually used.
    let needs_id = tokens.iter().any(|t| t.eq_ignore_ascii_case("nodeId"));
    let node_id = if needs_id {
        Some(api.resolve_node(&form).await.map_err(|e| e.to_string())?)
    } else {
        None
    };

    let mut out = key.to_string();
    for t in &tokens {
        let value = token_value(t, node, node_id)?;
        // Replace every spelling of this token (whitespace-tolerant).
        out = replace_token(&out, t, &value);
    }
    Ok(out)
}

/// The replacement value for one token, or an error if it cannot be derived from
/// the node reference's form.
fn token_value(
    token: &str,
    node: &NodeRef,
    node_id: Option<i64>,
) -> std::result::Result<String, String> {
    let t = token.to_ascii_lowercase();
    match t.as_str() {
        "nodeid" => node_id
            .map(|i| i.to_string())
            .ok_or_else(|| "internal: nodeId not resolved".to_string()),
        "foreignsource" => node.foreign_source.clone().ok_or_else(|| {
            "{{foreignSource}} requires a {foreignSource, foreignId} node".to_string()
        }),
        "foreignid" => node
            .foreign_id
            .clone()
            .ok_or_else(|| "{{foreignId}} requires a {foreignSource, foreignId} node".to_string()),
        "nodelabel" => node
            .label
            .clone()
            .ok_or_else(|| "{{nodeLabel}} requires a {label, location} node".to_string()),
        other => Err(format!("unsupported template token {{{{{other}}}}}")),
    }
}

/// Replace every `{{token}}` occurrence (whitespace-tolerant, case-insensitive)
/// with `value`.
fn replace_token(s: &str, token: &str, value: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        if let Some(close) = after.find("}}") {
            let inner = after[..close].trim();
            if inner.eq_ignore_ascii_case(token) {
                out.push_str(value);
            } else {
                // Leave non-matching tokens for a later pass.
                out.push_str("{{");
                out.push_str(&after[..close]);
                out.push_str("}}");
            }
            rest = &after[close + 2..];
        } else {
            out.push_str(&rest[open..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// Reject a child-reference cycle among the applied set (edges to services not
/// in the apply are leaves). DFS with a recursion stack.
fn detect_cycle(locals: &[BusinessServiceLocal]) -> Result<()> {
    let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();
    for l in locals {
        let children: Vec<&str> = l
            .spec
            .child_services
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        graph.insert(l.metadata.name.as_str(), children);
    }
    let mut state: HashMap<&str, u8> = HashMap::new(); // 0/absent = unvisited, 1 = on stack, 2 = done
    for &node in graph.keys() {
        if let Some(cycle) = visit(node, &graph, &mut state) {
            return Err(Error::Config(format!(
                "kind: BusinessService child references form a cycle: {}",
                cycle.join(" -> ")
            )));
        }
    }
    Ok(())
}

fn visit<'a>(
    node: &'a str,
    graph: &HashMap<&'a str, Vec<&'a str>>,
    state: &mut HashMap<&'a str, u8>,
) -> Option<Vec<String>> {
    match state.get(node) {
        Some(2) => return None,
        Some(1) => return Some(vec![node.to_string()]),
        _ => {}
    }
    state.insert(node, 1);
    if let Some(children) = graph.get(node) {
        for &c in children {
            // Only edges to applied services can form a cycle.
            if graph.contains_key(c)
                && let Some(mut path) = visit(c, graph, state)
            {
                path.insert(0, node.to_string());
                return Some(path);
            }
        }
    }
    state.insert(node, 2);
    None
}

/// First duplicate in an iterator of names, if any.
fn first_duplicate<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut seen = HashSet::new();
    for n in names {
        if !seen.insert(n) {
            return Some(n.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::kind::parse_documents;
    use onmsctl_core::{AuthCreds, OutputFormat, Url};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx_for(server: &MockServer) -> Context {
        Context {
            name: "test".into(),
            url: Url::parse(&format!("{}/", server.uri())).unwrap(),
            creds: AuthCreds::basic("admin", "secret"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        }
    }

    fn docs(specs: &[&str]) -> Vec<RawDoc> {
        parse_documents("bs.yaml", &specs.join("---\n")).unwrap()
    }

    fn svc(name: &str, body: &str) -> String {
        format!(
            "apiVersion: bsm.opennms.org/v1\nkind: BusinessService\nmetadata: {{ name: {name} }}\nspec:\n{body}"
        )
    }

    /// Mount a `GET /api/v2/business-services` list that returns 204 (empty) for
    /// the first call, then the populated `uris` for every later call. wiremock
    /// checks the last-mounted mock first, and `up_to_n_times(1)` exhausts the
    /// empty one after the plan's read so execute sees the created services.
    async fn mount_sequenced_list(server: &MockServer, uris: serde_json::Value) {
        // Default-priority (5) populated list for every call…
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "business-services": uris })),
            )
            .mount(server)
            .await;
        // …but a higher-priority (1) empty 204 wins for the first call only, then
        // exhausts so execute (after pass-1 create) sees the populated list.
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(204))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(server)
            .await;
    }

    fn count(reqs: &[wiremock::Request], m: &str, p: &str) -> usize {
        reqs.iter()
            .filter(|r| r.method.as_str() == m && r.url.path() == p)
            .count()
    }

    /// Empty server → a single service is created (POST minimal + PUT full),
    /// then exactly one bsmd reload.
    #[tokio::test]
    async fn create_from_empty_then_reload() {
        let server = MockServer::start().await;
        mount_sequenced_list(&server, serde_json::json!(["/api/v2/business-services/1"])).await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 1, "name": "web" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services/daemon/reload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let body = "  reduceFunction: { type: HighestSeverity }\n  reductionKeys:\n    - { reductionKey: \"k::1\" }\n";

        let plan = BusinessServiceHandler
            .plan(&docs(&[&svc("web", body)]), &params, &ctx)
            .await
            .unwrap();
        assert_eq!(plan.preview.len(), 1);
        assert_eq!(plan.preview[0].action, Action::Create);

        let outcomes = BusinessServiceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);

        let reqs = server.received_requests().await.unwrap();
        assert_eq!(
            count(&reqs, "POST", "/api/v2/business-services"),
            1,
            "one create"
        );
        assert_eq!(
            count(&reqs, "PUT", "/api/v2/business-services/1"),
            1,
            "one full PUT"
        );
        assert_eq!(
            count(&reqs, "POST", "/api/v2/business-services/daemon/reload"),
            1,
            "exactly one reload"
        );
    }

    /// A service already in sync is `Unchanged` — no POST, PUT, or reload.
    #[tokio::test]
    async fn no_op_reapply_writes_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "business-services": ["/api/v2/business-services/1"] }),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1, "name": "web",
                "reduce-function": { "type": "HighestSeverity", "properties": {} },
                "reduction-key-edges": [ { "reduction-keys": ["k::1"], "weight": 1 } ]
            })))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let body = "  reduceFunction: { type: HighestSeverity }\n  reductionKeys:\n    - { reductionKey: \"k::1\" }\n";
        let plan = BusinessServiceHandler
            .plan(&docs(&[&svc("web", body)]), &params, &ctx)
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::None);
        let outcomes = BusinessServiceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes[0].status, OutcomeStatus::Unchanged);

        let reqs = server.received_requests().await.unwrap();
        assert!(
            reqs.iter().all(|r| r.method.as_str() == "GET"),
            "no-op must issue no POST/PUT/reload"
        );
    }

    /// Removing an edge from an existing service yields an Update whose PUT body
    /// carries only the retained edge (full-replace prunes the omitted one).
    #[tokio::test]
    async fn edge_pruned_on_update() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "business-services": ["/api/v2/business-services/1"] }),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1, "name": "web",
                "reduce-function": { "type": "HighestSeverity", "properties": {} },
                "reduction-key-edges": [
                    { "reduction-keys": ["a"], "weight": 1 },
                    { "reduction-keys": ["b"], "weight": 1 }
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services/daemon/reload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let body = "  reduceFunction: { type: HighestSeverity }\n  reductionKeys:\n    - { reductionKey: \"a\" }\n";
        let plan = BusinessServiceHandler
            .plan(&docs(&[&svc("web", body)]), &params, &ctx)
            .await
            .unwrap();
        assert_eq!(plan.preview[0].action, Action::Update);
        let outcomes = BusinessServiceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes[0].status, OutcomeStatus::Updated);

        let reqs = server.received_requests().await.unwrap();
        let put = reqs
            .iter()
            .find(|r| r.method.as_str() == "PUT")
            .expect("a PUT was issued");
        let body = String::from_utf8_lossy(&put.body);
        assert!(body.contains("\"a\""), "retained edge present: {body}");
        assert!(!body.contains("\"b\""), "pruned edge absent: {body}");
    }

    /// Two new services where `a` references child `b`: the two-pass execute
    /// creates both, then PUTs `a` with `b`'s resolved child-id.
    #[tokio::test]
    async fn cross_document_child_two_pass() {
        let server = MockServer::start().await;
        // After pass 1, the list is populated with both services.
        mount_sequenced_list(
            &server,
            serde_json::json!(["/api/v2/business-services/1", "/api/v2/business-services/2"]),
        )
        .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        // /1 is "b", /2 is "a" (server-assigned, arbitrary).
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 1, "name": "b" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/2"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 2, "name": "a" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/business-services/2"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services/daemon/reload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let a = svc(
            "a",
            "  reduceFunction: { type: HighestSeverity }\n  childServices: [ { name: b } ]\n",
        );
        let b = svc("b", "  reduceFunction: { type: HighestSeverity }\n");
        let plan = BusinessServiceHandler
            .plan(&docs(&[&a, &b]), &params, &ctx)
            .await
            .unwrap();
        assert_eq!(plan.preview.len(), 2);
        let outcomes = BusinessServiceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert!(outcomes.iter().all(|o| o.status == OutcomeStatus::Created));

        // `a` (id 2) PUT must carry child-id 1 (b's id).
        let reqs = server.received_requests().await.unwrap();
        let put_a = reqs
            .iter()
            .find(|r| r.method.as_str() == "PUT" && r.url.path() == "/api/v2/business-services/2")
            .expect("a's PUT issued");
        let body = String::from_utf8_lossy(&put_a.body);
        assert!(
            body.contains("\"child-id\":1"),
            "a references b's id: {body}"
        );
        assert_eq!(
            count(&reqs, "POST", "/api/v2/business-services"),
            2,
            "both created"
        );
        assert_eq!(
            count(&reqs, "POST", "/api/v2/business-services/daemon/reload"),
            1,
            "exactly one reload for the whole apply"
        );
    }

    /// Driven through the kind-router: exactly one preview + one outcome per
    /// document (the router rejects a handler that emits a row per sub-resource).
    #[tokio::test]
    async fn router_path_one_row_per_document() {
        use onmsctl_core::Registry;
        use onmsctl_core::kind::apply_documents;

        let server = MockServer::start().await;
        mount_sequenced_list(&server, serde_json::json!(["/api/v2/business-services/1"])).await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 1, "name": "web" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services/daemon/reload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut reg = Registry::new();
        reg.register(900, Box::new(BusinessServiceHandler));
        let body = "  reduceFunction: { type: HighestSeverity }\n";
        let outcomes = apply_documents(
            &reg,
            docs(&[&svc("web", body)]),
            &ApplyParams::default(),
            &ctx_for(&server),
        )
        .await
        .expect("router accepts one row per document");
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);
    }

    #[tokio::test]
    async fn duplicate_name_gates_before_http() {
        let server = MockServer::start().await;
        let body = "  reduceFunction: { type: HighestSeverity }\n";
        let err = BusinessServiceHandler
            .plan(
                &docs(&[&svc("web", body), &svc("web", body)]),
                &ApplyParams::default(),
                &ctx_for(&server),
            )
            .await
            .err()
            .expect("expected a plan error");
        assert!(err.to_string().contains("unique"), "got: {err}");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "gate before any HTTP"
        );
    }

    #[tokio::test]
    async fn child_cycle_is_rejected_before_http() {
        let server = MockServer::start().await;
        let a = svc(
            "a",
            "  reduceFunction: { type: HighestSeverity }\n  childServices: [ { name: b } ]\n",
        );
        let b = svc(
            "b",
            "  reduceFunction: { type: HighestSeverity }\n  childServices: [ { name: a } ]\n",
        );
        let err = BusinessServiceHandler
            .plan(&docs(&[&a, &b]), &ApplyParams::default(), &ctx_for(&server))
            .await
            .err()
            .expect("expected a plan error");
        assert!(err.to_string().contains("cycle"), "got: {err}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn unresolved_application_aborts_plan() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/applications"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "application": [] })),
            )
            .mount(&server)
            .await;
        let body =
            "  reduceFunction: { type: HighestSeverity }\n  applications: [ { name: Ghost } ]\n";
        let err = BusinessServiceHandler
            .plan(
                &docs(&[&svc("web", body)]),
                &ApplyParams::default(),
                &ctx_for(&server),
            )
            .await
            .err()
            .expect("expected a plan error");
        assert!(err.to_string().contains("application"), "got: {err}");
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn unknown_child_not_in_apply_or_server_aborts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let body =
            "  reduceFunction: { type: HighestSeverity }\n  childServices: [ { name: nowhere } ]\n";
        let err = BusinessServiceHandler
            .plan(
                &docs(&[&svc("web", body)]),
                &ApplyParams::default(),
                &ctx_for(&server),
            )
            .await
            .err()
            .expect("expected a plan error");
        assert!(err.to_string().contains("child service"), "got: {err}");
    }

    /// A templated reduction key resolves its node and the PUT carries the
    /// fully-expanded literal (the `{{nodeId}}` end-to-end path).
    #[tokio::test]
    async fn reduction_key_template_expanded_in_put() {
        let server = MockServer::start().await;
        mount_sequenced_list(&server, serde_json::json!(["/api/v2/business-services/1"])).await;
        Mock::given(method("GET"))
            .and(path("/api/v2/nodes"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "node": [ { "id": 27 } ] })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 1, "name": "web" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services/daemon/reload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let body = "  reduceFunction: { type: HighestSeverity }\n  reductionKeys:\n    - { reductionKey: \"k::{{nodeId}}:x\", node: { label: webhost01 } }\n";
        let plan = BusinessServiceHandler
            .plan(&docs(&[&svc("web", body)]), &params, &ctx)
            .await
            .unwrap();
        let outcomes = BusinessServiceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);

        let reqs = server.received_requests().await.unwrap();
        let put = reqs
            .iter()
            .find(|r| r.method.as_str() == "PUT")
            .expect("PUT issued");
        let put_body = String::from_utf8_lossy(&put.body);
        assert!(
            put_body.contains("k::27:x"),
            "expanded key in PUT: {put_body}"
        );
    }

    /// A service present on the server but absent from the apply is NOT deleted
    /// (the across-apply non-deletion contract). Applying an in-sync `web` while
    /// `legacy` also exists issues no DELETE.
    #[tokio::test]
    async fn apply_does_not_delete_server_only_service() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "business-services": [
                    "/api/v2/business-services/1",
                    "/api/v2/business-services/2"
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1, "name": "web",
                "reduce-function": { "type": "HighestSeverity", "properties": {} }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 2, "name": "legacy",
                "reduce-function": { "type": "HighestSeverity", "properties": {} }
            })))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let body = "  reduceFunction: { type: HighestSeverity }\n";
        let plan = BusinessServiceHandler
            .plan(&docs(&[&svc("web", body)]), &params, &ctx)
            .await
            .unwrap();
        let outcomes = BusinessServiceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1, "only the applied service is reported");
        assert_eq!(outcomes[0].status, OutcomeStatus::Unchanged);
        let reqs = server.received_requests().await.unwrap();
        assert!(
            reqs.iter().all(|r| r.method.as_str() != "DELETE"),
            "apply must never delete a server-only service"
        );
    }

    /// An omitted `reduceFunction` and an edge with no `mapFunction` are
    /// materialized to HighestSeverity / Identity in the PUT — the server has no
    /// defaults and 500s on a null function, so onmsctl must always send them.
    #[tokio::test]
    async fn omitted_functions_are_defaulted_in_put() {
        let server = MockServer::start().await;
        mount_sequenced_list(&server, serde_json::json!(["/api/v2/business-services/1"])).await;
        Mock::given(method("GET"))
            .and(path("/api/v2/applications"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "application": [ { "id": 3, "name": "Webservers" } ] }),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 1, "name": "web" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services/daemon/reload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        // No reduceFunction; an application edge with no mapFunction.
        let body = "  applications: [ { name: Webservers } ]\n";
        let plan = BusinessServiceHandler
            .plan(&docs(&[&svc("web", body)]), &params, &ctx)
            .await
            .unwrap();
        let outcomes = BusinessServiceHandler
            .execute(plan, &params, &ctx)
            .await
            .unwrap();
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);

        let reqs = server.received_requests().await.unwrap();
        let put = reqs
            .iter()
            .find(|r| r.method.as_str() == "PUT")
            .expect("PUT issued");
        let put_body = String::from_utf8_lossy(&put.body);
        assert!(
            put_body.contains("HighestSeverity"),
            "reduce-function defaulted: {put_body}"
        );
        assert!(
            put_body.contains("Identity"),
            "edge map-function defaulted: {put_body}"
        );
        assert!(put_body.contains("\"application-id\":3"));
    }

    #[test]
    fn replace_token_is_case_insensitive_and_whitespace_tolerant() {
        assert_eq!(replace_token("a::{{nodeId}}:b", "nodeId", "27"), "a::27:b");
        assert_eq!(
            replace_token("a::{{ nodeid }}:b", "nodeId", "27"),
            "a::27:b"
        );
        // non-matching token left intact
        assert_eq!(
            replace_token("a::{{foreignId}}", "nodeId", "27"),
            "a::{{foreignId}}"
        );
    }
}
