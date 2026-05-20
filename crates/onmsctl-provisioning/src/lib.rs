/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Provisioning capability for `onmsctl`.
//!
//! **Scaffold stage.** This crate is a registered workspace member with
//! the module skeleton in place, but exposes no public API yet — the
//! `model`, `apply`, `convert`, `diff`, and `schema` modules are empty
//! placeholders. Only `cmd::RequisitionCmd` exists, as a clap subcommand
//! placeholder that returns "not implemented" if dispatched. Real
//! capability surface lands in subsequent tasks of the
//! `add-provisioning-capability` openspec change.

pub mod apply;
pub mod cmd;
pub mod convert;
pub mod diff;
pub mod model;
pub mod schema;

/// Capability crate version. Surfaced by the binary's `version` subcommand
/// so `onmsctl version` can list each linked capability and its release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Capability name as printed by `onmsctl version`.
pub const CAPABILITY_NAME: &str = "provisioning";

pub use cmd::RequisitionCmd;
