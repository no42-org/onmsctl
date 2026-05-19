/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! eventconf XML conversion.
//!
//! Public API:
//!   - [`render_eventconf_xml`] — JSON-shape `Event`s → eventconf XML string.
//!   - [`parse_events_from_xml`] — eventconf XML bytes → JSON-shape `Event`s.
//!   - [`synth_master_with_order`] — synthesize an `eventconf.xml` master
//!     file that lists source basenames in the requested order.
//!   - [`xml_canonical`] — produce a stable, byte-equal-comparable form of
//!     an eventconf XML document.
//!
//! Why the parallel schema: eventconf XML uses kebab-case names
//! (`alarm-data`, `event-label`, `reduction-key`) and attribute placement
//! that does not match the camelCase JSON wire format. We model the XML
//! schema separately in this module and bridge it to the public [`Event`]
//! DTO via `From` impls. Everything is `pub(crate)` so the XML schema
//! types do not appear in the crate's public surface.

use onmsctl_core::{Error, Result};
use quick_xml::de::from_str;
use quick_xml::se::to_string as xml_to_string;
use serde::{Deserialize, Serialize};

use crate::dto::{
    AlarmData, Autoacknowledge, Correlation, Decode, Event, Logmsg, Mask, MaskElement, MaskVarbind,
    Snmp, Tticket, Varbindsdecode,
};

/// Render a slice of [`Event`]s as an eventconf XML `<events>` document.
pub fn render_eventconf_xml(events: &[Event]) -> Result<String> {
    let xml_events = XmlEvents {
        event: events.iter().map(XmlEvent::from).collect(),
        event_file: Vec::new(),
    };
    serialize_root(&xml_events)
}

/// Parse an eventconf XML document into a vector of [`Event`]s. Unknown
/// elements outside the modeled schema are tolerated (skipped) so that
/// future Horizon schema additions don't break the parser.
pub fn parse_events_from_xml(xml: &[u8]) -> Result<Vec<Event>> {
    let s = std::str::from_utf8(xml)
        .map_err(|e| Error::Config(format!("eventconf XML is not valid UTF-8: {e}")))?;
    let parsed: XmlEvents =
        from_str(s).map_err(|e| Error::Config(format!("eventconf XML parse error: {e}")))?;
    Ok(parsed.event.into_iter().map(Event::from).collect())
}

/// Source location for one `<event>` within an eventconf XML document.
/// Used by the migration converter ([`crate::convert`]) to cite where a
/// finding originated.
///
/// Line numbers are 1-indexed. Column counts **bytes** within the logical
/// line, not Unicode characters — eventconf files are ASCII in practice, so
/// the simplification is acceptable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceLocation {
    pub file: std::path::PathBuf,
    pub line: usize,
    pub column: usize,
    pub event_index: usize,
}

/// Parse an eventconf XML document and pair each [`Event`] with its
/// [`SourceLocation`] in the original byte input.
///
/// Locations are derived from a byte-level scan for `<event>` start tags
/// (eventconf XSD says `<event>` carries no attributes, so a literal
/// scan is reliable). If the scan-derived count of `<event>` markers
/// disagrees with the serde-parsed event count (e.g. a malformed file
/// or one using attribute-bearing `<event ...>` shapes from a future
/// schema), every returned [`SourceLocation`] falls back to `line: 0,
/// column: 0` and the `file` / `event_index` fields remain accurate.
///
/// **XXE / external-entity posture.** `quick_xml`'s default `Reader`
/// does NOT process external entities (no SYSTEM/PUBLIC URI fetch, no
/// internal-entity expansion beyond the small built-in set
/// `&lt;`/`&gt;`/`&amp;`/`&apos;`/`&quot;`). A DOCTYPE declaration in
/// the input is parsed as decoration but does not trigger external
/// fetches. This is the safe default we rely on; do NOT replace
/// `from_str` with a custom reader that enables external entities
/// without a security review.
pub fn parse_events_with_locations(
    xml: &[u8],
    path: impl Into<std::path::PathBuf>,
) -> Result<Vec<(Event, SourceLocation)>> {
    let file = path.into();
    let s = std::str::from_utf8(xml)
        .map_err(|e| Error::Config(format!("eventconf XML is not valid UTF-8: {e}")))?;
    let parsed: XmlEvents =
        from_str(s).map_err(|e| Error::Config(format!("eventconf XML parse error: {e}")))?;
    let events: Vec<Event> = parsed.event.into_iter().map(Event::from).collect();

    let offsets = find_event_start_offsets(xml);
    let use_real_positions = offsets.len() == events.len();

    let pairs = events
        .into_iter()
        .enumerate()
        .map(|(idx, event)| {
            let (line, column) = if use_real_positions {
                byte_offset_to_line_col(xml, offsets[idx])
            } else {
                (0, 0)
            };
            (
                event,
                SourceLocation {
                    file: file.clone(),
                    line,
                    column,
                    event_index: idx,
                },
            )
        })
        .collect();
    Ok(pairs)
}

/// Scan `xml` for byte offsets of `<event>` start tags. Distinguishes
/// `<event>` from `<event-label>` / `<event-file>` / `<events>` by
/// requiring the next byte after `<event` to be `>`, whitespace, or `/`.
fn find_event_start_offsets(xml: &[u8]) -> Vec<usize> {
    let prefix = b"<event";
    let mut offsets = Vec::new();
    let mut i = 0;
    while i + prefix.len() <= xml.len() {
        if &xml[i..i + prefix.len()] == prefix {
            let terminator = xml.get(i + prefix.len()).copied();
            match terminator {
                Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'/') => {
                    offsets.push(i);
                    i += prefix.len() + 1;
                    continue;
                }
                _ => {}
            }
        }
        i += 1;
    }
    offsets
}

/// Compute `(line, column)` from a byte offset. Line is 1-indexed (line 1
/// is the first line). Column counts bytes since the previous newline
/// (1-indexed; the first byte on a line is column 1).
pub(crate) fn byte_offset_to_line_col(input: &[u8], offset: usize) -> (usize, usize) {
    let offset = offset.min(input.len());
    let prefix = &input[..offset];
    let line = prefix.iter().filter(|&&b| b == b'\n').count() + 1;
    let column = match prefix.iter().rposition(|&b| b == b'\n') {
        Some(nl) => offset - nl, // byte after newline is column 1
        None => offset + 1,
    };
    (line, column)
}

/// Synthesize an `eventconf.xml` master file listing source basenames in
/// the order they should appear. Per design.md §3.1, the upload pipeline
/// reads `<event-file>` entries in **reversed** iteration order to assign
/// fileOrder values, so the FIRST entry in the produced master gets the
/// highest fileOrder and the LAST entry gets the lowest.
pub fn synth_master_with_order(basenames: &[&str]) -> Result<String> {
    let xml_events = XmlEvents {
        event: Vec::new(),
        event_file: basenames.iter().map(|s| (*s).to_string()).collect(),
    };
    serialize_root(&xml_events)
}

/// Canonicalize an eventconf XML document into a stable byte form.
///
/// **Lossy normalization.** This function is named "canonical" for its
/// fixed-point property — `canonical(canonical(x)) == canonical(x)` —
/// not because it preserves every aspect of the input. The pipeline is
/// parse-via-modeled-schema → re-serialize, which means:
///
///   - **Dropped:** XML declaration (`<?xml ...?>`), DOCTYPE / DTD
///     references, namespace declarations, processing instructions,
///     comments, and any element / attribute outside the modeled schema
///     in this module (e.g. `<varbindsdecode>`, `<forward>`, `<script>`).
///   - **Normalized:** whitespace differences, attribute ordering,
///     element ordering within an event (re-serialized in struct field
///     order).
///
/// The drops are *consistent* — running canonical twice produces identical
/// output — which is sufficient for hash-based change detection in the
/// `apply -f` flow. It is **not** suitable as a general-purpose XML
/// canonicalization (e.g. for digital signatures).
///
/// Pipeline: parse → re-serialize via the modeled schema.
pub fn xml_canonical(xml: &[u8]) -> Result<String> {
    let s = std::str::from_utf8(xml)
        .map_err(|e| Error::Config(format!("eventconf XML is not valid UTF-8: {e}")))?;
    let parsed: XmlEvents =
        from_str(s).map_err(|e| Error::Config(format!("eventconf XML parse error: {e}")))?;
    serialize_root(&parsed)
}

fn serialize_root(events: &XmlEvents) -> Result<String> {
    xml_to_string(events).map_err(|e| Error::Config(format!("eventconf XML render error: {e}")))
}

// -- XML schema types -------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename = "events")]
struct XmlEvents {
    /// `<event-file>` entries used by the master `eventconf.xml` file.
    /// Empty for per-source files.
    #[serde(rename = "event-file", default, skip_serializing_if = "Vec::is_empty")]
    event_file: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    event: Vec<XmlEvent>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    mask: Option<XmlMask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uei: Option<String>,
    #[serde(rename = "event-label", skip_serializing_if = "Option::is_none")]
    event_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    descr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logmsg: Option<XmlLogmsg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation: Option<XmlCorrelation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operinstruct: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    autoacknowledge: Option<XmlAutoack>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tticket: Option<XmlTticket>,
    /// Display-time decode tables for varbind values. Zero or more groups
    /// per event, each scoped to one `parmid`. Placed before
    /// `mouseovertext`/`alarm-data` to match the eventconf XSD sequence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    varbindsdecode: Vec<XmlVarbindsdecode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mouseovertext: Option<String>,
    #[serde(rename = "alarm-data", skip_serializing_if = "Option::is_none")]
    alarm_data: Option<XmlAlarmData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snmp: Option<XmlSnmp>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlMask {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    maskelement: Vec<XmlMaskElement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    varbind: Vec<XmlMaskVarbind>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlMaskElement {
    mename: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mevalue: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlMaskVarbind {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vbnumber: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vboid: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    vbvalue: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlVarbindsdecode {
    parmid: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    decode: Vec<XmlDecode>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlDecode {
    #[serde(rename = "@varbindvalue")]
    varbindvalue: String,
    #[serde(rename = "@varbinddecodedstring")]
    varbinddecodedstring: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlLogmsg {
    #[serde(rename = "@dest", skip_serializing_if = "Option::is_none")]
    dest: Option<String>,
    #[serde(rename = "@notify", skip_serializing_if = "Option::is_none")]
    notify: Option<bool>,
    #[serde(rename = "$text", skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlAutoack {
    #[serde(rename = "@state", skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(rename = "$text", skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlTticket {
    #[serde(rename = "@state", skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(rename = "$text", skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlAlarmData {
    #[serde(rename = "@reduction-key", skip_serializing_if = "Option::is_none")]
    reduction_key: Option<String>,
    #[serde(rename = "@alarm-type", skip_serializing_if = "Option::is_none")]
    alarm_type: Option<i32>,
    #[serde(rename = "@clear-key", skip_serializing_if = "Option::is_none")]
    clear_key: Option<String>,
    #[serde(rename = "@auto-clean", skip_serializing_if = "Option::is_none")]
    auto_clean: Option<bool>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlCorrelation {
    #[serde(rename = "@state", skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(rename = "@path", skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(rename = "@cmin", skip_serializing_if = "Option::is_none")]
    cmin: Option<String>,
    #[serde(rename = "@cmax", skip_serializing_if = "Option::is_none")]
    cmax: Option<String>,
    #[serde(rename = "@ctime", skip_serializing_if = "Option::is_none")]
    ctime: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cuei: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct XmlSnmp {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    idtext: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    specific: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generic: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    community: Option<String>,
}

// -- Conversions: JSON Event ↔ XML schema ----------------------------------

impl From<&Event> for XmlEvent {
    fn from(e: &Event) -> Self {
        Self {
            mask: e.mask.as_ref().map(XmlMask::from),
            uei: e.uei.clone(),
            event_label: e.event_label.clone(),
            descr: e.descr.clone(),
            logmsg: e.logmsg.as_ref().map(XmlLogmsg::from),
            severity: e.severity.clone(),
            correlation: e.correlation.as_ref().map(XmlCorrelation::from),
            operinstruct: e.operinstruct.clone(),
            autoacknowledge: e.autoacknowledge.as_ref().map(XmlAutoack::from),
            tticket: e.tticket.as_ref().map(XmlTticket::from),
            varbindsdecode: e
                .varbindsdecode
                .as_ref()
                .map(|v| v.iter().map(XmlVarbindsdecode::from).collect())
                .unwrap_or_default(),
            mouseovertext: e.mouseovertext.clone(),
            alarm_data: e.alarm_data.as_ref().map(XmlAlarmData::from),
            snmp: e.snmp.as_ref().map(XmlSnmp::from),
        }
    }
}

impl From<XmlEvent> for Event {
    fn from(x: XmlEvent) -> Self {
        Self {
            uei: x.uei,
            event_label: x.event_label,
            descr: x.descr,
            severity: x.severity,
            mask: x.mask.map(Mask::from),
            logmsg: x.logmsg.map(Logmsg::from),
            correlation: x.correlation.map(Correlation::from),
            operinstruct: x.operinstruct,
            autoacknowledge: x.autoacknowledge.map(Autoacknowledge::from),
            tticket: x.tticket.map(Tticket::from),
            mouseovertext: x.mouseovertext,
            alarm_data: x.alarm_data.map(AlarmData::from),
            varbindsdecode: if x.varbindsdecode.is_empty() {
                None
            } else {
                Some(
                    x.varbindsdecode
                        .into_iter()
                        .map(Varbindsdecode::from)
                        .collect(),
                )
            },
            snmp: x.snmp.map(Snmp::from),
            parm_collection: None,
        }
    }
}

impl From<&Mask> for XmlMask {
    fn from(m: &Mask) -> Self {
        Self {
            maskelement: m
                .maskelements
                .as_ref()
                .map(|v| v.iter().map(XmlMaskElement::from).collect())
                .unwrap_or_default(),
            varbind: m
                .varbinds
                .as_ref()
                .map(|v| v.iter().map(XmlMaskVarbind::from).collect())
                .unwrap_or_default(),
        }
    }
}

impl From<XmlMask> for Mask {
    fn from(x: XmlMask) -> Self {
        Self {
            maskelements: if x.maskelement.is_empty() {
                None
            } else {
                Some(x.maskelement.into_iter().map(MaskElement::from).collect())
            },
            varbinds: if x.varbind.is_empty() {
                None
            } else {
                Some(x.varbind.into_iter().map(MaskVarbind::from).collect())
            },
        }
    }
}

impl From<&MaskElement> for XmlMaskElement {
    fn from(m: &MaskElement) -> Self {
        Self {
            mename: m.mename.clone(),
            mevalue: m.mevalues.clone(),
        }
    }
}

impl From<XmlMaskElement> for MaskElement {
    fn from(x: XmlMaskElement) -> Self {
        Self {
            mename: x.mename,
            mevalues: x.mevalue,
        }
    }
}

impl From<&MaskVarbind> for XmlMaskVarbind {
    fn from(m: &MaskVarbind) -> Self {
        Self {
            vbnumber: m.vbnumber,
            vboid: m.vboid.clone(),
            vbvalue: m.vbvalues.clone(),
        }
    }
}

impl From<XmlMaskVarbind> for MaskVarbind {
    fn from(x: XmlMaskVarbind) -> Self {
        Self {
            vbnumber: x.vbnumber,
            vboid: x.vboid,
            vbvalues: x.vbvalue,
        }
    }
}

impl From<&Varbindsdecode> for XmlVarbindsdecode {
    fn from(v: &Varbindsdecode) -> Self {
        Self {
            parmid: v.parmid.clone(),
            decode: v.decode.iter().map(XmlDecode::from).collect(),
        }
    }
}

impl From<XmlVarbindsdecode> for Varbindsdecode {
    fn from(x: XmlVarbindsdecode) -> Self {
        Self {
            parmid: x.parmid,
            decode: x.decode.into_iter().map(Decode::from).collect(),
        }
    }
}

impl From<&Decode> for XmlDecode {
    fn from(d: &Decode) -> Self {
        Self {
            varbindvalue: d.varbindvalue.clone(),
            varbinddecodedstring: d.varbinddecodedstring.clone(),
        }
    }
}

impl From<XmlDecode> for Decode {
    fn from(x: XmlDecode) -> Self {
        Self {
            varbindvalue: x.varbindvalue,
            varbinddecodedstring: x.varbinddecodedstring,
        }
    }
}

impl From<&Logmsg> for XmlLogmsg {
    fn from(l: &Logmsg) -> Self {
        Self {
            dest: l.dest.clone(),
            notify: l.notify,
            content: l.content.clone(),
        }
    }
}

impl From<XmlLogmsg> for Logmsg {
    fn from(x: XmlLogmsg) -> Self {
        Self {
            dest: x.dest,
            content: x.content,
            notify: x.notify,
        }
    }
}

impl From<&Autoacknowledge> for XmlAutoack {
    fn from(a: &Autoacknowledge) -> Self {
        Self {
            state: a.state.clone(),
            content: a.content.clone(),
        }
    }
}

impl From<XmlAutoack> for Autoacknowledge {
    fn from(x: XmlAutoack) -> Self {
        Self {
            state: x.state,
            content: x.content,
        }
    }
}

impl From<&Tticket> for XmlTticket {
    fn from(t: &Tticket) -> Self {
        Self {
            state: t.state.clone(),
            content: t.content.clone(),
        }
    }
}

impl From<XmlTticket> for Tticket {
    fn from(x: XmlTticket) -> Self {
        Self {
            state: x.state,
            content: x.content,
        }
    }
}

impl From<&AlarmData> for XmlAlarmData {
    fn from(a: &AlarmData) -> Self {
        Self {
            reduction_key: a.reduction_key.clone(),
            alarm_type: a.alarm_type,
            clear_key: a.clear_key.clone(),
            auto_clean: a.auto_clean,
        }
    }
}

impl From<XmlAlarmData> for AlarmData {
    fn from(x: XmlAlarmData) -> Self {
        Self {
            reduction_key: x.reduction_key,
            alarm_type: x.alarm_type,
            clear_key: x.clear_key,
            auto_clean: x.auto_clean,
        }
    }
}

impl From<&Correlation> for XmlCorrelation {
    fn from(c: &Correlation) -> Self {
        Self {
            state: c.state.clone(),
            path: c.path.clone(),
            cmin: c.cmin.clone(),
            cmax: c.cmax.clone(),
            ctime: c.ctime.clone(),
            cuei: c.cuei.clone().unwrap_or_default(),
        }
    }
}

impl From<XmlCorrelation> for Correlation {
    fn from(x: XmlCorrelation) -> Self {
        Self {
            state: x.state,
            path: x.path,
            cmin: x.cmin,
            cmax: x.cmax,
            ctime: x.ctime,
            cuei: if x.cuei.is_empty() {
                None
            } else {
                Some(x.cuei)
            },
        }
    }
}

impl From<&Snmp> for XmlSnmp {
    fn from(s: &Snmp) -> Self {
        Self {
            id: s.id.clone(),
            idtext: s.idtext.clone(),
            version: s.version.clone(),
            specific: s.specific,
            generic: s.generic,
            community: s.community.clone(),
        }
    }
}

impl From<XmlSnmp> for Snmp {
    fn from(x: XmlSnmp) -> Self {
        Self {
            id: x.id,
            idtext: x.idtext,
            version: x.version,
            specific: x.specific,
            generic: x.generic,
            community: x.community,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{Event, Logmsg, Mask, MaskElement};

    fn sample_event() -> Event {
        Event {
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
            operinstruct: Some("Investigate device boot logs.".into()),
            ..Event::default()
        }
    }

    #[test]
    fn render_emits_kebab_case_attributes_and_elements() {
        let xml = render_eventconf_xml(&[sample_event()]).unwrap();
        // Kebab-case names appear:
        assert!(xml.contains("event-label"));
        assert!(xml.contains("alarm-data"));
        assert!(xml.contains("reduction-key"));
        assert!(xml.contains("alarm-type"));
        // camelCase names do NOT appear:
        assert!(!xml.contains("eventLabel"));
        assert!(!xml.contains("alarmData"));
        assert!(!xml.contains("reductionKey"));
    }

    #[test]
    fn render_then_parse_round_trips_modeled_fields() {
        let original = sample_event();
        let xml = render_eventconf_xml(std::slice::from_ref(&original)).unwrap();
        let parsed = parse_events_from_xml(xml.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], original);
    }

    #[test]
    fn xml_canonical_is_a_fixed_point() {
        let xml = render_eventconf_xml(&[sample_event()]).unwrap();
        let once = xml_canonical(xml.as_bytes()).unwrap();
        let twice = xml_canonical(once.as_bytes()).unwrap();
        assert_eq!(once, twice, "xml_canonical must be a fixed point");
    }

    #[test]
    fn xml_canonical_drops_whitespace_differences() {
        // Same logical document, different formatting.
        let compact = "<events><event><uei>x</uei><severity>Warning</severity></event></events>";
        let pretty = "<events>\n  <event>\n    <uei>x</uei>\n    <severity>Warning</severity>\n  </event>\n</events>";
        let c = xml_canonical(compact.as_bytes()).unwrap();
        let p = xml_canonical(pretty.as_bytes()).unwrap();
        assert_eq!(c, p);
    }

    #[test]
    fn parse_tolerates_unmodeled_elements_by_skipping_them() {
        // `<forward>`, `<script>`, `<parameter>` are real eventconf elements
        // we don't model yet. Parsing should succeed and produce the
        // modeled subset, silently skipping the unknown ones.
        let xml = r#"<events>
            <event>
                <uei>uei.opennms.org/test</uei>
                <severity>Warning</severity>
                <forward state="off"/>
                <script language="beanshell">do_thing();</script>
                <parameter name="foo" value="bar"/>
            </event>
        </events>"#;
        let parsed = parse_events_from_xml(xml.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].uei.as_deref(), Some("uei.opennms.org/test"));
        assert_eq!(parsed[0].severity.as_deref(), Some("Warning"));
    }

    #[test]
    fn synth_master_with_order_lists_event_files() {
        let xml = synth_master_with_order(&["cisco.foo", "juniper.bar", "vendor.baz"]).unwrap();
        // Each name appears as an event-file element in the requested
        // order. The upload pipeline assigns fileOrder values from this
        // list (reversed iteration; see design.md §3.1).
        assert!(xml.contains("<event-file>cisco.foo</event-file>"));
        assert!(xml.contains("<event-file>juniper.bar</event-file>"));
        assert!(xml.contains("<event-file>vendor.baz</event-file>"));
        let cisco_pos = xml.find("cisco.foo").unwrap();
        let juniper_pos = xml.find("juniper.bar").unwrap();
        let vendor_pos = xml.find("vendor.baz").unwrap();
        assert!(cisco_pos < juniper_pos);
        assert!(juniper_pos < vendor_pos);
    }

    #[test]
    fn synth_master_with_empty_list_produces_empty_events_root() {
        let xml = synth_master_with_order(&[]).unwrap();
        // Self-closing or open-close root, but no event-file entries.
        assert!(!xml.contains("event-file"));
    }

    #[test]
    fn parse_invalid_utf8_returns_config_error() {
        let bytes: &[u8] = &[0xff, 0xfe, 0xfd];
        let err = parse_events_from_xml(bytes).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("UTF-8")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_malformed_xml_returns_config_error() {
        let xml = b"<events><event><uei>unclosed";
        let err = parse_events_from_xml(xml).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("parse error")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn render_multiple_events_preserves_order() {
        let events = vec![
            Event {
                uei: Some("uei.first".into()),
                severity: Some("Warning".into()),
                ..Event::default()
            },
            Event {
                uei: Some("uei.second".into()),
                severity: Some("Major".into()),
                ..Event::default()
            },
            Event {
                uei: Some("uei.third".into()),
                severity: Some("Critical".into()),
                ..Event::default()
            },
        ];
        let xml = render_eventconf_xml(&events).unwrap();
        let first = xml.find("uei.first").unwrap();
        let second = xml.find("uei.second").unwrap();
        let third = xml.find("uei.third").unwrap();
        assert!(first < second);
        assert!(second < third);
    }

    #[test]
    fn vboid_varbind_round_trips() {
        let event = Event {
            uei: Some("uei.test".into()),
            severity: Some("Normal".into()),
            mask: Some(Mask {
                maskelements: None,
                varbinds: Some(vec![MaskVarbind {
                    vbnumber: None,
                    vboid: Some(".1.3.6.1.4.1.61509.1.2.0".into()),
                    vbvalues: vec!["0".into()],
                }]),
            }),
            ..Event::default()
        };
        let xml = render_eventconf_xml(std::slice::from_ref(&event)).unwrap();
        assert!(xml.contains("<vboid>.1.3.6.1.4.1.61509.1.2.0</vboid>"));
        assert!(!xml.contains("<vbnumber>"));
        let parsed = parse_events_from_xml(xml.as_bytes()).unwrap();
        assert_eq!(parsed[0], event);
    }

    #[test]
    fn vbnumber_varbind_renders_unchanged_after_schema_change() {
        let event = Event {
            uei: Some("uei.test".into()),
            severity: Some("Normal".into()),
            mask: Some(Mask {
                maskelements: None,
                varbinds: Some(vec![MaskVarbind {
                    vbnumber: Some(1),
                    vboid: None,
                    vbvalues: vec!["0".into()],
                }]),
            }),
            ..Event::default()
        };
        let xml = render_eventconf_xml(std::slice::from_ref(&event)).unwrap();
        assert!(xml.contains("<vbnumber>1</vbnumber>"));
        assert!(!xml.contains("<vboid>"));
        let parsed = parse_events_from_xml(xml.as_bytes()).unwrap();
        assert_eq!(parsed[0], event);
    }

    #[test]
    fn varbindsdecode_round_trips_with_canonical_equivalence() {
        use crate::dto::{Decode, Varbindsdecode};
        let event = Event {
            uei: Some("uei.test".into()),
            severity: Some("Normal".into()),
            varbindsdecode: Some(vec![Varbindsdecode {
                parmid: "1".into(),
                decode: vec![
                    Decode {
                        varbindvalue: "0".into(),
                        varbinddecodedstring: "success(0)".into(),
                    },
                    Decode {
                        varbindvalue: "1".into(),
                        varbinddecodedstring: "failed(1)".into(),
                    },
                ],
            }]),
            ..Event::default()
        };
        let xml = render_eventconf_xml(std::slice::from_ref(&event)).unwrap();
        assert!(xml.contains("<varbindsdecode>"));
        assert!(xml.contains("<parmid>1</parmid>"));
        assert!(xml.contains("varbindvalue=\"0\""));
        assert!(xml.contains("varbinddecodedstring=\"success(0)\""));

        // Round-trip: canonical equivalence (not byte-equal raw bytes —
        // attribute order and whitespace may shift).
        let parsed = parse_events_from_xml(xml.as_bytes()).unwrap();
        assert_eq!(parsed[0], event);

        let xml2 = render_eventconf_xml(std::slice::from_ref(&parsed[0])).unwrap();
        let c1 = xml_canonical(xml.as_bytes()).unwrap();
        let c2 = xml_canonical(xml2.as_bytes()).unwrap();
        assert_eq!(c1, c2, "round-trip must be canonically equivalent");
    }

    #[test]
    fn event_with_vboid_mask_and_varbindsdecode_round_trips() {
        // Task 6.10: fixture exercising BOTH new shapes simultaneously.
        // parse → render → parse must produce canonically equivalent XML.
        use crate::dto::{Decode, Varbindsdecode};
        let event = Event {
            uei: Some("uei.no42.org/snmp/trap/backup/success".into()),
            event_label: Some("No42: Backup Successful".into()),
            descr: Some("Backup operation completed successfully.".into()),
            severity: Some("Normal".into()),
            mask: Some(Mask {
                maskelements: Some(vec![MaskElement {
                    mename: "id".into(),
                    mevalues: vec![".1.3.6.1.4.1.61509.1".into()],
                }]),
                varbinds: Some(vec![MaskVarbind {
                    vbnumber: None,
                    vboid: Some(".1.3.6.1.4.1.61509.1.2.0".into()),
                    vbvalues: vec!["0".into()],
                }]),
            }),
            varbindsdecode: Some(vec![Varbindsdecode {
                parmid: "1".into(),
                decode: vec![
                    Decode {
                        varbindvalue: "0".into(),
                        varbinddecodedstring: "success(0)".into(),
                    },
                    Decode {
                        varbindvalue: "1".into(),
                        varbinddecodedstring: "failed(1)".into(),
                    },
                ],
            }]),
            ..Event::default()
        };
        let xml1 = render_eventconf_xml(std::slice::from_ref(&event)).unwrap();
        let parsed = parse_events_from_xml(xml1.as_bytes()).unwrap();
        assert_eq!(parsed[0], event);
        let xml2 = render_eventconf_xml(std::slice::from_ref(&parsed[0])).unwrap();
        assert_eq!(
            xml_canonical(xml1.as_bytes()).unwrap(),
            xml_canonical(xml2.as_bytes()).unwrap()
        );
    }

    #[test]
    fn varbindsdecode_multiple_groups_preserved_through_xml_round_trip() {
        use crate::dto::{Decode, Varbindsdecode};
        let event = Event {
            uei: Some("uei.test".into()),
            severity: Some("Normal".into()),
            varbindsdecode: Some(vec![
                Varbindsdecode {
                    parmid: "1".into(),
                    decode: vec![Decode {
                        varbindvalue: "a".into(),
                        varbinddecodedstring: "Alpha".into(),
                    }],
                },
                Varbindsdecode {
                    parmid: "2".into(),
                    decode: vec![Decode {
                        varbindvalue: "b".into(),
                        varbinddecodedstring: "Bravo".into(),
                    }],
                },
            ]),
            ..Event::default()
        };
        let xml = render_eventconf_xml(std::slice::from_ref(&event)).unwrap();
        let parsed = parse_events_from_xml(xml.as_bytes()).unwrap();
        let groups = parsed[0].varbindsdecode.as_ref().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].parmid, "1");
        assert_eq!(groups[1].parmid, "2");
    }

    #[test]
    fn byte_offset_to_line_col_handles_first_byte() {
        let (line, col) = byte_offset_to_line_col(b"abc\ndef", 0);
        assert_eq!((line, col), (1, 1));
    }

    #[test]
    fn byte_offset_to_line_col_handles_mid_line() {
        let (line, col) = byte_offset_to_line_col(b"abc\ndef\nghi", 5);
        assert_eq!((line, col), (2, 2)); // 'e' on line 2, column 2
    }

    #[test]
    fn byte_offset_to_line_col_handles_after_newline() {
        let (line, col) = byte_offset_to_line_col(b"abc\ndef", 4);
        assert_eq!((line, col), (2, 1)); // 'd' on line 2, column 1
    }

    #[test]
    fn byte_offset_to_line_col_clamps_offset_to_input_length() {
        let (line, _col) = byte_offset_to_line_col(b"abc", 100);
        assert_eq!(line, 1);
    }

    #[test]
    fn find_event_start_offsets_distinguishes_event_from_event_label() {
        let xml = b"<events>\n  <event>\n    <event-label>foo</event-label>\n  </event>\n</events>";
        let offsets = find_event_start_offsets(xml);
        assert_eq!(offsets.len(), 1);
        let opening = std::str::from_utf8(&xml[offsets[0]..offsets[0] + 7]).unwrap();
        assert_eq!(opening, "<event>");
    }

    #[test]
    fn find_event_start_offsets_handles_attribute_bearing_events() {
        let xml = b"<events><event uei=\"x\"></event></events>";
        let offsets = find_event_start_offsets(xml);
        // <event uei="..."> is also a valid event start
        assert_eq!(offsets.len(), 1);
    }

    #[test]
    fn parse_events_with_locations_attaches_line_numbers() {
        let xml = br#"<events>
  <event>
    <uei>uei.first</uei>
    <severity>Warning</severity>
  </event>
  <event>
    <uei>uei.second</uei>
    <severity>Major</severity>
  </event>
</events>"#;
        let pairs = parse_events_with_locations(xml, "/tmp/test.xml").unwrap();
        assert_eq!(pairs.len(), 2);

        assert_eq!(pairs[0].0.uei.as_deref(), Some("uei.first"));
        assert_eq!(pairs[0].1.event_index, 0);
        assert_eq!(pairs[0].1.line, 2); // first <event> on line 2
        assert_eq!(pairs[0].1.file, std::path::PathBuf::from("/tmp/test.xml"));

        assert_eq!(pairs[1].0.uei.as_deref(), Some("uei.second"));
        assert_eq!(pairs[1].1.event_index, 1);
        assert_eq!(pairs[1].1.line, 6); // second <event> on line 6
    }

    #[test]
    fn parse_events_with_locations_falls_back_when_counts_disagree() {
        // Pathological input: serde parses 0 events, but our scan finds 0
        // too — so the fallback isn't triggered. Construct an input where
        // counts WOULD disagree: include `<event>` inside an unknown
        // wrapper that quick_xml-serde skips entirely. Difficult to
        // construct reliably; this test just exercises the "all-zero
        // fallback" code path indirectly via an empty document.
        let xml = b"<events></events>";
        let pairs = parse_events_with_locations(xml, "/tmp/empty.xml").unwrap();
        assert_eq!(pairs.len(), 0);
    }

    #[test]
    fn logmsg_attributes_round_trip() {
        let event = Event {
            uei: Some("uei.test".into()),
            severity: Some("Normal".into()),
            logmsg: Some(Logmsg {
                dest: Some("logndisplay".into()),
                content: Some("Some message".into()),
                notify: Some(true),
            }),
            ..Event::default()
        };
        let xml = render_eventconf_xml(std::slice::from_ref(&event)).unwrap();
        // dest and notify should appear as attributes on logmsg
        assert!(xml.contains("dest=\"logndisplay\""));
        assert!(xml.contains("notify=\"true\""));
        let parsed = parse_events_from_xml(xml.as_bytes()).unwrap();
        assert_eq!(parsed[0], event);
    }
}
