/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output rendering for the IAM CLI.
//!
//! Holds the table projection for `iam user list` / `get`, the per-user
//! `--diff` renderer (tasks 9.1–9.3), structured finding output (9.4), and the
//! combined plan summary (9.5).

use std::collections::BTreeMap;

use onmsctl_core::TableRow;
use serde::Serialize;

use crate::apply::multi::{ApplyReport, UserOutcome, UserResult};
use crate::apply::{Finding, Severity, UserPlan};
use crate::model::local::{PasswordRef, UserLocal};
use crate::model::wire::OnmsUserWire;

/// One row of `iam user list` / `iam user get` table output. A trimmed
/// projection of [`OnmsUserWire`] — the server password hash is never shown.
#[derive(Debug, Serialize)]
pub struct UserRow {
    pub username: String,
    pub full_name: String,
    pub email: String,
    pub roles: String,
}

impl From<&OnmsUserWire> for UserRow {
    fn from(u: &OnmsUserWire) -> Self {
        UserRow {
            username: u.user_id.clone(),
            full_name: u.full_name.clone().unwrap_or_default(),
            email: u.email.clone().unwrap_or_default(),
            roles: u.roles.join(","),
        }
    }
}

impl TableRow for UserRow {
    fn headers() -> Vec<&'static str> {
        vec!["USERNAME", "FULL NAME", "EMAIL", "ROLES"]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.username.clone(),
            self.full_name.clone(),
            self.email.clone(),
            self.roles.clone(),
        ]
    }
}

/// Render a structured finding to a single stderr line, e.g.
/// `[PR-IAM-006] warning: user "alice": role ...` (task 9.4 — code +
/// severity + message; the message carries the reference inline). IAM-001 /
/// IAM-002 propagate as `onmsctl_core::Error` and render via the binary's
/// error handler, whose Display already leads with the `IAM-00N:` code.
pub fn describe_finding(f: &Finding) -> String {
    let sev = match f.severity {
        Severity::Warning => "warning",
        Severity::Error => "error",
    };
    format!("[{}] {}: {}", f.code, sev, f.message)
}

/// Render the password **source** (never the secret value) for a Create diff
/// (task 9.2): `fromFile:/path`, `fromEnv:VAR`, `fromKeyring:service/account`.
fn describe_password_ref(p: &PasswordRef) -> String {
    match p {
        PasswordRef::FromFile(r) => format!("fromFile:{}", r.from_file.display()),
        PasswordRef::FromEnv(r) => format!("fromEnv:{}", r.from_env),
        PasswordRef::FromKeyring(r) => {
            format!(
                "fromKeyring:{}/{}",
                r.from_keyring.service, r.from_keyring.account
            )
        }
    }
}

/// Collapse the unmodeled passthrough annotation to a `<N entries>` one-liner
/// (task 9.3), same shape as provisioning. `None` when absent or empty.
fn collapse_unmodeled(local: &UserLocal) -> Option<String> {
    local
        .metadata
        .unmodeled
        .as_ref()
        .filter(|m| !m.is_empty())
        .map(|m| format!("<{} entries>", m.len()))
}

/// One scalar field-change line `field: "old" -> "new"` when the form
/// declares a new value (task 9.1). `old` is the pre-apply server value.
fn field_change(label: &str, new: Option<&str>, old: Option<&str>) -> Option<String> {
    new.map(|n| format!("    {label}: {:?} -> {:?}", old.unwrap_or(""), n))
}

/// Per-user diff for `--diff` mode (task 9.1). `server` is the matching
/// pre-apply server record (for `old -> new` on updates); `None` for creates.
/// Returns one header line (`+`/`~`/`=`/`!` `name`) plus indented detail lines.
pub fn render_user_apply_diff(outcome: &UserOutcome, server: Option<&OnmsUserWire>) -> Vec<String> {
    let name = &outcome.name;
    let is_create = outcome
        .planned
        .iter()
        .any(|p| matches!(p, UserPlan::Create { .. }));
    let is_unchanged = !outcome.planned.is_empty()
        && outcome
            .planned
            .iter()
            .all(|p| matches!(p, UserPlan::Unchanged { .. }));

    let (sym, suffix) = if outcome.planned.is_empty() {
        // No atomic plans — a PlanFailed user surfaced under --dry-run/--diff.
        // Mark it skipped (matching the plan summary) rather than "~" modify.
        ("!", " (skipped)")
    } else if is_create {
        ("+", " (create)")
    } else if is_unchanged {
        ("=", " (unchanged)")
    } else {
        ("~", "")
    };
    let mut out = vec![format!("{sym} {name}{suffix}")];

    for plan in &outcome.planned {
        match plan {
            UserPlan::Unchanged { .. } => {}
            UserPlan::Create { local, roles } => {
                if let Some(v) = &local.spec.full_name {
                    out.push(format!("    fullName: {v:?}"));
                }
                if let Some(v) = &local.spec.email {
                    out.push(format!("    email: {v:?}"));
                }
                if let Some(v) = &local.spec.comments {
                    out.push(format!("    comments: {v:?}"));
                }
                if let Some(v) = &local.spec.duty_schedule {
                    out.push(format!("    dutySchedule: {v:?}"));
                }
                if !roles.is_empty() {
                    out.push(format!("    roles: [{}]", roles.join(", ")));
                }
                if let Some(pref) = &local.spec.password_ref {
                    out.push(format!(
                        "    password: <set from passwordRef: {}>",
                        describe_password_ref(pref)
                    ));
                }
                if let Some(anno) = collapse_unmodeled(local) {
                    out.push(format!("    x-onmsctl-unmodeled: {anno}"));
                }
            }
            UserPlan::Update { form, .. } => {
                out.extend(field_change(
                    "fullName",
                    form.full_name.as_deref(),
                    server.and_then(|s| s.full_name.as_deref()),
                ));
                out.extend(field_change(
                    "email",
                    form.email.as_deref(),
                    server.and_then(|s| s.email.as_deref()),
                ));
                out.extend(field_change(
                    "comments",
                    form.comments.as_deref(),
                    server.and_then(|s| s.user_comments.as_deref()),
                ));
            }
            UserPlan::RoleAdd { role, .. } => out.push(format!("    + role {role}")),
            UserPlan::RoleRemove { role, .. } => out.push(format!("    - role {role}")),
        }
    }
    out
}

/// Combined plan-phase summary (task 9.5): counts by action type across all
/// users. `skipped` is the per-user plan failures (PR-IAM-005, GET errors).
pub fn plan_summary(report: &ApplyReport) -> String {
    let (mut create, mut update, mut role_delta, mut unchanged) = (0, 0, 0, 0);
    for u in &report.users {
        for p in &u.planned {
            match p {
                UserPlan::Create { .. } => create += 1,
                UserPlan::Update { .. } => update += 1,
                UserPlan::RoleAdd { .. } | UserPlan::RoleRemove { .. } => role_delta += 1,
                UserPlan::Unchanged { .. } => unchanged += 1,
            }
        }
    }
    let skipped = report
        .users
        .iter()
        .filter(|u| u.result == UserResult::PlanFailed)
        .count();
    format!(
        "plan: {create} create, {update} update, {role_delta} role-delta, \
         {unchanged} unchanged, {skipped} skipped"
    )
}

/// Emit a human-readable summary of an apply run to stderr. Findings and
/// per-user failures always print; the per-user diff prints when
/// `show_actions` is set (`--dry-run` / `--diff`) or for a FAILED user. Ends
/// with the plan-phase summary (9.5) and, for a real apply, the execute-phase
/// result line. Structured (`-o json|yaml`) apply output is a follow-up.
pub fn render_apply_report(report: &ApplyReport, show_actions: bool) {
    // Pre-apply server snapshot, keyed by username, for old→new on updates.
    let server: BTreeMap<&str, &OnmsUserWire> = report
        .server_users
        .iter()
        .map(|u| (u.user_id.as_str(), u))
        .collect();

    for f in &report.input_findings {
        eprintln!("{}", describe_finding(f));
    }
    for user in &report.users {
        for w in &user.warnings {
            eprintln!("{}", describe_finding(w));
        }
        for e in &user.errors {
            eprintln!("{}", describe_finding(e));
        }
        // Show the per-user diff under --dry-run/--diff, and always for a
        // FAILED user so the operator can see what was being attempted even on
        // a plain apply.
        if show_actions || user.result == UserResult::Failed {
            for line in render_user_apply_diff(user, server.get(user.name.as_str()).copied()) {
                eprintln!("{line}");
            }
        }
        if let Some(msg) = &user.failure {
            eprintln!("  {}: FAILED — {msg}", user.name);
        }
    }

    // Plan-phase summary (task 9.5).
    eprintln!("{}", plan_summary(report));
    // Execute-phase outcome (only meaningful once Phase 2 ran). DryRun and
    // AbortedInput never reach execution, so the tally would be a misleading
    // row of zeros.
    if matches!(
        report.state,
        crate::apply::multi::ApplyState::Completed | crate::apply::multi::ApplyState::StoppedEarly
    ) {
        let count = |r: UserResult| report.users.iter().filter(|u| u.result == r).count();
        eprintln!(
            "result: {} applied, {} failed, {} plan-failed",
            count(UserResult::Applied),
            count(UserResult::Failed),
            count(UserResult::PlanFailed),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(name: &str, full: Option<&str>, email: Option<&str>, roles: &[&str]) -> OnmsUserWire {
        OnmsUserWire {
            user_id: name.into(),
            full_name: full.map(str::to_owned),
            email: email.map(str::to_owned),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn user_row_projection_hides_password() {
        let mut u = wire("alice", Some("Alice"), Some("a@x.io"), &["ROLE_USER"]);
        u.password = Some("HASH".into());
        let row = UserRow::from(&u);
        assert_eq!(row.username, "alice");
        assert_eq!(row.roles, "ROLE_USER");
        // The serialized row carries no password field.
        let json = serde_json::to_string(&row).unwrap();
        assert!(!json.contains("HASH"));
        assert!(!json.to_lowercase().contains("password"));
    }

    #[test]
    fn describe_finding_includes_code_and_severity() {
        let f = Finding {
            code: "PR-IAM-006",
            severity: Severity::Warning,
            message: "unknown role".into(),
        };
        let s = describe_finding(&f);
        assert!(s.contains("PR-IAM-006"));
        assert!(s.contains("warning"));
    }

    // ---- Group 9 diff rendering ----

    use crate::apply::multi::ApplyState;
    use crate::model::local::{
        ApiVersion, FromEnvRef, KindUser, Metadata, PasswordRef, UserLocal, UserSpec,
    };
    use crate::model::wire::UpdateForm;

    fn outcome(name: &str, planned: Vec<UserPlan>, result: UserResult) -> UserOutcome {
        UserOutcome {
            name: name.into(),
            planned,
            warnings: vec![],
            errors: vec![],
            result,
            failure: None,
        }
    }

    fn create_local(name: &str, roles: &[&str], pw: bool) -> Box<UserLocal> {
        Box::new(UserLocal {
            api_version: ApiVersion,
            kind: KindUser,
            metadata: Metadata {
                name: name.into(),
                unmodeled: None,
            },
            spec: UserSpec {
                full_name: Some("Alice Example".into()),
                email: Some("alice@example.com".into()),
                comments: None,
                duty_schedule: None,
                roles: roles.iter().map(|s| s.to_string()).collect(),
                password_ref: pw.then(|| {
                    PasswordRef::FromEnv(FromEnvRef {
                        from_env: "ALICE_PW".into(),
                    })
                }),
            },
        })
    }

    #[test]
    fn create_diff_shows_fields_roles_and_password_source_only() {
        let local = create_local("alice", &["ROLE_USER"], true);
        let oc = outcome(
            "alice",
            vec![UserPlan::Create {
                local,
                roles: vec!["ROLE_USER".into()],
            }],
            UserResult::Planned,
        );
        let lines = render_user_apply_diff(&oc, None).join("\n");
        assert!(lines.contains("+ alice (create)"), "{lines}");
        assert!(lines.contains("fullName: \"Alice Example\""));
        assert!(lines.contains("roles: [ROLE_USER]"));
        // passwordRef renders the SOURCE, never the secret value.
        assert!(lines.contains("password: <set from passwordRef: fromEnv:ALICE_PW>"));
        assert!(!lines.contains("ROLE_ADMIN"));
    }

    #[test]
    fn create_diff_collapses_unmodeled_annotation() {
        let mut local = create_local("alice", &[], true);
        let mut m = serde_norway::Mapping::new();
        m.insert("a".into(), 1.into());
        m.insert("b".into(), 2.into());
        local.metadata.unmodeled = Some(m);
        let oc = outcome(
            "alice",
            vec![UserPlan::Create {
                local,
                roles: vec![],
            }],
            UserResult::Planned,
        );
        let lines = render_user_apply_diff(&oc, None).join("\n");
        assert!(
            lines.contains("x-onmsctl-unmodeled: <2 entries>"),
            "{lines}"
        );
    }

    #[test]
    fn update_diff_shows_old_to_new() {
        let form = UpdateForm {
            full_name: Some("New Name".into()),
            email: None,
            comments: None,
        };
        let oc = outcome(
            "alice",
            vec![UserPlan::Update {
                name: "alice".into(),
                form,
            }],
            UserResult::Applied,
        );
        let server = wire("alice", Some("Old Name"), None, &["ROLE_USER"]);
        let lines = render_user_apply_diff(&oc, Some(&server)).join("\n");
        assert!(lines.contains("~ alice"), "{lines}");
        assert!(
            lines.contains("fullName: \"Old Name\" -> \"New Name\""),
            "{lines}"
        );
        // email/comments unchanged → not shown.
        assert!(!lines.contains("email:"));
    }

    #[test]
    fn role_delta_diff_shapes() {
        let oc = outcome(
            "alice",
            vec![
                UserPlan::RoleAdd {
                    name: "alice".into(),
                    role: "ROLE_REST".into(),
                },
                UserPlan::RoleRemove {
                    name: "alice".into(),
                    role: "ROLE_ADMIN".into(),
                },
            ],
            UserResult::Applied,
        );
        let lines = render_user_apply_diff(&oc, None).join("\n");
        assert!(lines.contains("    + role ROLE_REST"), "{lines}");
        assert!(lines.contains("    - role ROLE_ADMIN"), "{lines}");
    }

    #[test]
    fn unchanged_diff_marks_equals() {
        let oc = outcome(
            "alice",
            vec![UserPlan::Unchanged {
                name: "alice".into(),
            }],
            UserResult::Unchanged,
        );
        let lines = render_user_apply_diff(&oc, None).join("\n");
        assert_eq!(lines, "= alice (unchanged)");
    }

    #[test]
    fn plan_summary_counts_by_action_type() {
        let report = ApplyReport {
            state: ApplyState::DryRun,
            input_findings: vec![],
            users: vec![
                outcome(
                    "alice",
                    vec![UserPlan::Create {
                        local: create_local("alice", &[], true),
                        roles: vec![],
                    }],
                    UserResult::Planned,
                ),
                outcome(
                    "bob",
                    vec![
                        UserPlan::Update {
                            name: "bob".into(),
                            form: UpdateForm::default(),
                        },
                        UserPlan::RoleAdd {
                            name: "bob".into(),
                            role: "ROLE_REST".into(),
                        },
                    ],
                    UserResult::Planned,
                ),
                outcome(
                    "carol",
                    vec![UserPlan::Unchanged {
                        name: "carol".into(),
                    }],
                    UserResult::Unchanged,
                ),
                outcome("dave", vec![], UserResult::PlanFailed),
            ],
            server_users: vec![],
        };
        let s = plan_summary(&report);
        assert_eq!(
            s,
            "plan: 1 create, 1 update, 1 role-delta, 1 unchanged, 1 skipped"
        );
    }

    #[test]
    fn skipped_diff_marks_bang_for_empty_plan() {
        // A PlanFailed user carries no atomic plans; under --dry-run/--diff it
        // must render as skipped, not as a phantom "~" modify.
        let oc = outcome("dave", vec![], UserResult::PlanFailed);
        let lines = render_user_apply_diff(&oc, None).join("\n");
        assert_eq!(lines, "! dave (skipped)");
    }

    // IAM-001 / IAM-002 are hard-stop `onmsctl_core::Error` variants, not
    // collectable `Finding`s — they render via the binary's error handler
    // (their `Display`), which leads with the `IAM-00N:` code and carries the
    // override guidance inline (tasks 9.4 + 9.6).
    #[test]
    fn iam_001_lockout_error_renders_code_and_guidance() {
        let s = onmsctl_core::Error::IamLockout {
            roles: "ROLE_ADMIN".into(),
        }
        .to_string();
        assert!(s.starts_with("IAM-001:"), "{s}");
        assert!(s.contains("ROLE_ADMIN"), "{s}");
        assert!(s.contains("--allow-admin-lockout --yes"), "{s}");
    }

    #[test]
    fn iam_002_self_lockout_error_renders_code_and_user() {
        let s = onmsctl_core::Error::IamSelfLockout {
            user: "alice".into(),
        }
        .to_string();
        assert!(s.starts_with("IAM-002:"), "{s}");
        assert!(s.contains("'alice'"), "{s}");
        assert!(s.contains("--context"), "{s}");
    }
}
