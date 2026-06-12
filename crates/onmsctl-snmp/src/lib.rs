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
//! This first increment carries the **wire-format DTOs** ([`server`]) mirrored
//! from the v2 JAXB/Jackson types. The local model, conversions, apply handler,
//! and the `export` / `lookup` verbs land in subsequent increments.

pub mod model;
pub mod secret;
pub mod server;

/// Capability name surfaced by the binary's `version` subcommand.
pub const CAPABILITY_NAME: &str = "snmp";

/// Capability crate version (mirrors the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
