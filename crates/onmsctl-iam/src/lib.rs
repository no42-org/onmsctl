/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! IAM capability for `onmsctl` — users + roles via Horizon's v1
//! `UserRestService`, declarative `iam apply -f`, lockout protection, and
//! `passwordRef` secret resolution.

pub mod api;
pub mod apply;
pub mod cmd;
pub mod model;
pub mod render;
pub mod schema;
pub mod secret;

pub use cmd::IamCmd;
pub use secret::{SecretString, resolve_password_ref};

/// Capability crate version, surfaced by `onmsctl version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Capability name, surfaced by `onmsctl version`.
pub const CAPABILITY_NAME: &str = "iam";
