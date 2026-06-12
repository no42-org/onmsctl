/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! SNMP configuration capability for `onmsctl`.
//!
//! Models OpenNMS's `/rest/v2/snmp-config` (the `snmp-config.xml` singleton:
//! `defaults` + `definitions` + `profiles`) as a declarative
//! `kind: SnmpConfig` document, reconciled by whole-config replace. See the
//! `add-snmp-config-capability` OpenSpec change for the design.
//!
//! The crate carries the wire-format DTOs ([`server`]), the local model
//! ([`model`]) with client-side secret references ([`secret`]), the
//! conversions ([`convert`]) and secret-free idempotency diff ([`diff`]), and
//! the [`apply`] kind-handler that reconciles a `kind: SnmpConfig` document via
//! whole-config replace over [`api`]. The `export` / `lookup` verbs land in a
//! subsequent increment.

pub mod api;
pub mod apply;
pub mod convert;
pub mod diff;
pub mod model;
pub mod secret;
pub mod server;

/// Capability name surfaced by the binary's `version` subcommand.
pub const CAPABILITY_NAME: &str = "snmp";

/// Capability crate version (mirrors the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
