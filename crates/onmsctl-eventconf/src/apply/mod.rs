/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `apply -f` for `EventSource`.
//!
//! Phase 5 commit 1 — local YAML schema, validation, and the
//! Local→wire-Event conversion. Diff algorithm and `ApplyTarget`
//! impl land in commits 2 and 3 per the OpenSpec change
//! `init-onmsctl-event-conf`.

pub mod conversion;
pub mod diff;
pub mod local;
pub mod target;

pub use target::{EventSourceRemote, EventSourceTarget};

pub use local::{
    AlarmDataDef, AutoackDef, CorrelationDef, EventDef, EventSourceLocal, EventSourceSpec,
    LogmsgDef, MaskDef, MaskElementDef, MaskVarbindDef, Metadata, TticketDef,
};
