/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Provisioning capability for `onmsctl`.
//!
//! **Partial.** The composite `kind: Requisition` data model (module
//! `model`) and the JSON Schema annotation table (module `schema`) are
//! live; the binary is not yet wired into this capability and the
//! `apply`, `convert`, `diff` modules remain stubs. `cmd::RequisitionCmd`
//! is a clap subcommand placeholder that returns "not implemented" if
//! dispatched. Remaining capability surface lands in subsequent tasks
//! of the `add-provisioning-capability` openspec change.

pub mod api;
pub mod apply;
pub mod cmd;
pub mod convert;
pub mod diff;
pub mod model;
pub mod render;
pub mod schema;
pub mod wait;

/// Capability crate version. Surfaced by the binary's `version` subcommand
/// so `onmsctl version` can list each linked capability and its release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Capability name as printed by `onmsctl version`.
pub const CAPABILITY_NAME: &str = "provisioning";

pub use cmd::RequisitionCmd;
