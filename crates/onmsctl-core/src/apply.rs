/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Opaque rendered-diff type for capability apply paths.
//!
//! [`Diff`] is produced by capability-specific diff algorithms (e.g. EventConf's
//! UEI-bucketed diff, `design.md §5.3`) and consumed by the kind-router handlers
//! and the `--diff` rendering as opaque display text. It intentionally
//! prescribes no structured shape: the algorithm lives where the data is
//! understood, and this type only carries the rendered result.

use std::fmt;

/// Opaque rendered diff. Capability impls construct one of these from their
/// domain-specific diff algorithm; consumers only know how to ask whether it is
/// empty and how to print it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff(String);

impl Diff {
    pub fn empty() -> Self {
        Self(String::new())
    }
    pub fn from_text(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Diff {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_empty_check() {
        assert!(Diff::empty().is_empty());
        assert!(!Diff::from_text("changed").is_empty());
    }
}
