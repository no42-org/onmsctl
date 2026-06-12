/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `SnmpConfigHandler` — the SNMP capability's adapter into the core
//! kind-router.
//!
//! `kind: SnmpConfig` is a **singleton**: exactly one document configures the
//! whole-server snmp-config, reconciled by whole-config replace (design
//! D1/D3). So `plan()` requires exactly one document in the bucket (anything
//! else is a gate `Err`), validates it, fetches the deployed config, and diffs
//! ignoring secrets — secrets are write-only, so a redacted/echoed secret on
//! the deployed side must not surface as a spurious diff; a rotation rides an
//! explicit non-secret change or is re-sent deliberately.
//!
//! `execute()` resolves the secret refs and uploads the whole config. Like the
//! other handlers, a transport failure on the upload is returned as a `Failed`
//! outcome (not an `Err`) so the router reports it uniformly; an `Err` from
//! `plan` is reserved for gate-class refusals (bad/duplicate documents).

use async_trait::async_trait;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OnmsClient, OutcomeStatus,
    Plan, RawDoc, Result,
};

use crate::api::SnmpConfigApi;
use crate::model::{KIND, SINGLETON_NAME, SnmpConfigLocal};
use crate::server::SnmpConfig;
use crate::{convert, diff};

/// Handler for `kind: SnmpConfig` documents.
#[derive(Default)]
pub struct SnmpConfigHandler;

/// Opaque execute payload: the validated local doc plus whether the deployed
/// config already matches (so `execute` can no-op without a second GET).
struct SnmpExecPayload {
    local: SnmpConfigLocal,
    unchanged: bool,
}

#[async_trait]
impl KindHandler for SnmpConfigHandler {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        // Singleton: exactly one document configures the whole server.
        if docs.len() != 1 {
            return Err(Error::Config(format!(
                "kind: SnmpConfig is a singleton — expected exactly one document, got {}",
                docs.len()
            )));
        }
        let d = &docs[0];
        let local: SnmpConfigLocal = serde_norway::from_value(d.value.clone()).map_err(|e| {
            Error::Config(format!(
                "{}: invalid `kind: SnmpConfig` document: {e}",
                d.label()
            ))
        })?;
        local.validate()?;

        // Read-only: fetch deployed, diff ignoring secrets.
        let client = OnmsClient::from_context(ctx)?;
        let api = SnmpConfigApi::new(&client);
        let deployed = api.get_config().await?;
        let desired = convert::to_wire(&local);
        let unchanged = diff::unchanged(&desired, &deployed);

        let preview = if unchanged {
            ApplyOutcome::new(
                KIND,
                SINGLETON_NAME,
                Action::None,
                OutcomeStatus::Unchanged,
                "in sync",
            )
        } else {
            ApplyOutcome::would(KIND, SINGLETON_NAME, Action::Update)
        };
        let diff_text = (!unchanged).then(|| render_diff(&deployed, &desired));

        Ok(Plan::new(
            vec![preview],
            Box::new(SnmpExecPayload { local, unchanged }),
        )
        .with_diff(diff_text))
    }

    async fn execute(
        &self,
        plan: Plan,
        _params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let payload = plan.payload.downcast::<SnmpExecPayload>().map_err(|_| {
            Error::Config("internal: SnmpConfigHandler payload type mismatch".into())
        })?;
        if payload.unchanged {
            return Ok(vec![ApplyOutcome::new(
                KIND,
                SINGLETON_NAME,
                Action::None,
                OutcomeStatus::Unchanged,
                "in sync",
            )]);
        }
        // Resolve secrets only now, on the real-apply path.
        let wire = convert::to_wire_resolved(&payload.local)?;
        let client = OnmsClient::from_context(ctx)?;
        let api = SnmpConfigApi::new(&client);
        let outcome = match api.upload_config(&wire).await {
            Ok(()) => ApplyOutcome::new(
                KIND,
                SINGLETON_NAME,
                Action::Update,
                OutcomeStatus::Updated,
                "snmp-config replaced",
            ),
            Err(e) => ApplyOutcome::failed(
                KIND,
                SINGLETON_NAME,
                Action::Update,
                e.to_string(),
                "verify connectivity and re-apply",
            ),
        };
        Ok(vec![outcome])
    }
}

/// A concise, secret-free `--diff` summary. A whole-config replace has no
/// per-field patch to show, and the wire JSON would leak nothing useful (secrets
/// are blanked) while being noisy — so report which tier changes and the
/// definition/profile counts. Both sides are canonicalized first (secrets
/// blanked, lists sorted) so the per-tier equality is meaningful.
fn render_diff(deployed: &SnmpConfig, desired: &SnmpConfig) -> String {
    let have = diff::canonical_struct(deployed);
    let want = diff::canonical_struct(desired);
    let flag = |same: bool| if same { "unchanged" } else { "changed" };
    format!(
        "snmp-config: whole-config replace via upload\n  \
         defaults:    {}\n  \
         definitions: {} ({} deployed -> {} desired)\n  \
         profiles:    {} ({} deployed -> {} desired)",
        flag(have.defaults == want.defaults),
        flag(have.definition == want.definition),
        have.definition.len(),
        want.definition.len(),
        flag(have.profiles.profile == want.profiles.profile),
        have.profiles.profile.len(),
        want.profiles.profile.len(),
    )
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

    fn doc(spec: &str) -> Vec<RawDoc> {
        let yaml = format!(
            "apiVersion: snmp.opennms.org/v1\nkind: SnmpConfig\nmetadata:\n  name: default\nspec:\n{spec}"
        );
        parse_documents("snmp.yaml", &yaml).unwrap()
    }

    /// GET returns a config the desired doc differs from → plan previews an
    /// Update, execute resolves the secret and uploads the whole config.
    #[tokio::test]
    async fn plan_diffs_then_execute_uploads() {
        // SAFETY: test-only env mutation for the readCommunity ref.
        unsafe {
            std::env::set_var("ONMS_SNMP_RO", "topsecret");
        }
        let server = MockServer::start().await;
        // Deployed: bare defaults, no definitions.
        Mock::given(method("GET"))
            .and(path("/api/v2/snmp-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "v2c"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/snmp-config/upload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let docs = doc(
            "  defaults:\n    version: v2c\n    readCommunity: { fromEnv: ONMS_SNMP_RO }\n  \
             definitions:\n    - location: hq\n      specifics: [192.168.8.8]\n",
        );
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let handler = SnmpConfigHandler;

        let plan = handler.plan(&docs, &params, &ctx).await.unwrap();
        assert_eq!(plan.preview.len(), 1);
        assert_eq!(plan.preview[0].action, Action::Update);
        assert!(plan.diff.is_some(), "a changed plan renders a diff");

        let outcomes = handler.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, OutcomeStatus::Updated);

        // The resolved secret and the new definition must reach the upload body.
        let reqs = server.received_requests().await.unwrap();
        let post = reqs
            .iter()
            .find(|r| r.method.as_str() == "POST")
            .expect("an upload POST");
        let body = String::from_utf8_lossy(&post.body);
        assert!(body.contains("topsecret"), "resolved community in body");
        assert!(body.contains("192.168.8.8"), "definition in body");

        unsafe {
            std::env::remove_var("ONMS_SNMP_RO");
        }
    }

    /// GET returns a config equal (ignoring secrets) to the desired doc → plan
    /// previews Unchanged and execute issues no upload.
    #[tokio::test]
    async fn unchanged_config_is_a_noop() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/snmp-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "v2c"
            })))
            .mount(&server)
            .await;
        // No upload mock: an in-sync config must not POST.

        let docs = doc("  defaults:\n    version: v2c\n");
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let handler = SnmpConfigHandler;

        let plan = handler.plan(&docs, &params, &ctx).await.unwrap();
        assert_eq!(plan.preview[0].status, OutcomeStatus::Unchanged);
        assert!(plan.diff.is_none(), "unchanged plan renders no diff");

        let outcomes = handler.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(outcomes[0].status, OutcomeStatus::Unchanged);
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| r.method.as_str() == "GET"),
            "unchanged config must issue no writes"
        );
    }

    /// Two SnmpConfig documents in one bucket is a gate error (singleton).
    #[tokio::test]
    async fn more_than_one_document_is_rejected() {
        let server = MockServer::start().await;
        let mut docs = doc("  defaults:\n    version: v2c\n");
        docs.extend(doc("  defaults:\n    version: v1\n"));
        let ctx = ctx_for(&server);
        let err = match SnmpConfigHandler
            .plan(&docs, &ApplyParams::default(), &ctx)
            .await
        {
            Ok(_) => panic!("expected a singleton rejection"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("singleton"), "got: {err}");
    }
}
