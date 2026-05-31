/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! DTOs and YAML model for IAM.
//!
//! - `local` — the typed shape operators write under `kind: User`.
//! - `wire` — the server wire DTOs: JSON responses ([`wire::OnmsUserWire`],
//!   [`wire::OnmsUserListWire`]), the form-encoded update body
//!   ([`wire::UpdateForm`]), and the XML create-body builder
//!   ([`wire::user_create_xml`]). Spike 0.1 (2026-05-29) verified the
//!   per-verb format split documented there.
//! - `convert` — server → local conversion ([`convert::wire_to_local`]).

pub mod convert;
pub mod local;
pub mod wire;

pub use local::{
    API_VERSION, ApiVersion, FromEnvRef, FromFileRef, FromKeyringRef, KIND_USER, KNOWN_ROLES,
    KeyringRef, KindUser, Metadata, PasswordRef, UserLocal, UserSpec,
};
pub use wire::{OnmsUserListWire, OnmsUserWire, UpdateForm, user_create_xml};
