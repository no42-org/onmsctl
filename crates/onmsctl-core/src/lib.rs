/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Shared infrastructure for `onmsctl` capabilities.
//!
//! Phase 2 — foundation: error types, authentication, configuration loader,
//! context resolution, output-format selection, HTTP client, output rendering,
//! and the declarative kind-router (`kind::`).
//!
//! See `openspec/changes/init-onmsctl-event-conf/design.md` for the full
//! architecture and `cli-core` spec deltas for observable requirements.

pub mod apply;
pub mod apply_input;
pub mod async_flags;
pub mod auth;
pub mod client;
pub mod cmd;
pub mod config;
pub mod context;
pub mod error;
pub mod format;
pub mod kind;
pub mod render;

pub use apply::Diff;
pub use kind::{
    Action, ApplyOutcome, ApplyParams, KindHandler, OutcomeStatus, Plan, RawDoc, Registry,
    apply_documents, parse_documents,
};
pub use async_flags::{AsyncFlags, parse_duration};
pub use auth::AuthCreds;
pub use client::OnmsClient;
pub use cmd::{Classify, CmdKind};
pub use context::{Context, Overrides};
pub use error::{Error, Result};
pub use format::OutputFormat;
pub use render::{TableRow, render_list, render_one};

/// Re-export of `reqwest::Url`. Capabilities use this to construct
/// `OnmsClient::from_parts(...)` in tests without taking a direct dep on
/// `reqwest`.
pub use reqwest::Url;
