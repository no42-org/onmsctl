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
//! - [`cmd::*`] — clap subcommand definitions for the `onmsctl source ...`
//!   tree (Phase 4 commit 1) and `onmsctl event ...` (Phase 4 commit 2).
//! - `TableRow` impls for the public DTOs so the binary's `-o table`
//!   path renders them via comfy-table.

pub mod api;
pub mod cmd;
pub mod dto;
mod render;
pub mod xml;

pub use api::{
    CreatedEvent, CreatedSource, EventConfApi, EventFilter, EventInSourceFilter, SourceFilter,
    SourceLookup, UploadFileError, UploadFileResult, UploadResult,
};
pub use cmd::SourceCmd;
pub use dto::{
    AddEventConfSourceRequest, AlarmData, Autoacknowledge, Correlation,
    EnableDisableConfSourceEventsPayload, Event, EventConfEventDeletePayload, EventConfEventDto,
    EventConfEventEditRequest, EventConfSourceDeletePayload, EventConfSourceDto,
    EventConfSrcEnableDisablePayload, Logmsg, Mask, MaskElement, MaskVarbind, Page, Parm,
    ParmValue, Severity, Snmp, SourceNameAndId, Tticket,
};
