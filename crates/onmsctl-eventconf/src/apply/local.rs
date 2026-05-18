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

use onmsctl_core::{Error, Result};

// -- Top-level shape --------------------------------------------------------

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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlarmDataDef {
    pub reduction_key: String,
    /// Alarm semantic. Must be one of:
    ///   - `1` — Problem: raises an alarm, paired with a Resolution event
    ///   - `2` — Resolution: clears a paired Problem alarm by `reductionKey`
    ///   - `3` — Unresolvable: a Problem-class event with no auto-clear from
    ///     the device. Common for hardware-failure traps. Requires manual
    ///     close or an alarmd cleanup policy.
    pub alarm_type: i32,
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
        if !matches!(self.alarm_type, 1 | 2 | 3) {
            return Err(Error::Config(format!(
                "spec.events[{idx}].alarmData.alarmType must be 1 (Problem), 2 (Resolution), or 3 (Unresolvable); got {}",
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
            Error::Config(m) => {
                assert!(m.contains("got 7"));
                // Error message must enumerate all three valid semantics so
                // the operator knows the full accepted set.
                assert!(m.contains("Problem"));
                assert!(m.contains("Resolution"));
                assert!(m.contains("Unresolvable"));
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
        assert_eq!(alarm.alarm_type, 3);
    }

    #[test]
    fn rejects_alarm_type_4_with_message_listing_all_valid_values() {
        // Hard-reject anything outside {1, 2, 3} in v0.1. If OpenNMS adds
        // alarm type 4 in the future, a follow-up change can widen the set.
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
        alarmType: 4
"#;
        let err = EventSourceLocal::from_yaml(yaml.as_bytes()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("got 4"));
                assert!(m.contains("Problem"));
                assert!(m.contains("Resolution"));
                assert!(m.contains("Unresolvable"));
            }
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
            ("minimal.yaml", |l| {
                assert_eq!(l.spec.events.len(), 1, "minimal.yaml: one event");
                assert!(l.spec.enabled, "minimal.yaml: enabled defaults true");
            }),
            ("full.yaml", |l| {
                let e = l.spec.events.first().expect("full.yaml: ≥1 event");
                // Every modeled nested type must populate so the fixture
                // continues to demonstrate the full schema surface.
                assert!(e.mask.is_some(), "full.yaml: mask must populate");
                assert!(e.alarm_data.is_some(), "full.yaml: alarmData must populate");
                assert!(e.logmsg.is_some(), "full.yaml: logmsg must populate");
                assert!(
                    e.correlation.is_some(),
                    "full.yaml: correlation must populate"
                );
                assert!(
                    e.autoacknowledge.is_some(),
                    "full.yaml: autoacknowledge must populate"
                );
                assert!(e.tticket.is_some(), "full.yaml: tticket must populate");
                assert!(
                    e.mouseovertext.is_some(),
                    "full.yaml: mouseovertext must populate"
                );
                assert!(
                    e.operinstruct.is_some(),
                    "full.yaml: operinstruct must populate"
                );
                let m = e.mask.as_ref().unwrap();
                assert!(!m.elements.is_empty(), "full.yaml: mask.elements non-empty");
                assert!(!m.varbinds.is_empty(), "full.yaml: mask.varbinds non-empty");
                // Both vbnumber-style and vboid-style varbinds must
                // appear so the fixture exercises both mask discriminators.
                assert!(
                    m.varbinds.iter().any(|v| v.vbnumber.is_some()),
                    "full.yaml: at least one vbnumber-style varbind"
                );
                assert!(
                    m.varbinds.iter().any(|v| v.vboid.is_some()),
                    "full.yaml: at least one vboid-style varbind"
                );
                assert!(
                    e.varbindsdecode.is_some(),
                    "full.yaml: varbindsdecode must populate"
                );
                assert!(
                    !e.varbindsdecode.as_ref().unwrap().is_empty(),
                    "full.yaml: varbindsdecode non-empty"
                );
            }),
            ("severities.yaml", |l| {
                assert_eq!(l.spec.events.len(), 7, "severities.yaml: 7 levels");
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
                        "severities.yaml: missing {expected}",
                    );
                }
            }),
            ("disabled.yaml", |l| {
                assert!(!l.spec.enabled, "disabled.yaml: spec.enabled must be false");
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
}
