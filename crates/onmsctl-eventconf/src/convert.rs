/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! XML → YAML conversion engine for the `event-source convert` migration command.
//!
//! Takes eventconf XML bytes and produces a [`ConversionResult`] carrying:
//!   - The serialized YAML (if conversion succeeded enough to produce one)
//!   - A list of structured [`Finding`]s describing rule violations, drops,
//!     and normalizations encountered during the conversion
//!   - Coverage metrics (events scanned / converted / dropped)
//!
//! The engine is `pub` so the CLI layer (`cmd::source::Convert`) and the
//! `event-source download --format yaml` path can both call it identically. The
//! engine itself does **no I/O** — bytes in, structured result out.
//!
//! See the `source-convert-with-migration-report` OpenSpec change for the
//! design rationale and the spec scenarios that drive this code.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::apply::from_wire::WireToLocalError;
use crate::apply::local::{EventDef, EventSourceLocal, EventSourceSpec, Metadata};
use crate::xml::{
    MODELED_EVENT_CHILDREN, SourceLocation, byte_offset_to_line_col, parse_events_with_locations,
    scan_event_direct_children,
};

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
/// `EC` (event-conf). The catalog is contiguous from `EC001` through
/// `EC008` after the EC009→EC006 renumber that landed alongside EC001.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FindingCode {
    /// EC001 — event contains direct-child elements not in the modeled
    /// allowlist. Forward-compat warning for any element under `<event>`
    /// that `onmsctl` doesn't model — including future Horizon schema
    /// additions. Structural-only (does NOT detect attribute extensions
    /// or enum-value drift).
    Ec001,
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
    /// EC006 — `EventSourceLocal::validate` rejected the assembled
    /// document after the converter's own findings ran. Acts as a
    /// safety net for schema rules the converter doesn't mirror
    /// individually. (Was `EC009` prior to the EC001-introducing
    /// renumber; renumbered to close the EC006 fallow slot.)
    Ec006,
    /// EC007 — alarm-type value outside the accepted set `{1, 2, 3}`
    /// (i.e. zero, negative, or ≥ 4).
    Ec007,
    /// EC008 — derived metadata.name contains characters the schema
    /// rejects, or is otherwise structurally invalid.
    Ec008,
}

impl FindingCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ec001 => "EC001",
            Self::Ec002 => "EC002",
            Self::Ec003 => "EC003",
            Self::Ec004 => "EC004",
            Self::Ec005 => "EC005",
            Self::Ec006 => "EC006",
            Self::Ec007 => "EC007",
            Self::Ec008 => "EC008",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "EC001" => Some(Self::Ec001),
            "EC002" => Some(Self::Ec002),
            "EC003" => Some(Self::Ec003),
            "EC004" => Some(Self::Ec004),
            "EC005" => Some(Self::Ec005),
            "EC006" => Some(Self::Ec006),
            "EC007" => Some(Self::Ec007),
            "EC008" => Some(Self::Ec008),
            _ => None,
        }
    }

    /// All defined codes. Catalog is contiguous EC001–EC008 with no
    /// fallow slots. Future codes append (EC009, EC010, ...).
    pub fn all() -> &'static [Self] {
        &[
            Self::Ec001,
            Self::Ec002,
            Self::Ec003,
            Self::Ec004,
            Self::Ec005,
            Self::Ec006,
            Self::Ec007,
            Self::Ec008,
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
    /// EC001 payload. `dropped_elements` lists distinct direct-child
    /// element names found under this `<event>` that are not in the
    /// modeled-element allowlist. `truncated_remaining`, when present,
    /// signals a summary-mode finding emitted after the per-file cap
    /// was reached (no per-event location data).
    UnmodeledElements {
        dropped_elements: Vec<String>,
        location: Option<SourceLocation>,
        /// When `Some(n)`, this is a synthetic summary finding emitted
        /// once after the cap; `n` is the count of additional events
        /// with unmodeled elements that were not individually reported.
        #[serde(skip_serializing_if = "Option::is_none")]
        truncated_remaining: Option<usize>,
    },
}

/// Options passed to [`convert`].
#[derive(Clone, Debug, Default)]
pub struct ConvertOpts {
    /// Override the metadata.name derived from the input filename. Required
    /// when the input has no filename (stdin).
    pub name_override: Option<String>,
    /// Cap on the number of `EC001` (unmodeled-element) findings emitted
    /// per input file. After the cap, a single summary finding is emitted
    /// describing how many additional events were truncated, and the
    /// per-event scan stops. `None` → default cap (1000). `Some(0)` →
    /// unlimited (disabled cap). `Some(n)` for n > 0 → cap at n.
    pub max_findings: Option<usize>,
}

/// Default cap on `EC001` findings emitted per converted file. Prevents
/// memory pressure on pathological or hostile inputs (e.g. a 16 MiB XML
/// where every event uses unmodeled elements).
pub const DEFAULT_EC001_FINDINGS_CAP: usize = 1000;

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

    // Step 4b: structural pre-walk for EC001 (unmodeled direct-child
    // elements). The typed deserializer silently drops anything not in
    // `XmlEvent`; this pass surfaces those drops as warnings so operators
    // know what their YAML is losing.
    let cap = match opts.max_findings {
        None => DEFAULT_EC001_FINDINGS_CAP,
        Some(0) => usize::MAX, // 0 means unlimited
        Some(n) => n,
    };
    let scan_result = scan_event_direct_children(xml);
    if let Err(e) = &scan_result {
        // The structural scan is a forward-compat safety net — silent
        // failure inverts its contract. Surface as a warning so operators
        // know unmodeled-element detection was skipped on this run. The
        // typed parse above already succeeded, so YAML emission proceeds.
        findings.push(Finding {
            code: FindingCode::Ec001,
            severity: Severity::Warning,
            message: format!(
                "EC001 structural scan failed; unmodeled-element detection skipped: {e}"
            ),
            details: FindingDetails::UnmodeledElements {
                dropped_elements: Vec::new(),
                location: None,
                truncated_remaining: None,
            },
            suggested_fix: "The typed XML parse succeeded but the auxiliary structural pass \
                used for EC001 did not. The converted YAML may be missing unmodeled-element \
                warnings. Inspect the XML for malformed structure (mismatched tags, \
                unsupported namespaces, comments containing literal `<event>`); rerun after \
                correcting."
                .into(),
        });
    }
    if let Ok(scans) = scan_result {
        let mut emitted: usize = 0;
        let mut truncated: usize = 0;
        for scan in &scans {
            let unmodeled: Vec<String> = scan
                .direct_children
                .iter()
                .filter(|c| !MODELED_EVENT_CHILDREN.contains(&c.as_str()))
                .cloned()
                .collect();
            if unmodeled.is_empty() {
                continue;
            }
            if emitted >= cap {
                truncated += 1;
                continue;
            }
            let (line, column) = byte_offset_to_line_col(xml, scan.offset);
            let location = SourceLocation {
                file: source_path.to_path_buf(),
                line,
                column,
                event_index: scan.event_index,
            };
            let names_joined = unmodeled
                .iter()
                .map(|n| format!("<{n}>"))
                .collect::<Vec<_>>()
                .join(", ");
            findings.push(Finding {
                code: FindingCode::Ec001,
                severity: Severity::Warning,
                message: format!(
                    "event contains unmodeled elements dropped on conversion: {names_joined}"
                ),
                details: FindingDetails::UnmodeledElements {
                    dropped_elements: unmodeled,
                    location: Some(location),
                    truncated_remaining: None,
                },
                suggested_fix: "These elements are not part of the local YAML schema and will be \
                    absent from the converted YAML. For full-fidelity round-tripping keep the \
                    eventconf XML alongside the YAML and use `event-source upload`. Run \
                    `onmsctl event-source convert --explain EC001` for the full rationale, or upgrade \
                    `onmsctl` if these elements are modeled in a newer release."
                    .into(),
            });
            emitted += 1;
        }
        if truncated > 0 {
            findings.push(Finding {
                code: FindingCode::Ec001,
                severity: Severity::Warning,
                message: format!(
                    "{truncated} additional events with unmodeled elements were truncated \
                     from this report; rerun with --max-findings <higher> (or 0 for unlimited) \
                     to see all"
                ),
                details: FindingDetails::UnmodeledElements {
                    dropped_elements: Vec::new(),
                    location: None,
                    truncated_remaining: Some(truncated),
                },
                suggested_fix: "Pass --max-findings 0 to disable the cap, or --max-findings <n> \
                    to raise it."
                    .into(),
            });
        }
    }
    // (Error path above emitted a synthetic EC001 warning before
    // reaching here; we do not silently swallow walker failures.)

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

        // Convert wire → local. EC007 (alarm-type out of range) and EC004
        // (missing required field) both surface here via WireToLocalError.
        match EventDef::try_from(&local_wire) {
            Ok(def) => {
                event_defs.push(def);
                metrics.events_converted += 1;
            }
            Err(WireToLocalError::AlarmDataAlarmTypeOutOfRange { value }) => {
                metrics.events_dropped += 1;
                findings.push(Finding {
                    code: FindingCode::Ec007,
                    severity: Severity::Error,
                    message: format!("alarm-type {value} is outside the accepted set {{1, 2, 3}}"),
                    details: FindingDetails::AlarmTypeOutOfRange {
                        value,
                        location: location.clone(),
                    },
                    suggested_fix: "The local schema strictly accepts alarm-type values 1 \
                        (raise), 2 (resolution), and 3 (unresolvable). The YAML form also \
                        accepts the symbolic names directly. Correct the source XML; if this \
                        is a new Horizon alarm semantic, file a follow-up change to widen the \
                        schema."
                        .into(),
                });
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

    // Step 6: cross-event validation — EC006 post-validate safety net
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
                code: FindingCode::Ec006,
                severity: Severity::Error,
                message: format!("post-conversion validation failed: {err_str}"),
                details: FindingDetails::PostValidationFailed {
                    validator_error: err_str,
                },
                suggested_fix: "The authoritative local-schema validator rejected the \
                    converted document. Read the validator's error message — it names the \
                    offending field path. If this rule could have been surfaced earlier by \
                    a more specific code (EC001-EC005, EC007, EC008), file a converter bug; \
                    otherwise correct the source XML and re-run."
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
        // EC007 (alarm-type out of range) takes its own code path in the
        // per-event conversion loop; field_name_for is only called for
        // EC004 emissions, so this is unreachable.
        WireToLocalError::AlarmDataAlarmTypeOutOfRange { .. } => "alarm-data.alarm-type",
    }
}

// -- explain() table -------------------------------------------------------

/// Long-form explanation for a finding code. Returned by
/// `onmsctl event-source convert --explain <code>`. Text is stable across patch
/// releases — wording may shift but the section structure does not.
pub fn explain(code: FindingCode) -> &'static str {
    match code {
        FindingCode::Ec001 => include_str!("explain/EC001.txt"),
        FindingCode::Ec002 => include_str!("explain/EC002.txt"),
        FindingCode::Ec003 => include_str!("explain/EC003.txt"),
        FindingCode::Ec004 => include_str!("explain/EC004.txt"),
        FindingCode::Ec005 => include_str!("explain/EC005.txt"),
        FindingCode::Ec006 => include_str!("explain/EC006.txt"),
        FindingCode::Ec007 => include_str!("explain/EC007.txt"),
        FindingCode::Ec008 => include_str!("explain/EC008.txt"),
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
                "    For the full rationale: onmsctl event-source convert --explain {}\n",
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
        FindingDetails::UnmodeledElements {
            dropped_elements,
            location,
            truncated_remaining,
        } => {
            if let Some(n) = truncated_remaining {
                out.push_str(&format!("    Truncated: {n} additional events\n"));
            } else {
                let names = dropped_elements
                    .iter()
                    .map(|n| format!("<{n}>"))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("    Dropped: {names}\n"));
                if let Some(loc) = location {
                    out.push_str(&format!("    At:      {}\n", fmt_location(loc)));
                }
            }
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
    fn alarm_type_4_produces_ec007_error_and_blocks_emission() {
        // Strict mode: alarm-type outside {1,2,3} is an error, not a
        // warning. The wire→local conversion fails with
        // WireToLocalError::AlarmDataAlarmTypeOutOfRange; convert.rs
        // surfaces that as EC007 (Error severity). YAML is NOT written;
        // exit code is 2.
        let xml = br#"<events>
  <event>
    <uei>uei.test/foo</uei>
    <event-label>Foo</event-label>
    <severity>Warning</severity>
    <alarm-data reduction-key="k" alarm-type="4"/>
  </event>
</events>"#;
        let result = convert(xml, Path::new("/tmp/foo.test.xml"), &ConvertOpts::default());
        let ec007s: Vec<_> = result
            .findings
            .iter()
            .filter(|f| f.code == FindingCode::Ec007)
            .collect();
        assert_eq!(ec007s.len(), 1, "exactly one EC007");
        assert_eq!(
            ec007s[0].severity,
            Severity::Error,
            "EC007 is Error severity"
        );
        assert!(
            ec007s[0].message.contains("4"),
            "message cites the bad value"
        );
        // EC004 is NOT emitted for this case — EC007 is the specific code.
        assert!(
            !result.findings.iter().any(|f| f.code == FindingCode::Ec004),
            "EC004 not emitted; EC007 covers alarm-type out of range"
        );
        assert!(result.yaml.is_none(), "blocking error → no YAML");
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
                max_findings: None,
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
                max_findings: None,
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
    fn catalog_is_contiguous_ec001_through_ec008() {
        // After the EC009→EC006 renumber and the EC001 (unmodeled-element)
        // activation, the catalog is contiguous EC001–EC008 with no fallow
        // slots.
        let codes: Vec<&str> = FindingCode::all().iter().map(|c| c.as_str()).collect();
        assert_eq!(
            codes,
            [
                "EC001", "EC002", "EC003", "EC004", "EC005", "EC006", "EC007", "EC008"
            ]
        );
        // Every code in `all()` parses round-trip from its string form.
        for &expected in &codes {
            let parsed = FindingCode::parse(expected).expect("known code parses");
            assert_eq!(parsed.as_str(), expected);
        }
        // EC009 is not yet assigned; parsing returns None.
        assert!(FindingCode::parse("EC009").is_none());
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

    // -- EC001 emission tests ---------------------------------------------

    #[test]
    fn ec001_event_with_unmodeled_elements_emits_one_finding_with_all_names() {
        // Uses `<autoaction>` and `<priority>` — both real eventconf
        // XSD children of `<event>` that this YAML schema doesn't
        // model. (After event-source-filters, `<filters>` is modeled.)
        let xml = br#"<events>
  <event>
    <uei>uei.test/foo</uei>
    <event-label>Foo</event-label>
    <severity>Warning</severity>
    <autoaction state="off">cleanup()</autoaction>
    <priority>17</priority>
  </event>
</events>"#;
        let result = convert(
            xml,
            Path::new("/tmp/test.events.xml"),
            &ConvertOpts::default(),
        );
        let ec001s: Vec<_> = result
            .findings
            .iter()
            .filter(|f| f.code == FindingCode::Ec001)
            .collect();
        assert_eq!(ec001s.len(), 1, "exactly one EC001 per event");
        let f = ec001s[0];
        assert_eq!(f.severity, Severity::Warning);
        // Message lists both unmodeled names in document order.
        assert!(f.message.contains("<autoaction>"));
        assert!(f.message.contains("<priority>"));
        let auto_pos = f.message.find("<autoaction>").unwrap();
        let pri_pos = f.message.find("<priority>").unwrap();
        assert!(auto_pos < pri_pos, "document order preserved");
        // YAML still written (warning, not error).
        assert!(result.yaml.is_some(), "YAML written despite EC001 warning");
        assert_eq!(result.exit_code(), 1, "warnings produce exit 1");
        // Finding details carry the location and dropped element list.
        match &f.details {
            FindingDetails::UnmodeledElements {
                dropped_elements,
                location,
                truncated_remaining,
            } => {
                assert_eq!(dropped_elements, &["autoaction", "priority"]);
                assert!(truncated_remaining.is_none());
                // Pin the file:line:column — spec requires file:line anchor
                // and 0-based event index. The `<event>` opens on line 2 of
                // the fixture (line 1 is `<events>`), at column 3 (after
                // the two-space indent).
                let loc = location.as_ref().expect("location present");
                assert_eq!(loc.line, 2, "line anchored at <event> opening");
                assert_eq!(loc.column, 3, "column anchored at the `<` byte");
                assert_eq!(loc.event_index, 0, "first event is index 0");
                assert_eq!(
                    loc.file.to_string_lossy(),
                    "/tmp/test.events.xml",
                    "file path preserved from source_path"
                );
            }
            other => panic!("unexpected details: {other:?}"),
        }
    }

    #[test]
    fn ec001_event_with_only_modeled_elements_emits_no_finding() {
        let xml = br#"<events>
  <event>
    <uei>uei.test/foo</uei>
    <event-label>Foo</event-label>
    <severity>Warning</severity>
    <logmsg dest="logndisplay">msg</logmsg>
    <alarm-data reduction-key="k" alarm-type="1"/>
  </event>
</events>"#;
        let result = convert(
            xml,
            Path::new("/tmp/test.events.xml"),
            &ConvertOpts::default(),
        );
        assert!(
            !result.findings.iter().any(|f| f.code == FindingCode::Ec001),
            "no EC001 expected for fully-modeled event"
        );
        assert_eq!(result.exit_code(), 0);
    }

    #[test]
    fn ec001_snmp_no_longer_fires_after_modeling() {
        // Regression test for the event-source-snmp change. `<snmp>` is
        // now in the modeled-element allowlist; XML containing it must
        // convert without firing EC001 for that element. All six
        // sub-fields must survive into the YAML output (idtext and
        // community in particular — under-asserted in the prior
        // version of this test).
        let xml = br#"<events>
  <event>
    <uei>uei.test/cold-start</uei>
    <event-label>Cold start</event-label>
    <severity>Warning</severity>
    <snmp>
      <id>.1.3.6.1.4.1.9.1.13</id>
      <idtext>Cisco</idtext>
      <version>v2c</version>
      <generic>6</generic>
      <specific>1</specific>
      <community>public</community>
    </snmp>
  </event>
</events>"#;
        let result = convert(
            xml,
            Path::new("/tmp/cisco.foo.events.xml"),
            &ConvertOpts::default(),
        );
        assert!(
            !result.findings.iter().any(|f| f.code == FindingCode::Ec001),
            "no EC001 expected — <snmp> is modeled. Findings: {:?}",
            result.findings.iter().map(|f| f.code).collect::<Vec<_>>()
        );
        assert_eq!(result.exit_code(), 0);
        let yaml = result.yaml.as_ref().expect("YAML emitted");
        // Confirm `snmp:` appears AT EVENT SCOPE — the assertion looks
        // for `    snmp:` (event-level indentation, 4 spaces) to catch
        // a serializer regression that placed snmp fields anywhere
        // outside the nested block.
        assert!(
            yaml.contains("    snmp:"),
            "snmp block at event scope:\n{yaml}"
        );
        assert!(yaml.contains("id: .1.3.6.1.4.1.9.1.13"));
        assert!(yaml.contains("idtext: Cisco"), "idtext preserved:\n{yaml}");
        assert!(yaml.contains("version: v2c"));
        assert!(yaml.contains("generic: 6"));
        assert!(yaml.contains("specific: 1"));
        assert!(
            yaml.contains("community: public"),
            "community preserved:\n{yaml}"
        );
    }

    #[test]
    fn ec001_parameter_no_longer_fires_after_modeling() {
        // event-source-parameter regression. <parameter> is now in the
        // modeled-element allowlist; XML containing it must convert
        // without firing EC001. All three attributes survive into YAML.
        let xml = br#"<events>
  <event>
    <uei>uei.test/with-params</uei>
    <event-label>With params</event-label>
    <severity>Warning</severity>
    <parameter name="endpoint" value="/var/log/foo"/>
    <parameter name="context" value="%parm[#1]%" expand="true"/>
  </event>
</events>"#;
        let result = convert(
            xml,
            Path::new("/tmp/vendor.foo.events.xml"),
            &ConvertOpts::default(),
        );
        assert!(
            !result.findings.iter().any(|f| f.code == FindingCode::Ec001),
            "no EC001 — <parameter> is modeled. Findings: {:?}",
            result.findings.iter().map(|f| f.code).collect::<Vec<_>>()
        );
        assert_eq!(result.exit_code(), 0);
        let yaml = result.yaml.as_ref().expect("YAML emitted");
        assert!(
            yaml.contains("parameters:"),
            "block under event scope:\n{yaml}"
        );
        assert!(yaml.contains("name: endpoint"));
        assert!(yaml.contains("value: /var/log/foo"));
        assert!(yaml.contains("name: context"));
        assert!(
            yaml.contains("expand: true"),
            "explicit expand preserved:\n{yaml}"
        );
    }

    #[test]
    fn ec001_filters_no_longer_fires_after_modeling() {
        // event-source-filters regression. `<filters>` is now in the
        // modeled-element allowlist; XML with the wrapper converts
        // cleanly. Content survives into YAML (flat, no wrapper).
        let xml = br#"<events>
  <event>
    <uei>uei.test/with-filters</uei>
    <event-label>With filters</event-label>
    <severity>Warning</severity>
    <filters>
      <filter eventparm="trapMsg" pattern="\bWARN\b" replacement="WARNING"/>
      <filter eventparm="ifAlias" pattern="^old-(.*)$" replacement="new-$1"/>
    </filters>
  </event>
</events>"#;
        let result = convert(
            xml,
            Path::new("/tmp/vendor.foo.events.xml"),
            &ConvertOpts::default(),
        );
        assert!(
            !result.findings.iter().any(|f| f.code == FindingCode::Ec001),
            "no EC001 — <filters> is modeled. Findings: {:?}",
            result.findings.iter().map(|f| f.code).collect::<Vec<_>>()
        );
        assert_eq!(result.exit_code(), 0);
        let yaml = result.yaml.as_ref().expect("YAML emitted");
        assert!(yaml.contains("filters:"), "filters block:\n{yaml}");
        assert!(yaml.contains("eventparm: trapMsg"));
        assert!(yaml.contains("eventparm: ifAlias"));
        assert!(yaml.contains("replacement: WARNING"));
        assert!(yaml.contains("replacement: new-$1"));
    }

    #[test]
    fn ec001_forward_and_script_no_longer_fire_after_modeling() {
        // event-source-forward-and-script regression. Both are in the
        // modeled-element allowlist now; XML containing them converts
        // without firing EC001. Content survives into YAML.
        let xml = br#"<events>
  <event>
    <uei>uei.test/with-fwd-script</uei>
    <event-label>With forward and script</event-label>
    <severity>Warning</severity>
    <forward state="on" mechanism="snmpudp">alarmcentral:162</forward>
    <script language="beanshell">do_thing();</script>
  </event>
</events>"#;
        let result = convert(
            xml,
            Path::new("/tmp/vendor.foo.events.xml"),
            &ConvertOpts::default(),
        );
        assert!(
            !result.findings.iter().any(|f| f.code == FindingCode::Ec001),
            "no EC001 — <forward> and <script> are modeled. Findings: {:?}",
            result.findings.iter().map(|f| f.code).collect::<Vec<_>>()
        );
        assert_eq!(result.exit_code(), 0);
        let yaml = result.yaml.as_ref().expect("YAML emitted");
        assert!(yaml.contains("forwards:"), "forwards block:\n{yaml}");
        // serde_norway emits `state: on` unquoted because its YAML 1.2
        // mode does not treat `on` as a boolean. Accept all common
        // quote styles defensively.
        assert!(
            yaml.contains("state: on")
                || yaml.contains("state: 'on'")
                || yaml.contains("state: \"on\""),
            "state preserved:\n{yaml}"
        );
        assert!(yaml.contains("mechanism: snmpudp"));
        assert!(
            yaml.contains("alarmcentral:162"),
            "target preserved:\n{yaml}"
        );
        assert!(yaml.contains("scripts:"), "scripts block:\n{yaml}");
        assert!(yaml.contains("language: beanshell"));
        assert!(
            yaml.contains("do_thing();"),
            "script body preserved:\n{yaml}"
        );
    }

    #[test]
    fn snmp_out_of_range_integers_round_trip_verbatim() {
        // README and proposal promise out-of-range integers round-trip
        // verbatim (no range enforcement). Pin this contract: generic=-1
        // and specific=999 survive XML → YAML without modification or
        // findings.
        let xml = br#"<events>
  <event>
    <uei>uei.test/odd</uei>
    <event-label>Odd</event-label>
    <severity>Warning</severity>
    <snmp>
      <generic>-1</generic>
      <specific>999</specific>
    </snmp>
  </event>
</events>"#;
        let result = convert(
            xml,
            Path::new("/tmp/odd.foo.events.xml"),
            &ConvertOpts::default(),
        );
        assert_eq!(
            result.exit_code(),
            0,
            "out-of-range snmp integers should not block emission: {:?}",
            result.findings.iter().map(|f| f.code).collect::<Vec<_>>()
        );
        let yaml = result.yaml.as_ref().expect("YAML emitted");
        assert!(
            yaml.contains("generic: -1"),
            "negative generic preserved:\n{yaml}"
        );
        assert!(
            yaml.contains("specific: 999"),
            "specific 999 preserved:\n{yaml}"
        );
    }

    #[test]
    fn ec001_two_events_with_overlapping_unmodeled_children_emit_separate_findings() {
        let xml = br#"<events>
  <event>
    <uei>uei.test/one</uei>
    <event-label>One</event-label>
    <severity>Warning</severity>
    <autoaction state="off">a()</autoaction>
  </event>
  <event>
    <uei>uei.test/two</uei>
    <event-label>Two</event-label>
    <severity>Major</severity>
    <autoaction state="off">b()</autoaction>
    <priority>17</priority>
  </event>
</events>"#;
        let result = convert(
            xml,
            Path::new("/tmp/test.events.xml"),
            &ConvertOpts::default(),
        );
        let ec001s: Vec<_> = result
            .findings
            .iter()
            .filter(|f| f.code == FindingCode::Ec001)
            .collect();
        assert_eq!(ec001s.len(), 2, "one EC001 per event");
        // Each finding anchored to a distinct event location.
        let locs: Vec<_> = ec001s
            .iter()
            .filter_map(|f| match &f.details {
                FindingDetails::UnmodeledElements { location, .. } => location.as_ref(),
                _ => None,
            })
            .collect();
        assert_eq!(locs.len(), 2);
        assert_ne!(locs[0].event_index, locs[1].event_index);
        assert_ne!(locs[0].line, locs[1].line);
    }

    #[test]
    fn ec001_default_cap_engages_on_oversized_input() {
        // Build a pathological XML with 1005 events, each carrying an
        // unmodeled <parameter>. Default cap (1000) triggers; we expect
        // 1000 per-event findings + 1 summary finding listing 5 truncated.
        let mut xml = String::from("<events>\n");
        for i in 0..1005 {
            xml.push_str(&format!(
                "  <event>\n    <uei>uei.test/{i}</uei>\n    <event-label>L{i}</event-label>\n    \
                 <severity>Warning</severity>\n    <autoaction state=\"off\">x()</autoaction>\n  </event>\n"
            ));
        }
        xml.push_str("</events>\n");
        let result = convert(
            xml.as_bytes(),
            Path::new("/tmp/big.events.xml"),
            &ConvertOpts::default(),
        );
        let per_event_ec001s = result
            .findings
            .iter()
            .filter(|f| {
                f.code == FindingCode::Ec001
                    && matches!(
                        &f.details,
                        FindingDetails::UnmodeledElements {
                            truncated_remaining: None,
                            ..
                        }
                    )
            })
            .count();
        let summary_ec001s = result
            .findings
            .iter()
            .filter(|f| {
                matches!(
                    &f.details,
                    FindingDetails::UnmodeledElements {
                        truncated_remaining: Some(_),
                        ..
                    }
                )
            })
            .count();
        assert_eq!(per_event_ec001s, 1000, "per-event findings capped at 1000");
        assert_eq!(summary_ec001s, 1, "one summary finding emitted");
        // Confirm the summary message includes the truncated count.
        let summary = result
            .findings
            .iter()
            .find(|f| {
                matches!(
                    &f.details,
                    FindingDetails::UnmodeledElements {
                        truncated_remaining: Some(_),
                        ..
                    }
                )
            })
            .unwrap();
        match &summary.details {
            FindingDetails::UnmodeledElements {
                truncated_remaining: Some(n),
                ..
            } => assert_eq!(*n, 5),
            _ => unreachable!(),
        }
    }

    #[test]
    fn ec001_max_findings_zero_disables_cap() {
        let mut xml = String::from("<events>\n");
        for i in 0..1500 {
            xml.push_str(&format!(
                "  <event>\n    <uei>uei.test/{i}</uei>\n    <event-label>L{i}</event-label>\n    \
                 <severity>Warning</severity>\n    <autoaction state=\"off\">x()</autoaction>\n  </event>\n"
            ));
        }
        xml.push_str("</events>\n");
        let opts = ConvertOpts {
            max_findings: Some(0),
            ..ConvertOpts::default()
        };
        let result = convert(xml.as_bytes(), Path::new("/tmp/big.events.xml"), &opts);
        let per_event_ec001s = result
            .findings
            .iter()
            .filter(|f| {
                f.code == FindingCode::Ec001
                    && matches!(
                        &f.details,
                        FindingDetails::UnmodeledElements {
                            truncated_remaining: None,
                            ..
                        }
                    )
            })
            .count();
        let summary_ec001s = result
            .findings
            .iter()
            .filter(|f| {
                matches!(
                    &f.details,
                    FindingDetails::UnmodeledElements {
                        truncated_remaining: Some(_),
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            per_event_ec001s, 1500,
            "all events reported when cap disabled"
        );
        assert_eq!(summary_ec001s, 0, "no summary needed when cap disabled");
    }

    #[test]
    fn explain_ec001_mentions_key_terms() {
        let text = explain(FindingCode::Ec001);
        // Per spec scenario: --explain EC001 mentions "unmodeled",
        // "structural", and pointer to `event-source upload`.
        assert!(
            text.contains("unmodeled"),
            "EC001 explainer must say 'unmodeled'"
        );
        assert!(
            text.contains("structural") || text.contains("STRUCTURAL"),
            "EC001 explainer must describe its structural-only scope"
        );
        assert!(
            text.contains("event-source upload"),
            "EC001 explainer must point to the event-source upload fallback"
        );
    }
}
