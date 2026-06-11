/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `apply -f` for `EventSource`.
//!
//! Submodules:
//!   - [`local`] — the user-authored YAML schema (`EventSourceLocal`)
//!     and validation.
//!   - [`conversion`] — `EventSourceLocal` → wire-format `Event` DTO.
//!   - [`from_wire`] — wire-format `Event` → `EventDef` (the inverse).
//!     Drives the `event-source convert` migration path.
//!   - [`diff`] — UEI-bucketed structured diff between local and
//!     server-state shapes (see `design.md §5.3`).
//!   - [`target`] — the server-state model + reconcile seams
//!     (`fetch_remote` / `diff_source` / `upload_then_optionally_disable`)
//!     that [`handler::EventSourceHandler`] drives.

pub mod conversion;
pub mod diff;
pub mod from_wire;
pub mod handler;
pub mod local;
pub mod target;

pub use from_wire::WireToLocalError;
pub use handler::EventSourceHandler;
pub use target::EventSourceRemote;

pub use local::{
    AlarmDataDef, AutoackDef, CorrelationDef, DecodeDef, EventDef, EventSourceLocal,
    EventSourceSpec, LogmsgDef, MaskDef, MaskElementDef, MaskVarbindDef, Metadata, TticketDef,
    VarbindsdecodeDef,
};
