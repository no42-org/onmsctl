/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Maintenance-window capability for `onmsctl`.
//!
//! Models OpenNMS scheduled outages (`poll-outages.xml`, the v1
//! `/rest/sched-outages` service) as a declarative, named, multi-instance
//! `kind: Maintenance` document: a `schedule` (when), `devices` (who), and
//! `suppress` (which daemons stop — polling / thresholds / collection /
//! notifications) for a planned window. See the
//! `2026-06-14-add-maintenance-capability` OpenSpec change for the design.
//!
//! Apply is a **composite** reconcile: the outage *definition* is created/updated
//! (readable → true diff), then *attached* to each declared daemon/package
//! (ensure-present, because the attachment set is not readable from this
//! service). The crate carries the wire DTOs ([`server`]), the local model
//! ([`model`]), the conversions ([`convert`]) and the definition diff ([`diff`]),
//! the REST wrapper ([`api`]), the [`apply`] kind-handler, and the read/Write
//! verbs ([`cmd`]).

pub mod api;
pub mod apply;
pub mod cmd;
pub mod convert;
pub mod diff;
pub mod model;
pub mod server;

pub use cmd::MaintenanceCmd;

/// Capability name surfaced by the binary's `version` subcommand.
pub const CAPABILITY_NAME: &str = "maintenance";

/// Capability crate version (mirrors the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
