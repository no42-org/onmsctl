/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! CLI subcommand surface for the EventConf capability.
//!
//! Exposed to the binary crate as [`SourceCmd`] (and in commit 2 of Phase 4,
//! `EventCmd`). The binary composes them into its top-level command tree.

pub mod source;

pub use source::SourceCmd;
