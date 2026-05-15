/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! EventConf capability for `onmsctl`.
//!
//! Public surface:
//!
//! - [`api::EventConfApi`] — typed wrapper around Horizon's `/eventconf/*`
//!   REST surface (16 endpoints + `find_source_by_name` lookup).
//! - [`dto::*`] — wire-format DTOs (sources, events, payloads, the unified
//!   Event type with nested Mask/Logmsg/AlarmData/etc.).
//! - [`xml::*`] — JSON Event ↔ eventconf XML conversion plus the
//!   master-file synthesis and stable canonicalization functions.
//! - [`cmd::*`] — clap subcommand definitions for `onmsctl source ...`
//!   and `onmsctl event ...`.
//! - `TableRow` impls for the public DTOs so the binary's `-o table`
//!   path renders them via comfy-table.

pub mod api;
pub mod apply;
pub mod cmd;
pub mod dto;
mod render;
pub mod xml;

/// Capability crate version. Surfaced by the binary's `version` subcommand
/// so `onmsctl version` can list each linked capability and its release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Capability name as printed by `onmsctl version`.
pub const CAPABILITY_NAME: &str = "eventconf";

pub use api::{
    CreatedSource, EventConfApi, EventFilter, EventInSourceFilter, SourceFilter, SourceLookup,
    UploadFileError, UploadFileResult, UploadResult,
};
pub use cmd::{EventCmd, SourceCmd};
pub use dto::{
    AddEventConfSourceRequest, AlarmData, Autoacknowledge, Correlation,
    EnableDisableConfSourceEventsPayload, Event, EventConfEventDeletePayload, EventConfEventDto,
    EventConfEventEditRequest, EventConfSourceDeletePayload, EventConfSourceDto,
    EventConfSrcEnableDisablePayload, Logmsg, Mask, MaskElement, MaskVarbind, Page, Parm,
    ParmValue, Severity, Snmp, SourceNameAndId, Tticket,
};
