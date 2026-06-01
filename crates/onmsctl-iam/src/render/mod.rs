/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output rendering for the IAM CLI.
//!
//! This module holds the table projection for `iam user list` / `get` and a
//! human-readable summary of an [`ApplyReport`]. The full per-user `--diff`
//! renderer (design §D / tasks 9.1–9.6) builds on these and lands in Group 9.

use onmsctl_core::{OutputFormat, TableRow};
use serde::Serialize;

use crate::apply::multi::{ApplyReport, UserResult};
use crate::apply::{Finding, Severity, UserPlan};
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

/// One-line human description of a planned atomic action, for the apply
/// summary and `--diff` preview.
pub fn describe_plan(plan: &UserPlan) -> String {
    match plan {
        UserPlan::Unchanged { name } => format!("  {name}: unchanged"),
        UserPlan::Create { local, roles } => format!(
            "  {}: create (roles: [{}])",
            local.metadata.name,
            roles.join(", ")
        ),
        UserPlan::Update { name, form } => {
            let mut fields = Vec::new();
            if let Some(v) = &form.full_name {
                fields.push(format!("fullName=\"{v}\""));
            }
            if let Some(v) = &form.email {
                fields.push(format!("email=\"{v}\""));
            }
            if let Some(v) = &form.comments {
                fields.push(format!("comments=\"{v}\""));
            }
            format!("  {name}: update ({})", fields.join(", "))
        }
        UserPlan::RoleAdd { name, role } => format!("  {name}: + role {role}"),
        UserPlan::RoleRemove { name, role } => format!("  {name}: - role {role}"),
    }
}

/// Render a structured finding to a single stderr line, e.g.
/// `[PR-IAM-006] warning: user "alice": role ...`.
pub fn describe_finding(f: &Finding) -> String {
    let sev = match f.severity {
        Severity::Warning => "warning",
        Severity::Error => "error",
    };
    format!("[{}] {}: {}", f.code, sev, f.message)
}

/// Emit a human-readable summary of an apply run to stderr (per-user actions
/// and findings) plus a one-line tally. Structured (`-o json|yaml`) apply
/// output is a Group 9 follow-up; `format` is accepted now so the signature
/// is stable.
pub fn render_apply_report(report: &ApplyReport, _format: OutputFormat) {
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
        for plan in &user.planned {
            eprintln!("{}", describe_plan(plan));
        }
        if let Some(msg) = &user.failure {
            eprintln!("  {}: FAILED — {msg}", user.name);
        }
    }

    let count = |r: UserResult| report.users.iter().filter(|u| u.result == r).count();
    eprintln!(
        "summary: {} applied, {} unchanged, {} planned, {} failed, {} plan-failed",
        count(UserResult::Applied),
        count(UserResult::Unchanged),
        count(UserResult::Planned),
        count(UserResult::Failed),
        count(UserResult::PlanFailed),
    );
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
    fn describe_plan_shapes() {
        assert!(
            describe_plan(&UserPlan::RoleAdd {
                name: "alice".into(),
                role: "ROLE_REST".into()
            })
            .contains("+ role ROLE_REST")
        );
        assert!(describe_plan(&UserPlan::Unchanged { name: "bob".into() }).contains("unchanged"));
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
}
