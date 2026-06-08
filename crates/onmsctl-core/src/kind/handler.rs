/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `KindHandler` contract capability crates implement (Decision A/D4).
//!
//! The trait is object-safe — no associated types, no generic methods — so the
//! registry can hold `Box<dyn KindHandler>` (Decision B). Dispatch is **per
//! kind-bucket**: the router groups all documents of a kind and hands the whole
//! slice to one `plan`/`execute` pair. This is required because several leaf
//! invariants are cross-document *within a kind* (e.g. IAM admin-lockout and
//! duplicate-name checks span every `User` document at once). The per-kind
//! reconciliation logic lives in the capability crate; the handler is a thin
//! adapter. `plan()` is read-only; `execute()` performs writes.

use std::any::Any;

use async_trait::async_trait;

use crate::context::Context;
use crate::error::Result;

use super::envelope::RawDoc;
use super::outcome::{Action, ApplyOutcome};

/// A router-visible summary of one document's planned reconciliation. The
/// router uses these to render `--dry-run` previews and to report
/// not-attempted documents after a stop-on-error halt — without consulting the
/// opaque payload. One per document in the bucket.
#[derive(Clone, Debug)]
pub struct PlanItem {
    pub kind: String,
    pub name: String,
    pub action: Action,
}

impl PlanItem {
    pub fn new(kind: impl Into<String>, name: impl Into<String>, action: Action) -> Self {
        Self {
            kind: kind.into(),
            name: name.into(),
            action,
        }
    }
}

/// The product of a handler's read-only plan phase for one kind-bucket.
/// Carries the router-visible per-document `items` (for dry-run + not-attempted
/// reporting), an optional rendered `diff` for `--diff`, and an opaque
/// `payload` the same handler downcasts in `execute`. Keeping the payload
/// `Box<dyn Any>` is what lets `KindHandler` stay object-safe.
pub struct Plan {
    pub items: Vec<PlanItem>,
    /// Pre-rendered diff for `--diff`, when the handler produced one.
    pub diff: Option<String>,
    /// Handler-private execution payload, downcast in `execute`.
    pub payload: Box<dyn Any + Send>,
}

impl Plan {
    /// A plan carrying per-document summaries and an execution payload.
    pub fn new(items: Vec<PlanItem>, payload: Box<dyn Any + Send>) -> Self {
        Self {
            items,
            diff: None,
            payload,
        }
    }

    /// Attach a rendered diff (builder style).
    pub fn with_diff(mut self, diff: Option<String>) -> Self {
        self.diff = diff;
        self
    }
}

/// A capability's adapter into the kind-router. One implementation per `kind`;
/// each call receives the whole bucket of documents of that kind.
#[async_trait]
pub trait KindHandler: Send + Sync {
    /// The `kind` discriminator this handler serves (e.g. `"Requisition"`).
    /// MUST be the capability's exported `KIND` constant, never a literal at
    /// the registration site.
    fn kind(&self) -> &'static str;

    /// Read-only plan for an entire bucket of documents of this kind:
    /// deserialize and validate them, run any cross-document invariants, fetch
    /// live state, and decide per-document actions. MUST NOT mutate server
    /// state. An error here aborts the whole apply at the router gate.
    async fn plan(&self, docs: &[RawDoc], ctx: &Context) -> Result<Plan>;

    /// Execute the planned writes for the bucket, returning one
    /// [`ApplyOutcome`] per document/resource. Per-document logical failures
    /// SHOULD be returned as outcomes with `Failed` status (the leaf may
    /// continue past them under its own `--keep-going`); `Err` is reserved for
    /// an unrecoverable transport fault affecting the whole bucket.
    async fn execute(&self, plan: Plan, ctx: &Context) -> Result<Vec<ApplyOutcome>>;
}
