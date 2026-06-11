/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Compile-time subcommand classification.
//!
//! Each capability's command enum implements [`Classify`] so the binary
//! can decide whether a given invocation is a read or a write **before
//! dispatching**. The primary consumer is the `--read-only` context
//! attribute (cli-core spec): in a read-only context, any [`CmdKind::Write`]
//! invocation is refused locally before any HTTP call is issued.
//!
//! Classification is defined by whether the variant would normally cause
//! an HTTP mutation against the server. A command that only writes to
//! local files (e.g. `event-source convert`, `event-source download`) is
//! [`CmdKind::Read`]; only commands that POST / PUT / PATCH / DELETE
//! against the server are [`CmdKind::Write`]. Runtime flags such as
//! `--dry-run` do not change the classification — the variant's *capability*
//! to write is what matters, not whether a given invocation exercises it.

/// Whether a subcommand variant can mutate server state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CmdKind {
    /// Only issues HTTP GETs (or no HTTP at all). Safe under `--read-only`.
    Read,
    /// May issue HTTP POST / PUT / PATCH / DELETE. Refused under
    /// `--read-only`.
    Write,
}

/// Trait implemented by every capability's top-level command enum so the
/// binary can classify dispatched invocations.
pub trait Classify {
    /// Return the kind for the variant currently held by this enum value.
    fn kind(&self) -> CmdKind;
}
