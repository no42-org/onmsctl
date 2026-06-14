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

use crate::api::{SnmpConfigApi, TrapdConfigApi};
use crate::model::{KIND, SINGLETON_NAME, SnmpConfigLocal};
use crate::server::{SnmpConfig, TrapdConfig};
use crate::{convert, diff};

/// Display name for the trap-daemon sub-resource in per-resource outcomes, so a
/// merged apply reports the snmp-config and trap-daemon halves distinctly.
const TRAPD_NAME: &str = "default (trapd)";

/// Handler for `kind: SnmpConfig` documents.
#[derive(Default)]
pub struct SnmpConfigHandler;

/// Opaque execute payload: the validated local doc plus the per-endpoint
/// in-sync verdicts computed during `plan` (so `execute` can no-op without a
/// second GET). `trapd_unchanged` is `None` when the document carries no
/// `spec.trapd` block — in that case `execute` never touches the trap endpoint.
struct SnmpExecPayload {
    local: SnmpConfigLocal,
    agent_unchanged: bool,
    trapd_unchanged: Option<bool>,
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

        // Read-only: fetch deployed, diff ignoring secrets. The snmp-config
        // (agent) half is always reconciled; the trap-daemon half only when the
        // document carries a `spec.trapd` block (so older Horizon is untouched).
        let client = OnmsClient::from_context(ctx)?;
        let snmp_api = SnmpConfigApi::new(&client);
        let deployed = snmp_api.get_config().await?;
        let desired = convert::to_wire(&local);
        let agent_unchanged = diff::unchanged(&desired, &deployed);

        let mut previews = vec![agent_preview(agent_unchanged)];
        let mut diff_sections = Vec::new();
        if !agent_unchanged {
            diff_sections.push(render_diff(&deployed, &desired));
        }

        // Trap-daemon half. A `404` on the GET means "no config persisted yet"
        // (a supported server's first-run state) OR "endpoint absent" (old
        // server); both surface here as an empty deployed config and reconcile
        // as a create — an unsupported server is then caught on the write path
        // (PUT → version error). Permission/5xx errors propagate and abort.
        let trapd_unchanged = if let Some(t) = &local.spec.trapd {
            let trapd_api = TrapdConfigApi::new(&client);
            let deployed_t = trapd_api.get_config().await?.unwrap_or_default();
            let desired_t = convert::trapd_to_wire(t);
            let uc = diff::trapd_unchanged(&desired_t, &deployed_t);
            previews.push(trapd_preview(uc));
            if !uc {
                diff_sections.push(render_trapd_diff(&deployed_t, &desired_t));
            }
            Some(uc)
        } else {
            None
        };

        let diff_text = (!diff_sections.is_empty()).then(|| diff_sections.join("\n\n"));

        Ok(Plan::new(
            previews,
            Box::new(SnmpExecPayload {
                local,
                agent_unchanged,
                trapd_unchanged,
            }),
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
        let client = OnmsClient::from_context(ctx)?;

        // There is no cross-endpoint transaction. Write the snmp-config (agent)
        // half first (lower blast radius), then the trap-daemon half, emitting a
        // distinct outcome for each so a mid-apply failure is reported precisely
        // rather than masked as overall success.
        let mut outcomes = Vec::new();

        if payload.agent_unchanged {
            outcomes.push(agent_preview(true));
        } else {
            // Resolve secrets only now, on the real-apply path.
            let wire = convert::to_wire_resolved(&payload.local)?;
            let snmp_api = SnmpConfigApi::new(&client);
            outcomes.push(match snmp_api.upload_config(&wire).await {
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
            });
        }

        if let Some(trapd_unchanged) = payload.trapd_unchanged {
            if trapd_unchanged {
                outcomes.push(trapd_preview(true));
            } else {
                let t = payload
                    .local
                    .spec
                    .trapd
                    .as_ref()
                    .expect("trapd_unchanged is Some only when spec.trapd is present");
                // Resolve passphrases into a Failed outcome (not an Err) so the
                // already-recorded agent outcome is preserved in the report.
                let outcome = match convert::trapd_to_wire_resolved(t) {
                    Ok(wire) => {
                        let trapd_api = TrapdConfigApi::new(&client);
                        match trapd_api.update_config(&wire).await {
                            Ok(()) => ApplyOutcome::new(
                                KIND,
                                TRAPD_NAME,
                                Action::Update,
                                OutcomeStatus::Updated,
                                "trap-daemon config updated",
                            ),
                            Err(e) => ApplyOutcome::failed(
                                KIND,
                                TRAPD_NAME,
                                Action::Update,
                                e.to_string(),
                                "verify the server exposes the Trapd REST API and re-apply",
                            ),
                        }
                    }
                    Err(e) => ApplyOutcome::failed(
                        KIND,
                        TRAPD_NAME,
                        Action::Update,
                        e.to_string(),
                        "resolve the trap-daemon passphrase reference and re-apply",
                    ),
                };
                outcomes.push(outcome);
            }
        }

        Ok(outcomes)
    }
}

/// The snmp-config (agent) per-resource preview/outcome for a given verdict.
fn agent_preview(unchanged: bool) -> ApplyOutcome {
    if unchanged {
        ApplyOutcome::new(
            KIND,
            SINGLETON_NAME,
            Action::None,
            OutcomeStatus::Unchanged,
            "in sync",
        )
    } else {
        ApplyOutcome::would(KIND, SINGLETON_NAME, Action::Update)
    }
}

/// The trap-daemon per-resource preview/outcome for a given verdict.
fn trapd_preview(unchanged: bool) -> ApplyOutcome {
    if unchanged {
        ApplyOutcome::new(
            KIND,
            TRAPD_NAME,
            Action::None,
            OutcomeStatus::Unchanged,
            "in sync",
        )
    } else {
        ApplyOutcome::would(KIND, TRAPD_NAME, Action::Update)
    }
}

/// A concise, secret-free `--diff` summary for the trap-daemon half. The
/// passphrases are never shown; the port and v3-user count are the salient,
/// safe signals of what an upload changes.
fn render_trapd_diff(deployed: &TrapdConfig, desired: &TrapdConfig) -> String {
    let port = |c: &TrapdConfig| {
        c.snmp_trap_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into())
    };
    format!(
        "trapd: trap-daemon config update\n  \
         port:     {} -> {}\n  \
         v3 users: {} deployed -> {} desired",
        port(deployed),
        port(desired),
        deployed.snmpv3_user.len(),
        desired.snmpv3_user.len(),
    )
}

/// A concise, secret-free `--diff` summary. A whole-config replace has no
/// per-field patch to show, and the wire JSON would leak nothing useful (secrets
/// are blanked) while being noisy — so report which tier changes and the
/// definition/profile counts. Both sides are canonicalized first (secrets
/// blanked, lists sorted) so the per-tier equality is meaningful.
fn render_diff(deployed: &SnmpConfig, desired: &SnmpConfig) -> String {
    let [defaults_ok, definitions_ok, profiles_ok] = diff::tiers_match(desired, deployed);
    let have = diff::canonical_struct(deployed);
    let want = diff::canonical_struct(desired);
    let flag = |ok: bool| if ok { "unchanged" } else { "changed" };
    format!(
        "snmp-config: whole-config replace via upload\n  \
         defaults:    {}\n  \
         definitions: {} ({} deployed -> {} desired)\n  \
         profiles:    {} ({} deployed -> {} desired)",
        flag(defaults_ok),
        flag(definitions_ok),
        have.definition.len(),
        want.definition.len(),
        flag(profiles_ok),
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

    /// A document with a `spec.trapd` block reconciles BOTH endpoints: the
    /// agent half is in sync (no upload), the trap-daemon half changes and is
    /// PUT with the resolved passphrase.
    #[tokio::test]
    async fn trapd_block_reconciles_both_endpoints() {
        // SAFETY: test-only env mutation for the v3 passphrase ref.
        unsafe {
            std::env::set_var("ONMS_TRAPD_AUTH", "trap-secret");
        }
        let server = MockServer::start().await;
        // Agent: deployed == desired (v2c) → unchanged, no upload mock needed.
        Mock::given(method("GET"))
            .and(path("/api/v2/snmp-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "v2c"
            })))
            .mount(&server)
            .await;
        // Trapd: deployed differs (port 100) from desired (port 162).
        Mock::given(method("GET"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "snmpTrapPort": 100, "newSuspectOnTrap": false
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let docs = doc(
            "  defaults:\n    version: v2c\n  trapd:\n    snmpTrapPort: 162\n    \
             newSuspectOnTrap: false\n    snmpv3Users:\n      - securityName: monitor\n        \
             securityLevel: authPriv\n        authPassphrase: { fromEnv: ONMS_TRAPD_AUTH }\n",
        );
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let handler = SnmpConfigHandler;

        let plan = handler.plan(&docs, &params, &ctx).await.unwrap();
        assert_eq!(plan.preview.len(), 2, "agent + trapd previews");
        let trapd_preview = plan
            .preview
            .iter()
            .find(|o| o.name == TRAPD_NAME)
            .expect("a trapd preview");
        assert_eq!(trapd_preview.action, Action::Update);

        let outcomes = handler.execute(plan, &params, &ctx).await.unwrap();
        let agent = outcomes.iter().find(|o| o.name == SINGLETON_NAME).unwrap();
        let trapd = outcomes.iter().find(|o| o.name == TRAPD_NAME).unwrap();
        assert_eq!(agent.status, OutcomeStatus::Unchanged);
        assert_eq!(trapd.status, OutcomeStatus::Updated);

        // The PUT body carries the new port and the resolved passphrase.
        let reqs = server.received_requests().await.unwrap();
        let put = reqs
            .iter()
            .find(|r| r.method.as_str() == "PUT")
            .expect("a trapd PUT");
        let body = String::from_utf8_lossy(&put.body);
        assert!(body.contains("162"), "new port in PUT body");
        assert!(
            body.contains("trap-secret"),
            "resolved passphrase in PUT body"
        );

        unsafe {
            std::env::remove_var("ONMS_TRAPD_AUTH");
        }
    }

    /// Regression guard for the additive contract: a document with NO `spec.trapd`
    /// block must never touch `/api/v2/trapd` (so older Horizon is unaffected).
    #[tokio::test]
    async fn no_trapd_block_issues_zero_trapd_traffic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/snmp-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "v2c"
            })))
            .mount(&server)
            .await;

        let docs = doc("  defaults:\n    version: v2c\n");
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let handler = SnmpConfigHandler;

        let plan = handler.plan(&docs, &params, &ctx).await.unwrap();
        assert_eq!(plan.preview.len(), 1, "only the agent preview, no trapd");
        let _ = handler.execute(plan, &params, &ctx).await.unwrap();

        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| !r.url.path().starts_with("/api/v2/trapd")),
            "a document without spec.trapd must issue no Trapd requests"
        );
    }

    /// A `404` on the trapd GET is a supported server's "no config yet": plan
    /// reconciles it as a create, execute PUTs successfully.
    #[tokio::test]
    async fn trapd_create_from_nothing_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/snmp-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "v2c"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(
                ResponseTemplate::new(404).set_body_string("Trapd configuration not found."),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let docs = doc("  trapd:\n    snmpTrapPort: 162\n    newSuspectOnTrap: false\n");
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let handler = SnmpConfigHandler;

        let plan = handler.plan(&docs, &params, &ctx).await.unwrap();
        let outcomes = handler.execute(plan, &params, &ctx).await.unwrap();
        let trapd = outcomes.iter().find(|o| o.name == TRAPD_NAME).unwrap();
        assert_eq!(trapd.status, OutcomeStatus::Updated, "create path");
    }

    /// An unsupported server (trapd route absent) GET 404s, then PUT 404s →
    /// the trapd half is a Failed outcome with a version hint; the agent half is
    /// unaffected and reported separately.
    #[tokio::test]
    async fn trapd_unsupported_server_fails_with_version_hint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/snmp-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "v2c"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(ResponseTemplate::new(404).set_body_string("no route"))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/trapd/config"))
            .respond_with(ResponseTemplate::new(404).set_body_string("no route"))
            .mount(&server)
            .await;

        let docs = doc("  trapd:\n    snmpTrapPort: 162\n    newSuspectOnTrap: false\n");
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();
        let outcomes = SnmpConfigHandler.plan(&docs, &params, &ctx).await.unwrap();
        let outcomes = SnmpConfigHandler
            .execute(outcomes, &params, &ctx)
            .await
            .unwrap();

        let agent = outcomes.iter().find(|o| o.name == SINGLETON_NAME).unwrap();
        let trapd = outcomes.iter().find(|o| o.name == TRAPD_NAME).unwrap();
        assert_eq!(
            agent.status,
            OutcomeStatus::Unchanged,
            "agent half unaffected"
        );
        assert_eq!(trapd.status, OutcomeStatus::Failed);
        assert!(
            trapd.message.contains("NMS-19128"),
            "version hint in trapd failure: {}",
            trapd.message
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
