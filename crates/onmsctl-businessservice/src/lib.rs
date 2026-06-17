/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Business Service Monitoring (BSM) capability for `onmsctl`.
//!
//! Models an OpenNMS Business Service (the v2 `/api/v2/business-services`
//! service) as a declarative, named, multi-instance `kind: BusinessService`
//! document: a per-service `reduceFunction`, optional `attributes`, and four
//! edge collections — `childServices`, `ipServices`, `applications`,
//! `reductionKeys` — each edge with a `weight` and optional per-edge
//! `mapFunction`. See the `add-business-service-capability` OpenSpec change for
//! the design (DD1–DD9, DD3a/DD3b).
//!
//! Apply is a **whole-object** reconcile: the BSM `PUT` is a destructive
//! full-replace of all edges, so a service's desired request DTO is its
//! authoritative state. `execute` runs **two passes** — POST a minimal body to
//! obtain each new service's id, then PUT the complete body with resolved
//! `child-id`s — which dissolves child-reference ordering. A single bsmd
//! `daemon/reload` runs after any mutating apply. References are by name and
//! resolved to numeric ids at apply time (BSM is ID-centric); a reduction-key
//! edge may template `{{nodeId}}` from a node reference.
//!
//! The crate carries the wire DTOs ([`server`]), the local model ([`model`]),
//! the local→wire conversion + diff ([`convert`]), the REST wrapper ([`api`]),
//! the [`apply`] kind-handler, and the read/Write verbs ([`cmd`]).

pub mod api;
pub mod apply;
pub mod cmd;
pub mod convert;
pub mod model;
pub mod server;

pub use cmd::BusinessServiceCmd;

/// Capability name surfaced by the binary's `version` subcommand.
pub const CAPABILITY_NAME: &str = "business-service";

/// Capability crate version (mirrors the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
