/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! IAM capability for `onmsctl`.
//!
//! **Scaffold only.** All modules are empty placeholders; the user/role
//! surface (model, api, apply, cmd, render) lands incrementally through
//! the remaining groups of the `add-iam-capability` openspec change.

pub mod api;
pub mod apply;
pub mod cmd;
pub mod model;
pub mod render;

pub use cmd::IamCmd;
