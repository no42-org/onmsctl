/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The kind-router: a document scheduler for declarative `onmsctl apply -f`.
//!
//! A single top-level `apply` peeks each YAML document's `kind`
//! ([`envelope`]), looks it up in a core-owned [`registry::Registry`] mapping
//! `kind → (rank, handler)`, orders all documents by a static precedence table
//! ([`precedence`]), and delegates each to its [`handler::KindHandler`] via a
//! plan → gate → execute flow ([`router`]). The router owns scheduling only;
//! all reconciliation lives in the capability handlers (INV1). Each document's
//! result is an [`outcome::ApplyOutcome`].

pub mod envelope;
pub mod handler;
pub mod outcome;
pub mod precedence;
pub mod registry;
pub mod router;

pub use envelope::{RawDoc, load_documents, parse_documents};
pub use handler::{KindHandler, Plan};
pub use outcome::{Action, ApplyOutcome, OutcomeStatus};
pub use precedence::{KNOWN_RANKS, default_rank};
pub use registry::Registry;
pub use router::{ApplyParams, apply_documents};
