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
use super::outcome::ApplyOutcome;

/// Knobs for an apply run, mapped from the `apply` CLI flags and threaded to
/// each handler. A handler needs `dry_run` to decide whether to enforce
/// real-apply-only invariants (e.g. IAM admin-lockout, which is deliberately
/// not gated under `--dry-run`), and `continue_on_error` to control its own
/// intra-bucket per-item failure handling.
#[derive(Clone, Copy, Debug, Default)]
pub struct ApplyParams {
    /// Stop after the plan phase; issue no mutating HTTP.
    pub dry_run: bool,
    /// Print each bucket's rendered diff to stderr before reconciling.
    pub show_diff: bool,
    /// Attempt every item/bucket instead of halting after the first failure.
    pub continue_on_error: bool,
}

/// The product of a handler's read-only plan phase for one kind-bucket.
/// Carries the router-visible per-document `preview` outcomes (used verbatim
/// for `--dry-run`, and as the source of `kind`/`name`/`action` for
/// not-attempted reporting after a stop-on-error halt), an optional rendered
/// `diff` for `--diff`, and an opaque `payload` the same handler downcasts in
/// `execute`. The handler owns its preview semantics — typically
/// [`ApplyOutcome::would`] per document, but a plan-failed document may preview
/// as `Failed`. Keeping the payload `Box<dyn Any>` is what lets `KindHandler`
/// stay object-safe.
pub struct Plan {
    pub preview: Vec<ApplyOutcome>,
    /// Pre-rendered diff for `--diff`, when the handler produced one.
    pub diff: Option<String>,
    /// Handler-private execution payload, downcast in `execute`.
    pub payload: Box<dyn Any + Send>,
}

impl Plan {
    /// A plan carrying per-document preview outcomes and an execution payload.
    pub fn new(preview: Vec<ApplyOutcome>, payload: Box<dyn Any + Send>) -> Self {
        Self {
            preview,
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
    /// state. An error here aborts the whole apply at the router gate — so
    /// gate-class refusals that carry a dedicated exit code (e.g. IAM
    /// admin-lockout) belong here, not in `execute`. Real-apply-only invariants
    /// SHOULD be skipped when `params.dry_run` is set.
    async fn plan(&self, docs: &[RawDoc], params: &ApplyParams, ctx: &Context) -> Result<Plan>;

    /// Execute the planned writes for the bucket, returning one
    /// [`ApplyOutcome`] per document/resource. Per-document logical failures
    /// SHOULD be returned as outcomes with `Failed` status (the leaf may
    /// continue past them under `params.continue_on_error`); `Err` is reserved
    /// for an unrecoverable transport fault affecting the whole bucket.
    async fn execute(
        &self,
        plan: Plan,
        params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>>;
}
