/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Phase 1 (plan) + Phase 2 (execute) orchestration for `iam apply`
//! (design §D4), built on the pure [`plan_user`] core.
//!
//! - [`plan_users`] (task 6.2) — one `GET /users/{name}` per declared user
//!   plus a single `GET /users` snapshot for the lockout invariant. Records
//!   per-user plan failures instead of aborting the whole run.
//! - [`execute_plans`] (task 6.3) — fans the atomic actions out in the order
//!   creates → updates → role-adds → role-removes, alphabetical by username
//!   within each bucket. Resolves each Create's `passwordRef` here (I/O).
//! - [`apply_users`] (task 6.4) — the orchestrator: input-uniqueness check →
//!   plan → (lockout invariants, Group 7) → render-if-dry-run → execute.
//!
//! The lockout invariant checks (IAM-001/IAM-002) land in Group 7; the
//! `server_users` snapshot is already collected here so they can run without
//! re-fetching. The hook point is marked in [`apply_users`].

use std::collections::BTreeSet;
use std::path::PathBuf;

use onmsctl_core::{Error, Result};

use crate::api::IamApi;
use crate::apply::{Finding, UserPlan, UserReconcile, check_input_uniqueness, lockout, plan_user};
use crate::model::local::UserLocal;
use crate::model::wire::OnmsUserWire;
use crate::secret::resolve_password_ref;

/// Refuse a `GET /users` snapshot larger than this rather than operate the
/// lockout invariant on a silently-truncated set (spike 0.4: the endpoint
/// returns the full set in one call, so exceeding this means an unexpectedly
/// large install we won't guess at). Matches `api::USER_LIST_LIMIT`.
const USER_LIST_LIMIT: i64 = 10_000;

/// Knobs for an apply run.
#[derive(Clone, Debug)]
pub struct ApplyOptions {
    /// Plan only — render and return without issuing any write.
    pub dry_run: bool,
    /// Continue past a per-user Phase-2 failure instead of stopping. Controls
    /// **only** Phase-2 execution; plan-phase findings (PR-IAM-002/005, GET
    /// failures) are unaffected. (Task 8.9.)
    pub keep_going: bool,
    /// Soft role-validation set (default [`crate::model::KNOWN_ROLES`],
    /// overridable per context).
    pub known_roles: BTreeSet<String>,
    /// Roles whose holder set must not be emptied (IAM-001). Default
    /// `[ROLE_ADMIN]`; an empty set disables the admin-lockout check.
    /// Per-context `iam.protected-roles` resolves into this (wired at the CLI
    /// layer, Group 8).
    pub protected_roles: BTreeSet<String>,
    /// `--allow-admin-lockout --yes` — skips the IAM-001 admin-lockout refusal
    /// only. IAM-002 self-lockout has no override.
    pub allow_admin_lockout: bool,
}

/// Top-level state of an apply run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyState {
    /// An input-level check (PR-IAM-002 duplicate name) aborted before any
    /// GET or write.
    AbortedInput,
    /// `--dry-run`: planned and rendered, nothing executed.
    DryRun,
    /// Phase 2 ran to completion (individual users may still have failed —
    /// inspect per-user outcomes).
    Completed,
    /// `--keep-going` was off and a per-user failure halted Phase 2.
    StoppedEarly,
}

/// What happened to one declared user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserResult {
    /// Plan phase failed for this user (PR-IAM-005, or the per-user GET
    /// errored); not executed.
    PlanFailed,
    /// Already in the desired state; no write.
    Unchanged,
    /// `--dry-run`: actions were planned but not executed.
    Planned,
    /// All of this user's actions executed successfully.
    Applied,
    /// A Phase-2 action failed.
    Failed,
}

/// Per-user outcome carrying the planned actions and findings.
#[derive(Clone, Debug)]
pub struct UserOutcome {
    pub name: String,
    pub planned: Vec<UserPlan>,
    pub warnings: Vec<Finding>,
    pub errors: Vec<Finding>,
    pub result: UserResult,
    /// Phase-2 error message, if `result == Failed`.
    pub failure: Option<String>,
}

/// Aggregate result of an apply run.
#[derive(Clone, Debug)]
pub struct ApplyReport {
    pub state: ApplyState,
    /// Input-level findings (PR-IAM-002). Non-empty ⇒ `state == AbortedInput`.
    pub input_findings: Vec<Finding>,
    pub users: Vec<UserOutcome>,
    /// `GET /users` snapshot taken in the plan phase; the Group 7 lockout
    /// checks consume this.
    pub server_users: Vec<OnmsUserWire>,
}

/// Phase-1 output: per-user reconciles (in input order) plus the lockout
/// snapshot.
pub struct PlannedUsers {
    pub reconciles: Vec<(String, UserReconcile)>,
    pub server_users: Vec<OnmsUserWire>,
}

/// Phase 1 — plan every declared user against its server state (task 6.2).
///
/// Issues one `GET /users/{name}` per user plus a single `GET /users` for the
/// lockout snapshot. A per-user GET error is recorded as a plan failure for
/// that user (so partial failure doesn't lose the other plans); only the
/// lockout-snapshot GET propagates as a hard error (we can't evaluate the
/// invariant without it).
pub async fn plan_users(
    docs: &[UserLocal],
    api: &IamApi<'_>,
    known_roles: &BTreeSet<String>,
) -> Result<PlannedUsers> {
    let list = api.list_users().await?;
    if list.total_count > USER_LIST_LIMIT {
        return Err(Error::Config(format!(
            "server reports {} users (> {USER_LIST_LIMIT}); refusing to plan against a possibly \
             truncated snapshot — file an issue so onmsctl can add real paging",
            list.total_count
        )));
    }
    let server_users = list.users;

    let mut reconciles = Vec::with_capacity(docs.len());
    for local in docs {
        let name = local.metadata.name.clone();
        match api.get_user(&name).await {
            Ok(server) => {
                reconciles.push((name, plan_user(local, server.as_ref(), known_roles)));
            }
            Err(e) => {
                // Record a plan failure for this user; keep planning the rest.
                let mut r = UserReconcile::default();
                r.errors.push(Finding {
                    code: "PR-IAM-007",
                    severity: super::Severity::Error,
                    message: format!("user {name:?}: plan-phase GET /users/{name} failed: {e}"),
                });
                reconciles.push((name, r));
            }
        }
    }

    Ok(PlannedUsers {
        reconciles,
        server_users,
    })
}

/// Execute rank of an atomic action: creates first, then updates, then role
/// adds, then role removes. `Unchanged` is never executed.
fn execute_rank(plan: &UserPlan) -> u8 {
    match plan {
        UserPlan::Create { .. } => 0,
        UserPlan::Update { .. } => 1,
        UserPlan::RoleAdd { .. } => 2,
        UserPlan::RoleRemove { .. } => 3,
        UserPlan::Unchanged { .. } => u8::MAX,
    }
}

/// Phase 2 — execute the planned actions (task 6.3).
///
/// Actions across all users are flattened and ordered by
/// (creates→updates→role-adds→role-removes, then alphabetical by username)
/// for deterministic output. A failure marks its user `Failed`; with
/// `keep_going` false the run stops (`StoppedEarly`) and not-yet-started
/// users stay `Planned`-but-unexecuted (reported as `Failed` only if they
/// were the failing one).
pub async fn execute_plans(
    planned: Vec<(String, UserReconcile)>,
    api: &IamApi<'_>,
    keep_going: bool,
) -> (Vec<UserOutcome>, ApplyState) {
    // Seed per-user outcomes; users with plan errors are PlanFailed and never
    // executed.
    let mut outcomes: Vec<UserOutcome> = planned
        .iter()
        .map(|(name, rec)| {
            let result = if !rec.errors.is_empty() {
                UserResult::PlanFailed
            } else if rec
                .plans
                .iter()
                .all(|p| matches!(p, UserPlan::Unchanged { .. }))
            {
                UserResult::Unchanged
            } else {
                UserResult::Applied // optimistic; downgraded on failure
            };
            UserOutcome {
                name: name.clone(),
                planned: rec.plans.clone(),
                warnings: rec.warnings.clone(),
                errors: rec.errors.clone(),
                result,
                failure: None,
            }
        })
        .collect();

    // Flatten executable actions (skip PlanFailed users and Unchanged).
    let mut actions: Vec<(usize, UserPlan)> = Vec::new();
    for (idx, (_, rec)) in planned.iter().enumerate() {
        if !rec.errors.is_empty() {
            continue;
        }
        for plan in &rec.plans {
            if !matches!(plan, UserPlan::Unchanged { .. }) {
                actions.push((idx, plan.clone()));
            }
        }
    }
    actions.sort_by(|(_, a), (_, b)| {
        execute_rank(a)
            .cmp(&execute_rank(b))
            .then_with(|| a.name().cmp(b.name()))
    });

    let mut state = ApplyState::Completed;
    for (idx, plan) in actions {
        if let Err(msg) = execute_one(&plan, api).await {
            outcomes[idx].result = UserResult::Failed;
            if outcomes[idx].failure.is_none() {
                outcomes[idx].failure = Some(msg);
            }
            if !keep_going {
                state = ApplyState::StoppedEarly;
                break;
            }
        }
    }

    (outcomes, state)
}

/// Run a single atomic action. Returns the error message string on failure
/// (kept `Serialize`-friendly for `-o json` consumers, matching provisioning).
async fn execute_one(plan: &UserPlan, api: &IamApi<'_>) -> std::result::Result<(), String> {
    match plan {
        UserPlan::Unchanged { .. } => Ok(()),
        UserPlan::Create { local, .. } => {
            // Resolve the passwordRef here (I/O). A Create without a ref never
            // reaches Phase 2 (PR-IAM-005 fails it in plan), so unwrap-as-error
            // the absent case defensively.
            let pref = local.spec.password_ref.as_ref().ok_or_else(|| {
                "internal: Create plan reached execute without a passwordRef".to_string()
            })?;
            let secret = resolve_password_ref(pref).map_err(|e| e.to_string())?;
            api.post_user(local, secret.expose())
                .await
                .map_err(|e| e.to_string())
        }
        UserPlan::Update { name, form } => api
            .put_user_form(name, form)
            .await
            .map_err(|e| e.to_string()),
        UserPlan::RoleAdd { name, role } => api
            .put_user_role(name, role)
            .await
            .map_err(|e| e.to_string()),
        UserPlan::RoleRemove { name, role } => api
            .delete_user_role(name, role)
            .await
            .map_err(|e| e.to_string()),
    }
}

/// Orchestrate an apply run (task 6.4): input-uniqueness → plan → (lockout,
/// Group 7) → render-if-dry-run → execute.
pub async fn apply_users(
    docs: &[(PathBuf, UserLocal)],
    api: &IamApi<'_>,
    opts: &ApplyOptions,
) -> Result<ApplyReport> {
    // ---- input-level uniqueness (PR-IAM-002) ----
    if let Err(findings) = check_input_uniqueness(docs) {
        return Ok(ApplyReport {
            state: ApplyState::AbortedInput,
            input_findings: findings,
            users: vec![],
            server_users: vec![],
        });
    }

    // ---- Phase 1: plan ----
    let locals: Vec<UserLocal> = docs.iter().map(|(_, l)| l.clone()).collect();
    let planned = plan_users(&locals, api, &opts.known_roles).await?;
    let server_users = planned.server_users.clone();

    // ---- lockout invariants (IAM-001 / IAM-002), §D6 / tasks 7.1–7.3 ----
    //
    // Enforced on the real-apply path only. `--dry-run` deliberately does NOT
    // gate on lockout (and does not require `whoami`): dry-run is a review
    // workflow that must stay usable in read-only / anonymous-token contexts
    // (design §D6), which is also why §D8 classifies `apply --dry-run` as a
    // Read. This resolves the §D4-vs-§D6 tension in favour of a usable
    // dry-run; the real apply below still refuses.
    if !opts.dry_run {
        let flat: Vec<&UserPlan> = planned
            .reconciles
            .iter()
            .flat_map(|(_, r)| r.plans.iter())
            .collect();
        lockout::check_admin_lockout(
            &flat,
            &server_users,
            &opts.protected_roles,
            opts.allow_admin_lockout,
        )?;
        // Only fetch the caller identity when an action could actually
        // self-lock; benign applies need no `whoami` round trip.
        if lockout::self_lockout_possible(&flat, &opts.protected_roles) {
            let whoami = api.get_whoami().await?.map(|u| u.user_id);
            lockout::check_self_lockout(&flat, whoami.as_deref(), &opts.protected_roles)?;
        }
    }

    // ---- dry-run short-circuit ----
    if opts.dry_run {
        let users = planned
            .reconciles
            .into_iter()
            .map(|(name, rec)| {
                let result = if !rec.errors.is_empty() {
                    UserResult::PlanFailed
                } else if rec
                    .plans
                    .iter()
                    .all(|p| matches!(p, UserPlan::Unchanged { .. }))
                {
                    UserResult::Unchanged
                } else {
                    UserResult::Planned
                };
                UserOutcome {
                    name,
                    planned: rec.plans,
                    warnings: rec.warnings,
                    errors: rec.errors,
                    result,
                    failure: None,
                }
            })
            .collect();
        return Ok(ApplyReport {
            state: ApplyState::DryRun,
            input_findings: vec![],
            users,
            server_users,
        });
    }

    // ---- Phase 2: execute ----
    let (users, state) = execute_plans(planned.reconciles, api, opts.keep_going).await;
    Ok(ApplyReport {
        state,
        input_findings: vec![],
        users,
        server_users,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::local::{ApiVersion, FromFileRef, KindUser, Metadata, PasswordRef, UserSpec};
    use onmsctl_core::{AuthCreds, Context, OnmsClient, OutputFormat, Url};
    use std::io::Write;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_client() -> (MockServer, OnmsClient) {
        let server = MockServer::start().await;
        let url = Url::parse(&format!("{}/", server.uri())).unwrap();
        let ctx = Context {
            name: "test".into(),
            url,
            creds: AuthCreds::basic("admin", "secret"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        };
        (server, OnmsClient::from_context(&ctx).unwrap())
    }

    fn known() -> BTreeSet<String> {
        crate::model::KNOWN_ROLES
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn opts(dry_run: bool, keep_going: bool) -> ApplyOptions {
        ApplyOptions {
            dry_run,
            keep_going,
            known_roles: known(),
            protected_roles: BTreeSet::from(["ROLE_ADMIN".to_string()]),
            allow_admin_lockout: false,
        }
    }

    async fn mount_whoami(server: &MockServer, user_id: &str) {
        Mock::given(method("GET"))
            .and(path("/rest/users/whoami"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": user_id
            })))
            .mount(server)
            .await;
    }

    /// A passwordRef pointing at a 0600 temp file (deterministic, no env race).
    fn pw_file() -> (tempfile::NamedTempFile, PasswordRef) {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "s3cr3t").unwrap();
        let pref = PasswordRef::FromFile(FromFileRef {
            from_file: f.path().to_path_buf(),
        });
        (f, pref)
    }

    fn local(name: &str, roles: &[&str], pref: Option<PasswordRef>) -> UserLocal {
        UserLocal {
            api_version: ApiVersion,
            kind: KindUser,
            metadata: Metadata {
                name: name.into(),
                unmodeled: None,
            },
            spec: UserSpec {
                full_name: Some("Full Name".into()),
                email: None,
                comments: None,
                duty_schedule: None,
                roles: roles.iter().map(|s| s.to_string()).collect(),
                password_ref: pref,
            },
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

    #[tokio::test]
    async fn create_when_absent_posts_xml_with_roles() {
        let (server, client) = mock_client().await;
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
        let api = IamApi::new(&client);
        let (_f, pref) = pw_file();
        let docs = vec![(
            PathBuf::from("a.yaml"),
            local("alice", &["ROLE_USER"], Some(pref)),
        )];
        let report = apply_users(&docs, &api, &opts(false, false)).await.unwrap();
        assert_eq!(report.state, ApplyState::Completed);
        assert_eq!(report.users[0].result, UserResult::Applied);
    }

    #[tokio::test]
    async fn create_without_password_ref_fails_plan_no_post() {
        let (server, client) = mock_client().await;
        mount_users_list(&server, serde_json::json!([]), 0).await;
        Mock::given(method("GET"))
            .and(path("/rest/users/bob"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // No POST mock: if execute tried to POST it would 404 and surface a
        // different result. PlanFailed means no POST was attempted.
        let api = IamApi::new(&client);
        let docs = vec![(PathBuf::from("b.yaml"), local("bob", &[], None))];
        let report = apply_users(&docs, &api, &opts(false, false)).await.unwrap();
        assert_eq!(report.users[0].result, UserResult::PlanFailed);
        assert_eq!(report.users[0].errors[0].code, "PR-IAM-005");
    }

    #[tokio::test]
    async fn unchanged_on_second_apply_idempotent() {
        let (server, client) = mock_client().await;
        let existing = serde_json::json!({
            "user-id": "alice", "full-name": "Full Name", "role": ["ROLE_USER"]
        });
        mount_users_list(&server, serde_json::json!([existing]), 1).await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice", "full-name": "Full Name", "role": ["ROLE_USER"]
            })))
            .mount(&server)
            .await;
        let api = IamApi::new(&client);
        let docs = vec![(
            PathBuf::from("a.yaml"),
            local("alice", &["ROLE_USER"], None),
        )];
        let report = apply_users(&docs, &api, &opts(false, false)).await.unwrap();
        assert_eq!(report.users[0].result, UserResult::Unchanged);
    }

    #[tokio::test]
    async fn update_scalar_puts_form() {
        let (server, client) = mock_client().await;
        mount_users_list(&server, serde_json::json!([]), 0).await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice", "full-name": "Old Name", "role": ["ROLE_USER"]
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let api = IamApi::new(&client);
        let docs = vec![(
            PathBuf::from("a.yaml"),
            local("alice", &["ROLE_USER"], None),
        )];
        let report = apply_users(&docs, &api, &opts(false, false)).await.unwrap();
        assert_eq!(report.users[0].result, UserResult::Applied);
        assert!(matches!(
            report.users[0].planned[0],
            UserPlan::Update { .. }
        ));
    }

    #[tokio::test]
    async fn role_set_diff_adds_and_removes() {
        let (server, client) = mock_client().await;
        mount_users_list(&server, serde_json::json!([]), 0).await;
        // server [DASHBOARD, REST]; local [DASHBOARD, USER] → Add USER, Remove
        // REST. Non-protected roles keep this focused on delta mechanics
        // without entangling the lockout invariant.
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice", "full-name": "Full Name", "role": ["ROLE_DASHBOARD", "ROLE_REST"]
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/users/alice/roles/ROLE_USER"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/rest/users/alice/roles/ROLE_REST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let api = IamApi::new(&client);
        let docs = vec![(
            PathBuf::from("a.yaml"),
            local("alice", &["ROLE_DASHBOARD", "ROLE_USER"], None),
        )];
        let report = apply_users(&docs, &api, &opts(false, false)).await.unwrap();
        assert_eq!(report.users[0].result, UserResult::Applied);
    }

    // ---- Group 7 lockout, end-to-end through apply_users ----

    /// A user (sole admin) being demoted from ROLE_ADMIN → IAM-001 before any
    /// write; whoami is never reached (admin check runs first).
    #[tokio::test]
    async fn admin_lockout_refused_end_to_end() {
        let (server, client) = mock_client().await;
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
        // No write mocks — must refuse before executing.
        let api = IamApi::new(&client);
        let docs = vec![(
            PathBuf::from("a.yaml"),
            local("alice", &["ROLE_USER"], None),
        )];
        let err = apply_users(&docs, &api, &opts(false, false))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::IamLockout { .. }));
    }

    /// The caller (alice) demoting their own ROLE_ADMIN, with a second admin
    /// surviving (so IAM-001 passes) → IAM-002, no override.
    #[tokio::test]
    async fn self_lockout_refused_end_to_end() {
        let (server, client) = mock_client().await;
        mount_users_list(
            &server,
            serde_json::json!([
                {"user-id": "alice", "role": ["ROLE_ADMIN"]},
                {"user-id": "bob", "role": ["ROLE_ADMIN"]}
            ]),
            2,
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice", "full-name": "Full Name", "role": ["ROLE_ADMIN"]
            })))
            .mount(&server)
            .await;
        mount_whoami(&server, "alice").await;
        let api = IamApi::new(&client);
        let docs = vec![(
            PathBuf::from("a.yaml"),
            local("alice", &["ROLE_USER"], None),
        )];
        let err = apply_users(&docs, &api, &opts(false, false))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::IamSelfLockout { user } if user == "alice"));
    }

    /// A risky (protected RoleRemove) apply where whoami is unavailable (401)
    /// → IamWhoamiUnavailable. Two admins so IAM-001 passes and we reach the
    /// self-lockout check.
    #[tokio::test]
    async fn whoami_unavailable_refused_end_to_end() {
        let (server, client) = mock_client().await;
        mount_users_list(
            &server,
            serde_json::json!([
                {"user-id": "alice", "role": ["ROLE_ADMIN"]},
                {"user-id": "bob", "role": ["ROLE_ADMIN"]}
            ]),
            2,
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice", "full-name": "Full Name", "role": ["ROLE_ADMIN"]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/users/whoami"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let api = IamApi::new(&client);
        let docs = vec![(
            PathBuf::from("a.yaml"),
            local("alice", &["ROLE_USER"], None),
        )];
        let err = apply_users(&docs, &api, &opts(false, false))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::IamWhoamiUnavailable));
    }

    /// `--allow-admin-lockout` skips IAM-001; with a non-self caller the
    /// demotion of the sole admin then proceeds to execute.
    #[tokio::test]
    async fn allow_admin_lockout_override_proceeds() {
        let (server, client) = mock_client().await;
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
        mount_whoami(&server, "root").await;
        // Plan = RoleAdd(ROLE_USER) + RoleRemove(ROLE_ADMIN); mock both.
        Mock::given(method("PUT"))
            .and(path("/rest/users/alice/roles/ROLE_USER"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/rest/users/alice/roles/ROLE_ADMIN"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let api = IamApi::new(&client);
        let docs = vec![(
            PathBuf::from("a.yaml"),
            local("alice", &["ROLE_USER"], None),
        )];
        let mut o = opts(false, false);
        o.allow_admin_lockout = true;
        let report = apply_users(&docs, &api, &o).await.unwrap();
        assert_eq!(report.state, ApplyState::Completed);
        assert_eq!(report.users[0].result, UserResult::Applied);
    }

    #[tokio::test]
    async fn dry_run_plans_but_does_not_execute() {
        let (server, client) = mock_client().await;
        mount_users_list(&server, serde_json::json!([]), 0).await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // No POST mock — a dry run must not attempt it.
        let api = IamApi::new(&client);
        let (_f, pref) = pw_file();
        let docs = vec![(
            PathBuf::from("a.yaml"),
            local("alice", &["ROLE_USER"], Some(pref)),
        )];
        let report = apply_users(&docs, &api, &opts(true, false)).await.unwrap();
        assert_eq!(report.state, ApplyState::DryRun);
        assert_eq!(report.users[0].result, UserResult::Planned);
    }

    #[tokio::test]
    async fn duplicate_input_name_aborts_before_any_get() {
        let (_server, client) = mock_client().await;
        // No mocks at all — abort must happen before any HTTP.
        let api = IamApi::new(&client);
        let docs = vec![
            (PathBuf::from("a.yaml"), local("alice", &[], None)),
            (PathBuf::from("dup.yaml"), local("alice", &[], None)),
        ];
        let report = apply_users(&docs, &api, &opts(false, false)).await.unwrap();
        assert_eq!(report.state, ApplyState::AbortedInput);
        assert_eq!(report.input_findings[0].code, "PR-IAM-002");
        assert!(report.users.is_empty());
    }

    #[tokio::test]
    async fn list_over_limit_refuses() {
        let (server, client) = mock_client().await;
        mount_users_list(&server, serde_json::json!([]), 10_001).await;
        let api = IamApi::new(&client);
        let docs = vec![(PathBuf::from("a.yaml"), local("alice", &[], None))];
        let err = apply_users(&docs, &api, &opts(false, false))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[tokio::test]
    async fn partial_failure_continues_with_keep_going() {
        let (server, client) = mock_client().await;
        mount_users_list(&server, serde_json::json!([]), 0).await;
        // alice: update succeeds. bob: update 500s.
        for u in ["alice", "bob"] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/users/{u}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "user-id": u, "full-name": "Old", "role": []
                })))
                .mount(&server)
                .await;
        }
        Mock::given(method("PUT"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/users/bob"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let api = IamApi::new(&client);
        let docs = vec![
            (PathBuf::from("a.yaml"), local("alice", &[], None)),
            (PathBuf::from("b.yaml"), local("bob", &[], None)),
        ];
        let report = apply_users(&docs, &api, &opts(false, true)).await.unwrap();
        assert_eq!(report.state, ApplyState::Completed);
        let by = |n: &str| report.users.iter().find(|u| u.name == n).unwrap();
        assert_eq!(by("alice").result, UserResult::Applied);
        assert_eq!(by("bob").result, UserResult::Failed);
    }

    #[tokio::test]
    async fn stop_on_error_halts_phase_two() {
        let (server, client) = mock_client().await;
        mount_users_list(&server, serde_json::json!([]), 0).await;
        for u in ["alice", "bob"] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/users/{u}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "user-id": u, "full-name": "Old", "role": []
                })))
                .mount(&server)
                .await;
        }
        // alice (first alphabetically) update 500s → stop before bob.
        Mock::given(method("PUT"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let api = IamApi::new(&client);
        let docs = vec![
            (PathBuf::from("a.yaml"), local("alice", &[], None)),
            (PathBuf::from("b.yaml"), local("bob", &[], None)),
        ];
        let report = apply_users(&docs, &api, &opts(false, false)).await.unwrap();
        assert_eq!(report.state, ApplyState::StoppedEarly);
        let alice = report.users.iter().find(|u| u.name == "alice").unwrap();
        assert_eq!(alice.result, UserResult::Failed);
    }

    #[tokio::test]
    async fn per_user_get_failure_records_plan_failure() {
        let (server, client) = mock_client().await;
        mount_users_list(&server, serde_json::json!([]), 0).await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let api = IamApi::new(&client);
        let docs = vec![(PathBuf::from("a.yaml"), local("alice", &[], None))];
        let report = apply_users(&docs, &api, &opts(false, false)).await.unwrap();
        assert_eq!(report.users[0].result, UserResult::PlanFailed);
        assert_eq!(report.users[0].errors[0].code, "PR-IAM-007");
    }
}
