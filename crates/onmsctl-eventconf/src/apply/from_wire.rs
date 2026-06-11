/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Wire DTO → local YAML shape conversion.
//!
//! Inverse of [`crate::apply::conversion`]. Takes a deserialized wire
//! [`Event`] (from eventconf XML parse or REST API JSON) and produces an
//! [`EventDef`] suitable for serialization as YAML.
//!
//! ## Fallibility
//!
//! Several local-schema fields are `String`/`i32` (required) where the wire
//! DTO has `Option<...>` (server permits absent). Converting wire → local
//! therefore needs to distinguish "absent on wire" from "present and
//! convertible". Top-level `TryFrom<&Event> for EventDef` returns a
//! [`WireToLocalError`] when a required field is missing; the converter
//! ([`crate::convert`]) maps each variant to a Finding code (EC004) and
//! emits a structured report with source-location citation.
//!
//! ## Data drops
//!
//! Wire fields with no local equivalent are dropped silently in this
//! layer. The converter records them as EC006 warnings via a separate
//! inspection pass on the input. Specifically:
//!
//!   - [`crate::dto::Event::snmp`] (no local model)
//!   - [`crate::dto::Event::parm_collection`] (runtime field, not eventconf)
//!   - [`crate::dto::Tticket::content`] (local `TticketDef` has no `content`)
//!
//! Sub-structs whose `state` field is `None` on the wire produce `None` on
//! the local side (treated as if the sub-struct weren't present at all).
//! This applies to `Tticket`, `Autoacknowledge`, and `Correlation`.

use crate::apply::local::{
    AlarmDataDef, AlarmType, AutoackDef, CorrelationDef, DecodeDef, EventDef, FilterDef,
    ForwardDef, LogmsgDef, MaskDef, MaskElementDef, MaskVarbindDef, ParameterDef, ScriptDef,
    SnmpDef, TticketDef, VarbindsdecodeDef,
};
use crate::dto::{
    AlarmData, Autoacknowledge, Correlation, Decode, Event, Forward, Logmsg, Mask, MaskElement,
    MaskVarbind, Snmp, Tticket, Varbindsdecode,
};

/// Error variants produced when converting a wire [`Event`] to an
/// [`EventDef`]. Each variant maps 1:1 to a Finding code in
/// [`crate::convert`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireToLocalError {
    /// Event has no `<uei>` on the wire; required by the local schema.
    MissingUei,
    /// Event has no `<event-label>` on the wire; required by the local schema.
    MissingLabel,
    /// Event has no `<severity>` on the wire; required by the local schema.
    MissingSeverity,
    /// Event has alarm-data but no `reduction-key` attribute.
    AlarmDataMissingReductionKey,
    /// Event has alarm-data but no `alarm-type` attribute.
    AlarmDataMissingAlarmType,
    /// Event has an `alarm-data` `alarm-type` whose integer value is
    /// outside the accepted set `{1, 2, 3}`. Surfaced as `EC007`
    /// (Error severity) during `event-source convert`.
    AlarmDataAlarmTypeOutOfRange { value: i32 },
    /// Event has a logmsg child but the inner text is empty / absent.
    LogmsgMissingContent,
    /// Event has a logmsg child but no `dest` attribute.
    LogmsgMissingDest,
    /// Mask varbind has neither `vbnumber` nor `vboid` populated.
    MaskVarbindMissingDiscriminator,
}

impl std::fmt::Display for WireToLocalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingUei => write!(f, "event has no <uei> (required by local schema)"),
            Self::MissingLabel => {
                write!(f, "event has no <event-label> (required by local schema)")
            }
            Self::MissingSeverity => {
                write!(f, "event has no <severity> (required by local schema)")
            }
            Self::AlarmDataMissingReductionKey => {
                write!(f, "<alarm-data> is missing the `reduction-key` attribute")
            }
            Self::AlarmDataMissingAlarmType => {
                write!(f, "<alarm-data> is missing the `alarm-type` attribute")
            }
            Self::AlarmDataAlarmTypeOutOfRange { value } => {
                write!(
                    f,
                    "<alarm-data> alarm-type {value} is outside the accepted set \
                     {{1 (raise), 2 (resolution), 3 (unresolvable)}}"
                )
            }
            Self::LogmsgMissingContent => write!(f, "<logmsg> has no inner text"),
            Self::LogmsgMissingDest => write!(f, "<logmsg> has no `dest` attribute"),
            Self::MaskVarbindMissingDiscriminator => write!(
                f,
                "<mask><varbind> has neither <vbnumber> nor <vboid> populated"
            ),
        }
    }
}

impl std::error::Error for WireToLocalError {}

// -- Top-level event conversion --------------------------------------------

impl TryFrom<&Event> for EventDef {
    type Error = WireToLocalError;

    fn try_from(e: &Event) -> Result<Self, Self::Error> {
        Ok(EventDef {
            uei: e.uei.clone().ok_or(WireToLocalError::MissingUei)?,
            label: e
                .event_label
                .clone()
                .ok_or(WireToLocalError::MissingLabel)?,
            severity: e
                .severity
                .clone()
                .ok_or(WireToLocalError::MissingSeverity)?,
            description: e.descr.clone(),
            logmsg: match e.logmsg.as_ref() {
                Some(l) => Some(LogmsgDef::try_from(l)?),
                None => None,
            },
            alarm_data: match e.alarm_data.as_ref() {
                Some(a) => Some(AlarmDataDef::try_from(a)?),
                None => None,
            },
            mask: match e.mask.as_ref() {
                Some(m) => Some(MaskDef::try_from(m)?),
                None => None,
            },
            operinstruct: e.operinstruct.clone().filter(|s| !s.is_empty()),
            mouseovertext: e.mouseovertext.clone().filter(|s| !s.is_empty()),
            autoacknowledge: e.autoacknowledge.as_ref().and_then(AutoackDef::from_wire),
            tticket: e.tticket.as_ref().and_then(TticketDef::from_wire),
            correlation: e.correlation.as_ref().and_then(CorrelationDef::from_wire),
            varbindsdecode: e
                .varbindsdecode
                .as_ref()
                .map(|groups| groups.iter().map(VarbindsdecodeDef::from).collect()),
            snmp: e.snmp.as_ref().map(SnmpDef::from),
            forwards: e
                .forward
                .as_ref()
                .map(|wire| wire.iter().map(ForwardDef::from).collect()),
            filters: e.filters.as_ref().map(|wire_filters| {
                wire_filters
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, f)| {
                        let event_label = e.event_label.as_deref().unwrap_or("?");
                        match (&f.eventparm, &f.pattern, &f.replacement) {
                            // `replacement` is XSD-required (must be
                            // present) but the empty string is a
                            // legitimate value — `Matcher.replaceAll("")`
                            // strips the matched text. Don't reject
                            // empty `replacement`.
                            (Some(ep), Some(p), Some(r)) if !ep.is_empty() && !p.is_empty() => {
                                Some(FilterDef {
                                    eventparm: ep.clone(),
                                    pattern: p.clone(),
                                    replacement: r.clone(),
                                })
                            }
                            _ => {
                                eprintln!(
                                    "warning: event '{event_label}' filter[{idx}] dropped — \
                                     wire shape is missing or has empty `eventparm` or \
                                     `pattern` (both XSD-required and non-empty)"
                                );
                                None
                            }
                        }
                    })
                    .collect::<Vec<_>>()
            }),
            scripts: e.script.as_ref().map(|wire_scripts| {
                wire_scripts
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, s)| {
                        let event_label = e.event_label.as_deref().unwrap_or("?");
                        match &s.language {
                            Some(l) if !l.is_empty() => Some(ScriptDef {
                                language: l.clone(),
                                body: s.body.clone(),
                            }),
                            _ => {
                                // Lossy with visibility: XSD requires
                                // `language`; downloaded XML missing it
                                // can't round-trip through the local
                                // schema. Drop the entry and warn.
                                eprintln!(
                                    "warning: event '{event_label}' script[{idx}] dropped — \
                                     wire shape lacks required `language` attribute"
                                );
                                None
                            }
                        }
                    })
                    .collect::<Vec<_>>()
            }),
            parameters: e.parameter.as_ref().map(|wire_params| {
                wire_params
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, p)| {
                        let event_label = e.event_label.as_deref().unwrap_or("?");
                        match (&p.name, &p.value) {
                            (Some(n), Some(v)) if !n.is_empty() && !v.is_empty() => {
                                Some(ParameterDef {
                                    name: n.clone(),
                                    value: v.clone(),
                                    expand: p.expand,
                                })
                            }
                            _ => {
                                // Lossy with visibility: drop the entry,
                                // emit a structured warning to stderr.
                                eprintln!(
                                    "warning: event '{event_label}' parameter[{idx}] dropped — \
                                     wire shape lacks required name or value (name={:?}, \
                                     value={:?})",
                                    p.name, p.value
                                );
                                None
                            }
                        }
                    })
                    .collect::<Vec<_>>()
            }),
        })
    }
}

impl From<&Forward> for ForwardDef {
    fn from(f: &Forward) -> Self {
        ForwardDef {
            state: f.state.clone(),
            mechanism: f.mechanism.clone(),
            target: f.target.clone(),
        }
    }
}

impl From<&Snmp> for SnmpDef {
    fn from(s: &Snmp) -> Self {
        SnmpDef {
            id: s.id.clone(),
            idtext: s.idtext.clone(),
            version: s.version.clone(),
            specific: s.specific,
            generic: s.generic,
            community: s.community.clone(),
        }
    }
}

// -- Sub-struct conversions ------------------------------------------------

impl TryFrom<&Logmsg> for LogmsgDef {
    type Error = WireToLocalError;

    fn try_from(l: &Logmsg) -> Result<Self, Self::Error> {
        Ok(LogmsgDef {
            dest: l.dest.clone().ok_or(WireToLocalError::LogmsgMissingDest)?,
            text: l
                .content
                .clone()
                .map(|c| c.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or(WireToLocalError::LogmsgMissingContent)?,
            notify: l.notify,
        })
    }
}

impl TryFrom<&AlarmData> for AlarmDataDef {
    type Error = WireToLocalError;

    fn try_from(a: &AlarmData) -> Result<Self, Self::Error> {
        Ok(AlarmDataDef {
            reduction_key: a
                .reduction_key
                .clone()
                .ok_or(WireToLocalError::AlarmDataMissingReductionKey)?,
            alarm_type: {
                let raw = a
                    .alarm_type
                    .ok_or(WireToLocalError::AlarmDataMissingAlarmType)?;
                AlarmType::from_wire(raw)
                    .ok_or(WireToLocalError::AlarmDataAlarmTypeOutOfRange { value: raw })?
            },
            auto_clean: a.auto_clean,
            clear_key: a.clear_key.clone(),
        })
    }
}

impl TryFrom<&Mask> for MaskDef {
    type Error = WireToLocalError;

    fn try_from(m: &Mask) -> Result<Self, Self::Error> {
        let elements = m
            .maskelements
            .as_ref()
            .map(|v| v.iter().map(MaskElementDef::from).collect())
            .unwrap_or_default();
        let varbinds = match m.varbinds.as_ref() {
            Some(v) => v
                .iter()
                .map(MaskVarbindDef::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };
        Ok(MaskDef { elements, varbinds })
    }
}

impl From<&MaskElement> for MaskElementDef {
    fn from(m: &MaskElement) -> Self {
        MaskElementDef {
            name: m.mename.clone(),
            values: m.mevalues.clone(),
        }
    }
}

impl TryFrom<&MaskVarbind> for MaskVarbindDef {
    type Error = WireToLocalError;

    fn try_from(v: &MaskVarbind) -> Result<Self, Self::Error> {
        // The local-schema validator enforces "exactly one of vbnumber/vboid",
        // so an entry with neither is already malformed at the wire layer.
        if v.vbnumber.is_none() && v.vboid.is_none() {
            return Err(WireToLocalError::MaskVarbindMissingDiscriminator);
        }
        Ok(MaskVarbindDef {
            vbnumber: v.vbnumber,
            vboid: v.vboid.clone(),
            values: v.vbvalues.clone(),
        })
    }
}

/// `AutoackDef::from_wire` returns `None` when the wire struct has no
/// `state` value (treats the substruct as absent). When present, the
/// `content` field becomes `text`.
impl AutoackDef {
    pub(crate) fn from_wire(a: &Autoacknowledge) -> Option<Self> {
        a.state.as_ref().map(|state| AutoackDef {
            state: state.clone(),
            text: a.content.clone(),
        })
    }
}

/// `TticketDef::from_wire` returns `None` when the wire struct has no
/// `state` value. The wire layer's `content` field has no local equivalent
/// and is dropped (documented in the module header).
impl TticketDef {
    pub(crate) fn from_wire(t: &Tticket) -> Option<Self> {
        t.state.as_ref().map(|state| TticketDef {
            state: state.clone(),
        })
    }
}

/// `CorrelationDef::from_wire` returns `None` when the wire struct has no
/// `state` value.
impl CorrelationDef {
    pub(crate) fn from_wire(c: &Correlation) -> Option<Self> {
        c.state.as_ref().map(|state| CorrelationDef {
            state: state.clone(),
            path: c.path.clone(),
            cmin: c.cmin.clone(),
            cmax: c.cmax.clone(),
            cuei: c.cuei.clone().unwrap_or_default(),
        })
    }
}

impl From<&Varbindsdecode> for VarbindsdecodeDef {
    fn from(g: &Varbindsdecode) -> Self {
        VarbindsdecodeDef {
            parmid: g.parmid.clone(),
            decode: g.decode.iter().map(DecodeDef::from).collect(),
        }
    }
}

impl From<&Decode> for DecodeDef {
    fn from(d: &Decode) -> Self {
        DecodeDef {
            value: d.varbindvalue.clone(),
            label: d.varbinddecodedstring.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire_event_full() -> Event {
        Event {
            uei: Some("uei.test/foo".into()),
            event_label: Some("Test Event".into()),
            descr: Some("description".into()),
            severity: Some("Warning".into()),
            mask: Some(Mask {
                maskelements: Some(vec![MaskElement {
                    mename: "id".into(),
                    mevalues: vec![".1.2.3".into()],
                }]),
                varbinds: Some(vec![MaskVarbind {
                    vbnumber: Some(1),
                    vboid: None,
                    vbvalues: vec!["0".into()],
                }]),
            }),
            logmsg: Some(Logmsg {
                dest: Some("logndisplay".into()),
                content: Some("log text".into()),
                notify: Some(true),
            }),
            correlation: None,
            operinstruct: Some("do the thing".into()),
            autoacknowledge: Some(Autoacknowledge {
                state: Some("off".into()),
                content: None,
            }),
            tticket: Some(Tticket {
                state: Some("off".into()),
                content: Some("dropped in conversion".into()),
            }),
            mouseovertext: Some("hover".into()),
            alarm_data: Some(AlarmData {
                reduction_key: Some("%uei%".into()),
                alarm_type: Some(1),
                clear_key: None,
                auto_clean: Some(false),
            }),
            varbindsdecode: Some(vec![Varbindsdecode {
                parmid: "1".into(),
                decode: vec![Decode {
                    varbindvalue: "0".into(),
                    varbinddecodedstring: "success(0)".into(),
                }],
            }]),
            snmp: None,
            parameter: None,
            forward: None,
            script: None,
            filters: None,
            parm_collection: None,
        }
    }

    #[test]
    fn full_event_round_trips_modeled_fields() {
        let wire = wire_event_full();
        let local = EventDef::try_from(&wire).unwrap();
        assert_eq!(local.uei, "uei.test/foo");
        assert_eq!(local.label, "Test Event");
        assert_eq!(local.severity, "Warning");
        assert_eq!(local.description.as_deref(), Some("description"));
        assert_eq!(local.operinstruct.as_deref(), Some("do the thing"));
        assert_eq!(local.mouseovertext.as_deref(), Some("hover"));

        let logmsg = local.logmsg.unwrap();
        assert_eq!(logmsg.dest, "logndisplay");
        assert_eq!(logmsg.text, "log text");

        let alarm = local.alarm_data.unwrap();
        assert_eq!(alarm.reduction_key, "%uei%");
        assert_eq!(alarm.alarm_type, AlarmType::Raise);

        let mask = local.mask.unwrap();
        assert_eq!(mask.elements[0].name, "id");
        assert_eq!(mask.elements[0].values, vec![".1.2.3"]);
        assert_eq!(mask.varbinds[0].vbnumber, Some(1));
        assert!(mask.varbinds[0].vboid.is_none());

        let vd = local.varbindsdecode.unwrap();
        assert_eq!(vd[0].parmid, "1");
        assert_eq!(vd[0].decode[0].value, "0");
        assert_eq!(vd[0].decode[0].label, "success(0)");
    }

    #[test]
    fn missing_uei_returns_error() {
        let mut wire = wire_event_full();
        wire.uei = None;
        assert_eq!(
            EventDef::try_from(&wire).unwrap_err(),
            WireToLocalError::MissingUei
        );
    }

    #[test]
    fn missing_label_returns_error() {
        let mut wire = wire_event_full();
        wire.event_label = None;
        assert_eq!(
            EventDef::try_from(&wire).unwrap_err(),
            WireToLocalError::MissingLabel
        );
    }

    #[test]
    fn missing_severity_returns_error() {
        let mut wire = wire_event_full();
        wire.severity = None;
        assert_eq!(
            EventDef::try_from(&wire).unwrap_err(),
            WireToLocalError::MissingSeverity
        );
    }

    #[test]
    fn alarm_data_missing_reduction_key_returns_error() {
        let mut wire = wire_event_full();
        wire.alarm_data.as_mut().unwrap().reduction_key = None;
        assert_eq!(
            EventDef::try_from(&wire).unwrap_err(),
            WireToLocalError::AlarmDataMissingReductionKey
        );
    }

    #[test]
    fn alarm_data_missing_alarm_type_returns_error() {
        let mut wire = wire_event_full();
        wire.alarm_data.as_mut().unwrap().alarm_type = None;
        assert_eq!(
            EventDef::try_from(&wire).unwrap_err(),
            WireToLocalError::AlarmDataMissingAlarmType
        );
    }

    #[test]
    fn mask_varbind_with_neither_discriminator_returns_error() {
        let mut wire = wire_event_full();
        let mvb = &mut wire.mask.as_mut().unwrap().varbinds.as_mut().unwrap()[0];
        mvb.vbnumber = None;
        mvb.vboid = None;
        assert_eq!(
            EventDef::try_from(&wire).unwrap_err(),
            WireToLocalError::MaskVarbindMissingDiscriminator
        );
    }

    #[test]
    fn mask_varbind_with_vboid_only_converts_cleanly() {
        let mut wire = wire_event_full();
        let mvb = &mut wire.mask.as_mut().unwrap().varbinds.as_mut().unwrap()[0];
        mvb.vbnumber = None;
        mvb.vboid = Some(".1.2.3.4".into());
        let local = EventDef::try_from(&wire).unwrap();
        let varbind = &local.mask.unwrap().varbinds[0];
        assert!(varbind.vbnumber.is_none());
        assert_eq!(varbind.vboid.as_deref(), Some(".1.2.3.4"));
    }

    #[test]
    fn tticket_with_no_state_produces_none_silently() {
        let mut wire = wire_event_full();
        wire.tticket.as_mut().unwrap().state = None;
        let local = EventDef::try_from(&wire).unwrap();
        assert!(local.tticket.is_none());
    }

    #[test]
    fn autoacknowledge_with_no_state_produces_none_silently() {
        let mut wire = wire_event_full();
        wire.autoacknowledge.as_mut().unwrap().state = None;
        let local = EventDef::try_from(&wire).unwrap();
        assert!(local.autoacknowledge.is_none());
    }

    #[test]
    fn correlation_with_no_state_produces_none_silently() {
        let mut wire = wire_event_full();
        wire.correlation = Some(Correlation {
            state: None,
            path: Some("/p".into()),
            cmin: None,
            cmax: None,
            ctime: None,
            cuei: None,
        });
        let local = EventDef::try_from(&wire).unwrap();
        assert!(local.correlation.is_none());
    }

    #[test]
    fn tticket_content_is_dropped() {
        // wire.tticket.content has no local equivalent — should be dropped
        // without producing an error or affecting other fields.
        let wire = wire_event_full();
        let local = EventDef::try_from(&wire).unwrap();
        assert_eq!(local.tticket.unwrap().state, "off");
    }

    #[test]
    fn empty_operinstruct_becomes_none() {
        // <operinstruct></operinstruct> in XML produces Some("") on the wire.
        // The local schema treats absence and empty equivalently — collapse
        // to None for cleaner YAML output.
        let mut wire = wire_event_full();
        wire.operinstruct = Some(String::new());
        let local = EventDef::try_from(&wire).unwrap();
        assert!(local.operinstruct.is_none());
    }

    #[test]
    fn logmsg_text_is_trimmed_during_conversion() {
        let mut wire = wire_event_full();
        wire.logmsg.as_mut().unwrap().content = Some("\n        leading and trailing\n    ".into());
        let local = EventDef::try_from(&wire).unwrap();
        assert_eq!(local.logmsg.unwrap().text, "leading and trailing");
    }
}
