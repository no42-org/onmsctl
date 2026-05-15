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
//!   - [`diff`] — UEI-bucketed structured diff between local and
//!     server-state shapes (see `design.md §5.3`).
//!   - [`target`] — the [`onmsctl_core::ApplyTarget`] impl that wires
//!     fetch / create / update / diff for the EventConf capability.

pub mod conversion;
pub mod diff;
pub mod local;
pub mod target;

pub use target::{EventSourceRemote, EventSourceTarget};

pub use local::{
    AlarmDataDef, AutoackDef, CorrelationDef, EventDef, EventSourceLocal, EventSourceSpec,
    LogmsgDef, MaskDef, MaskElementDef, MaskVarbindDef, Metadata, TticketDef,
};
