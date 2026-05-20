/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! CLI subcommand surface for the Provisioning capability.
//!
//! Exposed to the binary crate as [`RequisitionCmd`]; the binary composes
//! it into the top-level command tree at `onmsctl requisition`.
//!
//! At this scaffold stage only the enum skeleton exists — every variant
//! returns "not yet implemented" at runtime so the binary compiles and
//! `--help` is reachable. Behavior is added by subsequent tasks in the
//! `add-provisioning-capability` change.

use clap::Subcommand;
use onmsctl_core::{Context, Error, Result};

/// `onmsctl requisition ...` subcommands.
///
/// Three grouped families (per design.md §D8):
///
/// - **GitOps**: `apply`, `convert`, `export`
/// - **Lifecycle**: `list`, `get`, `delete`, `import`, `status`
/// - **Sub-resources**: `node`, `interface`, `service`, `category`, `asset`
///
/// At this scaffold stage only the surface placeholder exists; behavior
/// lands in later tasks. The single `Placeholder` variant keeps clap
/// happy until real verbs land.
#[derive(Subcommand, Debug, Clone)]
pub enum RequisitionCmd {
    /// Scaffold placeholder. Real verbs land in subsequent tasks of the
    /// `add-provisioning-capability` change.
    #[command(hide = true)]
    Placeholder,
}

impl RequisitionCmd {
    /// Dispatch entry point invoked by the binary crate's match arm
    /// once the capability is registered. At this scaffold stage the
    /// crate is not wired into the binary — this method exists so
    /// future verb implementations can extend the match below without
    /// re-introducing the signature.
    pub async fn run(self, _ctx: &Context) -> Result<()> {
        Err(Error::Config(
            "onmsctl-provisioning: scaffold only; no verbs implemented yet".into(),
        ))
    }
}
