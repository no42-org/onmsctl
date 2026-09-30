/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Thresholding capability for `onmsctl`.
//!
//! Models Horizon's threshold groups (`/api/v2/thresholding`) and threshd
//! packages (`/api/v2/threshd`), added by NMS-19837, as the declarative kinds
//! `ThresholdGroup` and `ThreshdPackage`. See the `add-threshold-kinds`
//! OpenSpec change for the design.
//!
//! - [`api`]: REST client and the version gate.
//! - [`apply`]: the two kind handlers.
//! - [`cmd`]: the `onmsctl threshold` subcommands.
//! - [`dto`]: wire-format DTOs.
//! - [`model`]: the local YAML model and its validation.
//! - [`convert`]: local and wire conversions, canonical comparison, export.
//! - [`diff`]: `--diff` rendering.

pub mod api;
pub mod apply;
pub mod cmd;
pub mod convert;
pub mod diff;
pub mod dto;
pub mod model;

pub use cmd::ThresholdCmd;

/// Capability name surfaced by the binary's `version` subcommand.
pub const CAPABILITY_NAME: &str = "thresholding";

/// Capability crate version (mirrors the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
