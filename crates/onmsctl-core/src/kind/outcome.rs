/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Structured per-document result model for the kind-router.
//!
//! Every document a handler processes produces one [`ApplyOutcome`]. The same
//! shape is emitted by both the plan and execute phases so `--dry-run` and a
//! real run are diffable. Outcomes render through the existing
//! `-o table|yaml|json` path: [`ApplyOutcome`] is `Serialize` (YAML/JSON) and
//! implements [`TableRow`] (table).

use std::fmt;

use serde::Serialize;

use crate::render::TableRow;

/// What a reconcile intends (in plan) or performed (in execute) for a
/// document. Distinct from [`OutcomeStatus`]: `action` is the verb,
/// `status` is the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Create,
    Update,
    Delete,
    /// No mutation needed — the resource already matches the document.
    None,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Action::Create => "create",
            Action::Update => "update",
            Action::Delete => "delete",
            Action::None => "none",
        })
    }
}

/// Stable, machine-readable result of reconciling one document. The vocabulary
/// is closed — handlers MUST NOT invent free-text statuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum OutcomeStatus {
    Created,
    Updated,
    Unchanged,
    Deleted,
    Failed,
    Skipped,
}

impl OutcomeStatus {
    /// True only for [`OutcomeStatus::Failed`]. Drives the router's
    /// stop-on-error decision and the binary's exit-code mapping.
    pub fn is_failure(&self) -> bool {
        matches!(self, OutcomeStatus::Failed)
    }
}

impl fmt::Display for OutcomeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OutcomeStatus::Created => "Created",
            OutcomeStatus::Updated => "Updated",
            OutcomeStatus::Unchanged => "Unchanged",
            OutcomeStatus::Deleted => "Deleted",
            OutcomeStatus::Failed => "Failed",
            OutcomeStatus::Skipped => "Skipped",
        })
    }
}

/// One document's outcome. `name` is the document's `metadata.name`;
/// `remediation` is populated only on `Failed`; `details` stays absent unless a
/// handler has a concrete need.
#[derive(Clone, Debug, Serialize)]
pub struct ApplyOutcome {
    pub kind: String,
    pub name: String,
    pub action: Action,
    pub status: OutcomeStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApplyOutcome {
    /// Build an outcome with no remediation or details.
    pub fn new(
        kind: impl Into<String>,
        name: impl Into<String>,
        action: Action,
        status: OutcomeStatus,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            name: name.into(),
            action,
            status,
            message: message.into(),
            remediation: None,
            details: None,
        }
    }

    /// A `Failed` outcome carrying a remediation hint.
    pub fn failed(
        kind: impl Into<String>,
        name: impl Into<String>,
        action: Action,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            remediation: Some(remediation.into()),
            ..Self::new(kind, name, action, OutcomeStatus::Failed, message)
        }
    }

    /// A `Skipped` (not-attempted) outcome, used when a prior failure halts a
    /// stop-on-error run before this document was reached.
    pub fn skipped(
        kind: impl Into<String>,
        name: impl Into<String>,
        action: Action,
        message: impl Into<String>,
    ) -> Self {
        Self::new(kind, name, action, OutcomeStatus::Skipped, message)
    }

    /// The standard `--dry-run` preview for a planned action: a true no-op is
    /// `Unchanged`, any other action is `Skipped` with a "would …" message (the
    /// predicted verb stays in `action` for diffing against a real run). The
    /// canonical Decision-2 behaviour, shared by all handlers; a handler may
    /// build a different preview (e.g. a `Failed` row for a plan error).
    pub fn would(kind: impl Into<String>, name: impl Into<String>, action: Action) -> Self {
        match action {
            Action::None => Self::new(kind, name, action, OutcomeStatus::Unchanged, "in sync"),
            other => Self::new(
                kind,
                name,
                action,
                OutcomeStatus::Skipped,
                format!("dry-run: would {other}"),
            ),
        }
    }
}

impl TableRow for ApplyOutcome {
    fn headers() -> Vec<&'static str> {
        vec!["kind", "name", "action", "status", "message"]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.kind.clone(),
            self.name.clone(),
            self.action.to_string(),
            self.status.to_string(),
            self.message.clone(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::OutputFormat;
    use crate::render::render_list;

    #[test]
    fn status_failure_predicate() {
        assert!(OutcomeStatus::Failed.is_failure());
        assert!(!OutcomeStatus::Created.is_failure());
        assert!(!OutcomeStatus::Unchanged.is_failure());
    }

    #[test]
    fn remediation_only_serialized_when_present() {
        let ok = ApplyOutcome::new("User", "alice", Action::Create, OutcomeStatus::Created, "ok");
        let json = serde_json::to_string(&ok).unwrap();
        assert!(!json.contains("remediation"));

        let bad = ApplyOutcome::failed("User", "bob", Action::Update, "boom", "retry");
        let json = serde_json::to_string(&bad).unwrap();
        assert!(json.contains("remediation"));
        assert!(json.contains("retry"));
    }

    #[test]
    fn renders_through_the_shared_table_path() {
        let outcomes = vec![
            ApplyOutcome::new("User", "alice", Action::Create, OutcomeStatus::Created, "created"),
            ApplyOutcome::new(
                "Requisition",
                "acme",
                Action::None,
                OutcomeStatus::Unchanged,
                "in sync",
            ),
        ];
        let table = render_list(&outcomes, OutputFormat::Table).unwrap();
        assert!(table.contains("Created"));
        assert!(table.contains("Unchanged"));
        let json = render_list(&outcomes, OutputFormat::Json).unwrap();
        assert!(json.contains("\"status\": \"Created\""));
    }
}
