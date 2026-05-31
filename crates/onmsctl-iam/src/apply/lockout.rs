/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Lockout invariants for `iam apply` (design §D6, tasks 7.1–7.3).
//!
//! Two safety checks run after the plan phase and before any write:
//!
//! - **IAM-001 — admin lockout** ([`check_admin_lockout`]): refuse if the
//!   apply would leave a *protected* role (default `ROLE_ADMIN`) with **zero**
//!   holders. Overridable with `--allow-admin-lockout --yes`.
//! - **IAM-002 — self lockout** ([`check_self_lockout`]): refuse if the apply
//!   would strip the **calling** user's own protected role (or delete their
//!   account). **No override.**
//!
//! ## Data source
//!
//! `server_users` is the single `GET /users` snapshot — spike 0.3 confirmed
//! list entries carry inline `role:[...]`, so no per-user GET fallback is
//! needed (task 7.1).
//!
//! ## Refinement over the literal §D6 pseudocode
//!
//! The pseudocode reads "if effective_holders is empty: refuse". Taken
//! literally that refuses *any* apply against a server that already has zero
//! protected-role holders — but such an apply didn't *cause* the lockout.
//! [`admin_lockout_roles`] therefore only reports a role whose holder set was
//! **non-empty before** and becomes empty after; a role already at zero
//! holders is the operator's pre-existing state, not something this apply
//! broke. (A first-admin `Create` still passes — it adds a holder.)
//!
//! ## Deletes
//!
//! `iam apply --prune` (delete-not-in-file) is out of scope for this change,
//! so there is no `Delete` plan variant. The self-lockout "is the target of a
//! Delete" clause is structurally unreachable today and is noted where it
//! would apply.

use std::collections::BTreeSet;

use onmsctl_core::{Error, Result};

use crate::apply::UserPlan;
use crate::model::wire::OnmsUserWire;

/// Roles in `protected` that the planned actions would leave with **zero**
/// holders, given the server snapshot. Empty ⇒ no admin lockout.
///
/// A role whose current holder set is already empty is **not** reported (this
/// apply didn't cause it). Returned sorted for deterministic messages.
pub fn admin_lockout_roles(
    plans: &[&UserPlan],
    server_users: &[OnmsUserWire],
    protected: &BTreeSet<String>,
) -> Vec<String> {
    let mut locked = Vec::new();
    for role in protected {
        // Holders on the server right now.
        let current: BTreeSet<&str> = server_users
            .iter()
            .filter(|u| u.roles.iter().any(|r| r == role))
            .map(|u| u.user_id.as_str())
            .collect();
        if current.is_empty() {
            // Already zero holders — not this apply's doing.
            continue;
        }

        // Who the apply adds this role to (RoleAdd, or a Create that includes
        // it) and removes it from (RoleRemove).
        let mut added: BTreeSet<&str> = BTreeSet::new();
        let mut removed: BTreeSet<&str> = BTreeSet::new();
        for plan in plans {
            match plan {
                UserPlan::RoleAdd { name, role: r } if r == role => {
                    added.insert(name.as_str());
                }
                UserPlan::RoleRemove { name, role: r } if r == role => {
                    removed.insert(name.as_str());
                }
                UserPlan::Create { local, roles } if roles.iter().any(|r| r == role) => {
                    added.insert(local.metadata.name.as_str());
                }
                _ => {}
            }
        }

        // effective = (current − removed) ∪ added
        let effective: BTreeSet<&str> = current
            .difference(&removed)
            .copied()
            .chain(added.iter().copied())
            .collect();
        if effective.is_empty() {
            locked.push(role.clone());
        }
    }
    locked.sort();
    locked
}

/// IAM-001 enforcement (task 7.1/7.3). `Err(Error::IamLockout)` when the apply
/// would empty a protected role's holder set, unless `allow_override` is set
/// (`--allow-admin-lockout --yes`), in which case it returns `Ok(())`.
pub fn check_admin_lockout(
    plans: &[&UserPlan],
    server_users: &[OnmsUserWire],
    protected: &BTreeSet<String>,
    allow_override: bool,
) -> Result<()> {
    let locked = admin_lockout_roles(plans, server_users, protected);
    if locked.is_empty() || allow_override {
        Ok(())
    } else {
        Err(Error::IamLockout {
            roles: locked.join(", "),
        })
    }
}

/// The set of usernames this apply would strip a protected role from (or
/// delete — no `Delete` variant exists today). When non-empty the self-lockout
/// check needs the caller's identity; when empty, no apply action could
/// self-lock, so `whoami` is not required.
pub fn self_lockout_candidates(
    plans: &[&UserPlan],
    protected: &BTreeSet<String>,
) -> BTreeSet<String> {
    plans
        .iter()
        .filter_map(|plan| match plan {
            UserPlan::RoleRemove { name, role } if protected.contains(role) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// IAM-002 enforcement (task 7.2/7.3). **No override.**
///
/// - If no planned action could self-lock (no protected `RoleRemove`/delete),
///   returns `Ok(())` — `whoami` is irrelevant and need not be fetched.
/// - Otherwise, a missing caller identity (`whoami_user == None`, i.e. a
///   non-2xx/empty `GET /users/whoami`) refuses with
///   [`Error::IamWhoamiUnavailable`] rather than skipping silently.
/// - If the caller is among the users losing a protected role, refuses with
///   [`Error::IamSelfLockout`].
pub fn check_self_lockout(
    plans: &[&UserPlan],
    whoami_user: Option<&str>,
    protected: &BTreeSet<String>,
) -> Result<()> {
    let candidates = self_lockout_candidates(plans, protected);
    if candidates.is_empty() {
        return Ok(());
    }
    match whoami_user {
        None => Err(Error::IamWhoamiUnavailable),
        Some(me) if candidates.contains(me) => Err(Error::IamSelfLockout {
            user: me.to_owned(),
        }),
        Some(_) => Ok(()),
    }
}

/// `true` when at least one planned action could self-lock the caller — used
/// by the orchestrator to decide whether a `whoami` fetch is needed.
pub fn self_lockout_possible(plans: &[&UserPlan], protected: &BTreeSet<String>) -> bool {
    !self_lockout_candidates(plans, protected).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::local::{ApiVersion, KindUser, Metadata, UserLocal, UserSpec};

    fn protected() -> BTreeSet<String> {
        BTreeSet::from(["ROLE_ADMIN".to_string()])
    }

    fn srv(name: &str, roles: &[&str]) -> OnmsUserWire {
        OnmsUserWire {
            user_id: name.into(),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn create_plan(name: &str, roles: &[&str]) -> UserPlan {
        let local = UserLocal {
            api_version: ApiVersion,
            kind: KindUser,
            metadata: Metadata {
                name: name.into(),
                unmodeled: None,
            },
            spec: UserSpec {
                full_name: None,
                email: None,
                comments: None,
                duty_schedule: None,
                roles: roles.iter().map(|s| s.to_string()).collect(),
                password_ref: None,
            },
        };
        UserPlan::Create {
            local: Box::new(local),
            roles: roles.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn remove(name: &str, role: &str) -> UserPlan {
        UserPlan::RoleRemove {
            name: name.into(),
            role: role.into(),
        }
    }

    fn add(name: &str, role: &str) -> UserPlan {
        UserPlan::RoleAdd {
            name: name.into(),
            role: role.into(),
        }
    }

    // ---- IAM-001 admin lockout ----

    #[test]
    fn single_admin_demotion_refused() {
        let server = vec![srv("alice", &["ROLE_ADMIN"])];
        let plans = [remove("alice", "ROLE_ADMIN")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        let err = check_admin_lockout(&refs, &server, &protected(), false).unwrap_err();
        assert!(matches!(err, Error::IamLockout { .. }));
    }

    #[test]
    fn two_admins_demote_one_allowed() {
        let server = vec![srv("alice", &["ROLE_ADMIN"]), srv("bob", &["ROLE_ADMIN"])];
        let plans = [remove("alice", "ROLE_ADMIN")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_admin_lockout(&refs, &server, &protected(), false).is_ok());
    }

    #[test]
    fn last_admin_demotion_saved_by_concurrent_create_admin() {
        // alice (sole admin) demoted, but a new admin is created in the same
        // apply → effective holders non-empty → allowed.
        let server = vec![srv("alice", &["ROLE_ADMIN"])];
        let plans = [
            remove("alice", "ROLE_ADMIN"),
            create_plan("carol", &["ROLE_ADMIN"]),
        ];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_admin_lockout(&refs, &server, &protected(), false).is_ok());
    }

    #[test]
    fn last_admin_demotion_saved_by_concurrent_role_add() {
        let server = vec![srv("alice", &["ROLE_ADMIN"]), srv("bob", &[])];
        let plans = [remove("alice", "ROLE_ADMIN"), add("bob", "ROLE_ADMIN")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_admin_lockout(&refs, &server, &protected(), false).is_ok());
    }

    #[test]
    fn override_allows_admin_lockout() {
        let server = vec![srv("alice", &["ROLE_ADMIN"])];
        let plans = [remove("alice", "ROLE_ADMIN")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_admin_lockout(&refs, &server, &protected(), true).is_ok());
    }

    #[test]
    fn already_zero_admins_does_not_refuse_benign_apply() {
        // Server has no admins; applying a non-admin change must NOT trip
        // IAM-001 (this apply didn't cause the empty set).
        let server = vec![srv("alice", &["ROLE_USER"])];
        let plans = [add("alice", "ROLE_REST")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_admin_lockout(&refs, &server, &protected(), false).is_ok());
    }

    #[test]
    fn first_admin_create_passes() {
        let server: Vec<OnmsUserWire> = vec![];
        let plans = [create_plan("alice", &["ROLE_ADMIN"])];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_admin_lockout(&refs, &server, &protected(), false).is_ok());
    }

    #[test]
    fn custom_protected_roles_honored() {
        // Protect ROLE_RTC instead of ROLE_ADMIN.
        let custom = BTreeSet::from(["ROLE_RTC".to_string()]);
        let server = vec![srv("rtc", &["ROLE_RTC"])];
        let plans = [remove("rtc", "ROLE_RTC")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(matches!(
            check_admin_lockout(&refs, &server, &custom, false).unwrap_err(),
            Error::IamLockout { .. }
        ));
        // ...and ROLE_ADMIN is NOT protected under this config.
        let server2 = vec![srv("alice", &["ROLE_ADMIN"])];
        let plans2 = [remove("alice", "ROLE_ADMIN")];
        let refs2: Vec<&UserPlan> = plans2.iter().collect();
        assert!(check_admin_lockout(&refs2, &server2, &custom, false).is_ok());
    }

    #[test]
    fn empty_protected_roles_disables_admin_check() {
        let none = BTreeSet::new();
        let server = vec![srv("alice", &["ROLE_ADMIN"])];
        let plans = [remove("alice", "ROLE_ADMIN")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_admin_lockout(&refs, &server, &none, false).is_ok());
    }

    // ---- IAM-002 self lockout ----

    #[test]
    fn self_demotion_of_protected_role_refused() {
        let plans = [remove("alice", "ROLE_ADMIN")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        let err = check_self_lockout(&refs, Some("alice"), &protected()).unwrap_err();
        assert!(matches!(err, Error::IamSelfLockout { user } if user == "alice"));
    }

    #[test]
    fn demoting_someone_else_is_not_self_lockout() {
        let plans = [remove("alice", "ROLE_ADMIN")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_self_lockout(&refs, Some("bob"), &protected()).is_ok());
    }

    #[test]
    fn self_lockout_has_no_override_path() {
        // There is deliberately no override parameter — the only signature
        // takes (plans, whoami, protected). This test documents that demoting
        // yourself always refuses regardless of any caller intent.
        let plans = [remove("admin", "ROLE_ADMIN")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(matches!(
            check_self_lockout(&refs, Some("admin"), &protected()).unwrap_err(),
            Error::IamSelfLockout { .. }
        ));
    }

    #[test]
    fn whoami_unavailable_refuses_when_risky() {
        // A protected RoleRemove with no caller identity → IamWhoamiUnavailable.
        let plans = [remove("alice", "ROLE_ADMIN")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(matches!(
            check_self_lockout(&refs, None, &protected()).unwrap_err(),
            Error::IamWhoamiUnavailable
        ));
    }

    #[test]
    fn whoami_not_required_for_benign_apply() {
        // No protected RoleRemove → no self-lock possible → whoami irrelevant.
        let plans = [add("alice", "ROLE_USER"), remove("alice", "ROLE_REST")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_self_lockout(&refs, None, &protected()).is_ok());
        assert!(!self_lockout_possible(&refs, &protected()));
    }

    #[test]
    fn non_protected_role_removal_is_not_self_lockout() {
        let plans = [remove("alice", "ROLE_REST")];
        let refs: Vec<&UserPlan> = plans.iter().collect();
        assert!(check_self_lockout(&refs, Some("alice"), &protected()).is_ok());
    }
}
