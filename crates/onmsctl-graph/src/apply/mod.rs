/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `kind: Graph` adapter into the core kind-router.

mod handler;

pub use handler::GraphHandler;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use onmsctl_core::kind::precedence::{RANK_GRAPH, RANK_REQUISITION};
    use onmsctl_core::{
        Action, ApplyOutcome, ApplyParams, AuthCreds, Context, KindHandler, OnmsClient,
        OutcomeStatus, OutputFormat, Plan, RawDoc, Registry, Result, Url, apply_documents,
        parse_documents,
    };
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Stands in for the provisioning handler: one POST per apply, so the
    /// request log shows when its bucket ran.
    struct StubRequisition;

    #[async_trait]
    impl KindHandler for StubRequisition {
        fn kind(&self) -> &'static str {
            "Requisition"
        }
        async fn plan(&self, docs: &[RawDoc], _: &ApplyParams, _: &Context) -> Result<Plan> {
            let preview = docs
                .iter()
                .map(|_| ApplyOutcome::would("Requisition", "r", Action::Create))
                .collect();
            Ok(Plan::new(preview, Box::new(())))
        }
        async fn execute(
            &self,
            _: Plan,
            _: &ApplyParams,
            ctx: &Context,
        ) -> Result<Vec<ApplyOutcome>> {
            OnmsClient::from_context(ctx)?
                .post_drain("rest/requisitions", &serde_json::json!({}))
                .await?;
            Ok(vec![ApplyOutcome::new(
                "Requisition",
                "r",
                Action::Create,
                OutcomeStatus::Created,
                "created",
            )])
        }
    }

    const REQUISITION: &str = "apiVersion: provisioning.opennms.org/v1\nkind: Requisition\nmetadata: {name: r}\nspec: {}\n";
    const GRAPH: &str = "apiVersion: graphml.opennms.org/v1\nkind: Graph\nmetadata: {name: g}\nspec:\n  layers: [{namespace: n, vertices: [{id: a}]}]\n";

    fn registry() -> Registry {
        let mut reg = Registry::new();
        reg.register(RANK_REQUISITION, Box::new(StubRequisition));
        reg.register(
            RANK_GRAPH,
            Box::new(GraphHandler {
                load_timeout: Duration::from_millis(10),
                poll_interval: Duration::from_millis(5),
            }),
        );
        reg
    }

    fn ctx(server: &MockServer) -> Context {
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

    async fn mount_graph_reads(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/v2/graphs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/graphml/g"))
            .respond_with(ResponseTemplate::new(404))
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
            .map(|r| r.url.path().to_string())
            .collect()
    }

    #[tokio::test]
    async fn requisition_listed_after_a_graph_executes_first() {
        let s = MockServer::start().await;
        mount_graph_reads(&s).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&s)
            .await;
        let input = format!("{GRAPH}---\n{REQUISITION}");
        let docs = parse_documents("in.yaml", &input).unwrap();
        let out = apply_documents(&registry(), docs, &ApplyParams::default(), &ctx(&s))
            .await
            .unwrap();
        assert!(
            out.iter().all(|o| o.status == OutcomeStatus::Created),
            "{out:?}"
        );
        assert_eq!(writes(&s).await, ["/rest/requisitions", "/rest/graphml/g"]);
    }

    #[tokio::test]
    async fn invalid_graph_aborts_the_whole_apply_before_any_request() {
        let s = MockServer::start().await;
        let bad = GRAPH.replace("namespace: n, ", "namespace: \"\", ");
        let input = format!("{REQUISITION}---\n{bad}");
        let docs = parse_documents("in.yaml", &input).unwrap();
        let err = apply_documents(&registry(), docs, &ApplyParams::default(), &ctx(&s)).await;
        assert!(err.is_err());
        assert!(s.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn dry_run_sends_no_writes() {
        let s = MockServer::start().await;
        mount_graph_reads(&s).await;
        let docs = parse_documents("in.yaml", GRAPH).unwrap();
        let params = ApplyParams {
            dry_run: true,
            ..ApplyParams::default()
        };
        let out = apply_documents(&registry(), docs, &params, &ctx(&s))
            .await
            .unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Skipped);
        assert!(writes(&s).await.is_empty());
    }
}
