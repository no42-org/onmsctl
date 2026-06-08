/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `UserHandler` — the IAM capability's adapter into the core kind-router.
//!
//! A thin [`KindHandler`] over the existing `apply_users` building blocks
//! (`check_input_uniqueness`, `plan_users`, `lockout::*`, `execute_plans`),
//! which stay the authoritative reconciler. `plan()` parses the bucket of
//! `kind: User` documents, runs the cross-document uniqueness check, plans each
//! user against server state, and — on a real apply — enforces the lockout
//! invariants (IAM-001/002) so their dedicated exit codes propagate through the
//! router gate. `execute()` runs the planned writes.
//!
//! Two deliberate adaptations to the generic contract:
//! - The IAM-001 admin-lockout override has no CLI flag on the generic
//!   `apply` (which stays generic like `kubectl apply`); it is read from the
//!   per-context config `iam.allow-admin-lockout` instead.
//! - `execute_plans` is always called with `keep_going = true` so every user in
//!   the bucket is attempted and reported accurately; the router's bucket-level
//!   stop-on-error governs whether *later kinds* run. (`execute_plans` with
//!   `keep_going = false` would leave not-attempted users seeded optimistically
//!   as "Applied", which would misreport them.)

use std::collections::BTreeSet;
use std::path::PathBuf;

use async_trait::async_trait;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OnmsClient, OutcomeStatus,
    Plan, RawDoc, Result,
};

use crate::api::IamApi;
use crate::apply::multi::{UserOutcome, UserResult, execute_plans, plan_users};
use crate::apply::{Finding, UserPlan, UserReconcile, check_input_uniqueness, lockout};
use crate::model::local::{KIND_USER, KNOWN_ROLES, UserLocal};

/// Handler for `kind: User` documents.
#[derive(Default)]
pub struct UserHandler;

/// Opaque execute payload: the planned per-user reconciles from `plan()`.
struct UserExecPayload {
    reconciles: Vec<(String, UserReconcile)>,
}

#[async_trait]
impl KindHandler for UserHandler {
    fn kind(&self) -> &'static str {
        KIND_USER
    }

    async fn plan(&self, docs: &[RawDoc], params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        // -- Parse each document into the strict local DTO (gate on failure). --
        let mut parsed: Vec<(PathBuf, UserLocal)> = Vec::with_capacity(docs.len());
        for d in docs {
            let local: UserLocal = serde_norway::from_value(d.value.clone()).map_err(|e| {
                Error::Config(format!("{}: invalid `kind: User` document: {e}", d.label()))
            })?;
            // Use the `source#index` label, not the bare source, so a duplicate
            // PR-IAM-002 message distinguishes two docs in one multi-doc file
            // (otherwise it lists the same path twice).
            parsed.push((PathBuf::from(d.label()), local));
        }

        // -- Cross-document uniqueness (PR-IAM-002) → gate. --
        if let Err(findings) = check_input_uniqueness(&parsed) {
            return Err(Error::Config(join_findings(&findings)));
        }

        let known = resolved_known_roles(ctx);
        let protected = resolved_protected_roles(ctx);
        let allow_override = ctx.iam.allow_admin_lockout.unwrap_or(false);

        let client = OnmsClient::from_context(ctx)?;
        let api = IamApi::new(&client);
        let locals: Vec<UserLocal> = parsed.iter().map(|(_, l)| l.clone()).collect();
        let planned = plan_users(&locals, &api, &known).await?;

        // -- Lockout invariants (IAM-001/002) on the real-apply path only;
        //    raised here so the router gate preserves exit codes 13/14/15. --
        if !params.dry_run {
            let flat: Vec<&UserPlan> = planned
                .reconciles
                .iter()
                .flat_map(|(_, r)| r.plans.iter())
                .collect();
            lockout::check_admin_lockout(&flat, &planned.server_users, &protected, allow_override)?;
            if lockout::self_lockout_possible(&flat, &protected) {
                let whoami = api.get_whoami().await?.map(|u| u.user_id);
                lockout::check_self_lockout(&flat, whoami.as_deref(), &protected)?;
            }
        }

        let preview = planned
            .reconciles
            .iter()
            .map(|(name, rec)| preview_for(name, rec))
            .collect();
        Ok(Plan::new(
            preview,
            Box::new(UserExecPayload {
                reconciles: planned.reconciles,
            }),
        ))
    }

    async fn execute(
        &self,
        plan: Plan,
        _params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let payload = plan
            .payload
            .downcast::<UserExecPayload>()
            .map_err(|_| Error::Config("internal: UserHandler payload type mismatch".into()))?;
        let client = OnmsClient::from_context(ctx)?;
        let api = IamApi::new(&client);
        // Always keep going within the bucket for accurate per-user reporting;
        // the router halts later kinds on any Failed outcome (see module docs).
        let (users, _state) = execute_plans(payload.reconciles, &api, true).await;
        Ok(users.into_iter().map(outcome_of).collect())
    }
}

/// Per-context known-roles set (`iam.known-roles` replaces the built-in default).
fn resolved_known_roles(ctx: &Context) -> BTreeSet<String> {
    match &ctx.iam.known_roles {
        Some(list) => list.iter().cloned().collect(),
        None => KNOWN_ROLES.iter().map(|s| s.to_string()).collect(),
    }
}

/// Per-context protected-roles set (`iam.protected-roles`; default `[ROLE_ADMIN]`).
fn resolved_protected_roles(ctx: &Context) -> BTreeSet<String> {
    match &ctx.iam.protected_roles {
        Some(list) => list.iter().cloned().collect(),
        None => BTreeSet::from(["ROLE_ADMIN".to_string()]),
    }
}

fn join_findings(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| format!("{}: {}", f.code, f.message))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The representative action for a user's planned reconcile. An empty plan set
/// is `None` explicitly (not by vacuous `all()`), so a future caller passing
/// `&[]` gets a defined answer rather than a quiet vacuous-truth result.
fn action_of(plans: &[UserPlan]) -> Action {
    if plans.is_empty() {
        return Action::None;
    }
    if plans.iter().any(|p| matches!(p, UserPlan::Create { .. })) {
        Action::Create
    } else if plans.iter().all(|p| matches!(p, UserPlan::Unchanged { .. })) {
        Action::None
    } else {
        Action::Update
    }
}

/// Attach warnings to the outcome: the full text goes in `details` (for
/// `-o json|yaml`), and a short code hint is appended to `message` so warnings
/// are not invisible under the default `-o table` (e.g. PR-IAM-008
/// "passwordRef ignored — password NOT rotated" must not vanish silently).
fn with_warnings(mut o: ApplyOutcome, warnings: &[Finding]) -> ApplyOutcome {
    if !warnings.is_empty() {
        let list: Vec<String> = warnings
            .iter()
            .map(|f| format!("{}: {}", f.code, f.message))
            .collect();
        let codes: Vec<&str> = warnings.iter().map(|f| f.code).collect();
        let hint = format!("{} warning(s): {}", warnings.len(), codes.join(", "));
        o.message = if o.message.is_empty() {
            hint
        } else {
            format!("{} ({hint})", o.message)
        };
        o.details = Some(serde_json::json!({ "warnings": list }));
    }
    o
}

/// Plan-phase preview for one user (used verbatim under `--dry-run`).
fn preview_for(name: &str, rec: &UserReconcile) -> ApplyOutcome {
    if !rec.errors.is_empty() {
        let o = ApplyOutcome::failed(
            KIND_USER,
            name,
            Action::None,
            join_findings(&rec.errors),
            "fix the document and re-apply",
        );
        return with_warnings(o, &rec.warnings);
    }
    with_warnings(ApplyOutcome::would(KIND_USER, name, action_of(&rec.plans)), &rec.warnings)
}

/// Map an execute-phase per-user outcome to an `ApplyOutcome`.
fn outcome_of(uo: UserOutcome) -> ApplyOutcome {
    let o = match uo.result {
        UserResult::Unchanged => ApplyOutcome::new(
            KIND_USER,
            uo.name.clone(),
            Action::None,
            OutcomeStatus::Unchanged,
            "in sync",
        ),
        UserResult::Applied => {
            if uo.planned.iter().any(|p| matches!(p, UserPlan::Create { .. })) {
                ApplyOutcome::new(
                    KIND_USER,
                    uo.name.clone(),
                    Action::Create,
                    OutcomeStatus::Created,
                    "created",
                )
            } else {
                ApplyOutcome::new(
                    KIND_USER,
                    uo.name.clone(),
                    Action::Update,
                    OutcomeStatus::Updated,
                    "updated",
                )
            }
        }
        UserResult::Failed => ApplyOutcome::failed(
            KIND_USER,
            uo.name.clone(),
            action_of(&uo.planned),
            uo.failure.clone().unwrap_or_else(|| "apply failed".into()),
            "re-run `onmsctl apply -f` after resolving the error",
        ),
        UserResult::PlanFailed => ApplyOutcome::failed(
            KIND_USER,
            uo.name.clone(),
            Action::None,
            join_findings(&uo.errors),
            "fix the document and re-apply",
        ),
        // Execute never yields Planned (that is the dry-run state); map defensively.
        UserResult::Planned => {
            ApplyOutcome::would(KIND_USER, uo.name.clone(), action_of(&uo.planned))
        }
    };
    with_warnings(o, &uo.warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::kind::parse_documents;
    use onmsctl_core::{AuthCreds, OutputFormat, Url};
    use std::io::Write;
    use wiremock::matchers::{method, path, query_param};
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

    async fn mount_users_list(server: &MockServer, users: serde_json::Value, total: i64) {
        Mock::given(method("GET"))
            .and(path("/rest/users"))
            .and(query_param("limit", "10000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "offset": 0, "count": total, "totalCount": total, "user": users
            })))
            .mount(server)
            .await;
    }

    fn user_doc(name: &str, pw_path: Option<&std::path::Path>) -> Vec<RawDoc> {
        let pref = pw_path
            .map(|p| format!("\n  passwordRef:\n    fromFile: {}", p.display()))
            .unwrap_or_default();
        let yaml = format!(
            "apiVersion: onmsctl.no42.org/v1alpha1\nkind: User\nmetadata:\n  name: {name}\nspec:\n  fullName: Full Name\n  roles: [ROLE_USER]{pref}\n"
        );
        parse_documents("u.yaml", &yaml).unwrap()
    }

    #[tokio::test]
    async fn plan_then_execute_creates_absent_user() {
        let server = MockServer::start().await;
        mount_users_list(&server, serde_json::json!([]), 0).await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/users"))
            .and(query_param("hashPassword", "true"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "s3cr3t").unwrap();
        let docs = user_doc("alice", Some(f.path()));
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();

        let handler = UserHandler;
        let plan = handler.plan(&docs, &params, &ctx).await.unwrap();
        assert_eq!(plan.preview.len(), 1);
        assert_eq!(plan.preview[0].status, OutcomeStatus::Skipped); // would-create preview
        assert_eq!(plan.preview[0].action, Action::Create);

        let outcomes = handler.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].kind, "User");
        assert_eq!(outcomes[0].name, "alice");
        assert_eq!(outcomes[0].status, OutcomeStatus::Created);

        // The resolved secret must actually reach the POST body (guards the
        // secret path — a dropped/empty password would still return 201).
        let reqs = server.received_requests().await.unwrap();
        let post = reqs
            .iter()
            .find(|r| r.method.as_str() == "POST" && r.url.path() == "/rest/users")
            .expect("a POST to /rest/users");
        let body = String::from_utf8_lossy(&post.body);
        assert!(
            body.contains("s3cr3t"),
            "POST body must carry the resolved password; got: {body}"
        );
    }

    #[tokio::test]
    async fn passwordref_on_existing_user_surfaces_pr_iam_008_in_outcome() {
        // An existing, in-sync user that declares a passwordRef → PR-IAM-008
        // warning (apply never rotates). It must surface in the outcome
        // message (table-visible hint) and details, not vanish.
        let server = MockServer::start().await;
        mount_users_list(
            &server,
            serde_json::json!([{"user-id":"alice","full-name":"Full Name","role":["ROLE_USER"]}]),
            1,
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice", "full-name": "Full Name", "role": ["ROLE_USER"]
            })))
            .mount(&server)
            .await;
        // No write mocks: an in-sync user must not mutate.
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "pw").unwrap();
        let docs = user_doc("alice", Some(f.path()));
        let ctx = ctx_for(&server);
        let params = ApplyParams::default();

        let handler = UserHandler;
        let plan = handler.plan(&docs, &params, &ctx).await.unwrap();
        assert!(
            plan.preview[0].message.contains("PR-IAM-008"),
            "preview should hint the warning; got: {}",
            plan.preview[0].message
        );

        let outcomes = handler.execute(plan, &params, &ctx).await.unwrap();
        assert_eq!(outcomes[0].status, OutcomeStatus::Unchanged);
        assert!(
            outcomes[0].message.contains("PR-IAM-008"),
            "outcome message must surface the warning; got: {}",
            outcomes[0].message
        );
        assert!(
            outcomes[0].details.is_some(),
            "full warning text must live in details for -o json|yaml"
        );
    }

    #[tokio::test]
    async fn plan_refuses_admin_lockout_with_exit_code_error() {
        let server = MockServer::start().await;
        // Sole admin alice; the document demotes her to ROLE_USER → IAM-001.
        mount_users_list(
            &server,
            serde_json::json!([{"user-id": "alice", "role": ["ROLE_ADMIN"]}]),
            1,
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice", "full-name": "Full Name", "role": ["ROLE_ADMIN"]
            })))
            .mount(&server)
            .await;
        // No write mocks: must refuse in plan() before any execute.
        let docs = user_doc("alice", None);
        let ctx = ctx_for(&server);
        // `Plan` is not `Debug` (opaque payload), so avoid `unwrap_err`.
        let err = match UserHandler.plan(&docs, &ApplyParams::default(), &ctx).await {
            Ok(_) => panic!("expected an IAM-001 lockout refusal"),
            Err(e) => e,
        };
        assert!(matches!(err, Error::IamLockout { .. }), "got {err:?}");
        assert_eq!(err.exit_code(), 13);
    }

    #[tokio::test]
    async fn dry_run_plan_skips_lockout_and_previews_without_writing() {
        let server = MockServer::start().await;
        // Same sole-admin demotion, but --dry-run must NOT gate on lockout
        // and must not require whoami.
        mount_users_list(
            &server,
            serde_json::json!([{"user-id": "alice", "role": ["ROLE_ADMIN"]}]),
            1,
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice", "full-name": "Full Name", "role": ["ROLE_ADMIN"]
            })))
            .mount(&server)
            .await;
        let docs = user_doc("alice", None);
        let ctx = ctx_for(&server);
        let params = ApplyParams {
            dry_run: true,
            ..Default::default()
        };
        // Must not error despite the would-be lockout.
        let plan = UserHandler.plan(&docs, &params, &ctx).await.unwrap();
        assert_eq!(plan.preview.len(), 1);
        // Demotion is an Update (role delta), not a create.
        assert_eq!(plan.preview[0].action, Action::Update);

        // "without writing": dry-run must issue only reads, and must NOT call
        // whoami (lockout/self-lockout are skipped under --dry-run).
        let reqs = server.received_requests().await.unwrap();
        assert!(
            reqs.iter().all(|r| r.method.as_str() == "GET"),
            "dry-run must issue no writes"
        );
        assert!(
            reqs.iter().all(|r| !r.url.path().ends_with("/whoami")),
            "dry-run must not call whoami"
        );
    }
}
