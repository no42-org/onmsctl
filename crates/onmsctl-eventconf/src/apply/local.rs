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
//!     tticket, correlation).
//!   - Rare elements (varbindsdecode, parameter, forward, script, snmp)
//!     are NOT modeled in v0.1. The spec amendment in `tasks.md`'s
//!     deferred-work tracker carries this forward.
//!
//! User-friendly field names are preserved (`label` not `eventLabel`,
//! `text` not `content`, `name`/`values` not `mename`/`mevalues`).
//! Conversion to the wire-format `Event` DTO lives in
//! [`crate::apply::conversion`].

use serde::{Deserialize, Serialize};

use onmsctl_core::{Error, Result};

// -- Top-level shape --------------------------------------------------------

/// The kubectl-style document the user authors. Validated at load time.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventSourceLocal {
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: EventSourceSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
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
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogmsgDef {
    pub dest: String,
    /// Human-readable substitution template. Maps to `content` on the wire.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlarmDataDef {
    pub reduction_key: String,
    /// 1 = problem, 2 = resolution.
    pub alarm_type: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_clean: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_key: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaskDef {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<MaskElementDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub varbinds: Vec<MaskVarbindDef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaskElementDef {
    /// Element name. Maps to `mename` on the wire.
    pub name: String,
    /// Match values. Maps to `mevalues` on the wire.
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaskVarbindDef {
    pub vbnumber: i32,
    /// Match values. Maps to `vbvalues` on the wire.
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutoackDef {
    /// "on" or "off".
    pub state: String,
    /// Maps to `content` on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TticketDef {
    /// "on" or "off".
    pub state: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
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
        // Reject multi-document YAML up front. `serde_norway::from_slice`
        // silently parses only the first document; a user concatenating
        // two EventSource docs into one file would otherwise lose all
        // but the first.
        if has_multiple_documents(bytes) {
            return Err(Error::Config(
                "multi-document YAML is not supported (one EventSource per file). \
                 Split documents into separate files."
                    .into(),
            ));
        }

        match serde_norway::from_slice::<Self>(bytes) {
            Ok(local) => {
                local.validate()?;
                Ok(local)
            }
            Err(strict_err) => {
                // Strict parse failed. Try a recovery pass to produce a
                // guided message for the well-known reserved spec.* keys.
                if let Some(guided) = guided_rejection_for_known_spec_keys(bytes) {
                    return Err(guided);
                }
                Err(Error::Config(format!(
                    "invalid EventSource YAML: {strict_err}"
                )))
            }
        }
    }
}

/// Detect whether `bytes` contains more than one YAML document
/// (separated by `---` lines). Counts only top-level document
/// separators; CDATA-style or string-literal `---` inside a document
/// don't count. Conservative: if the parser sees more than one document
/// stream entry, report `true`.
fn has_multiple_documents(bytes: &[u8]) -> bool {
    use serde_norway::Deserializer;
    let count = Deserializer::from_slice(bytes).count();
    count > 1
}

/// Inspect a parsed-as-Value YAML document for keys under `spec` that
/// are explicitly forbidden by the schema. Emits guided error messages
/// per the `event-conf` spec scenarios.
fn guided_rejection_for_known_spec_keys(bytes: &[u8]) -> Option<Error> {
    let raw: serde_norway::Value = serde_norway::from_slice(bytes).ok()?;
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
                         (the upload endpoint blanks it). Use `onmsctl source create --description ...` \
                         at first creation."
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
                 To remove a source, use `onmsctl source delete <id>`."
                    .into(),
            ));
        }
        for (i, e) in self.spec.events.iter().enumerate() {
            e.validate(i)?;
        }
        // Reject duplicate UEIs at parse time so the user gets a clear
        // error rather than an opaque duplicate-cluster diff at apply
        // time. Even though the diff algorithm tolerates clusters,
        // creating one in YAML is almost always a mistake.
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for (i, e) in self.spec.events.iter().enumerate() {
            if !seen.insert(&e.uei) {
                return Err(Error::Config(format!(
                    "spec.events[{i}].uei '{}' is a duplicate; declare each UEI at most once per source",
                    e.uei
                )));
            }
        }
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
    if !name.contains('.') {
        return Err(Error::Config(format!(
            "metadata.name '{name}' must contain at least one '.' (the prefix becomes the server-derived vendor)"
        )));
    }
    let vendor = name.split('.').next().unwrap_or("");
    if vendor.is_empty() {
        return Err(Error::Config(format!(
            "metadata.name '{name}' has empty vendor segment (prefix before first '.')"
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
        if !matches!(self.alarm_type, 1 | 2) {
            return Err(Error::Config(format!(
                "spec.events[{idx}].alarmData.alarmType must be 1 (problem) or 2 (resolution); got {}",
                self.alarm_type
            )));
        }
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
            if vb.vbnumber <= 0 {
                return Err(Error::Config(format!(
                    "spec.events[{idx}].mask.varbinds[{j}].vbnumber must be positive; got {}",
                    vb.vbnumber
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
    fn rejects_name_without_dot() {
        let yaml = minimal_yaml().replace("cisco.foo", "nodot");
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("must contain at least one '.'")),
            other => panic!("unexpected {other:?}"),
        }
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
    fn rejects_alarm_type_not_in_set() {
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
            Error::Config(m) => assert!(m.contains("alarmType must be 1") || m.contains("got 7")),
            other => panic!("unexpected {other:?}"),
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
        assert_eq!(e.alarm_data.as_ref().unwrap().alarm_type, 1);
        assert_eq!(e.mask.as_ref().unwrap().elements.len(), 2);
        assert_eq!(
            e.mask.as_ref().unwrap().elements[0].values,
            vec!["1.3.6.1.4.1.9.9.41.2.0.1"]
        );
        assert_eq!(e.autoacknowledge.as_ref().unwrap().state, "on");
        assert_eq!(e.correlation.as_ref().unwrap().cuei.len(), 1);
    }
}
