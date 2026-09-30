/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local (YAML) model for `kind: ThresholdGroup` and `kind: ThreshdPackage`,
//! and its validation.
//!
//! Field names follow the wire DTOs in camelCase. The deliberate differences
//! (design D4): `value`/`rearm`/`trigger` accept a number or a string,
//! `interval` accepts a duration or integer milliseconds, a service binds its
//! group with `thresholdingGroup`, and the server-managed `version`,
//! `readOnly`, `outageCalendars` and `userDefined` are not part of the model.

use std::net::IpAddr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use onmsctl_core::{Error, RawDoc, Result, parse_duration};

use crate::dto::THRESHOLDING_GROUP_PARAM;

pub const API_VERSION: &str = "thresholding.opennms.org/v1";
pub const GROUP_KIND: &str = "ThresholdGroup";
pub const PACKAGE_KIND: &str = "ThreshdPackage";

/// The five threshold types the server accepts.
pub const THRESHOLD_TYPES: &[&str] = &[
    "high",
    "low",
    "relativeChange",
    "absoluteChange",
    "rearmingAbsoluteChange",
];

/// RRDtool's datasource-name limit, enforced by the server too.
pub const MAX_DS_NAME: usize = 19;

// -- shared ------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
}

/// A number or a string. The server stores `value`, `rearm` and `trigger` as
/// strings so they can hold metadata references such as `${scv:key:value}`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum Scalar {
    Int(i64),
    Float(f64),
    Str(String),
}

impl Scalar {
    /// The wire string. Numbers render in their shortest form (`90`, `0.5`);
    /// strings pass through verbatim.
    pub fn to_wire(&self) -> String {
        match self {
            Scalar::Int(n) => n.to_string(),
            Scalar::Float(f) => f.to_string(),
            Scalar::Str(s) => s.clone(),
        }
    }
}

// -- ThresholdGroup ----------------------------------------------------------

/// A `ThresholdGroup` document: one threshold group.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThresholdGroupLocal {
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: ThresholdGroupSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThresholdGroupSpec {
    /// Directory holding the RRD files this group thresholds against.
    pub rrd_repository: String,
    /// Basic thresholds, in evaluation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thresholds: Vec<ThresholdDef>,
    /// Expression thresholds, in evaluation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expressions: Vec<ExpressionDef>,
}

/// A basic threshold on one collected datasource.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThresholdDef {
    #[serde(rename = "type")]
    pub kind: String,
    pub ds_type: String,
    /// Collected datasource name, at most 19 characters.
    pub ds_name: String,
    pub value: Scalar,
    pub rearm: Scalar,
    pub trigger: Scalar,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ds_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr_label: Option<String>,
    #[serde(
        default,
        rename = "triggeredUEI",
        skip_serializing_if = "Option::is_none"
    )]
    pub triggered_uei: Option<String>,
    #[serde(
        default,
        rename = "rearmedUEI",
        skip_serializing_if = "Option::is_none"
    )]
    pub rearmed_uei: Option<String>,
    /// `and` or `or` (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_operator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relaxed: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_filters: Vec<ResourceFilterDef>,
}

/// A threshold on an expression over one or more datasources.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpressionDef {
    #[serde(rename = "type")]
    pub kind: String,
    pub ds_type: String,
    pub expression: String,
    pub value: Scalar,
    pub rearm: Scalar,
    pub trigger: Scalar,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ds_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr_label: Option<String>,
    #[serde(
        default,
        rename = "triggeredUEI",
        skip_serializing_if = "Option::is_none"
    )]
    pub triggered_uei: Option<String>,
    #[serde(
        default,
        rename = "rearmedUEI",
        skip_serializing_if = "Option::is_none"
    )]
    pub rearmed_uei: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_operator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relaxed: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_filters: Vec<ResourceFilterDef>,
}

/// A Java regular expression matched against one resource field.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceFilterDef {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Borrowed view of the fields thresholds and expressions share, so
/// validation and conversion are written once.
pub struct CommonRef<'a> {
    pub kind: &'a str,
    pub ds_type: &'a str,
    pub value: &'a Scalar,
    pub rearm: &'a Scalar,
    pub trigger: &'a Scalar,
    pub description: &'a Option<String>,
    pub ds_label: &'a Option<String>,
    pub expr_label: &'a Option<String>,
    pub triggered_uei: &'a Option<String>,
    pub rearmed_uei: &'a Option<String>,
    pub filter_operator: &'a Option<String>,
    pub relaxed: Option<bool>,
    pub resource_filters: &'a [ResourceFilterDef],
}

macro_rules! common_ref {
    ($t:ty) => {
        impl $t {
            pub fn common(&self) -> CommonRef<'_> {
                CommonRef {
                    kind: &self.kind,
                    ds_type: &self.ds_type,
                    value: &self.value,
                    rearm: &self.rearm,
                    trigger: &self.trigger,
                    description: &self.description,
                    ds_label: &self.ds_label,
                    expr_label: &self.expr_label,
                    triggered_uei: &self.triggered_uei,
                    rearmed_uei: &self.rearmed_uei,
                    filter_operator: &self.filter_operator,
                    relaxed: self.relaxed,
                    resource_filters: &self.resource_filters,
                }
            }
        }
    };
}
common_ref!(ThresholdDef);
common_ref!(ExpressionDef);

impl ThresholdGroupLocal {
    /// Strictly parse one router document and validate it.
    pub fn from_raw(doc: &RawDoc) -> Result<Self> {
        let local: Self = parse_raw(doc, GROUP_KIND)?;
        local.validate().map_err(|e| label(doc, e))?;
        Ok(local)
    }

    /// Every local rule of design D4. No HTTP.
    pub fn validate(&self) -> Result<()> {
        check_envelope(&self.api_version, &self.kind, GROUP_KIND)?;
        check_name(&self.metadata.name)?;
        if self.spec.rrd_repository.trim().is_empty() {
            return Err(cfg("spec.rrdRepository is empty"));
        }
        for (i, t) in self.spec.thresholds.iter().enumerate() {
            let at = format!("spec.thresholds[{i}]");
            if t.ds_name.is_empty() {
                return Err(cfg(format!("{at}.dsName is empty")));
            }
            if t.ds_name.chars().count() > MAX_DS_NAME {
                return Err(cfg(format!(
                    "{at}.dsName '{}' is longer than {MAX_DS_NAME} characters; RRDtool cannot store it",
                    t.ds_name
                )));
            }
            check_common(&at, &t.common())?;
        }
        for (i, e) in self.spec.expressions.iter().enumerate() {
            let at = format!("spec.expressions[{i}]");
            if e.expression.trim().is_empty() {
                return Err(cfg(format!("{at}.expression is empty")));
            }
            check_common(&at, &e.common())?;
        }
        Ok(())
    }

    /// Every `(path, dsType)` in the group, for the plan-time `dsType` gate.
    pub fn ds_types(&self) -> Vec<(String, &str)> {
        let t = self
            .spec
            .thresholds
            .iter()
            .enumerate()
            .map(|(i, t)| (format!("spec.thresholds[{i}]"), t.ds_type.as_str()));
        let e = self
            .spec
            .expressions
            .iter()
            .enumerate()
            .map(|(i, e)| (format!("spec.expressions[{i}]"), e.ds_type.as_str()));
        t.chain(e).collect()
    }
}

fn check_common(at: &str, c: &CommonRef<'_>) -> Result<()> {
    if !THRESHOLD_TYPES.contains(&c.kind) {
        return Err(cfg(format!(
            "{at}.type '{}' is not one of {}",
            c.kind,
            THRESHOLD_TYPES.join(", ")
        )));
    }
    if c.ds_type.trim().is_empty() {
        return Err(cfg(format!("{at}.dsType is empty")));
    }
    if let Some(op) = c.filter_operator
        && op != "and"
        && op != "or"
    {
        return Err(cfg(format!(
            "{at}.filterOperator '{op}' must be 'and' or 'or'"
        )));
    }
    for (field, v) in [("value", c.value), ("rearm", c.rearm)] {
        if v.to_wire().trim().is_empty() {
            return Err(cfg(format!("{at}.{field} is empty")));
        }
    }
    if !trigger_ok(c.trigger) {
        return Err(cfg(format!(
            "{at}.trigger '{}' must be a positive integer or a ${{...}} metadata reference",
            c.trigger.to_wire()
        )));
    }
    for (j, f) in c.resource_filters.iter().enumerate() {
        if f.field.trim().is_empty() {
            return Err(cfg(format!("{at}.resourceFilters[{j}].field is empty")));
        }
    }
    Ok(())
}

fn trigger_ok(t: &Scalar) -> bool {
    match t {
        Scalar::Int(n) => *n > 0,
        Scalar::Float(_) => false,
        Scalar::Str(s) => {
            let s = s.trim();
            s.parse::<u64>().is_ok_and(|n| n > 0) || (s.starts_with("${") && s.ends_with('}'))
        }
    }
}

// -- ThreshdPackage ----------------------------------------------------------

/// A `ThreshdPackage` document: one threshd package.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreshdPackageLocal {
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: ThreshdPackageSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreshdPackageSpec {
    /// Filter rule selecting the interfaces this package applies to.
    pub filter: String,
    /// Individual IP addresses always included.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specifics: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_ranges: Vec<IpRangeDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_ranges: Vec<IpRangeDef>,
    /// URLs of files listing further addresses to include.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ServiceDef>,
}

/// An inclusive range of IP addresses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IpRangeDef {
    pub begin: String,
    pub end: String,
}

/// A service thresholded within the package.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceDef {
    pub name: String,
    /// Time between evaluations: a duration (`30s`, `5m`, `1h`, `1d`) or
    /// integer milliseconds.
    pub interval: Interval,
    /// `on` or `off`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The threshold group to apply (the `thresholding-group` parameter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thresholding_group: Option<String>,
    /// Other service parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ParameterDef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterDef {
    pub key: String,
    pub value: String,
}

/// A duration string or integer milliseconds.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum Interval {
    Millis(i64),
    Duration(String),
}

impl Interval {
    /// Milliseconds, or a message saying why the value is invalid.
    pub fn millis(&self) -> std::result::Result<i64, String> {
        let ms = match self {
            Interval::Millis(n) => *n,
            Interval::Duration(s) => {
                let d = parse_duration(s)?;
                i64::try_from(d.as_millis()).map_err(|_| "overflow".to_string())?
            }
        };
        if ms <= 0 {
            return Err(format!("{ms} ms is not greater than 0"));
        }
        Ok(ms)
    }

    /// The most readable exact form of `ms`: the largest of `d`, `h`, `m`,
    /// `s` that divides it, else plain milliseconds.
    pub fn from_millis(ms: i64) -> Self {
        for (unit, per) in [
            ("d", 86_400_000),
            ("h", 3_600_000),
            ("m", 60_000),
            ("s", 1_000),
        ] {
            if ms > 0 && ms % per == 0 {
                return Interval::Duration(format!("{}{unit}", ms / per));
            }
        }
        Interval::Millis(ms)
    }
}

impl ThreshdPackageLocal {
    /// Strictly parse one router document and validate it.
    pub fn from_raw(doc: &RawDoc) -> Result<Self> {
        let local: Self = parse_raw(doc, PACKAGE_KIND)?;
        local.validate().map_err(|e| label(doc, e))?;
        Ok(local)
    }

    /// Every local rule of design D4. No HTTP.
    pub fn validate(&self) -> Result<()> {
        check_envelope(&self.api_version, &self.kind, PACKAGE_KIND)?;
        check_name(&self.metadata.name)?;
        if self.spec.filter.trim().is_empty() {
            return Err(cfg("spec.filter is empty"));
        }
        for (i, s) in self.spec.specifics.iter().enumerate() {
            if s.parse::<IpAddr>().is_err() {
                return Err(cfg(format!(
                    "spec.specifics[{i}] '{s}' is not an IP address"
                )));
            }
        }
        for (list, ranges) in [
            ("includeRanges", &self.spec.include_ranges),
            ("excludeRanges", &self.spec.exclude_ranges),
        ] {
            for (i, r) in ranges.iter().enumerate() {
                check_range(&format!("spec.{list}[{i}]"), r)?;
            }
        }
        let mut seen: Vec<&str> = Vec::new();
        for (i, s) in self.spec.services.iter().enumerate() {
            let at = format!("spec.services[{i}]");
            if s.name.trim().is_empty() {
                return Err(cfg(format!("{at}.name is empty")));
            }
            if seen.contains(&s.name.as_str()) {
                return Err(cfg(format!(
                    "{at}: duplicate service '{}' in this package",
                    s.name
                )));
            }
            seen.push(&s.name);
            s.interval
                .millis()
                .map_err(|m| cfg(format!("{at}.interval: {m}")))?;
            if let Some(st) = &s.status
                && st != "on"
                && st != "off"
            {
                return Err(cfg(format!("{at}.status '{st}' must be 'on' or 'off'")));
            }
            if let Some(g) = &s.thresholding_group
                && g.trim().is_empty()
            {
                return Err(cfg(format!("{at}.thresholdingGroup is empty")));
            }
            for (j, p) in s.parameters.iter().enumerate() {
                if p.key == THRESHOLDING_GROUP_PARAM {
                    return Err(cfg(format!(
                        "{at}.parameters[{j}]: set the group with `thresholdingGroup`, not a \
                         `{THRESHOLDING_GROUP_PARAM}` parameter"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Every `(path, group)` the package binds, for the plan-time gate.
    pub fn bound_groups(&self) -> Vec<(String, &str)> {
        self.spec
            .services
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.thresholding_group
                    .as_deref()
                    .map(|g| (format!("spec.services[{i}].thresholdingGroup"), g))
            })
            .collect()
    }
}

fn check_range(at: &str, r: &IpRangeDef) -> Result<()> {
    let parse = |field: &str, v: &str| {
        v.parse::<IpAddr>()
            .map_err(|_| cfg(format!("{at}.{field} '{v}' is not an IP address")))
    };
    let (b, e) = (parse("begin", &r.begin)?, parse("end", &r.end)?);
    if b.is_ipv4() != e.is_ipv4() {
        return Err(cfg(format!(
            "{at}: begin '{}' and end '{}' are different address families",
            r.begin, r.end
        )));
    }
    if b > e {
        return Err(cfg(format!(
            "{at}: begin '{}' is after end '{}'",
            r.begin, r.end
        )));
    }
    Ok(())
}

// -- helpers -----------------------------------------------------------------

fn parse_raw<T: serde::de::DeserializeOwned>(doc: &RawDoc, kind: &str) -> Result<T> {
    // Parse the text, not the `Value`, so plain scalars keep the type they
    // had in the file (see `EventSourceLocal`).
    let text = serde_norway::to_string(&doc.value).map_err(|e| {
        Error::Config(format!(
            "{}: could not re-serialize document: {e}",
            doc.label()
        ))
    })?;
    serde_norway::from_str(&text).map_err(|e| {
        Error::Config(format!(
            "{}: invalid `kind: {kind}` document: {e}",
            doc.label()
        ))
    })
}

fn check_envelope(api_version: &str, kind: &str, want: &str) -> Result<()> {
    if api_version != API_VERSION {
        return Err(cfg(format!(
            "apiVersion must be {API_VERSION:?}, got {api_version:?}"
        )));
    }
    if kind != want {
        return Err(cfg(format!("kind must be {want:?}, got {kind:?}")));
    }
    Ok(())
}

fn check_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(cfg("metadata.name is empty"));
    }
    if name.contains('/') || name.chars().any(char::is_control) {
        return Err(cfg(format!(
            "metadata.name {name:?} must not contain '/' or control characters"
        )));
    }
    Ok(())
}

fn cfg(msg: impl Into<String>) -> Error {
    Error::Config(msg.into())
}

fn label(doc: &RawDoc, e: Error) -> Error {
    match e {
        Error::Config(m) => Error::Config(format!("{}: {m}", doc.label())),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::parse_documents;

    const GROUP: &str = "\
apiVersion: thresholding.opennms.org/v1
kind: ThresholdGroup
metadata:
  name: acme-cpu
spec:
  rrdRepository: /opt/opennms/share/rrd/snmp/
  thresholds:
    - type: high
      dsType: node
      dsName: cpuLoad
      value: 90
      rearm: 80
      trigger: 3
  expressions:
    - type: high
      dsType: if
      expression: ifHCInOctets * 8 / ifHighSpeed
      value: \"1.0\"
      rearm: 0.5
      trigger: ${requisition:trigger|2}
      resourceFilters:
        - field: ifName
          content: ^ge-.*
";

    const PACKAGE: &str = "\
apiVersion: thresholding.opennms.org/v1
kind: ThreshdPackage
metadata:
  name: acme
spec:
  filter: IPADDR != '0.0.0.0'
  specifics: [10.0.0.7]
  includeRanges:
    - begin: 10.0.0.1
      end: 10.0.255.254
  services:
    - name: SNMP
      interval: 5m
      thresholdingGroup: acme-cpu
    - name: JMX
      interval: 300000
      status: off
";

    fn group(yaml: &str) -> Result<ThresholdGroupLocal> {
        ThresholdGroupLocal::from_raw(&parse_documents("g.yaml", yaml).unwrap()[0])
    }

    fn package(yaml: &str) -> Result<ThreshdPackageLocal> {
        ThreshdPackageLocal::from_raw(&parse_documents("p.yaml", yaml).unwrap()[0])
    }

    fn msg(r: Result<impl std::fmt::Debug>) -> String {
        match r {
            Err(Error::Config(m)) => m,
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn valid_group_parses_numbers_and_strings() {
        let g = group(GROUP).unwrap();
        assert_eq!(g.spec.thresholds[0].value, Scalar::Int(90));
        let e = &g.spec.expressions[0];
        assert_eq!(e.value, Scalar::Str("1.0".into()), "quoted stays a string");
        assert_eq!(e.rearm, Scalar::Float(0.5));
        assert_eq!(e.trigger.to_wire(), "${requisition:trigger|2}");
    }

    #[test]
    fn scalar_renders_shortest_form() {
        assert_eq!(Scalar::Int(90).to_wire(), "90");
        assert_eq!(Scalar::Float(0.5).to_wire(), "0.5");
        assert_eq!(Scalar::Float(90.0).to_wire(), "90");
        assert_eq!(Scalar::Str("1.0".into()).to_wire(), "1.0");
    }

    #[test]
    fn group_rejects_wrong_envelope() {
        assert!(
            msg(group(&GROUP.replace("thresholding.opennms.org/v1", "v2"))).contains("apiVersion")
        );
        let m = msg(group(&GROUP.replace("name: acme-cpu", "name: a/b")));
        assert!(m.contains("metadata.name"), "{m}");
    }

    #[test]
    fn group_rejects_unknown_type_and_operator() {
        let m = msg(group(&GROUP.replacen("type: high", "type: higher", 1)));
        assert!(m.contains("spec.thresholds[0].type 'higher'"), "{m}");
        let m = msg(group(&GROUP.replace(
            "      resourceFilters:",
            "      filterOperator: xor\n      resourceFilters:",
        )));
        assert!(m.contains("spec.expressions[0].filterOperator"), "{m}");
    }

    #[test]
    fn group_rejects_long_ds_name() {
        let m = msg(group(
            &GROUP.replace("dsName: cpuLoad", "dsName: abcdefghijklmnopqrst"),
        ));
        assert!(
            m.contains("spec.thresholds[0].dsName") && m.contains("19"),
            "{m}"
        );
    }

    #[test]
    fn group_rejects_bad_trigger() {
        for bad in ["0", "-1", "1.5", "two"] {
            let m = msg(group(
                &GROUP.replace("trigger: 3", &format!("trigger: {bad}")),
            ));
            assert!(m.contains("spec.thresholds[0].trigger"), "{bad}: {m}");
        }
        assert!(group(&GROUP.replace("trigger: 3", "trigger: \"4\"")).is_ok());
    }

    #[test]
    fn group_rejects_unknown_and_server_managed_keys() {
        for key in ["readOnly: false", "version: abc", "colour: red"] {
            let m = msg(group(
                &GROUP.replace("  rrdRepository:", &format!("  {key}\n  rrdRepository:")),
            ));
            assert!(m.contains("unknown field"), "{key}: {m}");
        }
    }

    #[test]
    fn group_rejects_empty_filter_field() {
        let m = msg(group(&GROUP.replace("field: ifName", "field: ''")));
        assert!(m.contains("resourceFilters[0].field"), "{m}");
    }

    #[test]
    fn ds_types_lists_every_entry() {
        let g = group(GROUP).unwrap();
        let got: Vec<_> = g
            .ds_types()
            .into_iter()
            .map(|(p, t)| format!("{p}={t}"))
            .collect();
        assert_eq!(got, ["spec.thresholds[0]=node", "spec.expressions[0]=if"]);
    }

    #[test]
    fn valid_package_parses_interval_forms() {
        let p = package(PACKAGE).unwrap();
        assert_eq!(p.spec.services[0].interval.millis().unwrap(), 300_000);
        assert_eq!(p.spec.services[1].interval.millis().unwrap(), 300_000);
        assert_eq!(
            p.bound_groups(),
            vec![("spec.services[0].thresholdingGroup".to_string(), "acme-cpu")]
        );
    }

    #[test]
    fn interval_round_trips_through_millis() {
        assert_eq!(
            Interval::from_millis(300_000),
            Interval::Duration("5m".into())
        );
        assert_eq!(
            Interval::from_millis(86_400_000),
            Interval::Duration("1d".into())
        );
        assert_eq!(Interval::from_millis(1_500), Interval::Millis(1_500));
        for ms in [1_000, 90_000, 300_000, 3_600_000, 1_500] {
            assert_eq!(Interval::from_millis(ms).millis().unwrap(), ms);
        }
    }

    #[test]
    fn package_rejects_bad_interval() {
        for bad in ["0", "-5", "5x", "0s"] {
            let m = msg(package(
                &PACKAGE.replace("interval: 5m", &format!("interval: {bad}")),
            ));
            assert!(m.contains("spec.services[0].interval"), "{bad}: {m}");
        }
    }

    #[test]
    fn package_rejects_bad_ranges() {
        let cases = [
            ("end: 10.0.255.254", "end: 10.0.0.0", "is after end"),
            ("end: 10.0.255.254", "end: ::1", "address families"),
            ("begin: 10.0.0.1", "begin: 10.0.0.300", "not an IP address"),
        ];
        for (from, to, want) in cases {
            let m = msg(package(&PACKAGE.replace(from, to)));
            assert!(
                m.contains("spec.includeRanges[0]") && m.contains(want),
                "{to}: {m}"
            );
        }
        let m = msg(package(&PACKAGE.replace("[10.0.0.7]", "[nope]")));
        assert!(m.contains("spec.specifics[0]"), "{m}");
    }

    #[test]
    fn package_rejects_duplicate_service_and_bad_status() {
        let m = msg(package(&PACKAGE.replace("name: JMX", "name: SNMP")));
        assert!(m.contains("duplicate service 'SNMP'"), "{m}");
        let m = msg(package(&PACKAGE.replace("status: off", "status: maybe")));
        assert!(m.contains("spec.services[1].status"), "{m}");
    }

    #[test]
    fn package_rejects_group_as_parameter() {
        let yaml = PACKAGE.replace(
            "      status: off\n",
            "      status: off\n      parameters:\n        - key: thresholding-group\n          value: mib2\n",
        );
        let m = msg(package(&yaml));
        assert!(m.contains("thresholdingGroup"), "{m}");
    }

    #[test]
    fn package_rejects_server_managed_keys() {
        for key in ["outageCalendars: []", "version: abc"] {
            let m = msg(package(
                &PACKAGE.replace("  filter:", &format!("  {key}\n  filter:")),
            ));
            assert!(m.contains("unknown field"), "{key}: {m}");
        }
        let m = msg(package(&PACKAGE.replace(
            "      interval: 5m\n",
            "      interval: 5m\n      userDefined: true\n",
        )));
        assert!(m.contains("unknown field"), "{m}");
    }
}

#[cfg(test)]
mod examples {
    use super::*;
    use onmsctl_core::parse_documents;

    fn read(file: &str) -> Vec<RawDoc> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(file);
        parse_documents(file, &std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn published_examples_parse() {
        let g = ThresholdGroupLocal::from_raw(&read("threshold-group.yaml")[0]).unwrap();
        assert_eq!((g.spec.thresholds.len(), g.spec.expressions.len()), (1, 1));
        let p = ThreshdPackageLocal::from_raw(&read("threshd-package.yaml")[0]).unwrap();
        assert_eq!(
            p.bound_groups()[0].1,
            g.metadata.name,
            "the package binds the example group"
        );
    }
}
