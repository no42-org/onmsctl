/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `onmsctl business-service` subcommand surface.
//!
//! `list` snapshots the deployed Business Services; `get` shows one service's
//! edges; `delete` removes a service by name (resolve → `DELETE /{id}` → one
//! bsmd reload). `list`/`get` are Read; `delete` is Write. `apply -f` never
//! deletes a service that is absent from the applied set — removal is this
//! explicit verb (the across-apply non-deletion contract, DD8).

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, Result, TableRow, render_list};
use serde::Serialize;

use crate::api::BusinessServiceApi;
use crate::server::BusinessServiceResponse;

/// `onmsctl business-service …` verbs.
#[derive(Subcommand, Debug, Clone)]
pub enum BusinessServiceCmd {
    /// List the deployed Business Services.
    List,
    /// Show one Business Service (by name) and its edges.
    Get {
        /// The Business Service name (`metadata.name`).
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Delete a Business Service by name (removes it and reloads bsmd).
    Delete {
        /// The Business Service name (`metadata.name`).
        #[arg(value_name = "NAME")]
        name: String,
    },
}

impl Classify for BusinessServiceCmd {
    fn kind(&self) -> CmdKind {
        match self {
            BusinessServiceCmd::List | BusinessServiceCmd::Get { .. } => CmdKind::Read,
            BusinessServiceCmd::Delete { .. } => CmdKind::Write,
        }
    }
}

impl BusinessServiceCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = BusinessServiceApi::new(&client);
        match self {
            BusinessServiceCmd::List => run_list(&api, ctx).await,
            BusinessServiceCmd::Get { name } => run_get(&api, &name, ctx).await,
            BusinessServiceCmd::Delete { name } => run_delete(&api, &name).await,
        }
    }
}

/// One row of `business-service list`.
#[derive(Debug, Clone, Serialize)]
struct ServiceRow {
    name: String,
    children: usize,
    #[serde(rename = "ipServices")]
    ip_services: usize,
    applications: usize,
    #[serde(rename = "reductionKeys")]
    reduction_keys: usize,
}

impl From<&BusinessServiceResponse> for ServiceRow {
    fn from(r: &BusinessServiceResponse) -> Self {
        Self {
            name: r.name.clone(),
            children: r.child_edges.len(),
            ip_services: r.ip_service_edges.len(),
            applications: r.application_edges.len(),
            reduction_keys: r.reduction_key_edges.len(),
        }
    }
}

impl TableRow for ServiceRow {
    fn headers() -> Vec<&'static str> {
        vec![
            "NAME",
            "CHILDREN",
            "IP-SERVICES",
            "APPLICATIONS",
            "REDUCTION-KEYS",
        ]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.children.to_string(),
            self.ip_services.to_string(),
            self.applications.to_string(),
            self.reduction_keys.to_string(),
        ]
    }
}

async fn run_list(api: &BusinessServiceApi<'_>, ctx: &Context) -> Result<()> {
    let all = api.fetch_all().await?;
    let rows: Vec<ServiceRow> = all.iter().map(|(_, r)| ServiceRow::from(r)).collect();
    print!("{}", render_list(&rows, ctx.output_format)?);
    Ok(())
}

async fn run_get(api: &BusinessServiceApi<'_>, name: &str, ctx: &Context) -> Result<()> {
    let all = api.fetch_all().await?;
    let row = all
        .iter()
        .find(|(_, r)| r.name == name)
        .map(|(_, r)| ServiceRow::from(r))
        .ok_or_else(|| Error::Config(format!("Business Service {name:?} not found")))?;
    print!(
        "{}",
        render_list(std::slice::from_ref(&row), ctx.output_format)?
    );
    Ok(())
}

async fn run_delete(api: &BusinessServiceApi<'_>, name: &str) -> Result<()> {
    let all = api.fetch_all().await?;
    let id = all
        .iter()
        .find(|(_, r)| r.name == name)
        .map(|(id, _)| *id)
        .ok_or_else(|| Error::Config(format!("Business Service {name:?} not found")))?;
    api.delete(id).await?;
    // The destructive DELETE has already committed; a reload failure must not
    // fail the command (re-running would report "not found"). Warn instead, as
    // the apply path does, so the operator can reload bsmd manually.
    match api.reload().await {
        Ok(()) => eprintln!("Deleted Business Service {name} (bsmd reloaded)"),
        Err(e) => eprintln!(
            "Deleted Business Service {name}, but bsmd daemon/reload failed ({e}); \
             reload bsmd manually for the change to take effect"
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_read_and_write() {
        assert_eq!(BusinessServiceCmd::List.kind(), CmdKind::Read);
        assert_eq!(
            BusinessServiceCmd::Get { name: "w".into() }.kind(),
            CmdKind::Read
        );
        assert_eq!(
            BusinessServiceCmd::Delete { name: "w".into() }.kind(),
            CmdKind::Write
        );
    }

    #[test]
    fn service_row_counts_edges() {
        let r: BusinessServiceResponse = serde_json::from_value(serde_json::json!({
            "id": 1, "name": "web",
            "child-edges": [ { "child-id": 2 } ],
            "ip-service-edges": [ { "ip-service": { "id": 5 } }, { "ip-service": { "id": 6 } } ]
        }))
        .unwrap();
        let row = ServiceRow::from(&r);
        assert_eq!(row.row(), vec!["web", "1", "2", "0", "0"]);
    }

    /// `delete <name>` resolves the name → id, issues `DELETE /{id}`, then reloads.
    #[tokio::test]
    async fn delete_resolves_name_then_deletes_and_reloads() {
        use onmsctl_core::{AuthCreds, OutputFormat, Url};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

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
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": 1, "name": "web" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/v2/business-services/1"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/business-services/daemon/reload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let ctx = Context {
            name: "test".into(),
            url: Url::parse(&format!("{}/", server.uri())).unwrap(),
            creds: AuthCreds::basic("admin", "secret"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        };
        BusinessServiceCmd::Delete { name: "web".into() }
            .run(&ctx)
            .await
            .expect("delete succeeds");

        let reqs = server.received_requests().await.unwrap();
        assert!(
            reqs.iter()
                .any(|r| r.method.as_str() == "DELETE"
                    && r.url.path() == "/api/v2/business-services/1"),
            "issued the delete"
        );
        assert!(
            reqs.iter().any(|r| r.method.as_str() == "POST"
                && r.url.path() == "/api/v2/business-services/daemon/reload"),
            "reloaded bsmd after delete"
        );
    }
}
