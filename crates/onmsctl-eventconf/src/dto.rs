/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Wire-format DTOs for the Horizon `/eventconf/*` REST surface.
//!
//! Field naming follows the OpenAPI document: camelCase on the JSON wire,
//! snake_case in Rust, with `#[serde(rename_all = "camelCase")]` per
//! struct. Inconsistencies in the wire format (`enabled` vs `enable`,
//! `sourceIds` vs `eventsIds` typo) are preserved here so the
//! `EventConfApi` layer can absorb them without inventing a third spelling.

use serde::{Deserialize, Serialize};

// -- Source DTOs -------------------------------------------------------------

/// A row in the EventConf source table. Returned by GET-by-id, list, and
/// filter endpoints.
///
/// Required fields (`id`, `name`, `file_order`, `event_count`, `enabled`)
/// are strict on read — a server response that omits one fails parsing
/// rather than silently producing `id: 0`/`name: ""`. Truly optional
/// fields (vendor, description, created_time, …) keep `Option<>` and
/// `#[serde(default)]` individually.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EventConfSourceDto {
    pub id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub file_order: i32,
    pub event_count: i32,
    pub enabled: bool,
    /// Epoch milliseconds. Server emits as a bare integer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_time: Option<i64>,
    /// Epoch milliseconds. Server emits as a bare integer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploaded_by: Option<String>,
}

/// Body for `POST /eventconf/sources/eventConfSource`. Note: the API does
/// not accept a `fileOrder`; the server auto-bumps it (design.md §3.2).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AddEventConfSourceRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
}

/// Body for `DELETE /eventconf/sources`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EventConfSourceDeletePayload {
    pub source_ids: Vec<i64>,
}

/// Body for `PATCH /eventconf/sources/status`. Note: field is `enabled`
/// (past participle), matching the wire form.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EventConfSrcEnableDisablePayload {
    pub enabled: bool,
    pub cascade_to_events: bool,
    pub source_ids: Vec<i64>,
}

/// `{name, id}` tuple returned by `GET /eventconf/sources/names-and-ids`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceNameAndId {
    pub id: i64,
    pub name: String,
}

// -- Event DTOs --------------------------------------------------------------

/// A row in the EventConf event table. Returned by per-source listing,
/// filter, and vendor endpoints.
///
/// `source_id` is optional on read: Horizon's per-source events response
/// (`/eventconf/filter/{id}/events`) omits it entirely — the id is
/// implicit in the URL path — and the cross-source `/filter` response
/// shape has been observed without it too. Callers that need the source
/// id should track it out-of-band (e.g. from the URL they requested).
///
/// `created_time` and `last_modified` are epoch-milliseconds integers on
/// the wire, not ISO strings.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EventConfEventDto {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<i64>,
    pub uei: String,
    pub event_label: String,
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_time: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<i64>,
}

/// Body for `PUT /eventconf/sources/{sourceId}/events/{eventId}`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EventConfEventEditRequest {
    pub enabled: bool,
    pub event: Event,
}

/// Body for `DELETE /eventconf/sources/{sourceId}/events`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EventConfEventDeletePayload {
    pub event_ids: Vec<i64>,
}

/// Body for `PATCH /eventconf/sources/{sourceId}/events/status`.
///
/// The wire-format field is `eventsIds` (with the trailing `s`) and `enable`
/// (verb form, not `enabled`). Both inconsistencies are preserved here.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnableDisableConfSourceEventsPayload {
    pub enable: bool,
    #[serde(rename = "eventsIds")]
    pub events_ids: Vec<i64>,
}

// -- Event (the unified body type for POST/PUT) -----------------------------
//
// The Horizon Event schema unifies runtime and eventconf fields. For
// eventconf use we send a partial Event with only the eventconf-relevant
// fields. Every field is optional and elided when absent so we don't send
// stray nulls. Phase 3 commit 2 (XML conversion) adds the parsing path
// from eventconf XML into the same shape.

/// Partial event payload used by `POST /eventconf/sources/{id}/events` and
/// `PUT /…/events/{id}`. Only the eventconf-relevant fields are modeled;
/// runtime fields (nodeid, dbid, time, …) are intentionally absent because
/// they are server-assigned at runtime, not eventconf inputs.
///
/// Wire format is **kebab-case** (e.g. `event-label`, `alarm-data`) —
/// Horizon's eventconf Event class uses the same property names as the
/// eventconf XML, not the camelCase shape the filter-endpoint DTOs use.
/// A POST with `eventLabel` returns 500 `Unrecognized field` from the
/// Jackson layer. This is asymmetric with [`EventConfEventDto`] (read
/// shape), which is camelCase.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", default)]
pub struct Event {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uei: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<Mask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logmsg: Option<Logmsg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation: Option<Correlation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operinstruct: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoacknowledge: Option<Autoacknowledge>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tticket: Option<Tticket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouseovertext: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alarm_data: Option<AlarmData>,
    /// Display-time decode tables that map raw varbind values to
    /// human-readable labels (e.g. `"0"` → `"success(0)"`). One group per
    /// `parmid`. Local validation forbids duplicate `parmid` values within
    /// the list — see `apply/local.rs::EventDef::validate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub varbindsdecode: Option<Vec<Varbindsdecode>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "snmp")]
    pub snmp: Option<Snmp>,
    /// Static per-event configuration parameters. Mirrors the eventconf
    /// XSD's `<parameter name="..." value="..." expand="..."/>`. Survives
    /// the XML round-trip via [`crate::xml::XmlParameter`]; the local
    /// YAML schema [`crate::apply::local::ParameterDef`] is the
    /// operator-facing form.
    ///
    /// **NOT** the same as [`Event::parm_collection`] below — these are
    /// two different domains:
    ///
    ///   - `parameter` (THIS field): static, defined alongside the event
    ///     in eventconf XML. Round-trips through `apply` /
    ///     `event-source convert`. Per-event metadata that eventd attaches at
    ///     fire time. eventd evaluates entries in document order.
    ///   - `parm_collection` (below): runtime, present on a fired event
    ///     instance. JSON-only on the wire; the XML render in
    ///     [`crate::xml`] drops it.
    ///
    /// MUST NOT be conflated.
    #[serde(skip_serializing_if = "Option::is_none", rename = "parameter")]
    pub parameter: Option<Vec<Parameter>>,
    /// eventd forwarding directives. Each entry tells eventd to forward
    /// matched events to another destination (typically SNMP UDP / TCP).
    /// State is `on`/`off`; mechanism is `snmpudp`/`snmptcp`/...; target
    /// is the body text (destination address).
    #[serde(skip_serializing_if = "Option::is_none", rename = "forward")]
    pub forward: Option<Vec<Forward>>,
    /// Embedded executable logic that eventd runs on event arrival.
    /// Typically BeanShell. **Security note:** modeling this in YAML
    /// makes it trivial to ship server-side code via `apply` —
    /// the threat surface already existed via raw XML upload, but
    /// operators should ensure RBAC on eventconf write access is
    /// configured appropriately.
    #[serde(skip_serializing_if = "Option::is_none", rename = "script")]
    pub script: Option<Vec<Script>>,
    /// eventd regex-replacement filters. Each entry is a
    /// `(eventparm, pattern, replacement)` triple that eventd applies
    /// via `Pattern.compile(pattern).matcher(parm).replaceAll(replacement)`
    /// at event-fire time. NOT a suppression filter — a value-rewrite
    /// rule on named event parameters.
    #[serde(skip_serializing_if = "Option::is_none", rename = "filters")]
    pub filters: Option<Vec<Filter>>,
    /// Runtime parameters extracted from the event payload. Mapped from
    /// `parmCollection` in the Horizon schema. **Note:** this field has
    /// no representation in eventconf XML (eventconf is a static
    /// configuration; `parmCollection` is a *runtime* event field). A
    /// JSON → XML → JSON round-trip via [`crate::xml`] therefore drops
    /// this field. Setting it has no effect on uploaded source XML.
    ///
    /// **NOT** the same as [`Event::parameter`] above — see that field's
    /// doc-comment for the static-vs-runtime distinction.
    #[serde(skip_serializing_if = "Option::is_none", rename = "parmCollection")]
    pub parm_collection: Option<Vec<Parm>>,
}

/// Static per-event configuration parameter. Mirrors the eventconf XSD's
/// `<parameter name="..." value="..." expand="..."/>`. See
/// [`Event::parameter`] for the static-vs-runtime distinction with
/// [`Parm`] / [`Event::parm_collection`].
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Parameter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Whether eventd should expand placeholder syntax (e.g. `%parm[#1]%`)
    /// in `value` at fire time. Absent on the wire when not set in the
    /// source XML — the local schema preserves the absent vs explicit
    /// `true`/`false` distinction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expand: Option<bool>,
}

/// eventd forwarding directive. Mirrors the eventconf XSD's
/// `<forward state="on" mechanism="snmpudp">target</forward>`.
/// All fields are free strings — no closed-set enum on `state` or
/// `mechanism` so that Horizon-version-specific values (e.g. `kafka`)
/// round-trip verbatim.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Forward {
    /// Forwarding state — typically `on` or `off`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Forwarder mechanism — typically `snmpudp`, `snmptcp`, `xmpp`, ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mechanism: Option<String>,
    /// Destination identifier (body text of `<forward>`), e.g.
    /// `alarmcentral:162` for an SNMP trap forwarder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// Embedded executable logic. Mirrors the eventconf XSD's
/// `<script language="beanshell">body</script>`. Body is preserved
/// byte-for-byte through round-trip (no whitespace normalization).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Script {
    /// Script language — typically `beanshell`. Free string for
    /// forward-compat with future eventd-supported languages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Script source. Multi-line content is preserved; eventd evaluates
    /// the body verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// eventd regex-replacement filter. Mirrors the upstream
/// `<filter eventparm="..." pattern="..." replacement="..."/>` —
/// EMPTY element with three required attributes per the JAXB
/// `@XmlAttribute(required=true)` annotation. Wire-side fields are
/// `Option<String>` to be liberal in what we accept on download; the
/// local `FilterDef` requires all three.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    /// Name of the event parameter the filter operates on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eventparm: Option<String>,
    /// Java-style regular expression matched against the named parm
    /// value via `Pattern.compile(...).matcher(value).replaceAll(...)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Replacement string passed to `Matcher.replaceAll`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Mask {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maskelements: Option<Vec<MaskElement>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub varbinds: Option<Vec<MaskVarbind>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MaskElement {
    pub mename: String,
    #[serde(default)]
    pub mevalues: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct MaskVarbind {
    /// Match the SNMP trap PDU's varbind by 1-indexed position. Mutually
    /// exclusive with [`vboid`]; the local validator enforces exactly one
    /// is set. The wire layer accepts either form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vbnumber: Option<i32>,
    /// Match the SNMP trap PDU's varbind by OID. Mutually exclusive with
    /// [`vbnumber`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vboid: Option<String>,
    #[serde(default)]
    pub vbvalues: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Varbindsdecode {
    /// Identifies which event parameter this decode group annotates. XSD
    /// types it as `xs:string`; real-world values are typically numeric.
    pub parmid: String,
    #[serde(default)]
    pub decode: Vec<Decode>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Decode {
    /// The raw varbind value to match. Wire-format name matches the XSD
    /// attribute (`varbindvalue`).
    pub varbindvalue: String,
    /// The human-readable label shown in the Horizon event UI. Wire-format
    /// name matches the XSD attribute (`varbinddecodedstring`).
    pub varbinddecodedstring: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Logmsg {
    pub dest: Option<String>,
    pub content: Option<String>,
    pub notify: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Correlation {
    pub state: Option<String>,
    pub path: Option<String>,
    pub cmin: Option<String>,
    pub cmax: Option<String>,
    pub ctime: Option<String>,
    #[serde(rename = "cuei", skip_serializing_if = "Option::is_none")]
    pub cuei: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Autoacknowledge {
    pub state: Option<String>,
    pub content: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Tticket {
    pub state: Option<String>,
    pub content: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", default)]
pub struct AlarmData {
    pub reduction_key: Option<String>,
    /// Alarm semantic, stored as the wire-format integer Horizon expects.
    /// Operator-facing vocabulary (Web UI and YAML) uses the symbolic
    /// form; this field stays integer-typed for wire compatibility.
    ///
    ///   - `1` raise — alarmd Java code calls this `Problem`.
    ///   - `2` resolution — clears a paired Raise by reductionKey.
    ///   - `3` unresolvable — no auto-clear from device; manual close
    ///     or alarmd cleanup policy.
    ///
    /// Other integers round-trip verbatim and trigger `EC007` during
    /// `event-source convert`. The local YAML schema `AlarmType` enum projects
    /// to/from this integer via `to_wire()` / `from_wire()`.
    pub alarm_type: Option<i32>,
    pub clear_key: Option<String>,
    pub auto_clean: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Snmp {
    pub id: Option<String>,
    pub idtext: Option<String>,
    pub version: Option<String>,
    pub specific: Option<i32>,
    pub generic: Option<i32>,
    pub community: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Parm {
    pub parm_name: Option<String>,
    pub value: Option<ParmValue>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ParmValue {
    pub content: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub encoding: Option<String>,
}

// -- Pagination wrappers ----------------------------------------------------

/// Page returned by filter endpoints that include `totalRecords` alongside
/// the items array.
///
/// Horizon's eventconf controller emits the items array under the field
/// name `eventConfSourceList` — including on `/eventconf/filter/{id}/events`,
/// where the field carries `EventConfEvent` payloads despite the
/// `Source`-shaped name (copy-paste in the controller). We accept both
/// `eventConfSourceList` and `items` via serde aliases so wiremock tests
/// that pre-date the contract discovery still parse, and a future Horizon
/// rename to a generic name would not break us.
///
/// Both fields default — `total_records` to 0 and items to empty — so
/// servers that omit one or both still parse to a usable empty page.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    #[serde(default)]
    pub total_records: i64,
    #[serde(default = "Vec::new", alias = "eventConfSourceList")]
    pub items: Vec<T>,
}

// -- Severity enum (helper) -------------------------------------------------

/// Stable enum form of the seven OpenNMS severity levels. The wire format
/// uses these as strings; we expose them as a typed alternative for
/// command-line parsing while [`Event::severity`] stays as `Option<String>`
/// to round-trip exotic future values without panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Indeterminate,
    Cleared,
    Normal,
    Warning,
    Minor,
    Major,
    Critical,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Indeterminate => "Indeterminate",
            Self::Cleared => "Cleared",
            Self::Normal => "Normal",
            Self::Warning => "Warning",
            Self::Minor => "Minor",
            Self::Major => "Major",
            Self::Critical => "Critical",
        }
    }
}

impl std::str::FromStr for Severity {
    type Err = onmsctl_core::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Indeterminate" => Ok(Self::Indeterminate),
            "Cleared" => Ok(Self::Cleared),
            "Normal" => Ok(Self::Normal),
            "Warning" => Ok(Self::Warning),
            "Minor" => Ok(Self::Minor),
            "Major" => Ok(Self::Major),
            "Critical" => Ok(Self::Critical),
            other => Err(onmsctl_core::Error::Config(format!(
                "unknown severity '{other}'; expected one of: Indeterminate, Cleared, Normal, Warning, Minor, Major, Critical"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_dto_round_trips_with_optional_fields_omitted() {
        let s = EventConfSourceDto {
            id: 42,
            name: "cisco.foo".into(),
            vendor: Some("cisco".into()),
            description: None,
            file_order: 50,
            event_count: 17,
            enabled: true,
            created_time: None,
            last_modified: None,
            uploaded_by: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"name\":\"cisco.foo\""));
        assert!(json.contains("\"fileOrder\":50"));
        assert!(json.contains("\"eventCount\":17"));
        assert!(!json.contains("description"));
        assert!(!json.contains("createdTime"));
        let parsed: EventConfSourceDto = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, s);
    }

    #[test]
    fn add_source_request_omits_optionals_when_none() {
        let r = AddEventConfSourceRequest {
            name: "cisco.foo".into(),
            description: None,
            vendor: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"name":"cisco.foo"}"#);
    }

    #[test]
    fn enable_disable_payload_uses_camel_case_wire_form() {
        let p = EventConfSrcEnableDisablePayload {
            enabled: true,
            cascade_to_events: true,
            source_ids: vec![42, 43],
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"enabled\":true"));
        assert!(json.contains("\"cascadeToEvents\":true"));
        assert!(json.contains("\"sourceIds\":[42,43]"));
    }

    #[test]
    fn event_status_payload_preserves_wire_typos() {
        // The wire form is `enable` (verb) and `eventsIds` (with extra s).
        let p = EnableDisableConfSourceEventsPayload {
            enable: true,
            events_ids: vec![108, 109],
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"enable\":true"));
        assert!(json.contains("\"eventsIds\":[108,109]"));
        assert!(!json.contains("eventIds"));
    }

    #[test]
    fn event_omits_unset_nested_fields() {
        let e = Event {
            uei: Some("uei.opennms.org/foo".into()),
            severity: Some("Warning".into()),
            ..Event::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        // Required fields appear:
        assert!(json.contains("uei.opennms.org/foo"));
        assert!(json.contains("\"severity\":\"Warning\""));
        // Unset nested fields are NOT serialized. Names are the wire
        // (kebab-case) shape — see Event's serde annotation.
        for missing in [
            "mask",
            "logmsg",
            "alarm-data",
            "correlation",
            "autoacknowledge",
            "tticket",
            "mouseovertext",
            "snmp",
            "parmCollection",
        ] {
            assert!(!json.contains(missing), "unexpected '{missing}' in {json}");
        }
    }

    #[test]
    fn event_serializes_multi_word_fields_as_kebab_case() {
        // Horizon's eventconf Event POST endpoint rejects camelCase
        // field names. Pin the wire shape so a future refactor doesn't
        // silently regress us back to e.g. `eventLabel`.
        let e = Event {
            uei: Some("uei.opennms.org/x".into()),
            event_label: Some("L".into()),
            alarm_data: Some(AlarmData {
                reduction_key: Some("k".into()),
                alarm_type: Some(1),
                clear_key: Some("c".into()),
                auto_clean: Some(true),
            }),
            ..Event::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"event-label\":\"L\""));
        assert!(json.contains("\"alarm-data\""));
        assert!(json.contains("\"reduction-key\":\"k\""));
        assert!(json.contains("\"alarm-type\":1"));
        assert!(json.contains("\"clear-key\":\"c\""));
        assert!(json.contains("\"auto-clean\":true"));
        // Negative: no leftover camelCase.
        for camel in [
            "eventLabel",
            "alarmData",
            "reductionKey",
            "alarmType",
            "clearKey",
            "autoClean",
        ] {
            assert!(
                !json.contains(camel),
                "unexpected camelCase '{camel}' in {json}"
            );
        }
    }

    #[test]
    fn event_round_trips_full_payload() {
        let e = Event {
            uei: Some("uei.opennms.org/cisco/foo/coldStart".into()),
            event_label: Some("Cisco Foo Cold Start".into()),
            descr: Some("Foo device performed a cold start.".into()),
            severity: Some("Warning".into()),
            mask: Some(Mask {
                maskelements: Some(vec![MaskElement {
                    mename: "id".into(),
                    mevalues: vec!["1.3.6.1.4.1.9.9.41.2.0.1".into()],
                }]),
                varbinds: None,
            }),
            logmsg: Some(Logmsg {
                dest: Some("logndisplay".into()),
                content: Some("%nodelabel% performed a cold start".into()),
                notify: None,
            }),
            alarm_data: Some(AlarmData {
                reduction_key: Some("%uei%:%dpname%:%nodeid%".into()),
                alarm_type: Some(1),
                clear_key: None,
                auto_clean: Some(false),
            }),
            operinstruct: Some("Investigate.".into()),
            ..Event::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn page_deserializes_with_typed_items() {
        let json = r#"{"totalRecords": 2, "items": [
            {"id": 42, "name": "a", "fileOrder": 5, "eventCount": 0, "enabled": true},
            {"id": 43, "name": "b", "fileOrder": 6, "eventCount": 1, "enabled": false}
        ]}"#;
        let p: Page<EventConfSourceDto> = serde_json::from_str(json).unwrap();
        assert_eq!(p.total_records, 2);
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[0].name, "a");
        assert!(!p.items[1].enabled);
    }

    #[test]
    fn severity_round_trip_via_str() {
        for s in [
            "Indeterminate",
            "Cleared",
            "Normal",
            "Warning",
            "Minor",
            "Major",
            "Critical",
        ] {
            assert_eq!(s.parse::<Severity>().unwrap().as_str(), s);
        }
    }

    #[test]
    fn severity_unknown_value_lists_valid_set() {
        let err = "Bogus".parse::<Severity>().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Bogus"));
        assert!(msg.contains("Warning"));
        assert!(msg.contains("Critical"));
    }
}
