/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Plan / execute pipeline for `iam apply` (design §D4).
//!
//! Phase 1 (this module's pure core) reconciles each declared user against
//! its server state into a list of **atomic** [`UserPlan`] actions, plus
//! non-fatal warnings and fatal per-user errors. Phase 2 (the I/O
//! orchestration, added next) executes the actions in the order
//! creates → updates → role deltas → deletes.
//!
//! ## Why a per-user *list* of plans
//!
//! Task 6.1 sketches `plan_user -> UserPlan`, but a single existing user can
//! legitimately require several atomic actions at once — a scalar `Update`
//! **and** a `RoleAdd` **and** a `RoleRemove`. A single enum value can't
//! express that, so [`plan_user`] returns a [`UserReconcile`] carrying the
//! ordered atomic [`UserPlan`]s for that one user (plus findings). The
//! execute phase flattens reconciles across all users and buckets the
//! actions by kind.
//!
//! ## Findings emitted here
//!
//! - `PR-IAM-002` — duplicate `metadata.name` across input documents
//!   ([`check_input_uniqueness`], task 6.6, finding F8).
//! - `PR-IAM-004` — `dutySchedule` differs on an **existing** user; the
//!   form-encoded PUT can't round-trip a `List<String>`, so the change is
//!   **not applied** — a warning is emitted and other fields still apply
//!   (§D11.5).
//! - `PR-IAM-005` — a **Create** declares no `passwordRef`. Verified live
//!   (2026-05-31): `POST /users` without a password returns
//!   `500 'password' cannot be null!`, so this is a hard per-user error.
//! - `PR-IAM-006` — a declared role is outside the known-roles set; a
//!   warning only (operators may extend roles server-side), per task 3.10.
//!
//! Role deltas are computed as **exact** set differences (add only roles the
//! server lacks, remove only roles it has) because `DELETE …/roles/{role}`
//! of an unheld role returns 400 (verified live 2026-05-31).

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::model::convert::wire_to_local;
use crate::model::local::UserLocal;
use crate::model::wire::{OnmsUserWire, UpdateForm};

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

/// Severity of a planning [`Finding`]. `Error` fails the affected user's plan
/// (and, for input-level findings like `PR-IAM-002`, the whole apply);
/// `Warning` is informational and does not block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// A structured planning finding. `code` is the stable catalog id
/// (`PR-IAM-00N`); `message` is operator-facing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
}

impl Finding {
    fn warning(code: &'static str, message: String) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message,
        }
    }

    fn error(code: &'static str, message: String) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message,
        }
    }
}

// ---------------------------------------------------------------------------
// Plan model
// ---------------------------------------------------------------------------

/// A single atomic write action against the user surface. Execute order is
/// the variant order here: creates, then updates, then role adds/removes,
/// then deletes (deletes are `--prune`-only, out of scope for this change).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserPlan {
    /// The user already matches the declared spec; no write.
    Unchanged { name: String },
    /// `POST /users` — the user does not exist server-side. Roles ride in the
    /// create body (verified: the XML POST sets them), so a fresh create
    /// needs no follow-up `RoleAdd`. The resolved password is *not* stored on
    /// the plan (secret hygiene); execute resolves `local.spec.password_ref`.
    Create {
        local: Box<UserLocal>,
        roles: Vec<String>,
    },
    /// `PUT /users/{name}` form update carrying only the changed scalar
    /// fields. Never empty (an empty diff yields `Unchanged` instead).
    Update { name: String, form: UpdateForm },
    /// `PUT /users/{name}/roles/{role}` — add one role to an existing user.
    RoleAdd { name: String, role: String },
    /// `DELETE /users/{name}/roles/{role}` — remove one role the server
    /// currently holds.
    RoleRemove { name: String, role: String },
}

impl UserPlan {
    /// The username this action targets — for grouping/rendering.
    pub fn name(&self) -> &str {
        match self {
            UserPlan::Unchanged { name }
            | UserPlan::Update { name, .. }
            | UserPlan::RoleAdd { name, .. }
            | UserPlan::RoleRemove { name, .. } => name,
            UserPlan::Create { local, .. } => &local.metadata.name,
        }
    }
}

/// The outcome of reconciling **one** declared user against its server state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserReconcile {
    /// Atomic actions for this user, in execute order. Empty only when
    /// `errors` is non-empty (a user that failed to plan produces no writes).
    pub plans: Vec<UserPlan>,
    /// Non-fatal findings (e.g. `PR-IAM-004`, `PR-IAM-006`).
    pub warnings: Vec<Finding>,
    /// Fatal findings for this user (e.g. `PR-IAM-005`). When non-empty the
    /// user is **not** executed; the apply records the failure and (per
    /// `--keep-going`) may continue with other users.
    pub errors: Vec<Finding>,
}

// ---------------------------------------------------------------------------
// plan_user — task 6.1 (pure, no I/O)
// ---------------------------------------------------------------------------

/// Reconcile one declared [`UserLocal`] against its server state into atomic
/// [`UserPlan`]s. `server` is `None` when `GET /users/{name}` returned 404
/// (→ Create). `known_roles` is the soft-validation set (default
/// [`crate::model::KNOWN_ROLES`], overridable per context); roles outside it
/// emit a `PR-IAM-006` warning, never an error.
///
/// Pure: it performs no I/O and does not resolve `passwordRef` (execute
/// does). It only verifies a Create *has* a `passwordRef` (`PR-IAM-005`).
pub fn plan_user(
    local: &UserLocal,
    server: Option<&OnmsUserWire>,
    known_roles: &BTreeSet<String>,
) -> UserReconcile {
    let name = local.metadata.name.clone();
    let mut out = UserReconcile::default();

    // Soft role validation (PR-IAM-006) applies to both create and update.
    for role in &local.spec.roles {
        if !known_roles.contains(role) {
            out.warnings.push(Finding::warning(
                "PR-IAM-006",
                format!(
                    "user {name:?}: role {role:?} is not in the known-roles set; \
                     applying anyway (extend the set server-side or via `iam.known-roles` \
                     to silence this)"
                ),
            ));
        }
    }

    match server {
        // ---- Create ----
        None => {
            if local.spec.password_ref.is_none() {
                out.errors.push(Finding::error(
                    "PR-IAM-005",
                    format!(
                        "user {name:?}: creating a user requires a `passwordRef` — the server \
                         rejects a password-less create (500 'password cannot be null')"
                    ),
                ));
                return out;
            }
            let roles: Vec<String> = local.spec.roles.iter().cloned().collect();
            out.plans.push(UserPlan::Create {
                local: Box::new(local.clone()),
                roles,
            });
        }
        // ---- Reconcile against existing ----
        Some(srv) => {
            // Convert the server record through the same normalization the
            // diff baseline uses (empty→None, duty-first) so scalar
            // comparisons can't trip a false diff.
            let baseline = wire_to_local(srv);

            // Scalar field diff → form (merge semantics: only fields the
            // document declares and that differ are sent; onmsctl does not
            // clear a field the document omits).
            let form = UpdateForm {
                full_name: changed_scalar(&local.spec.full_name, &baseline.spec.full_name),
                email: changed_scalar(&local.spec.email, &baseline.spec.email),
                comments: changed_scalar(&local.spec.comments, &baseline.spec.comments),
            };
            if !form.is_empty() {
                out.plans.push(UserPlan::Update {
                    name: name.clone(),
                    form,
                });
            }

            // dutySchedule is create-only; a diff on an existing user warns
            // (PR-IAM-004) and is NOT applied.
            if local.spec.duty_schedule.is_some()
                && local.spec.duty_schedule != baseline.spec.duty_schedule
            {
                out.warnings.push(Finding::warning(
                    "PR-IAM-004",
                    format!(
                        "user {name:?}: dutySchedule changes are not supported on update \
                         (delete and recreate the user, or edit upstream); leaving it unchanged"
                    ),
                ));
            }

            // Exact role delta. Add only roles the server lacks; remove only
            // roles it has (DELETE of an unheld role returns 400).
            let server_roles: BTreeSet<&String> = srv.roles.iter().collect();
            let local_roles: BTreeSet<&String> = local.spec.roles.iter().collect();
            for role in local_roles.difference(&server_roles) {
                out.plans.push(UserPlan::RoleAdd {
                    name: name.clone(),
                    role: (*role).clone(),
                });
            }
            for role in server_roles.difference(&local_roles) {
                out.plans.push(UserPlan::RoleRemove {
                    name: name.clone(),
                    role: (*role).clone(),
                });
            }

            if out.plans.is_empty() {
                out.plans.push(UserPlan::Unchanged { name });
            }
        }
    }

    out
}

/// `Some(local_value)` when the document declares a value that differs from
/// the server baseline; `None` (omit from the form) otherwise. A document
/// that omits the field (`None`) never clears the server value.
fn changed_scalar(local: &Option<String>, baseline: &Option<String>) -> Option<String> {
    match local {
        Some(v) if Some(v) != baseline.as_ref() => Some(v.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// check_input_uniqueness — task 6.6 (PR-IAM-002, F8)
// ---------------------------------------------------------------------------

/// Refuse if two or more input documents declare the same `metadata.name`.
/// Runs before the plan phase so a duplicate never reaches the per-user GET.
/// Returns one `PR-IAM-002` [`Finding`] per colliding name, listing every
/// source path that declares it. `Ok(())` when all names are distinct.
pub fn check_input_uniqueness(docs: &[(PathBuf, UserLocal)]) -> Result<(), Vec<Finding>> {
    use std::collections::BTreeMap;

    let mut by_name: BTreeMap<&str, Vec<&PathBuf>> = BTreeMap::new();
    for (path, local) in docs {
        by_name
            .entry(local.metadata.name.as_str())
            .or_default()
            .push(path);
    }

    let findings: Vec<Finding> = by_name
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(name, paths)| {
            let list = paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Finding::error(
                "PR-IAM-002",
                format!(
                    "user {name:?} is declared in {} documents ({list}); each user must be \
                     declared at most once across the apply input",
                    paths.len()
                ),
            )
        })
        .collect();

    if findings.is_empty() {
        Ok(())
    } else {
        Err(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::local::{ApiVersion, FromEnvRef, KindUser, Metadata, PasswordRef, UserSpec};

    fn known() -> BTreeSet<String> {
        crate::model::KNOWN_ROLES
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn local_user(name: &str, roles: &[&str], with_password: bool) -> UserLocal {
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
                password_ref: with_password.then(|| {
                    PasswordRef::FromEnv(FromEnvRef {
                        from_env: "PW".into(),
                    })
                }),
            },
        }
    }

    fn server_user(name: &str, full_name: Option<&str>, roles: &[&str]) -> OnmsUserWire {
        OnmsUserWire {
            user_id: name.into(),
            full_name: full_name.map(str::to_owned),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    // ---- Create ----

    #[test]
    fn create_when_server_absent() {
        let local = local_user("alice", &["ROLE_USER"], true);
        let r = plan_user(&local, None, &known());
        assert!(r.errors.is_empty());
        assert_eq!(r.plans.len(), 1);
        match &r.plans[0] {
            UserPlan::Create { roles, .. } => assert_eq!(roles, &["ROLE_USER"]),
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn create_without_password_ref_is_pr_iam_005_error() {
        let local = local_user("alice", &[], false);
        let r = plan_user(&local, None, &known());
        assert!(r.plans.is_empty(), "no plan when create errors");
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].code, "PR-IAM-005");
    }

    // ---- Unchanged ----

    #[test]
    fn unchanged_when_identical() {
        let local = local_user("alice", &["ROLE_USER"], true);
        let srv = server_user("alice", Some("Full Name"), &["ROLE_USER"]);
        let r = plan_user(&local, Some(&srv), &known());
        assert_eq!(
            r.plans,
            vec![UserPlan::Unchanged {
                name: "alice".into()
            }]
        );
        assert!(r.warnings.is_empty());
    }

    // ---- Update ----

    #[test]
    fn update_only_changed_scalar() {
        let mut local = local_user("alice", &["ROLE_USER"], true);
        local.spec.full_name = Some("New Name".into());
        let srv = server_user("alice", Some("Old Name"), &["ROLE_USER"]);
        let r = plan_user(&local, Some(&srv), &known());
        assert_eq!(r.plans.len(), 1);
        match &r.plans[0] {
            UserPlan::Update { name, form } => {
                assert_eq!(name, "alice");
                assert_eq!(form.full_name.as_deref(), Some("New Name"));
                assert!(form.email.is_none() && form.comments.is_none());
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn omitted_field_does_not_clear_server_value() {
        // local omits email; server has one → no form change for email.
        let local = local_user("alice", &["ROLE_USER"], true);
        let mut srv = server_user("alice", Some("Full Name"), &["ROLE_USER"]);
        srv.email = Some("keep@example.com".into());
        let r = plan_user(&local, Some(&srv), &known());
        assert_eq!(
            r.plans,
            vec![UserPlan::Unchanged {
                name: "alice".into()
            }]
        );
    }

    // ---- Role deltas ----

    #[test]
    fn role_set_diff_adds_and_removes_exactly() {
        // server [A,B], local [B,C] → AddC + RemoveA, leaves B.
        let local = local_user("alice", &["ROLE_REST", "ROLE_USER"], true); // B=REST, C=USER
        let srv = server_user("alice", Some("Full Name"), &["ROLE_ADMIN", "ROLE_REST"]); // A=ADMIN,B=REST
        let r = plan_user(&local, Some(&srv), &known());
        let adds: Vec<&str> = r
            .plans
            .iter()
            .filter_map(|p| match p {
                UserPlan::RoleAdd { role, .. } => Some(role.as_str()),
                _ => None,
            })
            .collect();
        let removes: Vec<&str> = r
            .plans
            .iter()
            .filter_map(|p| match p {
                UserPlan::RoleRemove { role, .. } => Some(role.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(adds, vec!["ROLE_USER"]);
        assert_eq!(removes, vec!["ROLE_ADMIN"]);
    }

    #[test]
    fn update_and_role_delta_combine_for_one_user() {
        let mut local = local_user("alice", &["ROLE_USER", "ROLE_REST"], true);
        local.spec.full_name = Some("Renamed".into());
        let srv = server_user("alice", Some("Old"), &["ROLE_USER"]);
        let r = plan_user(&local, Some(&srv), &known());
        // One Update + one RoleAdd(ROLE_REST), update first.
        assert!(matches!(r.plans[0], UserPlan::Update { .. }));
        assert!(
            r.plans
                .iter()
                .any(|p| matches!(p, UserPlan::RoleAdd { role, .. } if role == "ROLE_REST"))
        );
    }

    // ---- Findings ----

    #[test]
    fn unknown_role_warns_pr_iam_006_but_still_plans() {
        let local = local_user("alice", &["ROLE_MADE_UP"], true);
        let r = plan_user(&local, None, &known());
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(r.warnings[0].code, "PR-IAM-006");
        assert_eq!(r.plans.len(), 1, "unknown role still plans the create");
    }

    #[test]
    fn duty_schedule_change_on_update_warns_pr_iam_004_and_is_not_applied() {
        let mut local = local_user("alice", &["ROLE_USER"], true);
        local.spec.duty_schedule = Some("MoTuWeThFr0800-1700".into());
        let srv = server_user("alice", Some("Full Name"), &["ROLE_USER"]); // no duty schedule
        let r = plan_user(&local, Some(&srv), &known());
        assert!(r.warnings.iter().any(|f| f.code == "PR-IAM-004"));
        // No write action carries dutySchedule; the only diff was duty → Unchanged.
        assert_eq!(
            r.plans,
            vec![UserPlan::Unchanged {
                name: "alice".into()
            }]
        );
    }

    // ---- check_input_uniqueness ----

    #[test]
    fn uniqueness_ok_when_distinct() {
        let docs = vec![
            (PathBuf::from("a.yaml"), local_user("alice", &[], true)),
            (PathBuf::from("b.yaml"), local_user("bob", &[], true)),
        ];
        assert!(check_input_uniqueness(&docs).is_ok());
    }

    #[test]
    fn uniqueness_reports_pr_iam_002_with_paths() {
        let docs = vec![
            (PathBuf::from("a.yaml"), local_user("alice", &[], true)),
            (PathBuf::from("dup.yaml"), local_user("alice", &[], true)),
            (PathBuf::from("c.yaml"), local_user("carol", &[], true)),
        ];
        let err = check_input_uniqueness(&docs).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].code, "PR-IAM-002");
        assert!(err[0].message.contains("a.yaml"));
        assert!(err[0].message.contains("dup.yaml"));
        assert!(err[0].message.contains("alice"));
    }
}
