/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! DTOs and YAML model for IAM.
//!
//! - `local` — the typed shape operators write under `kind: User`.
//!
//! The wire DTO (`OnmsUserWire`, `OnmsUserListWire`) and convert helpers
//! between local and wire shapes land in Group 3's wire half (tasks 3.1
//! and 3.6) once spike 0.1 verifies the upstream POST body format.

pub mod local;

pub use local::{
    API_VERSION, ApiVersion, FromEnvRef, FromFileRef, FromKeyringRef, KIND_USER, KNOWN_ROLES,
    KeyringRef, KindUser, Metadata, PasswordRef, UserLocal, UserSpec,
};
