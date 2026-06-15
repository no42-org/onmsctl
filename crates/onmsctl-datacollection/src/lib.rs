/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! SNMP data-collection capability for `onmsctl`.
//!
//! Models OpenNMS's database-backed data-collection config (the v2
//! `DataCollectionConfRestService`) as a declarative, named, multi-instance
//! `kind: DataCollectionSource` document — one per `datacollection-group`. The
//! `spec` carries the group tree (`resourceTypes` / `groups` / `systemDefs`),
//! the snmp-collection `profiles` that include the source, and an optional
//! inline `profileSpec` to author/tune a profile from zero (the "C+" model). See
//! the `2026-06-15-add-datacollection-capability` OpenSpec change for the design.
//!
//! Per-source, additive-prune reconcile: a source is replaced as a whole unit
//! (multipart XML upload), its `profiles` associations are ensured (idempotent),
//! and the optional `profileSpec` is created/updated. The DB-backed endpoint is
//! `develop`-only (absent from released Horizon ≤ 37.0.0), so apply preflights
//! the endpoint and fails the whole apply early when it is unavailable.
//!
//! NOTE: the local model ([`model`]) and read-side wire DTOs ([`server`]) are
//! implemented and verified against a live `37.0.0-SNAPSHOT` capture (OpenSpec
//! task 1). The remaining wire-facing layers (convert/api/apply/cmd) build on
//! these shapes.

pub mod api;
pub mod apply;
pub mod cmd;
pub mod convert;
pub mod model;
pub mod server;

pub use apply::DataCollectionSourceHandler;
pub use cmd::DatacollectionCmd;

/// Capability name surfaced by the binary's `version` subcommand.
pub const CAPABILITY_NAME: &str = "datacollection";

/// Capability crate version (mirrors the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
