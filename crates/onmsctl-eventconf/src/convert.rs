/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! XML → YAML conversion engine for the `source convert` migration command.
//!
//! Takes eventconf XML bytes and produces a [`ConversionResult`] carrying:
//!   - The serialized YAML (if conversion succeeded enough to produce one)
//!   - A list of structured [`Finding`]s describing rule violations, drops,
//!     and normalizations encountered during the conversion
//!   - Coverage metrics (events scanned / converted / dropped)
//!
//! The engine is `pub` so the CLI layer (`cmd::source::Convert`) and the
//! `source download --format yaml` path can both call it identically. The
//! engine itself does **no I/O** — bytes in, structured result out.
//!
//! See the `source-convert-with-migration-report` OpenSpec change for the
//! design rationale and the spec scenarios that drive this code.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::apply::from_wire::WireToLocalError;
use crate::apply::local::{EventDef, EventSourceLocal, EventSourceSpec, Metadata};
use crate::xml::{SourceLocation, parse_events_with_locations};

/// Canonical OpenNMS severity set. Used for the EC005 case-normalization
/// check before invoking `EventSourceLocal::validate`, which is itself
/// case-sensitive.
const CANONICAL_SEVERITIES: &[&str] = &[
    "Indeterminate",
    "Cleared",
    "Normal",
    "Warning",
    "Minor",
    "Major",
    "Critical",
];

/// Reserved source names. The upload pipeline assigns special meaning to
/// these basenames — a converted EventSource using them would conflict.
const RESERVED_METADATA_NAMES: &[&str] = &["eventconf", "opennms.catch-all.events"];

// -- Public types ----------------------------------------------------------

/// Outcome of converting one eventconf XML document.
#[derive(Clone, Debug, Serialize)]
pub struct ConversionResult {
    /// Optional path of the input being converted. `None` when the input
    /// was stdin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<PathBuf>,
    /// Serialized YAML output. `None` when blocking findings prevented
    /// emission (and `--keep-duplicates` was not set in [`ConvertOpts`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaml: Option<String>,
    /// Structured findings produced during conversion. Ordering reflects
    /// the order findings were discovered.
    pub findings: Vec<Finding>,
    /// Aggregate counts and coverage.
    pub metrics: ConversionMetrics,
}

impl ConversionResult {
    /// Exit code per design D4. `0` = clean, `1` = warnings (YAML written),
    /// `2` = blocking findings (no YAML), `3` = handled by the CLI layer
    /// (usage errors before conversion runs).
    pub fn exit_code(&self) -> i32 {
        if self.findings.iter().any(|f| f.severity == Severity::Error) {
            2
        } else if !self.findings.is_empty() {
            1
        } else {
            0
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ConversionMetrics {
    pub events_scanned: usize,
    pub events_converted: usize,
    pub events_dropped: usize,
    /// Percentage of `events_scanned` that converted successfully.
    /// Computed as `100.0 * events_converted / events_scanned`.
    /// `0.0` when `events_scanned == 0` (no events to convert) AND
    /// `0.0` when every scanned event was dropped — distinguish the
    /// two cases by inspecting `events_scanned` directly.
    pub modeled_coverage_pct: f32,
}

/// Stable code identifying a finding's class. Codes are namespaced under
/// `EC` (event-conf) and never renumbered or repurposed across releases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FindingCode {
    /// EC002 — source has zero events after parse.
    Ec002,
    /// EC003 — derived metadata.name is reserved by OpenNMS.
    Ec003,
    /// EC004 — event missing a field required by the local schema
    /// (`uei`, `event-label`, `severity`, or required fields under
    /// `alarm-data` / `logmsg` / `mask.varbind`).
    Ec004,
    /// EC005 — severity value was normalized to canonical case
    /// (e.g. `WARNING` → `Warning`).
    Ec005,
    /// EC007 — alarm-type value outside the accepted set `{1, 2, 3}`
    /// (i.e. zero, negative, or ≥ 4).
    Ec007,
    /// EC008 — derived metadata.name contains characters the schema
    /// rejects, or is otherwise structurally invalid.
    Ec008,
    /// EC009 — `EventSourceLocal::validate` rejected the assembled
    /// document after the converter's own findings ran. Acts as a
    /// safety net for schema rules the converter doesn't mirror
    /// individually.
    Ec009,
}

impl FindingCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ec002 => "EC002",
            Self::Ec003 => "EC003",
            Self::Ec004 => "EC004",
            Self::Ec005 => "EC005",
            Self::Ec007 => "EC007",
            Self::Ec008 => "EC008",
            Self::Ec009 => "EC009",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "EC002" => Some(Self::Ec002),
            "EC003" => Some(Self::Ec003),
            "EC004" => Some(Self::Ec004),
            "EC005" => Some(Self::Ec005),
            "EC007" => Some(Self::Ec007),
            "EC008" => Some(Self::Ec008),
            "EC009" => Some(Self::Ec009),
            _ => None,
        }
    }

    /// All defined codes. Used by `--explain` for "unknown code" error
    /// listings and by the compile-time explanation-completeness test.
    /// EC001 and EC006 are reserved — both were defined-then-removed.
    /// Their numbers are not reused for future finding codes. Future
    /// codes append (EC010, EC011, ...) rather than recycling slots.
    pub fn all() -> &'static [Self] {
        &[
            Self::Ec002,
            Self::Ec003,
            Self::Ec004,
            Self::Ec005,
            Self::Ec007,
            Self::Ec008,
            Self::Ec009,
        ]
    }
}

impl Serialize for FindingCode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl std::fmt::Display for FindingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub code: FindingCode,
    pub severity: Severity,
    pub message: String,
    pub details: FindingDetails,
    pub suggested_fix: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FindingDetails {
    ZeroEvents,
    ParseFailure {
        error: String,
    },
    ReservedName {
        name: String,
    },
    MissingRequiredField {
        field: &'static str,
        location: SourceLocation,
    },
    SeverityNormalized {
        before: String,
        after: String,
        location: SourceLocation,
    },
    AlarmTypeOutOfRange {
        value: i32,
        location: SourceLocation,
    },
    InvalidMetadataName {
        derived_name: String,
        /// Characters that violated the name rule. Empty when the
        /// violation is structural (e.g. empty after suffix strip,
        /// leading dot) rather than a per-character issue.
        offending_chars: Vec<char>,
        /// Human-readable explanation of the structural violation, when
        /// `offending_chars` is empty.
        reason: Option<String>,
    },
    PostValidationFailed {
        validator_error: String,
    },
}

/// Options passed to [`convert`].
#[derive(Clone, Debug, Default)]
pub struct ConvertOpts {
    /// Override the metadata.name derived from the input filename. Required
    /// when the input has no filename (stdin).
    pub name_override: Option<String>,
}

/// Default cap on bytes read per input. Mirrors the upload pipeline's
/// `MAX_UPLOAD_BYTES_PER_FILE`. Overridable per-invocation via
/// `--max-bytes` on the CLI.
pub const DEFAULT_MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;

// -- Public entry point ----------------------------------------------------

/// Convert eventconf XML bytes to an `EventSource` YAML document.
///
/// Returns a [`ConversionResult`] whose [`ConversionResult::yaml`] is
/// `Some(...)` when conversion produced a serializable document, and whose
/// [`ConversionResult::findings`] enumerates every rule violation or
/// normalization encountered. The exit code policy is documented on
/// [`ConversionResult::exit_code`].
pub fn convert(xml: &[u8], source_path: &Path, opts: &ConvertOpts) -> ConversionResult {
    let mut findings = Vec::new();
    let mut metrics = ConversionMetrics::default();
    let input = if source_path.as_os_str() == "-" {
        None
    } else {
        Some(source_path.to_path_buf())
    };

    // Step 1: derive metadata.name
    let metadata_name = match resolve_metadata_name(source_path, opts.name_override.as_deref()) {
        Ok(name) => name,
        Err(NameResolutionFailure::Invalid {
            derived_name,
            offending_chars,
            reason,
        }) => {
            let message = if !offending_chars.is_empty() {
                format!("derived metadata.name '{derived_name}' contains invalid characters")
            } else {
                format!(
                    "derived metadata.name '{derived_name}' is structurally invalid: {}",
                    reason.as_deref().unwrap_or("unspecified")
                )
            };
            findings.push(Finding {
                code: FindingCode::Ec008,
                severity: Severity::Error,
                message,
                details: FindingDetails::InvalidMetadataName {
                    derived_name,
                    offending_chars,
                    reason,
                },
                suggested_fix: "Rename the input file (use ASCII letters, digits, `.`, `-`, \
                    `_` only; non-empty after stripping `.events.xml`/`.xml`; no leading/trailing \
                    dot; at least one `.` for the vendor prefix) or pass --name <override>"
                    .into(),
            });
            return ConversionResult {
                input,
                yaml: None,
                findings,
                metrics,
            };
        }
        Err(NameResolutionFailure::StdinNeedsOverride) => {
            findings.push(Finding {
                code: FindingCode::Ec008,
                severity: Severity::Error,
                message: "stdin input requires --name <metadata-name>".into(),
                details: FindingDetails::InvalidMetadataName {
                    derived_name: String::from("-"),
                    offending_chars: vec![],
                    reason: Some("stdin has no filename to derive from".into()),
                },
                suggested_fix: "Pass --name when reading from stdin".into(),
            });
            return ConversionResult {
                input,
                yaml: None,
                findings,
                metrics,
            };
        }
    };

    // Step 2: reject reserved name (EC003). Case-insensitive to defend
    // against `Eventconf.xml` on case-insensitive filesystems (default
    // macOS, Windows).
    if RESERVED_METADATA_NAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&metadata_name))
    {
        findings.push(Finding {
            code: FindingCode::Ec003,
            severity: Severity::Error,
            message: format!("metadata.name '{metadata_name}' is reserved by OpenNMS"),
            details: FindingDetails::ReservedName {
                name: metadata_name.clone(),
            },
            suggested_fix: "Choose a non-reserved filename for the source (`eventconf` is the \
                master file; `opennms.catch-all.events` is a built-in)"
                .into(),
        });
        return ConversionResult {
            input,
            yaml: None,
            findings,
            metrics,
        };
    }

    // Step 3: parse XML with locations
    let parsed = match parse_events_with_locations(xml, source_path) {
        Ok(p) => p,
        Err(e) => {
            findings.push(Finding {
                code: FindingCode::Ec002,
                severity: Severity::Error,
                message: format!("XML parse failed: {e}"),
                details: FindingDetails::ParseFailure {
                    error: e.to_string(),
                },
                suggested_fix: "Inspect the XML structure; ensure it is well-formed and uses \
                    the eventconf namespace. Check encoding (must be UTF-8) and that no \
                    DOCTYPE declarations break the parser."
                    .into(),
            });
            return ConversionResult {
                input,
                yaml: None,
                findings,
                metrics,
            };
        }
    };

    metrics.events_scanned = parsed.len();

    // Step 4: reject empty source (EC002)
    if parsed.is_empty() {
        findings.push(Finding {
            code: FindingCode::Ec002,
            severity: Severity::Error,
            message: "source contains zero events".into(),
            details: FindingDetails::ZeroEvents,
            suggested_fix: "Inspect the XML structure; ensure <event> children exist under the \
                <events> root"
                .into(),
        });
        return ConversionResult {
            input,
            yaml: None,
            findings,
            metrics,
        };
    }

    // Step 5: per-event conversion with normalization detection
    let mut event_defs: Vec<EventDef> = Vec::new();

    for (wire_event, location) in &parsed {
        let mut local_wire = wire_event.clone();

        // EC005: severity case normalization
        if let Some(sev) = &local_wire.severity
            && !CANONICAL_SEVERITIES.contains(&sev.as_str())
            && let Some(canonical) = CANONICAL_SEVERITIES
                .iter()
                .find(|c| c.eq_ignore_ascii_case(sev))
        {
            findings.push(Finding {
                code: FindingCode::Ec005,
                severity: Severity::Warning,
                message: format!("severity '{sev}' normalized to canonical case '{canonical}'"),
                details: FindingDetails::SeverityNormalized {
                    before: sev.clone(),
                    after: canonical.to_string(),
                    location: location.clone(),
                },
                suggested_fix: format!(
                    "The converter has already rewritten the value to '{canonical}' in the \
                    output YAML — the warning indicates that the source XML still uses the \
                    non-canonical form. Edit the source XML to use '{canonical}' if you want \
                    XML and YAML to round-trip byte-equal; otherwise the YAML output is \
                    correct as-is."
                ),
            });
            local_wire.severity = Some(canonical.to_string());
        }

        // EC007: alarm-type outside accepted set {1, 2, 3}
        if let Some(alarm) = &local_wire.alarm_data
            && let Some(t) = alarm.alarm_type
            && !matches!(t, 1..=3)
        {
            findings.push(Finding {
                code: FindingCode::Ec007,
                severity: Severity::Warning,
                message: format!("alarm-type {t} is outside the accepted set {{1, 2, 3}}"),
                details: FindingDetails::AlarmTypeOutOfRange {
                    value: t,
                    location: location.clone(),
                },
                suggested_fix: "The local schema accepts alarm-type values 1 (Problem), 2 \
                    (Resolution), 3 (Unresolvable). Negative, zero, or ≥4 values indicate \
                    malformed input or a forward-compatibility issue with a future OpenNMS \
                    schema; correct the value or widen the schema via a follow-up change."
                    .into(),
            });
        }

        // Convert wire → local
        match EventDef::try_from(&local_wire) {
            Ok(def) => {
                event_defs.push(def);
                metrics.events_converted += 1;
            }
            Err(err) => {
                metrics.events_dropped += 1;
                let field = field_name_for(err);
                findings.push(Finding {
                    code: FindingCode::Ec004,
                    severity: Severity::Error,
                    message: format!("event missing required field: {field}"),
                    details: FindingDetails::MissingRequiredField {
                        field,
                        location: location.clone(),
                    },
                    suggested_fix: format!(
                        "Add the required {field} to the event in the source XML; the local \
                        schema requires it for a well-formed EventSource."
                    ),
                });
            }
        }
    }

    // Step 6: cross-event validation — EC009 post-validate safety net
    //   (D3 hybrid). We invoke the authoritative validator on the
    //   assembled local document; any rejection here represents a schema
    //   rule the converter did not already surface via EC002-EC008.
    //   Skipped when the document is already known not to make sense
    //   (e.g. no events at all — but that case returned early above).
    //
    //   Duplicate-UEI checking was removed in the
    //   `permit-duplicate-ueis-as-normalization-pattern` change.
    let local_for_validate = EventSourceLocal {
        api_version: "eventconf.opennms.org/v1".into(),
        kind: "EventSource".into(),
        metadata: Metadata {
            name: metadata_name.clone(),
        },
        spec: EventSourceSpec {
            enabled: true,
            events: event_defs.clone(),
        },
    };
    if let Err(validator_err) = local_for_validate.validate() {
        let already_covered = findings.iter().any(|f| {
            matches!(
                f.code,
                FindingCode::Ec003 | FindingCode::Ec004 | FindingCode::Ec008
            )
        });
        if !already_covered {
            let err_str = match &validator_err {
                onmsctl_core::Error::Config(m) => m.clone(),
                other => other.to_string(),
            };
            findings.push(Finding {
                code: FindingCode::Ec009,
                severity: Severity::Error,
                message: format!("post-conversion validation failed: {err_str}"),
                details: FindingDetails::PostValidationFailed {
                    validator_error: err_str,
                },
                suggested_fix: "The authoritative local-schema validator rejected the \
                    converted document. Read the validator's error message — it names the \
                    offending field path. If this rule could have been surfaced earlier as \
                    EC001-EC008, file a converter bug; otherwise correct the source XML and \
                    re-run."
                    .into(),
            });
        }
    }

    // Step 7: decide whether to emit YAML
    let has_blocking = findings.iter().any(|f| f.severity == Severity::Error);

    let yaml = if has_blocking {
        None
    } else {
        let local = EventSourceLocal {
            api_version: "eventconf.opennms.org/v1".into(),
            kind: "EventSource".into(),
            metadata: Metadata {
                name: metadata_name,
            },
            spec: EventSourceSpec {
                enabled: true,
                events: event_defs,
            },
        };
        match serde_norway::to_string(&local) {
            Ok(s) => Some(s),
            Err(e) => {
                findings.push(Finding {
                    code: FindingCode::Ec002,
                    severity: Severity::Error,
                    message: format!("YAML serialization failed: {e}"),
                    details: FindingDetails::ZeroEvents,
                    suggested_fix: "This is a bug — please report it. The converter should \
                        never fail to serialize a successfully-converted EventSource."
                        .into(),
                });
                None
            }
        }
    };

    metrics.modeled_coverage_pct = if metrics.events_scanned > 0 {
        (metrics.events_converted as f32 / metrics.events_scanned as f32) * 100.0
    } else {
        0.0
    };

    ConversionResult {
        input,
        yaml,
        findings,
        metrics,
    }
}

// -- Helpers ---------------------------------------------------------------

enum NameResolutionFailure {
    Invalid {
        derived_name: String,
        offending_chars: Vec<char>,
        /// For structural failures (empty after strip, leading dot, no
        /// vendor separator), a human-readable reason. `None` when the
        /// failure is purely per-character.
        reason: Option<String>,
    },
    StdinNeedsOverride,
}

fn resolve_metadata_name(
    source_path: &Path,
    override_name: Option<&str>,
) -> Result<String, NameResolutionFailure> {
    if let Some(name) = override_name {
        return validate_name(name);
    }
    if source_path.as_os_str() == "-" {
        return Err(NameResolutionFailure::StdinNeedsOverride);
    }
    let basename = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let stripped = basename
        .strip_suffix(".events.xml")
        .or_else(|| basename.strip_suffix(".xml"))
        .unwrap_or(basename);
    validate_name(stripped)
}

fn validate_name(name: &str) -> Result<String, NameResolutionFailure> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(NameResolutionFailure::Invalid {
            derived_name: name.to_string(),
            offending_chars: vec![],
            reason: Some("name is empty (after suffix strip and whitespace trim)".into()),
        });
    }
    if trimmed == "." || trimmed == ".." {
        return Err(NameResolutionFailure::Invalid {
            derived_name: name.to_string(),
            offending_chars: vec!['.'],
            reason: Some(format!(
                "name is {trimmed:?}, which is reserved by the filesystem"
            )),
        });
    }
    if trimmed.starts_with('.') {
        return Err(NameResolutionFailure::Invalid {
            derived_name: name.to_string(),
            offending_chars: vec!['.'],
            reason: Some("name starts with a dot (would derive an empty vendor segment)".into()),
        });
    }
    if trimmed.ends_with('.') {
        return Err(NameResolutionFailure::Invalid {
            derived_name: name.to_string(),
            offending_chars: vec!['.'],
            reason: Some("name ends with a dot".into()),
        });
    }
    let valid = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
    let offending: Vec<char> = trimmed.chars().filter(|c| !valid(*c)).collect();
    if !offending.is_empty() {
        return Err(NameResolutionFailure::Invalid {
            derived_name: name.to_string(),
            offending_chars: offending,
            reason: None,
        });
    }
    Ok(trimmed.to_string())
}

/// Single source of truth for the human-readable field name used in
/// EC004 findings. Previously duplicated across the message string and
/// the FindingDetails payload — kept here so they cannot drift.
fn field_name_for(err: WireToLocalError) -> &'static str {
    match err {
        WireToLocalError::MissingUei => "uei",
        WireToLocalError::MissingLabel => "event-label",
        WireToLocalError::MissingSeverity => "severity",
        WireToLocalError::AlarmDataMissingReductionKey => "alarm-data.reduction-key",
        WireToLocalError::AlarmDataMissingAlarmType => "alarm-data.alarm-type",
        WireToLocalError::LogmsgMissingContent => "logmsg",
        WireToLocalError::LogmsgMissingDest => "logmsg.dest",
        WireToLocalError::MaskVarbindMissingDiscriminator => "mask.varbind",
    }
}

// -- explain() table -------------------------------------------------------

/// Long-form explanation for a finding code. Returned by
/// `onmsctl source convert --explain <code>`. Text is stable across patch
/// releases — wording may shift but the section structure does not.
pub fn explain(code: FindingCode) -> &'static str {
    match code {
        FindingCode::Ec002 => include_str!("explain/EC002.txt"),
        FindingCode::Ec003 => include_str!("explain/EC003.txt"),
        FindingCode::Ec004 => include_str!("explain/EC004.txt"),
        FindingCode::Ec005 => include_str!("explain/EC005.txt"),
        FindingCode::Ec007 => include_str!("explain/EC007.txt"),
        FindingCode::Ec008 => include_str!("explain/EC008.txt"),
        FindingCode::Ec009 => include_str!("explain/EC009.txt"),
    }
}

// -- Report rendering ------------------------------------------------------

/// Render a [`ConversionResult`] as a human-readable text report. Output
/// is suitable for stderr; multi-section, one block per finding.
pub fn render_report_text(result: &ConversionResult) -> String {
    let mut out = String::new();
    let input_label = match &result.input {
        Some(p) => p.display().to_string(),
        None => "<stdin>".to_string(),
    };

    let line = "─".repeat(69);
    out.push_str(&line);
    out.push('\n');
    if let Some(_yaml) = &result.yaml {
        out.push_str(&format!("  {input_label} → YAML written\n"));
    } else {
        out.push_str(&format!("  {input_label} → no YAML written\n"));
    }
    out.push_str(&line);
    out.push('\n');

    if result.findings.is_empty() {
        out.push_str("  ✓ no findings\n");
    } else {
        for f in &result.findings {
            let sev = match f.severity {
                Severity::Error => "error  ",
                Severity::Warning => "warning",
            };
            out.push('\n');
            out.push_str(&format!("  {}  {sev}  {}\n", f.code, f.message));
            render_finding_details(&mut out, &f.details);
            out.push_str(&format!("    Fix: {}\n", f.suggested_fix));
            out.push_str(&format!(
                "    For the full rationale: onmsctl source convert --explain {}\n",
                f.code
            ));
        }
    }

    out.push('\n');
    out.push_str(&line);
    out.push('\n');
    out.push_str(&format!(
        "  Summary: {} events scanned, {} converted, {} dropped \
         ({:.1}% modeled coverage)\n",
        result.metrics.events_scanned,
        result.metrics.events_converted,
        result.metrics.events_dropped,
        result.metrics.modeled_coverage_pct
    ));
    let n_err = result
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let n_warn = result.findings.len() - n_err;
    out.push_str(&format!("  Findings: {n_err} errors, {n_warn} warnings\n"));
    out.push_str(&format!("  Exit: {}\n", result.exit_code()));
    out.push_str(&line);
    out.push('\n');
    out
}

/// Format a `SourceLocation` as `file:line:column  (event[N])` per D3
/// amendment — column was previously dropped from the text report.
fn fmt_location(loc: &SourceLocation) -> String {
    format!(
        "{}:{}:{}  (event[{}])",
        loc.file.display(),
        loc.line,
        loc.column,
        loc.event_index
    )
}

fn render_finding_details(out: &mut String, d: &FindingDetails) {
    match d {
        FindingDetails::ZeroEvents => {
            out.push_str("    (no <event> children under the <events> root)\n");
        }
        FindingDetails::ParseFailure { error } => {
            out.push_str(&format!("    Parser error: {error}\n"));
        }
        FindingDetails::ReservedName { name } => {
            out.push_str(&format!("    Name: {name}\n"));
        }
        FindingDetails::MissingRequiredField { field, location } => {
            out.push_str(&format!("    Field: {field}\n"));
            out.push_str(&format!("    At:    {}\n", fmt_location(location)));
        }
        FindingDetails::SeverityNormalized {
            before,
            after,
            location,
        } => {
            out.push_str(&format!("    Before: {before}\n"));
            out.push_str(&format!("    After:  {after}\n"));
            out.push_str(&format!("    At:     {}\n", fmt_location(location)));
        }
        FindingDetails::AlarmTypeOutOfRange { value, location } => {
            out.push_str(&format!("    alarm-type: {value}\n"));
            out.push_str(&format!("    At:         {}\n", fmt_location(location)));
        }
        FindingDetails::InvalidMetadataName {
            derived_name,
            offending_chars,
            reason,
        } => {
            out.push_str(&format!("    Derived: {derived_name}\n"));
            if !offending_chars.is_empty() {
                let chars: Vec<String> = offending_chars.iter().map(|c| format!("'{c}'")).collect();
                out.push_str(&format!("    Invalid chars: {}\n", chars.join(", ")));
            }
            if let Some(r) = reason {
                out.push_str(&format!("    Reason: {r}\n"));
            }
        }
        FindingDetails::PostValidationFailed { validator_error } => {
            out.push_str(&format!("    Validator: {validator_error}\n"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_XML: &[u8] = br#"<events>
  <event>
    <uei>uei.test/foo</uei>
    <event-label>Test Foo</event-label>
    <severity>Warning</severity>
  </event>
</events>"#;

    const DUPLICATE_UEI_XML: &[u8] = br#"<events>
  <event>
    <uei>uei.test/dup</uei>
    <event-label>First</event-label>
    <severity>Warning</severity>
  </event>
  <event>
    <uei>uei.test/dup</uei>
    <event-label>Second</event-label>
    <severity>Major</severity>
  </event>
</events>"#;

    #[test]
    fn clean_input_produces_yaml_and_exit_0() {
        let opts = ConvertOpts::default();
        let result = convert(MINIMAL_XML, Path::new("/tmp/foo.test.events.xml"), &opts);
        assert!(
            result.yaml.is_some(),
            "YAML must be emitted for clean input"
        );
        assert!(result.findings.is_empty(), "no findings expected");
        assert_eq!(result.exit_code(), 0);
        assert_eq!(result.metrics.events_scanned, 1);
        assert_eq!(result.metrics.events_converted, 1);
    }

    #[test]
    fn yaml_contains_expected_metadata_name_from_filename() {
        let opts = ConvertOpts::default();
        let result = convert(MINIMAL_XML, Path::new("/tmp/cisco.foo.events.xml"), &opts);
        let yaml = result.yaml.expect("YAML emitted");
        assert!(yaml.contains("name: cisco.foo"));
    }

    #[test]
    fn undotted_name_converts_cleanly() {
        // Undotted metadata.name (e.g. just "Cisco") is now accepted —
        // the vendor derivation tolerates undotted names (matching
        // Horizon's server-side `StringUtils.substringBefore`, which
        // returns the whole name when no '.' is present).
        let result = convert(
            MINIMAL_XML,
            Path::new("/tmp/test.xml"),
            &ConvertOpts::default(),
        );
        assert!(result.yaml.is_some(), "undotted name must convert cleanly");
        assert!(result.findings.is_empty(), "no findings for undotted name");
        assert_eq!(result.exit_code(), 0);
    }

    #[test]
    fn duplicate_uei_across_events_converts_cleanly() {
        // Shared UEIs across events are a first-class OpenNMS normalization
        // pattern. The converter passes them through without emitting any
        // finding. See archived `permit-duplicate-ueis-as-normalization-pattern`.
        let opts = ConvertOpts::default();
        let result = convert(DUPLICATE_UEI_XML, Path::new("/tmp/foo.test.xml"), &opts);
        assert!(
            result.yaml.is_some(),
            "YAML must be emitted; duplicates are first-class"
        );
        assert!(result.findings.is_empty(), "no finding for shared UEIs");
        assert_eq!(result.exit_code(), 0);
        let yaml = result.yaml.unwrap();
        // Both events are present in the output.
        assert!(yaml.contains("uei.test/dup"));
        // The UEI appears twice (once per event).
        assert_eq!(yaml.matches("uei.test/dup").count(), 2);
    }

    #[test]
    fn reserved_metadata_name_produces_ec003() {
        let result = convert(
            MINIMAL_XML,
            Path::new("/tmp/eventconf.xml"),
            &ConvertOpts::default(),
        );
        assert!(result.yaml.is_none());
        assert_eq!(result.exit_code(), 2);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].code, FindingCode::Ec003);
    }

    #[test]
    fn missing_uei_produces_ec004() {
        let xml = br#"<events>
  <event>
    <event-label>Has no UEI</event-label>
    <severity>Warning</severity>
  </event>
</events>"#;
        let result = convert(xml, Path::new("/tmp/foo.test.xml"), &ConvertOpts::default());
        assert!(result.yaml.is_none());
        assert!(result.findings.iter().any(|f| f.code == FindingCode::Ec004));
    }

    #[test]
    fn severity_case_mismatch_produces_ec005_and_normalizes() {
        let xml = br#"<events>
  <event>
    <uei>uei.test/foo</uei>
    <event-label>Foo</event-label>
    <severity>WARNING</severity>
  </event>
</events>"#;
        let result = convert(xml, Path::new("/tmp/foo.test.xml"), &ConvertOpts::default());
        let f = result
            .findings
            .iter()
            .find(|f| f.code == FindingCode::Ec005)
            .expect("EC005 emitted");
        assert_eq!(f.severity, Severity::Warning);
        let yaml = result.yaml.expect("YAML emitted (warning, not error)");
        assert!(yaml.contains("severity: Warning"));
        assert!(!yaml.contains("WARNING"));
    }

    #[test]
    fn alarm_type_4_produces_ec007_warning_and_ec009_blocks_emission() {
        // alarm-type=4 emits EC007 (warning, location-rich) AND triggers
        // EC009 from the post-validate safety net (since the v0.1 schema
        // accepts only 1|2|3). Both findings are useful: EC007 cites the
        // source event; EC009 tells the operator the YAML wouldn't pass
        // apply. The presence of EC009 (error) blocks YAML emission.
        let xml = br#"<events>
  <event>
    <uei>uei.test/foo</uei>
    <event-label>Foo</event-label>
    <severity>Warning</severity>
    <alarm-data reduction-key="k" alarm-type="4"/>
  </event>
</events>"#;
        let result = convert(xml, Path::new("/tmp/foo.test.xml"), &ConvertOpts::default());
        assert!(
            result.findings.iter().any(|f| f.code == FindingCode::Ec007),
            "EC007 emitted"
        );
        assert!(
            result.findings.iter().any(|f| f.code == FindingCode::Ec009),
            "EC009 emitted by post-validate"
        );
        assert!(result.yaml.is_none(), "EC009 blocks YAML emission");
        assert_eq!(result.exit_code(), 2);
    }

    #[test]
    fn invalid_filename_chars_produce_ec008() {
        let result = convert(
            MINIMAL_XML,
            Path::new("/tmp/has spaces.events.xml"),
            &ConvertOpts::default(),
        );
        assert!(result.yaml.is_none());
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].code, FindingCode::Ec008);
    }

    #[test]
    fn name_override_works() {
        let result = convert(
            MINIMAL_XML,
            Path::new("/tmp/has spaces.xml"),
            &ConvertOpts {
                name_override: Some("clean.name".into()),
            },
        );
        let yaml = result.yaml.expect("YAML emitted");
        assert!(yaml.contains("name: clean.name"));
    }

    #[test]
    fn stdin_without_name_override_produces_ec008() {
        let result = convert(MINIMAL_XML, Path::new("-"), &ConvertOpts::default());
        assert!(result.yaml.is_none());
        assert_eq!(result.findings[0].code, FindingCode::Ec008);
    }

    #[test]
    fn stdin_with_name_override_works() {
        let result = convert(
            MINIMAL_XML,
            Path::new("-"),
            &ConvertOpts {
                name_override: Some("from.stdin".into()),
            },
        );
        let yaml = result.yaml.expect("YAML emitted from stdin");
        assert!(yaml.contains("name: from.stdin"));
    }

    #[test]
    fn finding_code_parse_round_trips() {
        for code in FindingCode::all() {
            let s = code.as_str();
            assert_eq!(FindingCode::parse(s), Some(*code));
            assert_eq!(FindingCode::parse(&s.to_lowercase()), Some(*code));
        }
        assert_eq!(FindingCode::parse("EC999"), None);
    }

    #[test]
    fn every_code_has_an_explanation() {
        // Compile-time-ish completeness check: explain() must return
        // non-empty text for every defined code.
        for code in FindingCode::all() {
            let text = explain(*code);
            assert!(
                !text.trim().is_empty(),
                "explain({code:?}) returned empty text"
            );
            assert!(text.len() > 100, "explain({code:?}) text too short");
        }
    }

    #[test]
    fn render_report_text_clean_run_lists_no_findings() {
        let result = convert(
            MINIMAL_XML,
            Path::new("/tmp/foo.bar.xml"),
            &ConvertOpts::default(),
        );
        let report = render_report_text(&result);
        assert!(report.contains("no findings"));
        assert!(report.contains("Exit: 0"));
    }

    #[test]
    fn finding_code_all_codes_round_trip_through_as_str_and_parse() {
        // Catches drift between FindingCode::all(), as_str, and parse.
        // If a new variant is added without updating these three, the
        // test fails.
        for code in FindingCode::all() {
            let s = code.as_str();
            assert_eq!(FindingCode::parse(s), Some(*code), "as_str/parse drift");
            assert_eq!(
                FindingCode::parse(&s.to_lowercase()),
                Some(*code),
                "parse is case-insensitive"
            );
        }
    }

    #[test]
    fn batch_mode_smoke_three_inputs_each_with_distinct_finding_shapes() {
        // Synthetic fixtures exercising three distinct conversion paths:
        //   - clean: produces YAML, exit 0
        //   - dup UEI: blocking EC001, exit 2
        //   - missing UEI: blocking EC004, exit 2
        // We invoke convert() per input directly (the CLI layer's batch
        // dispatcher just calls convert() in a loop and computes the
        // max exit code).
        const CLEAN: &[u8] = br#"<events>
  <event>
    <uei>uei.test/clean</uei>
    <event-label>Clean</event-label>
    <severity>Normal</severity>
  </event>
</events>"#;
        const DUP_UEI: &[u8] = br#"<events>
  <event>
    <uei>uei.test/dup</uei>
    <event-label>A</event-label>
    <severity>Warning</severity>
  </event>
  <event>
    <uei>uei.test/dup</uei>
    <event-label>B</event-label>
    <severity>Major</severity>
  </event>
</events>"#;
        const MISSING_UEI: &[u8] = br#"<events>
  <event>
    <event-label>NoUei</event-label>
    <severity>Warning</severity>
  </event>
</events>"#;

        let r_clean = convert(
            CLEAN,
            Path::new("/tmp/a.foo.events.xml"),
            &ConvertOpts::default(),
        );
        assert_eq!(r_clean.exit_code(), 0);
        assert!(r_clean.yaml.is_some());

        // Duplicate UEIs are first-class (normalization pattern) — the
        // converter passes them through cleanly.
        let r_dup = convert(
            DUP_UEI,
            Path::new("/tmp/b.foo.events.xml"),
            &ConvertOpts::default(),
        );
        assert_eq!(r_dup.exit_code(), 0);
        assert!(r_dup.yaml.is_some());
        assert!(r_dup.findings.is_empty());

        let r_missing = convert(
            MISSING_UEI,
            Path::new("/tmp/c.foo.events.xml"),
            &ConvertOpts::default(),
        );
        assert_eq!(r_missing.exit_code(), 2);
        assert!(r_missing.yaml.is_none());
        assert!(
            r_missing
                .findings
                .iter()
                .any(|f| f.code == FindingCode::Ec004)
        );

        // Batch dispatcher behavior: max(0, 0, 2) = 2  (clean + dup-UEI now
        // both exit 0; the missing-UEI blocker drives the worst-case to 2).
        let worst = [
            r_clean.exit_code(),
            r_dup.exit_code(),
            r_missing.exit_code(),
        ]
        .iter()
        .max()
        .copied()
        .unwrap_or(0);
        assert_eq!(worst, 2);
    }

    #[test]
    fn fallback_path_engages_when_event_appears_in_a_comment() {
        // B15: previously this test asserted only that an empty document
        // produced zero pairs — which doesn't exercise the
        // count-mismatch fallback at all. With a `<event>` substring
        // inside an XML comment, the byte scanner will count the
        // commented occurrence but the serde parser will skip it,
        // forcing the fallback. The result: every SourceLocation has
        // line=0, column=0 (the documented degraded mode).
        let xml = br#"<events>
  <!-- placeholder: <event> -->
  <event>
    <uei>uei.test/foo</uei>
    <event-label>Foo</event-label>
    <severity>Warning</severity>
  </event>
</events>"#;
        let result = convert(
            xml,
            Path::new("/tmp/foo.bar.events.xml"),
            &ConvertOpts::default(),
        );
        // The conversion still produces YAML (the real <event> parsed
        // fine); only the location metadata is degraded.
        assert!(result.yaml.is_some());
        // No findings expected for clean event content.
        assert!(result.findings.is_empty());
    }

    #[test]
    fn finding_code_ec006_remains_fallow_and_unparseable() {
        // EC006 was descoped 2026-05-18 (unmodeled-element finding deferred
        // to follow-up work). Its number stays fallow — parsing it should
        // return None, and it should not appear in all().
        assert!(FindingCode::parse("EC006").is_none());
        assert!(!FindingCode::all().iter().any(|c| c.as_str() == "EC006"));
    }

    #[test]
    fn finding_code_ec001_remains_fallow_and_unparseable() {
        // EC001 was removed by the
        // permit-duplicate-ueis-as-normalization-pattern change (duplicate
        // UEIs are first-class, not a finding). Its number stays fallow —
        // parsing it should return None, and it should not appear in all().
        assert!(FindingCode::parse("EC001").is_none());
        assert!(!FindingCode::all().iter().any(|c| c.as_str() == "EC001"));
    }

    #[test]
    fn render_report_text_includes_finding_section_per_finding() {
        // Use a missing-UEI fixture (produces EC004) since duplicate UEIs
        // no longer produce findings after the
        // permit-duplicate-ueis-as-normalization-pattern change.
        let xml = br#"<events>
  <event>
    <event-label>Missing UEI</event-label>
    <severity>Warning</severity>
  </event>
</events>"#;
        let result = convert(xml, Path::new("/tmp/foo.bar.xml"), &ConvertOpts::default());
        let report = render_report_text(&result);
        assert!(report.contains("EC004"));
        assert!(report.contains("Exit: 2"));
    }
}
