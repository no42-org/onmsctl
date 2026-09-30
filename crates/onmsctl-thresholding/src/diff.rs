/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `--diff` rendering for the threshold kinds (design D8).
//!
//! Inputs are canonical DTOs ([`crate::convert`]). Thresholds, expressions
//! and resource filters are matched by list position, because list order is
//! evaluation order. Package services are matched by name; ranges, specifics
//! and include URLs are compared as sets. A create renders against an empty
//! object, so every entry shows as `+`.

use serde::Serialize;
use serde_json::Value;

use crate::dto::{
    ExpressionDto, IpRangeDto, THRESHOLDING_GROUP_PARAM, ThreshdPackageDto, ThreshdServiceDto,
    ThresholdDto, ThresholdGroupDto,
};
use crate::model::{GROUP_KIND, Interval, PACKAGE_KIND};

/// The diff for moving a group from `before` (`None` = absent) to `after`.
pub fn group_diff(before: Option<&ThresholdGroupDto>, after: &ThresholdGroupDto) -> String {
    let empty = ThresholdGroupDto::default();
    let b = before.unwrap_or(&empty);
    let mut out = vec![header(GROUP_KIND, &after.name, before.is_none())];
    if b.rrd_repository != after.rrd_repository {
        out.push(format!(
            "  ~ rrdRepository: {} -> {}",
            show_str(&b.rrd_repository),
            show_str(&after.rrd_repository)
        ));
    }
    indexed(
        &mut out,
        "thresholds",
        &b.thresholds,
        &after.thresholds,
        threshold_label,
    );
    indexed(
        &mut out,
        "expressions",
        &b.expressions,
        &after.expressions,
        expression_label,
    );
    out.join("\n")
}

/// The diff for moving a package from `before` (`None` = absent) to `after`.
pub fn package_diff(before: Option<&ThreshdPackageDto>, after: &ThreshdPackageDto) -> String {
    let empty = ThreshdPackageDto::default();
    let b = before.unwrap_or(&empty);
    let mut out = vec![header(PACKAGE_KIND, &after.name, before.is_none())];
    if b.filter != after.filter {
        out.push(format!(
            "  ~ filter: {} -> {}",
            show_str(&b.filter),
            show_str(&after.filter)
        ));
    }
    let range = |r: &IpRangeDto| format!("{}-{}", r.begin, r.end);
    set(&mut out, "specifics", &b.specifics, &after.specifics, |s| {
        s.clone()
    });
    set(
        &mut out,
        "includeRanges",
        &b.include_ranges,
        &after.include_ranges,
        range,
    );
    set(
        &mut out,
        "excludeRanges",
        &b.exclude_ranges,
        &after.exclude_ranges,
        range,
    );
    set(
        &mut out,
        "includeUrls",
        &b.include_urls,
        &after.include_urls,
        |s| s.clone(),
    );

    let names = |p: &ThreshdPackageDto| {
        p.services
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
    };
    let mut all = names(b);
    all.extend(names(after).into_iter().filter(|n| !names(b).contains(n)));
    for name in all {
        let find =
            |p: &ThreshdPackageDto| p.services.iter().find(|s| s.name == name).map(service_view);
        match (find(b), find(after)) {
            (Some(x), Some(y)) => {
                let changes = fields(&x, &y);
                if !changes.is_empty() {
                    out.push(format!("  ~ services[{name}]: {changes}"));
                }
            }
            (None, Some(y)) => out.push(format!("  + services[{name}]: {}", summary(&y))),
            (Some(x), None) => out.push(format!("  - services[{name}]: {}", summary(&x))),
            (None, None) => {}
        }
    }
    out.join("\n")
}

fn header(kind: &str, name: &str, create: bool) -> String {
    format!(
        "{kind}/{name}: {}",
        if create { "create" } else { "update" }
    )
}

fn threshold_label(t: &ThresholdDto) -> String {
    format!("{} {}/{}", t.common.kind, t.common.ds_type, t.ds_name)
}

fn expression_label(e: &ExpressionDto) -> String {
    format!("{} {}: {}", e.common.kind, e.common.ds_type, e.expression)
}

/// Position-matched list diff.
fn indexed<T: Serialize>(
    out: &mut Vec<String>,
    list: &str,
    before: &[T],
    after: &[T],
    label: fn(&T) -> String,
) {
    for i in 0..before.len().max(after.len()) {
        match (before.get(i), after.get(i)) {
            (Some(x), Some(y)) => {
                let changes = fields(&json(x), &json(y));
                if !changes.is_empty() {
                    out.push(format!("  ~ {list}[{i}] {}: {changes}", label(y)));
                }
            }
            (None, Some(y)) => out.push(format!("  + {list}[{i}] {}", label(y))),
            (Some(x), None) => out.push(format!("  - {list}[{i}] {}", label(x))),
            (None, None) => {}
        }
    }
}

/// Set diff: `-` for entries only in `before`, `+` for entries only in `after`.
fn set<T: PartialEq>(
    out: &mut Vec<String>,
    list: &str,
    before: &[T],
    after: &[T],
    show: impl Fn(&T) -> String,
) {
    for x in before.iter().filter(|x| !after.contains(x)) {
        out.push(format!("  - {list}: {}", show(x)));
    }
    for y in after.iter().filter(|y| !before.contains(y)) {
        out.push(format!("  + {list}: {}", show(y)));
    }
}

/// A service in document terms: `interval` as a duration, the group as
/// `thresholdingGroup`, `userDefined` left out (apply never changes it).
fn service_view(s: &ThreshdServiceDto) -> Value {
    let interval = match Interval::from_millis(s.interval) {
        Interval::Duration(d) => Value::String(d),
        Interval::Millis(ms) => Value::from(ms),
    };
    let group = s
        .parameters
        .iter()
        .find(|p| p.key == THRESHOLDING_GROUP_PARAM)
        .map(|p| Value::String(p.value.clone()))
        .unwrap_or(Value::Null);
    let params: Vec<_> = s
        .parameters
        .iter()
        .filter(|p| p.key != THRESHOLDING_GROUP_PARAM)
        .map(|p| format!("{}={}", p.key, p.value))
        .collect();
    serde_json::json!({
        "interval": interval,
        "status": s.status,
        "thresholdingGroup": group,
        "parameters": params,
    })
}

/// One-line summary of an added or removed service.
fn summary(view: &Value) -> String {
    let mut parts = vec![format!("interval {}", show(&view["interval"]))];
    if !view["thresholdingGroup"].is_null() {
        parts.push(format!("group {}", show(&view["thresholdingGroup"])));
    }
    parts.join(", ")
}

/// `field a -> b` for every top-level field that differs, alphabetically.
fn fields(before: &Value, after: &Value) -> String {
    let (Some(b), Some(a)) = (before.as_object(), after.as_object()) else {
        return String::new();
    };
    let mut keys: Vec<&String> = b.keys().chain(a.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter_map(|k| {
            let (x, y) = (
                b.get(k).unwrap_or(&Value::Null),
                a.get(k).unwrap_or(&Value::Null),
            );
            (x != y).then(|| format!("{k} {} -> {}", show(x), show(y)))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn json<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn show(v: &Value) -> String {
    match v {
        Value::Null => "(none)".into(),
        Value::String(s) => show_str(s),
        other => other.to_string(),
    }
}

fn show_str(s: &str) -> String {
    if s.is_empty() {
        "(empty)".into()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{ParameterDto, ThresholdCommon};

    fn threshold(kind: &str, ds: &str, value: &str, trigger: &str) -> ThresholdDto {
        ThresholdDto {
            common: ThresholdCommon {
                kind: kind.into(),
                ds_type: "node".into(),
                value: value.into(),
                rearm: "0".into(),
                trigger: trigger.into(),
                filter_operator: "or".into(),
                ..Default::default()
            },
            ds_name: ds.into(),
        }
    }

    fn group(thresholds: Vec<ThresholdDto>, expressions: Vec<ExpressionDto>) -> ThresholdGroupDto {
        ThresholdGroupDto {
            name: "acme-cpu".into(),
            rrd_repository: "/r/".into(),
            thresholds,
            expressions,
            ..Default::default()
        }
    }

    fn expression(expr: &str) -> ExpressionDto {
        ExpressionDto {
            common: threshold("high", "x", "90", "2").common,
            expression: expr.into(),
        }
    }

    #[test]
    fn group_diff_shows_field_changes_additions_and_removals() {
        let before = group(
            vec![
                threshold("high", "cpuLoad", "85", "2"),
                threshold("low", "memFree", "5", "1"),
                threshold("low", "swap", "1", "1"),
            ],
            vec![],
        );
        let after = group(
            vec![
                threshold("high", "cpuLoad", "90", "3"),
                threshold("low", "memFree", "5", "1"),
            ],
            vec![expression("ifHCOutOctets * 8")],
        );
        let d = group_diff(Some(&before), &after);
        let lines: Vec<&str> = d.lines().collect();
        assert_eq!(lines[0], "ThresholdGroup/acme-cpu: update");
        assert!(
            d.contains("  ~ thresholds[0] high node/cpuLoad: trigger 2 -> 3, value 85 -> 90"),
            "{d}"
        );
        assert!(
            !d.contains("thresholds[1]"),
            "unchanged entries are not listed: {d}"
        );
        assert!(d.contains("  - thresholds[2] low node/swap"), "{d}");
        assert!(
            d.contains("  + expressions[0] high node: ifHCOutOctets * 8"),
            "{d}"
        );
    }

    #[test]
    fn group_create_lists_every_entry() {
        let after = group(
            vec![threshold("high", "cpuLoad", "90", "3")],
            vec![expression("a + b")],
        );
        let d = group_diff(None, &after);
        assert_eq!(
            d,
            "ThresholdGroup/acme-cpu: create\n  ~ rrdRepository: (empty) -> /r/\n  \
             + thresholds[0] high node/cpuLoad\n  + expressions[0] high node: a + b"
        );
    }

    fn service(name: &str, interval: i64, group: Option<&str>) -> ThreshdServiceDto {
        ThreshdServiceDto {
            name: name.into(),
            interval,
            user_defined: false,
            status: Some("on".into()),
            parameters: group
                .map(|g| {
                    vec![ParameterDto {
                        key: THRESHOLDING_GROUP_PARAM.into(),
                        value: g.into(),
                    }]
                })
                .unwrap_or_default(),
        }
    }

    fn package(services: Vec<ThreshdServiceDto>, ranges: Vec<(&str, &str)>) -> ThreshdPackageDto {
        ThreshdPackageDto {
            name: "acme".into(),
            filter: "IPADDR != '0.0.0.0'".into(),
            include_ranges: ranges
                .into_iter()
                .map(|(b, e)| IpRangeDto {
                    begin: b.into(),
                    end: e.into(),
                })
                .collect(),
            services,
            ..Default::default()
        }
    }

    #[test]
    fn package_diff_matches_services_by_name_and_ranges_as_sets() {
        let before = package(
            vec![
                service("SNMP", 300_000, Some("acme-cpu")),
                service("OLD", 60_000, None),
            ],
            vec![("10.0.0.1", "10.0.0.9")],
        );
        let after = package(
            vec![
                service("NEW", 1_500, Some("g")),
                service("SNMP", 600_000, Some("acme-cpu")),
            ],
            vec![("10.0.0.1", "10.0.0.9"), ("10.1.0.1", "10.1.255.254")],
        );
        let d = package_diff(Some(&before), &after);
        assert!(d.starts_with("ThreshdPackage/acme: update"), "{d}");
        assert!(
            d.contains("  + includeRanges: 10.1.0.1-10.1.255.254"),
            "{d}"
        );
        assert!(
            !d.contains("10.0.0.1-10.0.0.9"),
            "unchanged range listed: {d}"
        );
        assert!(d.contains("  ~ services[SNMP]: interval 5m -> 10m"), "{d}");
        assert!(d.contains("  - services[OLD]: interval 1m"), "{d}");
        assert!(
            d.contains("  + services[NEW]: interval 1500, group g"),
            "{d}"
        );
    }

    #[test]
    fn package_diff_ignores_user_defined() {
        let before = package(vec![service("SNMP", 300_000, None)], vec![]);
        let mut after = before.clone();
        after.services[0].user_defined = true;
        assert_eq!(
            package_diff(Some(&before), &after),
            "ThreshdPackage/acme: update"
        );
    }
}
