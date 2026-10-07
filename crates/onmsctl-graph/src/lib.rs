/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! GraphML topology capability for `onmsctl`.
//!
//! Models one GraphML upload as a declarative, named, multi-instance
//! `kind: Graph` document: a container label plus ordered `layers`, each a
//! GraphML `<graph>` with its namespace, vertices and edges. See the
//! `add-graph-capability` OpenSpec change for the design (G1–G14, D1–D12).
//!
//! Writes go to `rest/graphml/{name}` (the v2 Graph API is read-only). There is
//! no PUT, so an update is DELETE then POST, rolled back from the stored XML
//! read during plan if the POST fails. Plan parses that stored XML back into
//! the same model and compares structs. Reads, views and searches use the v2
//! `api/v2/graphs` API.
//!
//! The crate carries the local model ([`model`]), offline validation
//! ([`validate`]), the GraphML writer and parser ([`graphml`]), the REST
//! wrappers ([`api`]), the [`apply`] kind-handler, and the read/write verbs
//! ([`cmd`]).

pub mod api;
pub mod apply;
pub mod cmd;
pub mod graphml;
pub mod model;
pub mod validate;

pub use cmd::GraphCmd;

/// Capability name surfaced by the binary's `version` subcommand.
pub const CAPABILITY_NAME: &str = "graph";

/// Capability crate version (mirrors the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
