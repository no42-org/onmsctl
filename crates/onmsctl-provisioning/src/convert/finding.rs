/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Structured findings emitted by the XML→YAML migrator.
//!
//! Codes are namespaced under `PR` (provisioning), allocated
//! contiguously from `PR001`. The catalog is intentionally small to
//! start (PR001–PR004 land with tasks 8.1–8.4); additional codes
//! follow as fixture work in 8.6–8.7 surfaces new cases.

use serde::Serialize;
use std::path::PathBuf;

/// Stable code identifying a finding's class. The numeric portion is
/// allocated contiguously from `PR001` upward; reserved range is
/// `PR001`–`PR099`. The variants are listed in numeric order to make
/// the catalog easy to audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FindingCode {
    /// **PR001** — input XML contains an element or attribute from
    /// the migrator's catalog of known-unmodeled fields (`@location`,
    /// `@city`, `@status`, `@descr`, `<meta-data>`). Today the catalog
    /// is hand-rolled in `pipeline::flag_unmodeled`; truly-unknown
    /// elements added in future Horizon releases are dropped silently
    /// until the catalog is updated (proper forward-compat detection
    /// — parse into `serde_json::Value` and walk against the DTO
    /// field names — is deferred).
    Pr001,
    /// **PR002** — foreign-source XML was provided that doesn't
    /// match any requisition (orphaned). Operator likely intended
    /// to migrate it alongside the requisition; flag so they can
    /// rename or remove it.
    Pr002,
    /// **PR003** — policy `<parameter>` declares a type that
    /// doesn't match the policy class's expected parameter shape.
    /// Won't block conversion (we preserve the value verbatim) but
    /// signals likely misconfiguration the operator should review.
    Pr003,
    /// **PR004** — requisition XML present with no matching
    /// foreign-source XML. Conversion omits `spec.foreignSource`
    /// from the emitted YAML; on `apply`, Horizon's default-FS
    /// will be inherited (per design D1). Informational, not a
    /// blocker — operators running portable-style YAML want exactly
    /// this behavior.
    Pr004,
    /// **PR005** — `<interface>` declared an `@snmp-primary` value
    /// other than `P` / `S` / `N`. The migrator drops the attribute
    /// from the emitted YAML (the local model rejects unknown
    /// variants at parse-time) and continues. Operator should
    /// inspect the source XML — most likely a typo.
    Pr005,
}

impl FindingCode {
    /// Canonical string form for printing and CLI matching:
    /// `"PR001"`, `"PR002"`, etc.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pr001 => "PR001",
            Self::Pr002 => "PR002",
            Self::Pr003 => "PR003",
            Self::Pr004 => "PR004",
            Self::Pr005 => "PR005",
        }
    }

    /// Parse a CLI code string (case-insensitive `"pr001"` /
    /// `"PR001"`) back into a [`FindingCode`]. Used by the
    /// `--explain <code>` flag on the convert verb.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "PR001" => Some(Self::Pr001),
            "PR002" => Some(Self::Pr002),
            "PR003" => Some(Self::Pr003),
            "PR004" => Some(Self::Pr004),
            "PR005" => Some(Self::Pr005),
            _ => None,
        }
    }

    /// All allocated codes, in catalog order. Useful for `--explain`
    /// without an argument (list all known codes) and for tests
    /// that want to assert exhaustive coverage.
    pub const fn all() -> &'static [FindingCode] {
        &[
            Self::Pr001,
            Self::Pr002,
            Self::Pr003,
            Self::Pr004,
            Self::Pr005,
        ]
    }

    /// Default severity for the code. Most informational findings
    /// are `Warning`; the [`Severity::Info`] tier exists for
    /// transparency-of-behavior notes (PR004) that don't suggest
    /// the operator change anything.
    pub fn default_severity(self) -> Severity {
        match self {
            Self::Pr001 => Severity::Warning,
            Self::Pr002 => Severity::Warning,
            Self::Pr003 => Severity::Warning,
            Self::Pr004 => Severity::Info,
            Self::Pr005 => Severity::Warning,
        }
    }
}

/// Severity bucket for a finding. Drives the `convert` verb's exit
/// code (matches eventconf's design D4): `0` = no findings or info-
/// only, `1` = warnings present (YAML still emitted), `2` = errors
/// present (YAML withheld).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// A single structured finding emitted during conversion.
#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub code: FindingCode,
    pub severity: Severity,
    /// Free-form context — the element name, attribute name, or
    /// path that triggered the finding. Kept short so a stderr
    /// stream of findings stays scannable.
    pub message: String,
    /// Input file the finding came from, when known. `None` for
    /// findings discovered before any specific file was opened
    /// (e.g. directory-level orphan checks).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<PathBuf>,
}

impl Finding {
    /// Build a finding using the code's default severity.
    pub fn new(code: FindingCode, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: code.default_severity(),
            message: message.into(),
            source_path: None,
        }
    }

    /// Attach the source path to a finding. Pre-existing fields
    /// are preserved.
    pub fn with_source(mut self, path: PathBuf) -> Self {
        self.source_path = Some(path);
        self
    }
}

/// `convert --explain <code>` rationale for each catalog entry.
/// Returns one paragraph of guidance per code; never panics.
pub fn explain(code: FindingCode) -> &'static str {
    match code {
        FindingCode::Pr001 => "\
PR001 — Unmodeled XML element or attribute.

The migrator encountered an XML element or attribute it doesn't model. \
The input was preserved as best it could be (unknown elements may be \
dropped from the YAML output), but the YAML is no longer a lossless \
round-trip of the source XML.

Common causes: a newer Horizon schema addition the CLI doesn't know \
about; a deprecated XML element still present in legacy files; a \
typo in the source XML.

What to do: inspect the named element/attribute in the source file. \
If it's load-bearing, file an issue against onmsctl with the example.",

        FindingCode::Pr002 => "\
PR002 — Orphan foreign-source XML.

A foreign-source XML file was supplied (or auto-discovered in \
`--foreign-sources-dir`) whose name does not match any requisition \
XML being converted. Without a matching requisition the foreign-source \
has no apply target.

What to do: rename the foreign-source XML to match its requisition's \
`foreign-source` attribute, OR delete the orphan if it's no longer in \
use, OR pass the matching requisition XML alongside.",

        FindingCode::Pr003 => "\
PR003 — Policy parameter type mismatch.

A `<parameter>` under a `<policy>` declares a type that doesn't match \
the policy class's expected parameter shape. The value is preserved \
verbatim in the YAML output (no data loss), but the apply may behave \
differently than the operator expects.

What to do: cross-check the policy class's documentation for the \
expected parameter key/value shape. Either correct the source XML \
before conversion, or correct the YAML after conversion and re-apply.",

        FindingCode::Pr005 => "\
PR005 — Unrecognized snmp-primary value.

An `<interface>` declared an `@snmp-primary` attribute whose value \
isn't one of `P` (primary), `S` (secondary), or `N` (not eligible). \
The migrator drops the attribute from the emitted YAML — the local \
model rejects unknown variants at parse time, so passing the value \
through would only fail the apply step downstream.

What to do: inspect the source XML for the named interface. Most \
likely a typo. If the source value was intentional, file an issue \
against onmsctl with the example so we can extend the catalog.",

        FindingCode::Pr004 => "\
PR004 — No foreign-source XML for this requisition.

A requisition XML was converted with no matching foreign-source XML \
in scope. The emitted YAML omits `spec.foreignSource`, so an `apply` \
against Horizon will inherit Horizon's default foreign-source (no \
custom detectors, no custom policies).

This is INFORMATIONAL — it's exactly what operators using the \
\"portable\" YAML style want. Flag is emitted so operators who DID \
have a foreign-source XML and forgot to include it notice the omission.

What to do: nothing if intentional. Otherwise, locate the matching \
foreign-source XML and re-run convert with `--foreign-sources-dir`.",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_matches_variant_number() {
        assert_eq!(FindingCode::Pr001.as_str(), "PR001");
        assert_eq!(FindingCode::Pr002.as_str(), "PR002");
        assert_eq!(FindingCode::Pr003.as_str(), "PR003");
        assert_eq!(FindingCode::Pr004.as_str(), "PR004");
    }

    #[test]
    fn parse_round_trips_through_as_str() {
        for code in FindingCode::all() {
            assert_eq!(FindingCode::parse(code.as_str()), Some(*code));
        }
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(FindingCode::parse("pr001"), Some(FindingCode::Pr001));
        assert_eq!(FindingCode::parse("Pr002"), Some(FindingCode::Pr002));
    }

    #[test]
    fn parse_rejects_unknown_codes() {
        assert_eq!(FindingCode::parse("PR999"), None);
        assert_eq!(FindingCode::parse(""), None);
        assert_eq!(FindingCode::parse("EC001"), None); // eventconf code
    }

    #[test]
    fn all_catalog_codes_have_non_empty_explain() {
        for code in FindingCode::all() {
            let text = explain(*code);
            assert!(
                text.starts_with(code.as_str()),
                "explain({code:?}) does not start with its code"
            );
            assert!(text.len() > 80, "explain({code:?}) is suspiciously short");
        }
    }

    #[test]
    fn pr004_is_info_not_warning() {
        // PR004 is informational by design — it fires on the
        // portable-YAML happy path.
        assert_eq!(FindingCode::Pr004.default_severity(), Severity::Info);
    }

    #[test]
    fn finding_new_attaches_default_severity() {
        let f = Finding::new(FindingCode::Pr001, "unknown element <foo>");
        assert_eq!(f.severity, Severity::Warning);
        assert!(f.source_path.is_none());
    }

    #[test]
    fn finding_with_source_attaches_path() {
        let f = Finding::new(FindingCode::Pr001, "x").with_source("/tmp/r.xml".into());
        assert_eq!(f.source_path.as_deref(), Some(std::path::Path::new("/tmp/r.xml")));
    }
}
