/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The kind registry (Decision B).
//!
//! Maps `kind → (precedence rank, handler)`. The type lives in core; it is
//! *populated* by the `onmsctl` binary, the only crate that depends on every
//! capability. Core therefore holds the registry shape without ever
//! referencing a capability type.

use std::collections::HashMap;

use super::handler::KindHandler;

/// A populated set of kind handlers with their precedence ranks.
#[derive(Default)]
pub struct Registry {
    handlers: HashMap<&'static str, (u32, Box<dyn KindHandler>)>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `handler` at precedence `rank`, keyed by the handler's own
    /// `kind()` (Pattern 2 — no string literals at the call site). A second
    /// registration for the same kind replaces the first.
    pub fn register(&mut self, rank: u32, handler: Box<dyn KindHandler>) {
        let kind = handler.kind();
        debug_assert!(
            !self.handlers.contains_key(kind),
            "duplicate KindHandler registration for kind {kind:?} \
             (handler and rank would be silently replaced) — wiring bug"
        );
        self.handlers.insert(kind, (rank, handler));
    }

    /// The handler for `kind`, if registered.
    pub fn handler(&self, kind: &str) -> Option<&dyn KindHandler> {
        self.handlers.get(kind).map(|(_, h)| h.as_ref())
    }

    /// The precedence rank for `kind`, if registered.
    pub fn rank(&self, kind: &str) -> Option<u32> {
        self.handlers.get(kind).map(|(r, _)| *r)
    }

    /// Whether a handler is registered for `kind`.
    pub fn contains(&self, kind: &str) -> bool {
        self.handlers.contains_key(kind)
    }

    /// All registered kinds (unordered).
    pub fn known_kinds(&self) -> Vec<&'static str> {
        self.handlers.keys().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}
