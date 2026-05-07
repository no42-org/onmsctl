/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! EventConf capability for `onmsctl`.
//!
//! Phase 3 commit 1 — wire-format DTOs and the typed `EventConfApi<'_>`
//! wrapper around Horizon's `/eventconf/*` REST surface (16 endpoints +
//! `find_source_by_name` lookup helper). YAML ↔ XML conversion lands in
//! Phase 3 commit 2; `apply -f` integration lands in Phase 5 per the
//! OpenSpec change `init-onmsctl-event-conf`.

pub mod api;
pub mod dto;
pub mod xml;

pub use api::{
    CreatedEvent, CreatedSource, EventConfApi, EventFilter, EventInSourceFilter, SourceFilter,
    SourceLookup, UploadFileError, UploadFileResult, UploadResult,
};
pub use dto::{
    AddEventConfSourceRequest, AlarmData, Autoacknowledge, Correlation,
    EnableDisableConfSourceEventsPayload, Event, EventConfEventDeletePayload, EventConfEventDto,
    EventConfEventEditRequest, EventConfSourceDeletePayload, EventConfSourceDto,
    EventConfSrcEnableDisablePayload, Logmsg, Mask, MaskElement, MaskVarbind, Page, Parm,
    ParmValue, Severity, Snmp, SourceNameAndId, Tticket,
};
