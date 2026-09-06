/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local YAML schema for `EventSource` (the user-authored shape).
//!
//! Mirrors `design.md §5.1` modulo a few simplifications:
//!
//!   - Common fields modeled (uei, label, severity, description, logmsg,
//!     mask, alarmData, operinstruct, mouseovertext, autoacknowledge,
//!     tticket, correlation, varbindsdecode).
//!   - Mask varbinds support both `vbnumber` (positional, 1-indexed) and
//!     `vboid` (OID-based) discriminators; exactly one is required per
//!     entry.
//!   - Rare elements (parameter, forward, script, snmp) are NOT modeled
//!     yet. Tracked as deferred work.
//!
//! User-friendly field names are preserved (`label` not `eventLabel`,
//! `text` not `content`, `name`/`values` not `mename`/`mevalues`).
//! Conversion to the wire-format `Event` DTO lives in
//! [`crate::apply::conversion`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use onmsctl_core::kind::envelope::parse_documents;
use onmsctl_core::{Error, Result};

// -- Top-level shape --------------------------------------------------------

/// Kind literal for EventSource documents — the kind-router discriminator.
pub const KIND: &str = "EventSource";

/// The kubectl-style document the user authors. Validated at load time.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventSourceLocal {
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: EventSourceSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventSourceSpec {
    /// Defaults to `true` when absent.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub events: Vec<EventDef>,
}

fn default_enabled() -> bool {
    true
}

// -- Event-level shape ------------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventDef {
    pub uei: String,
    pub label: String,
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logmsg: Option<LogmsgDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alarm_data: Option<AlarmDataDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<MaskDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operinstruct: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouseovertext: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoacknowledge: Option<AutoackDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tticket: Option<TticketDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation: Option<CorrelationDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub varbindsdecode: Option<Vec<VarbindsdecodeDef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snmp: Option<SnmpDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<ParameterDef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwards: Option<Vec<ForwardDef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scripts: Option<Vec<ScriptDef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<FilterDef>>,
}

/// eventd regex-replacement filter on a named event parameter.
/// Mirrors the upstream `<filter eventparm="..." pattern="..."
/// replacement="..."/>` — EMPTY element with three REQUIRED
/// attributes per JAXB.
///
/// At event-fire time eventd compiles `pattern` as a Java regex and
/// applies `Matcher.replaceAll(replacement)` to the value of the
/// event parameter named by `eventparm`. NOT a suppression filter —
/// a value-rewrite rule.
///
/// `pattern` uses Java regex syntax (`java.util.regex.Pattern`). We
/// do NOT validate the syntax locally because Rust's `regex` crate
/// has subtly different feature support (no lookahead/lookbehind).
/// Bad patterns surface at event-fire time as eventd warnings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilterDef {
    /// Event-parameter name the filter targets. Required.
    pub eventparm: String,
    /// Java regex matched against the parameter's value. Required.
    pub pattern: String,
    /// Replacement string for `Matcher.replaceAll`. Required —
    /// `$1`/`$2`/... backreferences supported per Java regex.
    pub replacement: String,
}

/// eventd forwarding directive. Mirrors the eventconf XSD's
/// `<forward state="..." mechanism="...">target</forward>`. The XSD
/// constrains `state` to `{on, off}` and `mechanism` to a closed set
/// (`snmpudp`, `snmptcp`, `xmltcp`, `xmludp`) — the local validator
/// matches the XSD so YAML caught locally instead of producing a
/// 400 from Horizon's upload endpoint. Every field is still
/// individually optional (XSD attributes default).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForwardDef {
    /// `on` or `off`. Validated against the XSD-closed set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// One of `snmpudp` / `snmptcp` / `xmltcp` / `xmludp` per the
    /// eventconf XSD. Validated against the XSD-closed set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mechanism: Option<String>,
    /// Destination identifier (e.g. `alarmcentral:162` for SNMP UDP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// Accepted `forward.mechanism` values per the eventconf XSD pattern.
const FORWARD_MECHANISMS: &[&str] = &["snmpudp", "snmptcp", "xmltcp", "xmludp"];

/// Embedded executable logic. Mirrors the eventconf XSD's
/// `<script language="beanshell">body</script>`. eventd runs the body
/// at event-fire time.
///
/// **Security note.** Modeling this element in YAML makes it trivial
/// to ship server-side code via `apply`. The threat surface
/// already exists at the eventconf-XML upload path; this change does
/// not introduce new authority. Operators should ensure RBAC on
/// eventconf write access is appropriately scoped.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptDef {
    /// Script language. The eventconf XSD declares this attribute
    /// REQUIRED (`use="required"`), so the local schema makes it
    /// required too. Typical values: `beanshell`, `groovy`. Free
    /// string in the value space (XSD is `xs:string`).
    pub language: String,
    /// Script source. Multi-line content via YAML `|` literal block.
    /// Preserved byte-for-byte through round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// Static per-event configuration parameter. Mirrors the eventconf XSD's
/// `<parameter name="..." value="..." expand="..."/>`. eventd evaluates
/// entries in document order at event-fire time.
///
/// Operator vocabulary note: this is the *static* per-event parameter
/// list, distinct from the *runtime* `parmCollection` carried on fired
/// events (which the wire DTO has as [`crate::dto::Event::parm_collection`]
/// and which this YAML schema deliberately doesn't model — it's a
/// runtime-only concern that doesn't round-trip through eventconf XML).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterDef {
    /// Parameter name. Required; rejected if empty or whitespace-only.
    pub name: String,
    /// Parameter value. Required; rejected if empty or whitespace-only.
    /// May contain `%parm[#N]%`-style placeholders that eventd expands
    /// when `expand: true`.
    pub value: String,
    /// Whether eventd should expand `%parm[#N]%` placeholders in `value`
    /// at event-fire time. Absent on the wire when not set in the source
    /// YAML — the absent vs explicit `true`/`false` distinction
    /// round-trips through `apply` / `event-source download`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand: Option<bool>,
}

/// SNMP trap discriminator + metadata on an event. Mirrors the
/// eventconf XSD's `<snmp>` element. Every field is optional; the
/// XSD does not require any particular combination. Real vendor MIBs
/// often use only `id`/`generic`/`specific` and omit the rest.
///
/// Documented practical ranges (NOT enforced, per design — values
/// outside these ranges round-trip verbatim through the wire/XML
/// layer because future SNMP semantics or vendor extensions may
/// legitimately use them):
///   - `generic`: `0..=6` per RFC 1157.
///   - `specific`: `>= 0`.
///   - `version`: typically `v1`, `v2c`, or `v3`. Free string —
///     `v3-auth-priv` and other Horizon-version-specific variants
///     are accepted verbatim.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnmpDef {
    /// Enterprise OID (e.g. `.1.3.6.1.4.1.9.1.13`). XSD types this
    /// as `xs:string`; no OID-format validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Textual variant of the id (vendor-supplied human label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idtext: Option<String>,
    /// SNMP protocol version. Free string; common values are `v1`,
    /// `v2c`, `v3`. Not enum-validated for forward-compat with
    /// Horizon-version-specific variants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// SNMP specific-trap number. Per RFC 1157 this is `>= 0`;
    /// out-of-range values round-trip verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specific: Option<i32>,
    /// SNMP generic-trap number. Per RFC 1157 this is `0..=6`;
    /// out-of-range values round-trip verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic: Option<i32>,
    /// SNMP community string (typically `public`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub community: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogmsgDef {
    pub dest: String,
    /// Human-readable substitution template. Maps to `content` on the wire.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
}

/// Alarm-type discriminator on an event's `alarmData` block. Strictly
/// accepts only the three known states — anything else fails at parse
/// or wire→local conversion. Accepts both integer (`1` / `2` / `3`)
/// and symbolic-string (`raise` / `resolution` / `unresolvable`) forms
/// in YAML; symbolic input is case-insensitive.
///
/// Vocabulary: the Horizon Web UI displays `raise` (1), `resolution`
/// (2), `unresolvable` (3). The alarmd Java code uses `Problem` for
/// state 1; this YAML schema follows the Web UI vocabulary so
/// operators see the same term across surfaces.
///
/// Whitespace note: the deserializer does NOT trim. However, YAML's
/// own scalar handling strips leading/trailing whitespace from
/// *unquoted* plain scalars before the deserializer sees them — so
/// `alarmType: raise ` (unquoted, trailing space) parses successfully.
/// `alarmType: "raise "` (quoted) reaches the deserializer with the
/// space and is rejected as unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmType {
    /// State 1 — Raise: raises an alarm, paired with a Resolution event.
    /// Also known as `Problem` in the alarmd Java code; the YAML uses
    /// `raise` to match the Horizon Web UI display.
    Raise,
    /// State 2 — Resolution: clears a paired Raise alarm by reductionKey.
    Resolution,
    /// State 3 — Unresolvable: a Raise-class event with no auto-clear
    /// from the device. Common for hardware-failure traps. Requires
    /// manual close or an alarmd cleanup policy.
    Unresolvable,
}

impl AlarmType {
    /// Project to the wire-format integer representation Horizon expects.
    pub fn to_wire(self) -> i32 {
        match self {
            Self::Raise => 1,
            Self::Resolution => 2,
            Self::Unresolvable => 3,
        }
    }

    /// Build from the wire integer. Returns `None` for any integer
    /// outside `{1, 2, 3}` — the caller is expected to surface that
    /// to operators (e.g. `from_wire.rs` returns
    /// `WireToLocalError::AlarmDataAlarmTypeOutOfRange` so
    /// `event-source convert` emits `EC007` against it).
    pub fn from_wire(n: i32) -> Option<Self> {
        match n {
            1 => Some(Self::Raise),
            2 => Some(Self::Resolution),
            3 => Some(Self::Unresolvable),
            _ => None,
        }
    }
}

impl Serialize for AlarmType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Raise => s.serialize_str("raise"),
            Self::Resolution => s.serialize_str("resolution"),
            Self::Unresolvable => s.serialize_str("unresolvable"),
        }
    }
}

impl<'de> Deserialize<'de> for AlarmType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = AlarmType;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "alarmType: integer 1, 2, or 3, or symbolic string \
                     'raise', 'resolution', or 'unresolvable' (case-insensitive)",
                )
            }

            fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<AlarmType, E> {
                let narrowed = i32::try_from(n).map_err(|_| {
                    E::custom(format!(
                        "alarmType {n} is outside i32 range; expected 1, 2, or 3"
                    ))
                })?;
                AlarmType::from_wire(narrowed).ok_or_else(|| {
                    E::custom(format!(
                        "alarmType {narrowed} is not in the accepted set; \
                         expected 1 (raise), 2 (resolution), or 3 (unresolvable)"
                    ))
                })
            }

            fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<AlarmType, E> {
                let narrowed = i32::try_from(n).map_err(|_| {
                    E::custom(format!(
                        "alarmType {n} is outside i32 range; expected 1, 2, or 3"
                    ))
                })?;
                AlarmType::from_wire(narrowed).ok_or_else(|| {
                    E::custom(format!(
                        "alarmType {narrowed} is not in the accepted set; \
                         expected 1 (raise), 2 (resolution), or 3 (unresolvable)"
                    ))
                })
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<AlarmType, E> {
                // Case-insensitive ASCII matching. Whitespace is NOT
                // trimmed — leading/trailing spaces in *quoted* YAML
                // strings fail as unknown; unquoted plain scalars are
                // already trimmed by the YAML parser before reaching us.
                match s.to_ascii_lowercase().as_str() {
                    "raise" => Ok(AlarmType::Raise),
                    "resolution" => Ok(AlarmType::Resolution),
                    "unresolvable" => Ok(AlarmType::Unresolvable),
                    other => Err(E::custom(format!(
                        "unknown alarmType {other:?}; expected one of \
                         'raise', 'resolution', 'unresolvable', or integer 1, 2, 3"
                    ))),
                }
            }

            fn visit_string<E: serde::de::Error>(self, s: String) -> Result<AlarmType, E> {
                self.visit_str(&s)
            }

            // Friendly diagnostics for YAML inputs the visitor doesn't
            // accept. Without these, serde's stock errors don't mention
            // alarmType at all.
            fn visit_bool<E: serde::de::Error>(self, b: bool) -> Result<AlarmType, E> {
                Err(E::custom(format!(
                    "alarmType: expected integer or symbolic string, got boolean {b}"
                )))
            }

            fn visit_f64<E: serde::de::Error>(self, f: f64) -> Result<AlarmType, E> {
                Err(E::custom(format!(
                    "alarmType: expected integer 1, 2, or 3 or symbolic string, \
                     got float {f} (whole-number floats like 1.0 are not accepted)"
                )))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<AlarmType, E> {
                Err(E::custom(
                    "alarmType: expected integer or symbolic string, got null",
                ))
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<AlarmType, E> {
                Err(E::custom(
                    "alarmType: expected integer or symbolic string, got null/absent",
                ))
            }
        }
        d.deserialize_any(V)
    }
}

impl JsonSchema for AlarmType {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "AlarmType".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // oneOf: symbolic string (preferred, first so editors offer it
        // as the default autocomplete) OR integer constrained to {1,2,3}.
        schemars::json_schema!({
            "oneOf": [
                {
                    "type": "string",
                    "enum": ["raise", "resolution", "unresolvable"]
                },
                {
                    "type": "integer",
                    "enum": [1, 2, 3]
                }
            ],
            "description": "Alarm-type discriminator. Strictly accepts the three known states — 'raise' (1), 'resolution' (2), or 'unresolvable' (3). Symbolic input is case-insensitive."
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlarmDataDef {
    pub reduction_key: String,
    /// Alarm semantic. Strictly one of:
    ///   - `1` / `"raise"` — raises an alarm, paired with a Resolution
    ///     event. (Known as `Problem` in the alarmd Java code.)
    ///   - `2` / `"resolution"` — clears a paired Raise by `reductionKey`.
    ///   - `3` / `"unresolvable"` — Raise-class with no auto-clear from
    ///     the device. Common for hardware-failure traps.
    ///
    /// Anything else fails at parse (for YAML inputs) or surfaces as
    /// `EC007` during `event-source convert` (for downloaded XML that uses
    /// an unrecognized integer).
    pub alarm_type: AlarmType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_clean: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_key: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaskDef {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<MaskElementDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub varbinds: Vec<MaskVarbindDef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaskElementDef {
    /// Element name. Maps to `mename` on the wire.
    pub name: String,
    /// Match values. Maps to `mevalues` on the wire.
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaskVarbindDef {
    /// Match the SNMP trap PDU's varbind by 1-indexed position. Mutually
    /// exclusive with [`vboid`]; the validator enforces exactly one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vbnumber: Option<i32>,
    /// Match the SNMP trap PDU's varbind by OID (e.g. `.1.3.6.1.4.1.61509.1.2.0`).
    /// Mutually exclusive with [`vbnumber`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vboid: Option<String>,
    /// Match values. Maps to `vbvalues` on the wire.
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VarbindsdecodeDef {
    /// Identifies which event parameter this decode group annotates.
    /// String-typed per the eventconf XSD; typical values are numeric.
    pub parmid: String,
    /// Value→label mappings rendered in the Horizon event UI.
    pub decode: Vec<DecodeDef>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecodeDef {
    /// The raw varbind value being annotated. Maps to `varbindvalue` on
    /// the wire (an XSD attribute on `<decode>`).
    pub value: String,
    /// The human-readable label. Maps to `varbinddecodedstring` on the wire.
    pub label: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutoackDef {
    /// "on" or "off".
    pub state: String,
    /// Maps to `content` on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TticketDef {
    /// "on" or "off".
    pub state: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorrelationDef {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmax: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cuei: Vec<String>,
}

// -- Loader -----------------------------------------------------------------

impl EventSourceLocal {
    /// Parse YAML/JSON bytes and validate the result. Errors propagate as
    /// `Error::Config` with a single user-actionable message.
    ///
    /// On strict-parse failure, attempts a recovery pass that gives
    /// guided messages for the most common spec-forbidden keys
    /// (`spec.fileOrder`, `spec.vendor`, `spec.description`) instead of
    /// the bare serde "unknown field" diagnostic.
    pub fn from_yaml(bytes: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| Error::Config(format!("invalid EventSource YAML: not UTF-8: {e}")))?;

        // One parse, through the same splitter `apply -f` uses. It skips
        // null documents (a leading or trailing `---`, a comment-only
        // document) and stops at the first malformed one, so a broken
        // file cannot hang here (the `event_source_from_yaml` fuzz target
        // found a `.count()` over the document iterator that never
        // returned). `serde_norway::from_slice` rejects a second document
        // outright; splitting first lets us say something more useful.
        let mut docs = parse_documents("EventSource", text)?;
        let doc = match docs.len() {
            0 => {
                return Err(Error::Config(
                    "invalid EventSource YAML: the input holds no document".into(),
                ));
            }
            1 => docs.remove(0),
            _ => {
                return Err(Error::Config(
                    "multi-document YAML is not supported (one EventSource per file). \
                     Split documents into separate files."
                        .into(),
                ));
            }
        };

        // Re-serialize the one document and parse the text, not the
        // `Value`: `from_value` types plain scalars first, so `name: 12345`
        // would fail with "expected a string" where the text parser reads
        // it as the string it was in the file.
        let single = serde_norway::to_string(&doc.value).map_err(|e| {
            Error::Config(format!(
                "invalid EventSource YAML: could not re-serialize document: {e}"
            ))
        })?;
        match serde_norway::from_str::<Self>(&single) {
            Ok(local) => {
                local.validate()?;
                Ok(local)
            }
            Err(strict_err) => {
                // Strict parse failed. Try a recovery pass to produce a
                // guided message for the well-known reserved spec.* keys.
                if let Some(guided) = guided_rejection_for_known_spec_keys(&doc.value) {
                    return Err(guided);
                }
                Err(Error::Config(format!(
                    "invalid EventSource YAML: {strict_err}"
                )))
            }
        }
    }
}

/// Inspect a parsed YAML document for keys under `spec` that are
/// explicitly forbidden by the schema. Emits guided error messages per the
/// `event-conf` spec scenarios.
fn guided_rejection_for_known_spec_keys(raw: &serde_norway::Value) -> Option<Error> {
    let spec = raw.get("spec")?.as_mapping()?;
    for (key, _) in spec {
        if let Some(k) = key.as_str() {
            match k {
                "fileOrder" => {
                    return Some(Error::Config(
                        "spec.fileOrder is not declarative in v0.1; ordering is server-managed. \
                         Declarative ordering moves to a future `kind: EventConfMaster` resource."
                            .into(),
                    ));
                }
                "vendor" => {
                    return Some(Error::Config(
                        "spec.vendor is not allowed; vendor is server-derived from the prefix \
                         of `metadata.name` before the first '.'. Choose `metadata.name` accordingly."
                            .into(),
                    ));
                }
                "description" => {
                    return Some(Error::Config(
                        "spec.description cannot be set or preserved through `apply` \
                         (the upload endpoint blanks it). Set it out-of-band via the \
                         Horizon REST API or web UI."
                            .into(),
                    ));
                }
                _ => {}
            }
        }
    }
    None
}

// -- Validation -------------------------------------------------------------

const RESERVED_NAMES: &[&str] = &["eventconf", "opennms.catch-all.events"];

const SEVERITIES: &[&str] = &[
    "Indeterminate",
    "Cleared",
    "Normal",
    "Warning",
    "Minor",
    "Major",
    "Critical",
];

const VALID_NAME_CHARS: fn(char) -> bool =
    |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');

impl EventSourceLocal {
    /// Validate every aspect of the document. Each error message names
    /// the offending field path so the user can fix it without guessing.
    pub fn validate(&self) -> Result<()> {
        // -- API surface ----
        if self.api_version != "eventconf.opennms.org/v1" {
            return Err(Error::Config(format!(
                "apiVersion must be 'eventconf.opennms.org/v1', got '{}'",
                self.api_version
            )));
        }
        if self.kind != "EventSource" {
            return Err(Error::Config(format!(
                "kind must be 'EventSource', got '{}'",
                self.kind
            )));
        }

        // -- metadata.name ----
        validate_name(&self.metadata.name)?;

        // -- spec.events ----
        if self.spec.events.is_empty() {
            return Err(Error::Config(
                "spec.events is empty; refusing to apply an EventSource with no events. \
                 To remove a source, use `onmsctl event-source delete <id>`."
                    .into(),
            ));
        }
        for (i, e) in self.spec.events.iter().enumerate() {
            e.validate(i)?;
        }
        // Duplicate UEIs across events are explicitly permitted —
        // they are a first-class OpenNMS normalization pattern (the
        // mask conditions are the runtime discriminators; the shared
        // UEI is the operator-visible event class). See the archived
        // `permit-duplicate-ueis-as-normalization-pattern` change.
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::Config("metadata.name is empty".into()));
    }
    if name.len() > 256 {
        return Err(Error::Config(format!(
            "metadata.name '{name}' exceeds 256 chars (got {})",
            name.len()
        )));
    }
    if !name.chars().all(VALID_NAME_CHARS) {
        return Err(Error::Config(format!(
            "metadata.name '{name}' contains invalid characters; only ASCII letters, digits, '.', '-', '_' allowed"
        )));
    }
    if name.starts_with('.') {
        return Err(Error::Config(format!(
            "metadata.name '{name}' must not start with a dot"
        )));
    }
    if name.ends_with('.') {
        return Err(Error::Config(format!(
            "metadata.name '{name}' must not end with a dot"
        )));
    }
    if name.contains("..") {
        return Err(Error::Config(format!(
            "metadata.name '{name}' must not contain consecutive dots"
        )));
    }
    // Reserved-name check runs BEFORE the dot-required check because at
    // least one reserved name (`eventconf`) does not contain a dot; the
    // user deserves the more specific error. Case-insensitive to defend
    // against `Eventconf` / `OPENNMS.CATCH-ALL.EVENTS` style bypass.
    if RESERVED_NAMES.iter().any(|r| r.eq_ignore_ascii_case(name)) {
        return Err(Error::Config(format!(
            "metadata.name '{name}' is reserved by OpenNMS"
        )));
    }
    // Vendor derivation matches Horizon's server-side
    // `StringUtils.substringBefore(name, ".")`: when no '.' is present,
    // the whole name becomes the vendor. So `Cisco` → vendor `Cisco`;
    // `cisco.foo` → vendor `cisco`. Empty-vendor still rejected (e.g.
    // `.foo` would be caught by the earlier starts_with('.') check).
    let vendor = match name.split_once('.') {
        Some((prefix, _)) => prefix,
        None => name,
    };
    if vendor.is_empty() {
        return Err(Error::Config(format!(
            "metadata.name '{name}' has empty vendor segment"
        )));
    }
    if vendor.len() > 128 {
        return Err(Error::Config(format!(
            "metadata.name '{name}' vendor segment '{vendor}' exceeds 128 chars"
        )));
    }
    Ok(())
}

impl EventDef {
    fn validate(&self, idx: usize) -> Result<()> {
        if self.uei.is_empty() {
            return Err(Error::Config(format!("spec.events[{idx}].uei is empty")));
        }
        if !self.uei.starts_with("uei.") {
            return Err(Error::Config(format!(
                "spec.events[{idx}].uei '{}' must start with 'uei.'",
                self.uei
            )));
        }
        if self.uei.len() <= 4 {
            return Err(Error::Config(format!(
                "spec.events[{idx}].uei '{}' must have content after the 'uei.' prefix",
                self.uei
            )));
        }
        if self.label.trim().is_empty() {
            return Err(Error::Config(format!(
                "spec.events[{idx}].label is empty or whitespace-only"
            )));
        }
        if !SEVERITIES.contains(&self.severity.as_str()) {
            return Err(Error::Config(format!(
                "spec.events[{idx}].severity '{}' is not a valid OpenNMS severity (expected one of: {})",
                self.severity,
                SEVERITIES.join(", ")
            )));
        }
        if let Some(a) = &self.alarm_data {
            a.validate(idx)?;
        }
        if let Some(m) = &self.mask {
            m.validate(idx)?;
        }
        if let Some(a) = &self.autoacknowledge {
            validate_state(
                &a.state,
                &format!("spec.events[{idx}].autoacknowledge.state"),
            )?;
        }
        if let Some(t) = &self.tticket {
            validate_state(&t.state, &format!("spec.events[{idx}].tticket.state"))?;
        }
        if let Some(c) = &self.correlation {
            validate_state(&c.state, &format!("spec.events[{idx}].correlation.state"))?;
        }
        if let Some(groups) = &self.varbindsdecode {
            for (k, g) in groups.iter().enumerate() {
                g.validate(idx, k)?;
            }
            // D6: duplicate parmid values within an event's varbindsdecode
            // produce ambiguous server-side behavior; reject at parse time.
            let mut seen: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for (k, g) in groups.iter().enumerate() {
                if let Some(prev_k) = seen.insert(g.parmid.as_str(), k) {
                    return Err(Error::Config(format!(
                        "spec.events[{idx}].varbindsdecode entries [{prev_k}] and [{k}] both declare parmid='{}'; parmid values must be unique within an event",
                        g.parmid
                    )));
                }
            }
        }
        if let Some(s) = &self.snmp {
            s.validate(idx)?;
        }
        if let Some(params) = &self.parameters {
            for (k, p) in params.iter().enumerate() {
                p.validate(idx, k)?;
            }
        }
        if let Some(forwards) = &self.forwards {
            for (k, f) in forwards.iter().enumerate() {
                f.validate(idx, k)?;
            }
        }
        if let Some(scripts) = &self.scripts {
            for (k, s) in scripts.iter().enumerate() {
                s.validate(idx, k)?;
            }
        }
        if let Some(filters) = &self.filters {
            for (k, f) in filters.iter().enumerate() {
                f.validate(idx, k)?;
            }
        }
        Ok(())
    }
}

impl FilterDef {
    /// All three fields are XSD-required. The struct type guarantees
    /// non-Option. Validation:
    ///
    ///   - `eventparm` and `pattern` must be non-empty AND non-trimmed
    ///     (no leading/trailing whitespace). Stored values flow
    ///     verbatim to eventd's filter-map key — whitespace padding
    ///     would silently break match lookups at runtime.
    ///   - `replacement` is XSD-required (presence-only); the empty
    ///     string is legitimate (`Matcher.replaceAll("")` strips the
    ///     match). No empty / whitespace check.
    fn validate(&self, event_idx: usize, filter_idx: usize) -> Result<()> {
        let check_no_padding = |field: &str, value: &str| -> Result<()> {
            if value.is_empty() {
                return Err(Error::Config(format!(
                    "spec.events[{event_idx}].filters[{filter_idx}].{field} is empty"
                )));
            }
            if value != value.trim() {
                return Err(Error::Config(format!(
                    "spec.events[{event_idx}].filters[{filter_idx}].{field} has \
                     leading or trailing whitespace; eventd's filter-map key uses \
                     the raw value, so padding would silently prevent matches"
                )));
            }
            Ok(())
        };
        check_no_padding("eventparm", &self.eventparm)?;
        check_no_padding("pattern", &self.pattern)?;
        Ok(())
    }
}

impl ForwardDef {
    /// Validate against the eventconf XSD's closed sets for `state` and
    /// `mechanism`. Reject all-None entries (an empty `<forward/>` is
    /// XSD-legal but operationally meaningless).
    fn validate(&self, event_idx: usize, fwd_idx: usize) -> Result<()> {
        if self.state.is_none() && self.mechanism.is_none() && self.target.is_none() {
            return Err(Error::Config(format!(
                "spec.events[{event_idx}].forwards[{fwd_idx}] is empty; at least one of \
                 state, mechanism, or target must be set"
            )));
        }
        if let Some(s) = &self.state {
            validate_state(
                s,
                &format!("spec.events[{event_idx}].forwards[{fwd_idx}].state"),
            )?;
        }
        if let Some(m) = &self.mechanism
            && !FORWARD_MECHANISMS.contains(&m.as_str())
        {
            return Err(Error::Config(format!(
                "spec.events[{event_idx}].forwards[{fwd_idx}].mechanism '{m}' is not in the \
                 XSD-accepted set ({}); Horizon will reject the upload at validation time",
                FORWARD_MECHANISMS.join(", ")
            )));
        }
        if let Some(v) = &self.target
            && v.trim().is_empty()
        {
            return Err(Error::Config(format!(
                "spec.events[{event_idx}].forwards[{fwd_idx}].target is empty or \
                 whitespace-only; omit the field or provide a value"
            )));
        }
        Ok(())
    }
}

impl ScriptDef {
    /// `language` is required by the XSD and the struct type guarantees
    /// it is `String` (not `Option`), but we still reject explicit empty
    /// or whitespace-only values to catch typos. `body` may contain
    /// leading/trailing whitespace (script content is preserved
    /// byte-for-byte) so it is NOT trim-checked.
    fn validate(&self, event_idx: usize, script_idx: usize) -> Result<()> {
        if self.language.trim().is_empty() {
            return Err(Error::Config(format!(
                "spec.events[{event_idx}].scripts[{script_idx}].language is empty or \
                 whitespace-only; required by the eventconf XSD"
            )));
        }
        Ok(())
    }
}

impl ParameterDef {
    fn validate(&self, event_idx: usize, param_idx: usize) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::Config(format!(
                "spec.events[{event_idx}].parameters[{param_idx}].name is empty"
            )));
        }
        if self.value.trim().is_empty() {
            return Err(Error::Config(format!(
                "spec.events[{event_idx}].parameters[{param_idx}].value is empty"
            )));
        }
        Ok(())
    }
}

impl SnmpDef {
    /// Reject explicit empty or whitespace-only strings on `Some(...)`
    /// fields — they're structurally legal per XSD but operationally
    /// meaningless and usually indicate a typo or template-engine
    /// misfire. Mirrors the `trim().is_empty()` policy used by sibling
    /// validators (`EventDef::label`, `MaskDef::elements[].name`,
    /// `VarbindsdecodeDef::parmid`).
    ///
    /// Numeric fields are NOT range-checked here; the doc-comment
    /// ranges are guidance, not validation.
    fn validate(&self, idx: usize) -> Result<()> {
        let check_str = |field: &str, value: &Option<String>| -> Result<()> {
            if let Some(v) = value
                && v.trim().is_empty()
            {
                return Err(Error::Config(format!(
                    "spec.events[{idx}].snmp.{field} is set but empty or whitespace-only; \
                     omit the field or provide a value"
                )));
            }
            Ok(())
        };
        check_str("id", &self.id)?;
        check_str("idtext", &self.idtext)?;
        check_str("version", &self.version)?;
        check_str("community", &self.community)?;
        Ok(())
    }
}

impl AlarmDataDef {
    fn validate(&self, idx: usize) -> Result<()> {
        if self.reduction_key.is_empty() {
            return Err(Error::Config(format!(
                "spec.events[{idx}].alarmData.reductionKey is empty"
            )));
        }
        // Range validation: unknown integers (`AlarmType::Other(n)`) are
        // permitted at the validator level. They round-trip verbatim and
        // surface as `EC007` during `event-source convert` rather than as a
        // blocking validation error here. The deserializer already
        // rejects unknown symbolic strings (typos) up front.
        Ok(())
    }
}

impl MaskDef {
    fn validate(&self, idx: usize) -> Result<()> {
        for (j, el) in self.elements.iter().enumerate() {
            if el.name.is_empty() {
                return Err(Error::Config(format!(
                    "spec.events[{idx}].mask.elements[{j}].name is empty"
                )));
            }
        }
        for (j, vb) in self.varbinds.iter().enumerate() {
            match (&vb.vbnumber, &vb.vboid) {
                (None, None) => {
                    return Err(Error::Config(format!(
                        "spec.events[{idx}].mask.varbinds[{j}] declares neither 'vbnumber' nor 'vboid'; exactly one is required"
                    )));
                }
                (Some(_), Some(_)) => {
                    return Err(Error::Config(format!(
                        "spec.events[{idx}].mask.varbinds[{j}] declares both 'vbnumber' and 'vboid'; they are mutually exclusive"
                    )));
                }
                (Some(n), None) => {
                    if *n <= 0 {
                        return Err(Error::Config(format!(
                            "spec.events[{idx}].mask.varbinds[{j}].vbnumber must be positive; got {n}"
                        )));
                    }
                }
                (None, Some(oid)) => {
                    if oid.trim().is_empty() {
                        return Err(Error::Config(format!(
                            "spec.events[{idx}].mask.varbinds[{j}].vboid is empty or whitespace-only"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

impl VarbindsdecodeDef {
    fn validate(&self, event_idx: usize, group_idx: usize) -> Result<()> {
        if self.parmid.trim().is_empty() {
            return Err(Error::Config(format!(
                "spec.events[{event_idx}].varbindsdecode[{group_idx}].parmid is empty or whitespace-only"
            )));
        }
        if self.decode.is_empty() {
            return Err(Error::Config(format!(
                "spec.events[{event_idx}].varbindsdecode[{group_idx}].decode is empty; declare at least one decode entry or remove the group"
            )));
        }
        for (k, d) in self.decode.iter().enumerate() {
            if d.value.is_empty() {
                return Err(Error::Config(format!(
                    "spec.events[{event_idx}].varbindsdecode[{group_idx}].decode[{k}].value is empty"
                )));
            }
            if d.label.is_empty() {
                return Err(Error::Config(format!(
                    "spec.events[{event_idx}].varbindsdecode[{group_idx}].decode[{k}].label is empty"
                )));
            }
        }
        Ok(())
    }
}

fn validate_state(state: &str, path: &str) -> Result<()> {
    if state != "on" && state != "off" {
        return Err(Error::Config(format!(
            "{path} must be 'on' or 'off'; got '{state}'"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_yaml() -> &'static str {
        r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/cisco/foo/coldStart
      label: "Cisco Foo Cold Start"
      severity: Warning
"#
    }

    #[test]
    fn parses_minimal_document() {
        let local = EventSourceLocal::from_yaml(minimal_yaml().as_bytes()).unwrap();
        assert_eq!(local.metadata.name, "cisco.foo");
        assert_eq!(local.spec.events.len(), 1);
        assert!(local.spec.enabled, "enabled defaults to true");
    }

    #[test]
    fn rejects_wrong_api_version() {
        let yaml = minimal_yaml().replace("eventconf.opennms.org/v1", "v2alpha1");
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("apiVersion")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_wrong_kind() {
        let yaml = minimal_yaml().replace("EventSource", "Pod");
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("kind")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn from_yaml_accepts_a_trailing_document_separator() {
        let yaml = format!("{}\n---\n", minimal_yaml());
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        assert_eq!(local.metadata.name, "cisco.foo");
    }

    #[test]
    fn null_documents_do_not_make_a_file_multi_document() {
        // Explicit start marker, comment-only document, leading blank docs.
        for yaml in [
            format!("---\n{}", minimal_yaml()),
            format!("{}\n---\n# nothing here\n", minimal_yaml()),
            format!("---\n---\n{}", minimal_yaml()),
        ] {
            let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
            assert_eq!(local.metadata.name, "cisco.foo", "input: {yaml}");
        }
    }

    #[test]
    fn two_documents_are_rejected_with_guidance() {
        let yaml = format!("{}\n---\n{}", minimal_yaml(), minimal_yaml());
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("multi-document"), "msg: {m}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_input_is_an_error() {
        for input in ["", "# just a comment\n", "---\n"] {
            let err = EventSourceLocal::from_yaml(input.as_bytes()).unwrap_err();
            match err {
                Error::Config(m) => assert!(m.contains("no document"), "msg: {m}"),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn plain_scalars_still_read_as_strings() {
        // `from_value` would type `12345` as an integer first and reject
        // it for a String field; the text parser keeps the file's reading.
        let yaml = minimal_yaml()
            .replace("name: cisco.foo", "name: 12345")
            .replace(r#"label: "Cisco Foo Cold Start""#, "label: 2024");
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        assert_eq!(local.metadata.name, "12345");
        assert_eq!(local.spec.events[0].label, "2024");
    }

    #[test]
    fn document_separator_inside_a_block_scalar_is_not_a_boundary() {
        let yaml = format!(
            "{}      description: |\n        first line\n        ---\n        still the same document\n",
            minimal_yaml()
        );
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        assert_eq!(local.metadata.name, "cisco.foo");
    }

    #[test]
    fn non_utf8_input_is_an_error() {
        let err = EventSourceLocal::from_yaml(&[0x6b, 0x69, 0x6e, 0x64, 0x3a, 0xff]).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("not UTF-8"), "msg: {m}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn malformed_yaml_is_an_error_not_a_hang() {
        // Found by fuzz/fuzz_targets/event_source_from_yaml: a `.count()`
        // over the serde_norway document iterator never returned on
        // malformed input. The core splitter stops at the first error.
        for input in ["spec: [1, 2\n", "spec: {}\n---\nspec: [1, 2\n"] {
            let err = EventSourceLocal::from_yaml(input.as_bytes()).unwrap_err();
            match err {
                Error::Config(m) => assert!(m.contains("invalid YAML"), "msg: {m}"),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = format!("{}\nextraneous: oops", minimal_yaml());
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        // serde_norway error message wording varies, so just check for
        // "unknown field" substring.
        match err {
            Error::Config(m) => {
                assert!(
                    m.contains("unknown field") || m.contains("extraneous"),
                    "msg: {m}"
                )
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn accepts_name_without_dot_and_derives_vendor_as_whole_name() {
        // Horizon's server-side `StringUtils.substringBefore(name, ".")`
        // returns the whole string when no '.' is present, so vendor =
        // name itself. The local validator matches that behavior.
        let yaml = minimal_yaml().replace("cisco.foo", "Cisco");
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        assert_eq!(local.metadata.name, "Cisco");
    }

    #[test]
    fn rejects_reserved_name() {
        for reserved in RESERVED_NAMES {
            let yaml = minimal_yaml().replace("cisco.foo", reserved);
            let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
            match err {
                Error::Config(m) => assert!(m.contains("reserved")),
                other => panic!("unexpected {other:?} for {reserved}"),
            }
        }
    }

    #[test]
    fn rejects_name_with_invalid_chars() {
        let yaml = minimal_yaml().replace("cisco.foo", "cisco/foo");
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("invalid characters")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_uei_without_prefix() {
        let yaml = minimal_yaml().replace("uei.opennms.org", "opennms.org");
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("must start with 'uei.'")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_severity() {
        let yaml = minimal_yaml().replace("Warning", "SuperCritical");
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("severity"));
                assert!(m.contains("Warning") && m.contains("Critical"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_event_label() {
        let yaml = minimal_yaml().replace("\"Cisco Foo Cold Start\"", "\"\"");
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("label is empty")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_integer_alarm_type() {
        // Strict mode: out-of-range integers reject at deserialize time.
        // The error message must name the offending value AND the
        // accepted set so operators can fix the YAML directly.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      alarmData:
        reductionKey: "key"
        alarmType: 7
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("7"), "error must cite the bad value: {m}");
                assert!(
                    m.contains("1") && m.contains("2") && m.contains("3"),
                    "error must enumerate the accepted integers: {m}"
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn accepts_alarm_type_3_unresolvable() {
        // alarm-type=3 is "Unresolvable" — a Problem-class event the device
        // never sends a matching Resolution for. Common in vendor MIBs
        // (hardware-failure traps). 75 of 454 events in Cisco.events.xml
        // use this value.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Minor
      alarmData:
        reductionKey: "%uei%:%nodeid%"
        alarmType: 3
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let alarm = local.spec.events[0].alarm_data.as_ref().unwrap();
        assert_eq!(alarm.alarm_type, AlarmType::Unresolvable);
    }

    #[test]
    fn accepts_symbolic_alarm_type_case_insensitive() {
        // YAML accepts "raise" / "resolution" / "unresolvable" alongside
        // the integer forms; case-insensitive on input. Unknown integers
        // round-trip; unknown strings reject at deserialize time.
        for (input, expected) in &[
            ("raise", AlarmType::Raise),
            ("Raise", AlarmType::Raise),
            ("RAISE", AlarmType::Raise),
            ("resolution", AlarmType::Resolution),
            ("Unresolvable", AlarmType::Unresolvable),
        ] {
            let yaml = format!(
                r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      alarmData:
        reductionKey: "key"
        alarmType: {input}
"#,
            );
            let local = EventSourceLocal::from_yaml(yaml.as_bytes())
                .unwrap_or_else(|e| panic!("input {input:?} failed: {e}"));
            let got = local.spec.events[0].alarm_data.as_ref().unwrap().alarm_type;
            assert_eq!(got, *expected, "input {input:?}");
        }
    }

    #[test]
    fn rejects_unknown_symbolic_alarm_type() {
        // Unknown strings (typos or the alarmd Java alias `problem`) fail
        // at deserialize time so operator intent surfaces immediately.
        // Whitespace is NOT trimmed — leading/trailing spaces count.
        // Includes "Problem" (capitalized Java alias) — lowercased before
        // matching, so the strict rejection still fires.
        for bad in &["problem", "Problem", "crit", "Raise ", " raise"] {
            let yaml = format!(
                r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      alarmData:
        reductionKey: "key"
        alarmType: "{bad}"
"#,
            );
            let result = EventSourceLocal::from_yaml(yaml.as_bytes());
            assert!(
                result.is_err(),
                "input {bad:?} should fail at deserialize, got {:?}",
                result.as_ref().map(|_| "Ok"),
            );
            // Error message should reference the unknown value.
            if let Err(Error::Config(m)) = &result {
                assert!(
                    m.contains("unknown alarmType") || m.contains("alarmType"),
                    "input {bad:?}: error message should mention alarmType: {m}"
                );
            }
        }
    }

    #[test]
    fn rejects_invalid_state_in_autoack() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      autoacknowledge:
        state: maybe
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("'on' or 'off'")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_mask_element_name() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      mask:
        elements:
          - name: ""
            values: ["1"]
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("mask.elements[0].name is empty")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn enabled_defaults_to_true_when_omitted() {
        let local = EventSourceLocal::from_yaml(minimal_yaml().as_bytes()).unwrap();
        assert!(local.spec.enabled);
    }

    #[test]
    fn enabled_can_be_set_explicitly_false() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  enabled: false
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        assert!(!local.spec.enabled);
    }

    #[test]
    fn parses_full_event_with_all_sections() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.full
spec:
  events:
    - uei: uei.opennms.org/cisco/full/test
      label: "Cisco Full Test"
      severity: Major
      description: |
        Multi-line
        description
      logmsg:
        dest: logndisplay
        text: "%nodelabel% test"
        notify: true
      alarmData:
        reductionKey: "%uei%:%nodeid%"
        alarmType: 1
        autoClean: false
        clearKey: "%uei%:cleared"
      mask:
        elements:
          - name: id
            values: ["1.3.6.1.4.1.9.9.41.2.0.1"]
          - name: severity
            values: ["Warning", "Major"]
        varbinds:
          - vbnumber: 1
            values: ["3"]
      operinstruct: "Investigate."
      mouseovertext: "Cold start"
      autoacknowledge:
        state: "on"
        text: "auto-acked"
      tticket:
        state: "off"
      correlation:
        state: "off"
        path: "/some/path"
        cmin: "0"
        cmax: "10"
        cuei: ["uei.opennms.org/foo"]
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let e = &local.spec.events[0];
        assert_eq!(e.severity, "Major");
        assert_eq!(e.logmsg.as_ref().unwrap().dest, "logndisplay");
        assert_eq!(e.alarm_data.as_ref().unwrap().alarm_type, AlarmType::Raise);
        assert_eq!(e.mask.as_ref().unwrap().elements.len(), 2);
        assert_eq!(
            e.mask.as_ref().unwrap().elements[0].values,
            vec!["1.3.6.1.4.1.9.9.41.2.0.1"]
        );
        assert_eq!(e.autoacknowledge.as_ref().unwrap().state, "on");
        assert_eq!(e.correlation.as_ref().unwrap().cuei.len(), 1);
    }

    /// Catches drift between the published `examples/` fixtures and the
    /// schema. Each fixture has a per-file assertion that exercises the
    /// shape it's supposed to demonstrate — losing a nested type to a
    /// renamed serde field fails the test instead of silently passing.
    #[test]
    fn published_examples_parse_against_the_schema() {
        type Asserter = fn(&EventSourceLocal);
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let examples_dir = std::path::Path::new(manifest_dir)
            .join("..")
            .join("..")
            .join("examples");

        let cases: &[(&str, Asserter)] = &[
            ("event-source-minimal.yaml", |l| {
                assert_eq!(
                    l.spec.events.len(),
                    1,
                    "event-source-minimal.yaml: one event"
                );
                assert!(
                    l.spec.enabled,
                    "event-source-minimal.yaml: enabled defaults true"
                );
            }),
            ("event-source-full.yaml", |l| {
                let e = l
                    .spec
                    .events
                    .first()
                    .expect("event-source-full.yaml: ≥1 event");
                // Every modeled nested type must populate so the fixture
                // continues to demonstrate the full schema surface.
                assert!(
                    e.mask.is_some(),
                    "event-source-full.yaml: mask must populate"
                );
                assert!(
                    e.alarm_data.is_some(),
                    "event-source-full.yaml: alarmData must populate"
                );
                assert!(
                    e.logmsg.is_some(),
                    "event-source-full.yaml: logmsg must populate"
                );
                assert!(
                    e.correlation.is_some(),
                    "event-source-full.yaml: correlation must populate"
                );
                assert!(
                    e.autoacknowledge.is_some(),
                    "event-source-full.yaml: autoacknowledge must populate"
                );
                assert!(
                    e.tticket.is_some(),
                    "event-source-full.yaml: tticket must populate"
                );
                assert!(
                    e.mouseovertext.is_some(),
                    "event-source-full.yaml: mouseovertext must populate"
                );
                assert!(
                    e.operinstruct.is_some(),
                    "event-source-full.yaml: operinstruct must populate"
                );
                let m = e.mask.as_ref().unwrap();
                assert!(
                    !m.elements.is_empty(),
                    "event-source-full.yaml: mask.elements non-empty"
                );
                assert!(
                    !m.varbinds.is_empty(),
                    "event-source-full.yaml: mask.varbinds non-empty"
                );
                // Both vbnumber-style and vboid-style varbinds must
                // appear so the fixture exercises both mask discriminators.
                assert!(
                    m.varbinds.iter().any(|v| v.vbnumber.is_some()),
                    "event-source-full.yaml: at least one vbnumber-style varbind"
                );
                assert!(
                    m.varbinds.iter().any(|v| v.vboid.is_some()),
                    "event-source-full.yaml: at least one vboid-style varbind"
                );
                assert!(
                    e.snmp.is_some(),
                    "event-source-full.yaml: snmp must populate"
                );
                assert!(
                    e.varbindsdecode.is_some(),
                    "event-source-full.yaml: varbindsdecode must populate"
                );
                assert!(
                    !e.varbindsdecode.as_ref().unwrap().is_empty(),
                    "event-source-full.yaml: varbindsdecode non-empty"
                );
            }),
            ("event-source-severities.yaml", |l| {
                assert_eq!(
                    l.spec.events.len(),
                    7,
                    "event-source-severities.yaml: 7 levels"
                );
                let levels: Vec<&str> = l.spec.events.iter().map(|e| e.severity.as_str()).collect();
                for expected in [
                    "Indeterminate",
                    "Cleared",
                    "Normal",
                    "Warning",
                    "Minor",
                    "Major",
                    "Critical",
                ] {
                    assert!(
                        levels.contains(&expected),
                        "event-source-severities.yaml: missing {expected}",
                    );
                }
            }),
            ("event-source-disabled.yaml", |l| {
                assert!(
                    !l.spec.enabled,
                    "event-source-disabled.yaml: spec.enabled must be false"
                );
            }),
        ];

        for (name, asserter) in cases {
            let path = examples_dir.join(name);
            let bytes =
                std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let local = EventSourceLocal::from_yaml(&bytes)
                .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
            asserter(&local);
        }
    }

    // -- vboid / varbindsdecode (new in this change) ----------------------

    #[test]
    fn mask_varbind_with_vboid_only_is_accepted() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      mask:
        varbinds:
          - vboid: ".1.3.6.1.4.1.61509.1.2.0"
            values: ["0"]
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let vb = &local.spec.events[0].mask.as_ref().unwrap().varbinds[0];
        assert_eq!(vb.vboid.as_deref(), Some(".1.3.6.1.4.1.61509.1.2.0"));
        assert!(vb.vbnumber.is_none());
    }

    #[test]
    fn mask_varbind_with_vbnumber_only_still_works() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      mask:
        varbinds:
          - vbnumber: 1
            values: ["0"]
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let vb = &local.spec.events[0].mask.as_ref().unwrap().varbinds[0];
        assert_eq!(vb.vbnumber, Some(1));
        assert!(vb.vboid.is_none());
    }

    #[test]
    fn mask_varbind_with_neither_vbnumber_nor_vboid_is_rejected() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      mask:
        varbinds:
          - values: ["0"]
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("mask.varbinds[0]"));
                assert!(m.contains("vbnumber") && m.contains("vboid"));
                assert!(m.contains("neither") || m.contains("exactly one"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn mask_varbind_with_both_vbnumber_and_vboid_is_rejected() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      mask:
        varbinds:
          - vbnumber: 1
            vboid: ".1.3.6.1.4.1.61509.1.2.0"
            values: ["0"]
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("mask.varbinds[0]"));
                assert!(m.contains("mutually exclusive") || m.contains("both"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn mask_can_mix_vbnumber_and_vboid_across_entries() {
        // The mutex applies WITHIN each entry, not across the list. A mask
        // may declare some varbinds by position and others by OID.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      mask:
        varbinds:
          - vbnumber: 1
            values: ["0"]
          - vboid: ".1.3.6.1.4.1.61509.1.3.0"
            values: ["xyz"]
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let varbinds = &local.spec.events[0].mask.as_ref().unwrap().varbinds;
        assert_eq!(varbinds.len(), 2);
        assert_eq!(varbinds[0].vbnumber, Some(1));
        assert!(varbinds[0].vboid.is_none());
        assert!(varbinds[1].vbnumber.is_none());
        assert_eq!(
            varbinds[1].vboid.as_deref(),
            Some(".1.3.6.1.4.1.61509.1.3.0")
        );
    }

    #[test]
    fn varbindsdecode_single_group_is_accepted() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Normal
      varbindsdecode:
        - parmid: "1"
          decode:
            - value: "0"
              label: "success(0)"
            - value: "1"
              label: "failed(1)"
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let vd = local.spec.events[0]
            .varbindsdecode
            .as_ref()
            .expect("varbindsdecode populates");
        assert_eq!(vd.len(), 1);
        assert_eq!(vd[0].parmid, "1");
        assert_eq!(vd[0].decode.len(), 2);
        assert_eq!(vd[0].decode[0].value, "0");
        assert_eq!(vd[0].decode[0].label, "success(0)");
    }

    #[test]
    fn varbindsdecode_multiple_groups_preserved_in_order() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Normal
      varbindsdecode:
        - parmid: "1"
          decode: [{value: "a", label: "A"}]
        - parmid: "2"
          decode: [{value: "b", label: "B"}]
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let vd = local.spec.events[0].varbindsdecode.as_ref().unwrap();
        assert_eq!(vd.len(), 2);
        assert_eq!(vd[0].parmid, "1");
        assert_eq!(vd[1].parmid, "2");
    }

    #[test]
    fn varbindsdecode_empty_decode_list_is_rejected() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Normal
      varbindsdecode:
        - parmid: "1"
          decode: []
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("varbindsdecode[0].decode is empty"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn varbindsdecode_empty_value_or_label_is_rejected() {
        for (yaml_snippet, expected_path) in [
            (
                r#"varbindsdecode:
        - parmid: "1"
          decode:
            - value: ""
              label: "x""#,
                "varbindsdecode[0].decode[0].value",
            ),
            (
                r#"varbindsdecode:
        - parmid: "1"
          decode:
            - value: "x"
              label: """#,
                "varbindsdecode[0].decode[0].label",
            ),
        ] {
            let yaml = format!(
                r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Normal
      {yaml_snippet}
"#
            );
            let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
            match err {
                Error::Config(m) => assert!(
                    m.contains(expected_path),
                    "expected path '{expected_path}' in msg: {m}"
                ),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn varbindsdecode_empty_parmid_is_rejected() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Normal
      varbindsdecode:
        - parmid: "   "
          decode: [{value: "x", label: "X"}]
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("varbindsdecode[0].parmid"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn duplicate_uei_across_events_is_accepted_as_normalization_pattern() {
        // Multiple events sharing a UEI is a first-class OpenNMS
        // normalization pattern (mask conditions discriminate at runtime;
        // the UEI is the operator-visible event class). See the archived
        // `permit-duplicate-ueis-as-normalization-pattern` change.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/vendor/cisco/traps/rpsFailed
      label: "CISCO-FASTHUB-MIB defined trap event: rpsFailed"
      severity: Minor
    - uei: uei.opennms.org/vendor/cisco/traps/rpsFailed
      label: "STAND-ALONE-ETHERNET-SWITCH-MIB defined trap event: rpsFailed"
      severity: Minor
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        assert_eq!(local.spec.events.len(), 2);
        assert_eq!(local.spec.events[0].uei, local.spec.events[1].uei);
        assert_ne!(local.spec.events[0].label, local.spec.events[1].label);
    }

    #[test]
    fn varbindsdecode_duplicate_parmid_is_rejected() {
        // D6: duplicate parmids within a single event produce ambiguous
        // server-side behavior; rejected at parse time.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Normal
      varbindsdecode:
        - parmid: "1"
          decode: [{value: "a", label: "A"}]
        - parmid: "1"
          decode: [{value: "b", label: "B"}]
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("varbindsdecode"));
                assert!(m.contains("parmid='1'"));
                assert!(m.contains("unique"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // -- AlarmType serialization round-trip tests --------------------------

    #[test]
    fn alarm_type_serializes_as_symbolic_for_known_variants() {
        // Direct YAML serialization: AlarmType::X → "x".
        let raise_yaml = serde_norway::to_string(&AlarmType::Raise).unwrap();
        let resolution_yaml = serde_norway::to_string(&AlarmType::Resolution).unwrap();
        let unresolvable_yaml = serde_norway::to_string(&AlarmType::Unresolvable).unwrap();
        assert!(
            raise_yaml.trim_end().ends_with("raise"),
            "got {raise_yaml:?}"
        );
        assert!(
            resolution_yaml.trim_end().ends_with("resolution"),
            "got {resolution_yaml:?}"
        );
        assert!(
            unresolvable_yaml.trim_end().ends_with("unresolvable"),
            "got {unresolvable_yaml:?}"
        );
    }

    #[test]
    fn alarm_type_from_wire_returns_none_for_out_of_range() {
        // Strict mode: from_wire returns None for any integer outside
        // {1, 2, 3}. Callers (apply/from_wire.rs) surface that as
        // WireToLocalError::AlarmDataAlarmTypeOutOfRange.
        assert_eq!(AlarmType::from_wire(1), Some(AlarmType::Raise));
        assert_eq!(AlarmType::from_wire(2), Some(AlarmType::Resolution));
        assert_eq!(AlarmType::from_wire(3), Some(AlarmType::Unresolvable));
        assert_eq!(AlarmType::from_wire(0), None);
        assert_eq!(AlarmType::from_wire(4), None);
        assert_eq!(AlarmType::from_wire(-1), None);
        assert_eq!(AlarmType::from_wire(i32::MAX), None);
    }

    #[test]
    fn alarm_type_yaml_round_trip_symbolic_form() {
        // YAML containing `alarmType: raise` round-trips back to YAML
        // containing the same symbolic form after parse + re-emit.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      alarmData:
        reductionKey: "key"
        alarmType: raise
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let emitted = serde_norway::to_string(&local).unwrap();
        assert!(
            emitted.contains("alarmType: raise"),
            "expected `alarmType: raise` after round-trip, got:\n{emitted}"
        );
    }

    #[test]
    fn alarm_type_yaml_round_trip_integer_normalizes_to_symbolic() {
        // YAML containing `alarmType: 1` normalizes to `alarmType: raise`
        // on re-emit — same canonical state regardless of input form.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/test
      label: "Test"
      severity: Warning
      alarmData:
        reductionKey: "key"
        alarmType: 1
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let emitted = serde_norway::to_string(&local).unwrap();
        assert!(
            emitted.contains("alarmType: raise"),
            "integer input should normalize to symbolic on re-emit:\n{emitted}"
        );
    }

    // -- SnmpDef parser / validator tests --------------------------------

    #[test]
    fn accepts_event_with_snmp_block() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.test/cold-start
      label: "Cold start"
      severity: Warning
      snmp:
        id: ".1.3.6.1.4.1.9.1.13"
        idtext: "Cisco"
        version: v2c
        generic: 6
        specific: 1
        community: public
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let snmp = local.spec.events[0].snmp.as_ref().expect("snmp parsed");
        assert_eq!(snmp.id.as_deref(), Some(".1.3.6.1.4.1.9.1.13"));
        assert_eq!(snmp.idtext.as_deref(), Some("Cisco"));
        assert_eq!(snmp.version.as_deref(), Some("v2c"));
        assert_eq!(snmp.generic, Some(6));
        assert_eq!(snmp.specific, Some(1));
        assert_eq!(snmp.community.as_deref(), Some("public"));
    }

    #[test]
    fn accepts_event_with_partial_snmp_block() {
        // Real vendor MIBs often populate only id/generic/specific.
        // Every field is optional and the validator must accept that.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      snmp:
        id: ".1.3.6.1.4.1.9.1.13"
        generic: 6
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let snmp = local.spec.events[0].snmp.as_ref().unwrap();
        assert_eq!(snmp.id.as_deref(), Some(".1.3.6.1.4.1.9.1.13"));
        assert_eq!(snmp.generic, Some(6));
        assert!(snmp.version.is_none());
        assert!(snmp.specific.is_none());
    }

    #[test]
    fn accepts_event_with_unknown_snmp_version_string() {
        // Forward-compat: `version` is a free string. Horizon may use
        // variants like `v3-auth-priv` in the future; verbatim round-trip.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      snmp:
        version: "v3-auth-priv"
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let snmp = local.spec.events[0].snmp.as_ref().unwrap();
        assert_eq!(snmp.version.as_deref(), Some("v3-auth-priv"));
    }

    #[test]
    fn rejects_snmp_with_explicit_empty_or_whitespace_string_field() {
        // Empty AND whitespace-only string fields fail validation,
        // mirroring the sibling validators' `trim().is_empty()` policy.
        for (label, value) in &[("empty", "\"\""), ("whitespace", "\"   \"")] {
            let yaml = format!(
                r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      snmp:
        id: {value}
"#,
            );
            let err = EventSourceLocal::from_yaml(yaml.as_bytes())
                .err()
                .unwrap_or_else(|| panic!("input {label} should fail"));
            match err {
                Error::Config(m) => {
                    assert!(
                        m.contains("snmp.id"),
                        "{label}: error should cite the empty field: {m}"
                    );
                    assert!(
                        m.contains("empty") || m.contains("whitespace"),
                        "{label}: error should describe the issue: {m}"
                    );
                }
                other => panic!("{label}: unexpected {other:?}"),
            }
        }
    }

    // -- ParameterDef tests -----------------------------------------------

    #[test]
    fn accepts_event_with_parameters_list() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      parameters:
        - name: endpoint
          value: /var/log/foo
        - name: context
          value: "%parm[#1]%"
          expand: true
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let params = local.spec.events[0]
            .parameters
            .as_ref()
            .expect("parameters parsed");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "endpoint");
        assert_eq!(params[0].value, "/var/log/foo");
        assert!(params[0].expand.is_none(), "absent expand stays absent");
        assert_eq!(params[1].name, "context");
        assert_eq!(params[1].value, "%parm[#1]%");
        assert_eq!(params[1].expand, Some(true));
    }

    #[test]
    fn rejects_parameter_with_empty_name() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      parameters:
        - name: ""
          value: "v"
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("parameters[0].name"), "{m}");
                assert!(m.contains("empty"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_parameter_with_empty_value() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      parameters:
        - name: "n"
          value: ""
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("parameters[0].value"), "{m}");
                assert!(m.contains("empty"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parameters_round_trip_preserves_order_and_expand_absence() {
        // event-source-parameter task 6.3 + 6.3a:
        //   - order preservation through YAML → wire → XML → wire → YAML
        //   - absent `expand` stays absent (not materialized to a default)
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      parameters:
        - name: first
          value: "1"
        - name: second
          value: "2"
          expand: false
        - name: third
          value: "3"
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let emitted = serde_norway::to_string(&local).unwrap();
        // Re-emit places fields in struct-declared order; pin presence
        // and ordering of the three parameter names.
        let p1 = emitted.find("name: first").expect("first present");
        let p2 = emitted.find("name: second").expect("second present");
        let p3 = emitted.find("name: third").expect("third present");
        assert!(p1 < p2 && p2 < p3, "document order preserved:\n{emitted}");
        // Absent `expand` on first/third stays absent.
        let first_block = &emitted[p1..p2];
        let third_block = &emitted[p3..];
        assert!(
            !first_block.contains("expand:"),
            "absent expand on first should not materialize:\n{first_block}"
        );
        assert!(
            !third_block.contains("expand:"),
            "absent expand on third should not materialize:\n{third_block}"
        );
        // Explicit `expand: false` on second survives.
        let second_block = &emitted[p2..p3];
        assert!(
            second_block.contains("expand: false"),
            "explicit expand: false preserved:\n{second_block}"
        );
    }

    #[test]
    fn snmp_block_round_trips_through_yaml() {
        // YAML → parse → re-emit preserves every populated field.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.test/cold-start
      label: "Cold start"
      severity: Warning
      snmp:
        id: ".1.3.6.1.4.1.9.1.13"
        version: v2c
        generic: 6
        specific: 1
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let emitted = serde_norway::to_string(&local).unwrap();
        assert!(emitted.contains("snmp:"));
        assert!(emitted.contains("id: .1.3.6.1.4.1.9.1.13"));
        assert!(emitted.contains("version: v2c"));
        assert!(emitted.contains("generic: 6"));
        assert!(emitted.contains("specific: 1"));
    }

    // -- ForwardDef + ScriptDef tests -------------------------------------

    #[test]
    fn accepts_event_with_forwards_and_scripts() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      forwards:
        - state: "on"
          mechanism: snmpudp
          target: "alarmcentral:162"
      scripts:
        - language: beanshell
          body: |
            do_thing();
            another_thing();
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let e = &local.spec.events[0];
        let fwds = e.forwards.as_ref().expect("forwards parsed");
        assert_eq!(fwds.len(), 1);
        assert_eq!(fwds[0].state.as_deref(), Some("on"));
        assert_eq!(fwds[0].mechanism.as_deref(), Some("snmpudp"));
        assert_eq!(fwds[0].target.as_deref(), Some("alarmcentral:162"));
        let scripts = e.scripts.as_ref().expect("scripts parsed");
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].language, "beanshell");
        let body = scripts[0].body.as_deref().expect("body present");
        assert!(body.contains("do_thing();"));
        assert!(body.contains("another_thing();"));
    }

    #[test]
    fn rejects_forward_mechanism_outside_xsd_set() {
        // The eventconf XSD restricts mechanism to {snmpudp, snmptcp,
        // xmltcp, xmludp}. Anything else (`kafka`, `xmpp`, …) is
        // rejected locally so the operator sees the typo before Horizon
        // produces a 400.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      forwards:
        - state: "on"
          mechanism: kafka
          target: "topic.example"
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("kafka"), "error cites the bad value: {m}");
                assert!(m.contains("snmpudp"), "error enumerates the XSD set: {m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn forwards_accept_all_xsd_mechanisms_and_states() {
        for mechanism in &["snmpudp", "snmptcp", "xmltcp", "xmludp"] {
            for state in &["on", "off"] {
                let yaml = format!(
                    r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      forwards:
        - state: "{state}"
          mechanism: {mechanism}
          target: "host:162"
"#,
                );
                EventSourceLocal::from_yaml(yaml.as_bytes())
                    .unwrap_or_else(|e| panic!("state={state} mechanism={mechanism}: {e}"));
            }
        }
    }

    #[test]
    fn rejects_forward_with_invalid_state() {
        // state is XSD-constrained to {on, off}; whitespace, typos, and
        // any other value fail via validate_state.
        for bad in &["   ", "yes", "Off", "1"] {
            let yaml = format!(
                r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      forwards:
        - state: "{bad}"
          mechanism: snmpudp
          target: "host:162"
"#,
            );
            let err = EventSourceLocal::from_yaml(yaml.as_bytes())
                .err()
                .unwrap_or_else(|| panic!("input {bad:?} should fail"));
            match err {
                Error::Config(m) => {
                    assert!(m.contains("forwards[0].state"), "{bad:?}: {m}");
                    assert!(m.contains("on") && m.contains("off"), "{bad:?}: {m}");
                }
                other => panic!("{bad:?}: unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn script_body_clip_mode_preserves_one_trailing_newline_byte_for_byte() {
        // YAML `|` clip mode emits a single trailing newline. The body
        // bytes the deserializer sees MUST equal exactly
        // "do_thing();\nanother_thing();\n" — nothing added, nothing
        // stripped, no leading whitespace from the YAML indent.
        let yaml = "
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: \"Foo\"
      severity: Warning
      scripts:
        - language: beanshell
          body: |
            do_thing();
            another_thing();
";
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let body = local.spec.events[0].scripts.as_ref().unwrap()[0]
            .body
            .as_deref()
            .expect("body present");
        assert_eq!(body, "do_thing();\nanother_thing();\n");
    }

    #[test]
    fn script_body_strip_mode_preserves_no_trailing_newline_byte_for_byte() {
        // YAML `|-` strip mode emits no trailing newline. The body
        // bytes MUST equal exactly "do_thing();\nanother_thing();" —
        // no trailing newline, no other modification.
        let yaml = "
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: \"Foo\"
      severity: Warning
      scripts:
        - language: beanshell
          body: |-
            do_thing();
            another_thing();
";
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let body = local.spec.events[0].scripts.as_ref().unwrap()[0]
            .body
            .as_deref()
            .expect("body present");
        assert_eq!(body, "do_thing();\nanother_thing();");
    }

    #[test]
    fn rejects_all_empty_forward_entry() {
        // An entry with no state, mechanism, or target is operationally
        // meaningless even though XSD attributes are individually
        // optional.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      forwards:
        - {}
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("forwards[0]"), "{m}");
                assert!(m.contains("empty") || m.contains("at least one"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_script_with_empty_language() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      scripts:
        - language: ""
          body: "do_thing();"
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("scripts[0].language"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn forwards_and_scripts_preserve_order_through_full_round_trip() {
        // Full round-trip: YAML → wire → XML → wire → YAML. Order is
        // preserved at every layer (eventd evaluates in document
        // order at fire time).
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      forwards:
        - state: "on"
          mechanism: snmpudp
          target: "host1:162"
        - state: "off"
          mechanism: snmptcp
          target: "host2:162"
      scripts:
        - language: beanshell
          body: "first_script();"
        - language: groovy
          body: "second_script();"
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        // Render through the wire layer + XML and back to local.
        use crate::dto::Event;
        use crate::xml::{parse_events_from_xml, render_eventconf_xml};
        let wire: Event = (&local.spec.events[0]).into();
        let xml = render_eventconf_xml(std::slice::from_ref(&wire)).unwrap();
        let parsed = parse_events_from_xml(xml.as_bytes()).unwrap();
        let round_tripped = EventDef::try_from(&parsed[0]).unwrap();
        // Forwards order survives the full loop.
        let fwds = round_tripped.forwards.as_ref().expect("forwards present");
        assert_eq!(fwds.len(), 2);
        assert_eq!(fwds[0].target.as_deref(), Some("host1:162"));
        assert_eq!(fwds[1].target.as_deref(), Some("host2:162"));
        // Scripts order survives.
        let scripts = round_tripped.scripts.as_ref().expect("scripts present");
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[0].language, "beanshell");
        assert_eq!(scripts[1].language, "groovy");
        assert_eq!(scripts[0].body.as_deref(), Some("first_script();"));
        assert_eq!(scripts[1].body.as_deref(), Some("second_script();"));
    }

    // -- FilterDef tests --------------------------------------------------

    #[test]
    fn accepts_event_with_flat_filters_list() {
        // YAML is flat — no `<filters>` wrapper. Each entry has the
        // three required attributes (eventparm, pattern, replacement).
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      filters:
        - eventparm: trapMsg
          pattern: '\bWARN\b'
          replacement: "WARNING"
        - eventparm: ifAlias
          pattern: '^old-(.*)$'
          replacement: "new-$1"
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let filters = local.spec.events[0]
            .filters
            .as_ref()
            .expect("filters parsed");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].eventparm, "trapMsg");
        // YAML single-quoted scalars preserve backslashes literally.
        // `\bWARN\b` is the canonical Java regex for "match WARN at
        // word boundaries" — single backslash, NOT double.
        assert_eq!(filters[0].pattern, r"\bWARN\b");
        assert_eq!(filters[0].replacement, "WARNING");
        assert_eq!(filters[1].eventparm, "ifAlias");
        assert_eq!(filters[1].replacement, "new-$1");
    }

    #[test]
    fn rejects_filter_with_empty_eventparm() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      filters:
        - eventparm: ""
          pattern: "x"
          replacement: "y"
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("filters[0].eventparm"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_filter_with_empty_pattern() {
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      filters:
        - eventparm: "x"
          pattern: ""
          replacement: "y"
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("filters[0].pattern"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn accepts_filter_with_empty_replacement() {
        // `Matcher.replaceAll("")` strips the matched text — a
        // legitimate use case for "match this regex, delete the
        // match." The XSD requires `replacement` to be PRESENT,
        // not non-empty. The local schema follows.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      filters:
        - eventparm: "x"
          pattern: '\bsecret\b'
          replacement: ""
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let filter = &local.spec.events[0].filters.as_ref().unwrap()[0];
        assert_eq!(filter.replacement, "");
    }

    #[test]
    fn rejects_filter_with_padded_eventparm() {
        // Whitespace around eventparm would silently prevent eventd
        // from finding the filter in its (eventparm|uei) map.
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      filters:
        - eventparm: "  trapMsg  "
          pattern: "x"
          replacement: "y"
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("filters[0].eventparm"), "{m}");
                assert!(m.contains("whitespace"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn filters_yaml_is_flat_xml_renders_with_filters_wrapper() {
        // YAML → wire → XML inserts the `<filters>` wrapper;
        // parsing back strips it. Order is preserved.
        use crate::dto::Event;
        use crate::xml::{parse_events_from_xml, render_eventconf_xml};
        let yaml = r#"
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata:
  name: vendor.foo
spec:
  events:
    - uei: uei.test/foo
      label: "Foo"
      severity: Warning
      filters:
        - eventparm: a
          pattern: "p1"
          replacement: "r1"
        - eventparm: b
          pattern: "p2"
          replacement: "r2"
"#;
        let local = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap();
        let wire: Event = (&local.spec.events[0]).into();
        let xml = render_eventconf_xml(std::slice::from_ref(&wire)).unwrap();
        // Wrapper present on render.
        assert!(xml.contains("<filters>"), "wrapper present:\n{xml}");
        assert!(xml.contains("</filters>"));
        // Two filter children with attributes.
        assert!(xml.contains(r#"eventparm="a""#));
        assert!(xml.contains(r#"eventparm="b""#));
        assert!(xml.contains(r#"pattern="p1""#));
        assert!(xml.contains(r#"replacement="r2""#));
        // Document order preserved (a before b).
        let a_pos = xml.find(r#"eventparm="a""#).unwrap();
        let b_pos = xml.find(r#"eventparm="b""#).unwrap();
        assert!(a_pos < b_pos, "document order");
        // Round-trip XML → wire → local.
        let parsed = parse_events_from_xml(xml.as_bytes()).unwrap();
        let round_tripped = EventDef::try_from(&parsed[0]).unwrap();
        let rt_filters = round_tripped.filters.as_ref().expect("filters present");
        assert_eq!(rt_filters.len(), 2);
        assert_eq!(rt_filters[0].eventparm, "a");
        assert_eq!(rt_filters[1].eventparm, "b");
    }
}
