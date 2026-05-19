/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Conversion between user-facing YAML shapes (`EventSourceLocal`,
//! `EventDef`, ...) and the wire-format `Event` DTO.
//!
//! User-facing YAML uses friendlier names (`label`, `text`, `name`,
//! `values`); the wire DTO uses the OpenNMS-canonical names
//! (`eventLabel`, `content`, `mename`, `mevalues`). This module is the
//! one place that translation lives.

use crate::apply::local::{
    AlarmDataDef, AutoackDef, CorrelationDef, DecodeDef, EventDef, FilterDef, ForwardDef,
    LogmsgDef, MaskDef, ParameterDef, ScriptDef, SnmpDef, TticketDef, VarbindsdecodeDef,
};
use crate::dto::{
    AlarmData, Autoacknowledge, Correlation, Decode, Event, Filter, Forward, Logmsg, Mask,
    MaskElement, MaskVarbind, Parameter, Script, Snmp, Tticket, Varbindsdecode,
};

impl From<&EventDef> for Event {
    fn from(e: &EventDef) -> Self {
        Event {
            uei: Some(e.uei.clone()),
            event_label: Some(e.label.clone()),
            descr: e.description.clone(),
            severity: Some(e.severity.clone()),
            mask: e.mask.as_ref().map(Mask::from),
            logmsg: e.logmsg.as_ref().map(Logmsg::from),
            correlation: e.correlation.as_ref().map(Correlation::from),
            operinstruct: e.operinstruct.clone(),
            autoacknowledge: e.autoacknowledge.as_ref().map(Autoacknowledge::from),
            tticket: e.tticket.as_ref().map(Tticket::from),
            mouseovertext: e.mouseovertext.clone(),
            alarm_data: e.alarm_data.as_ref().map(AlarmData::from),
            varbindsdecode: e
                .varbindsdecode
                .as_ref()
                .map(|groups| groups.iter().map(Varbindsdecode::from).collect()),
            snmp: e.snmp.as_ref().map(Snmp::from),
            parameter: e
                .parameters
                .as_ref()
                .map(|params| params.iter().map(Parameter::from).collect()),
            forward: e
                .forwards
                .as_ref()
                .map(|v| v.iter().map(Forward::from).collect()),
            script: e
                .scripts
                .as_ref()
                .map(|v| v.iter().map(Script::from).collect()),
            filters: e
                .filters
                .as_ref()
                .map(|v| v.iter().map(Filter::from).collect()),
            parm_collection: None,
        }
    }
}

impl From<&FilterDef> for Filter {
    fn from(f: &FilterDef) -> Self {
        Filter {
            eventparm: Some(f.eventparm.clone()),
            pattern: Some(f.pattern.clone()),
            replacement: Some(f.replacement.clone()),
        }
    }
}

impl From<&ForwardDef> for Forward {
    fn from(f: &ForwardDef) -> Self {
        Forward {
            state: f.state.clone(),
            mechanism: f.mechanism.clone(),
            target: f.target.clone(),
        }
    }
}

impl From<&ScriptDef> for Script {
    fn from(s: &ScriptDef) -> Self {
        Script {
            language: Some(s.language.clone()),
            body: s.body.clone(),
        }
    }
}

impl From<&ParameterDef> for Parameter {
    fn from(p: &ParameterDef) -> Self {
        Parameter {
            name: Some(p.name.clone()),
            value: Some(p.value.clone()),
            expand: p.expand,
        }
    }
}

impl From<&SnmpDef> for Snmp {
    fn from(s: &SnmpDef) -> Self {
        Snmp {
            id: s.id.clone(),
            idtext: s.idtext.clone(),
            version: s.version.clone(),
            specific: s.specific,
            generic: s.generic,
            community: s.community.clone(),
        }
    }
}

impl From<&VarbindsdecodeDef> for Varbindsdecode {
    fn from(g: &VarbindsdecodeDef) -> Self {
        Varbindsdecode {
            parmid: g.parmid.clone(),
            decode: g.decode.iter().map(Decode::from).collect(),
        }
    }
}

impl From<&DecodeDef> for Decode {
    fn from(d: &DecodeDef) -> Self {
        Decode {
            varbindvalue: d.value.clone(),
            varbinddecodedstring: d.label.clone(),
        }
    }
}

impl From<&LogmsgDef> for Logmsg {
    fn from(l: &LogmsgDef) -> Self {
        Logmsg {
            dest: Some(l.dest.clone()),
            content: Some(l.text.clone()),
            notify: l.notify,
        }
    }
}

impl From<&AlarmDataDef> for AlarmData {
    fn from(a: &AlarmDataDef) -> Self {
        AlarmData {
            reduction_key: Some(a.reduction_key.clone()),
            alarm_type: Some(a.alarm_type.to_wire()),
            clear_key: a.clear_key.clone(),
            auto_clean: a.auto_clean,
        }
    }
}

impl From<&MaskDef> for Mask {
    fn from(m: &MaskDef) -> Self {
        Mask {
            maskelements: if m.elements.is_empty() {
                None
            } else {
                Some(
                    m.elements
                        .iter()
                        .map(|e| MaskElement {
                            mename: e.name.clone(),
                            mevalues: e.values.clone(),
                        })
                        .collect(),
                )
            },
            varbinds: if m.varbinds.is_empty() {
                None
            } else {
                Some(
                    m.varbinds
                        .iter()
                        .map(|v| MaskVarbind {
                            vbnumber: v.vbnumber,
                            vboid: v.vboid.clone(),
                            vbvalues: v.values.clone(),
                        })
                        .collect(),
                )
            },
        }
    }
}

impl From<&AutoackDef> for Autoacknowledge {
    fn from(a: &AutoackDef) -> Self {
        Autoacknowledge {
            state: Some(a.state.clone()),
            content: a.text.clone(),
        }
    }
}

impl From<&TticketDef> for Tticket {
    fn from(t: &TticketDef) -> Self {
        Tticket {
            state: Some(t.state.clone()),
            content: None,
        }
    }
}

impl From<&CorrelationDef> for Correlation {
    fn from(c: &CorrelationDef) -> Self {
        Correlation {
            state: Some(c.state.clone()),
            path: c.path.clone(),
            cmin: c.cmin.clone(),
            cmax: c.cmax.clone(),
            ctime: None,
            cuei: if c.cuei.is_empty() {
                None
            } else {
                Some(c.cuei.clone())
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::local::{
        AlarmDataDef, AlarmType, EventDef, LogmsgDef, MaskDef, MaskElementDef, MaskVarbindDef,
    };

    #[test]
    fn event_def_maps_to_wire_event_with_renames() {
        let e = EventDef {
            uei: "uei.opennms.org/test".into(),
            label: "Test".into(),
            severity: "Warning".into(),
            description: Some("descr".into()),
            logmsg: Some(LogmsgDef {
                dest: "logndisplay".into(),
                text: "%nodelabel% test".into(),
                notify: Some(true),
            }),
            alarm_data: Some(AlarmDataDef {
                reduction_key: "%uei%".into(),
                alarm_type: AlarmType::Raise,
                auto_clean: Some(false),
                clear_key: None,
            }),
            mask: Some(MaskDef {
                elements: vec![MaskElementDef {
                    name: "id".into(),
                    values: vec!["1.2.3".into()],
                }],
                varbinds: vec![MaskVarbindDef {
                    vbnumber: Some(1),
                    vboid: None,
                    values: vec!["3".into()],
                }],
            }),
            ..EventDef::default()
        };
        let wire: Event = Event::from(&e);

        // Top-level renames
        assert_eq!(wire.event_label.as_deref(), Some("Test"));
        assert_eq!(wire.descr.as_deref(), Some("descr"));
        assert_eq!(wire.severity.as_deref(), Some("Warning"));

        // logmsg.text → content
        assert_eq!(
            wire.logmsg.as_ref().unwrap().content.as_deref(),
            Some("%nodelabel% test")
        );
        assert_eq!(
            wire.logmsg.as_ref().unwrap().dest.as_deref(),
            Some("logndisplay")
        );

        // alarmData
        assert_eq!(
            wire.alarm_data.as_ref().unwrap().reduction_key.as_deref(),
            Some("%uei%")
        );
        assert_eq!(wire.alarm_data.as_ref().unwrap().alarm_type, Some(1));

        // mask: name/values → mename/mevalues
        let mask = wire.mask.as_ref().unwrap();
        let me = &mask.maskelements.as_ref().unwrap()[0];
        assert_eq!(me.mename, "id");
        assert_eq!(me.mevalues, vec!["1.2.3"]);
        let vb = &mask.varbinds.as_ref().unwrap()[0];
        assert_eq!(vb.vbnumber, Some(1));
        assert!(vb.vboid.is_none());
        assert_eq!(vb.vbvalues, vec!["3"]);
    }

    #[test]
    fn empty_mask_collections_become_none_on_wire() {
        let m = MaskDef::default();
        let wire = Mask::from(&m);
        assert!(wire.maskelements.is_none());
        assert!(wire.varbinds.is_none());
    }

    #[test]
    fn unset_optional_fields_stay_none_on_wire() {
        let e = EventDef {
            uei: "uei.foo".into(),
            label: "L".into(),
            severity: "Normal".into(),
            ..EventDef::default()
        };
        let wire = Event::from(&e);
        assert!(wire.descr.is_none());
        assert!(wire.logmsg.is_none());
        assert!(wire.mask.is_none());
        assert!(wire.alarm_data.is_none());
        assert!(wire.operinstruct.is_none());
        assert!(wire.mouseovertext.is_none());
        assert!(wire.autoacknowledge.is_none());
        assert!(wire.tticket.is_none());
        assert!(wire.correlation.is_none());
    }
}
