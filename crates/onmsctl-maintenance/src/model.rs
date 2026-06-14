/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local (YAML) model for `kind: Maintenance` — the operator-facing maintenance
//! window that `onmsctl apply -f` reconciles into an OpenNMS scheduled outage.
//!
//! Named, multi-instance: `metadata.name` is the outage name. The `spec` carries
//! `schedule` (when — `type` + `times`), `devices` (who — `interfaces` and/or
//! foreign-referenced `nodes`), and `suppress` (which daemons stop — each of
//! `polling`/`thresholds`/`collection` with EXPLICIT packages, plus a global
//! `notifications` boolean). All validation that can be done without the server
//! happens in [`MaintenanceLocal::validate`], before any HTTP request.

use onmsctl_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only accepted `apiVersion`.
pub const API_VERSION: &str = "maintenance.opennms.org/v1";
/// The only accepted `kind`.
pub const KIND: &str = "Maintenance";
/// The literal interface selector meaning "every interface".
pub const MATCH_ANY: &str = "match-any";

/// A `kind: Maintenance` document.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceLocal {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: Spec,
}

/// Document metadata. `name` is the scheduled-outage name.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
}

/// The maintenance-window body.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub schedule: Schedule,
    pub devices: Devices,
    pub suppress: Suppress,
}

/// When the window is in effect.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    #[serde(rename = "type")]
    pub schedule_type: ScheduleType,
    pub times: Vec<TimeWindow>,
}

/// Outage recurrence type (maps to the server `type` attribute).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleType {
    Specific,
    Daily,
    Weekly,
    Monthly,
}

/// One start/end window. `day` is required for `weekly` (a weekday) and
/// `monthly` (a day-of-month 1–31), and forbidden for `specific`/`daily`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TimeWindow {
    pub begins: String,
    pub ends: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
}

/// Which devices the window covers.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Devices {
    /// IP addresses, or the single literal `match-any` (= all interfaces).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<String>,
    /// Nodes by foreign reference (resolved to the server nodeId at apply).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeRef>,
    /// Select every node in ANY of the named OpenNMS categories (resolved to
    /// nodeIds at apply via the v2 nodes search). Additive with `nodes`/`asset`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// Select every node at ANY of the named OpenNMS monitoring (Minion)
    /// locations (resolved to nodeIds at apply). Additive with the other
    /// selectors. Selects whole nodes by id — the outage model has no location
    /// field, so an interface IP cannot be scoped to a location.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<String>,
    /// Select nodes whose OpenNMS asset-record `field` equals `value` (the
    /// searchable key/value). Node meta-data (`context:key=value`) is NOT
    /// searchable by the node-list API, so it is intentionally not a selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<AssetSelector>,
}

/// A single asset-record `field == value` selector.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetSelector {
    pub field: String,
    pub value: String,
}

/// A node foreign reference. Server nodeIds are not stable in GitOps, so the
/// manifest names nodes by `{foreignSource, foreignId}` and onmsctl resolves the
/// nodeId at apply.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeRef {
    pub foreign_source: String,
    pub foreign_id: String,
}

/// Which daemons honor the window. Each of `polling`/`thresholds`/`collection`
/// requires an explicit, non-empty package list (there is no default package);
/// `notifications` is global.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Suppress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling: Option<DaemonSuppress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<DaemonSuppress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<DaemonSuppress>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub notifications: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The package list for one per-package daemon (pollerd / threshd / collectd).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DaemonSuppress {
    pub packages: Vec<String>,
}

impl MaintenanceLocal {
    /// Validate the document, returning the first user-actionable error. Covers
    /// the API literals, the schedule/time shape (per `type`, `begins < ends`),
    /// the device selectors (`match-any` exclusivity, IP syntax), and the
    /// suppress rules (explicit non-empty packages, ≥1 daemon).
    pub fn validate(&self) -> Result<()> {
        if self.api_version != API_VERSION {
            return Err(cfg(format!(
                "apiVersion must be {API_VERSION:?}, got {:?}",
                self.api_version
            )));
        }
        if self.kind != KIND {
            return Err(cfg(format!("kind must be {KIND:?}, got {:?}", self.kind)));
        }
        if self.metadata.name.trim().is_empty() {
            return Err(cfg("metadata.name must not be empty".into()));
        }
        self.validate_schedule()?;
        self.validate_devices()?;
        self.validate_suppress()?;
        Ok(())
    }

    fn validate_schedule(&self) -> Result<()> {
        if self.spec.schedule.times.is_empty() {
            return Err(cfg(
                "spec.schedule.times must declare at least one window".into()
            ));
        }
        for (i, t) in self.spec.schedule.times.iter().enumerate() {
            validate_time(self.spec.schedule.schedule_type, t)
                .map_err(|m| cfg(format!("spec.schedule.times[{i}]: {m}")))?;
        }
        Ok(())
    }

    fn validate_devices(&self) -> Result<()> {
        let d = &self.spec.devices;
        if d.interfaces.is_empty()
            && d.nodes.is_empty()
            && d.categories.is_empty()
            && d.locations.is_empty()
            && d.asset.is_none()
        {
            return Err(cfg(
                "spec.devices must declare at least one selector (interfaces, nodes, categories, \
                 locations, and/or asset)"
                    .into(),
            ));
        }
        let has_match_any = d.interfaces.iter().any(|i| i == MATCH_ANY);
        if has_match_any
            && (d.interfaces.len() > 1
                || !d.nodes.is_empty()
                || !d.categories.is_empty()
                || !d.locations.is_empty()
                || d.asset.is_some())
        {
            return Err(cfg(
                "spec.devices: `match-any` selects every interface and cannot be combined with \
                 other interfaces, nodes, categories, locations, or asset"
                    .into(),
            ));
        }
        for ip in d.interfaces.iter().filter(|i| *i != MATCH_ANY) {
            if ip.parse::<std::net::IpAddr>().is_err() {
                return Err(cfg(format!(
                    "spec.devices.interfaces: invalid IP {ip:?} (use a valid address or `match-any`)"
                )));
            }
        }
        for (i, n) in d.nodes.iter().enumerate() {
            if n.foreign_source.trim().is_empty() || n.foreign_id.trim().is_empty() {
                return Err(cfg(format!(
                    "spec.devices.nodes[{i}]: foreignSource and foreignId must both be non-empty"
                )));
            }
        }
        for (i, c) in d.categories.iter().enumerate() {
            if c.trim().is_empty() {
                return Err(cfg(format!(
                    "spec.devices.categories[{i}] must not be empty"
                )));
            }
            reject_fiql_metachar(&format!("spec.devices.categories[{i}]"), c)?;
        }
        for (i, l) in d.locations.iter().enumerate() {
            if l.trim().is_empty() {
                return Err(cfg(format!(
                    "spec.devices.locations[{i}] must not be empty"
                )));
            }
            reject_fiql_metachar(&format!("spec.devices.locations[{i}]"), l)?;
        }
        if let Some(a) = &d.asset {
            if a.field.trim().is_empty() || a.value.trim().is_empty() {
                return Err(cfg(
                    "spec.devices.asset.field and asset.value must both be non-empty".into(),
                ));
            }
            reject_fiql_metachar("spec.devices.asset.field", &a.field)?;
            reject_fiql_metachar("spec.devices.asset.value", &a.value)?;
        }
        Ok(())
    }

    fn validate_suppress(&self) -> Result<()> {
        let s = &self.spec.suppress;
        let enabled = s.polling.is_some()
            || s.thresholds.is_some()
            || s.collection.is_some()
            || s.notifications;
        if !enabled {
            return Err(cfg(
                "spec.suppress must enable at least one of polling / thresholds / collection / \
                 notifications"
                    .into(),
            ));
        }
        for (name, ds) in [
            ("polling", &s.polling),
            ("thresholds", &s.thresholds),
            ("collection", &s.collection),
        ] {
            if let Some(ds) = ds
                && ds.packages.is_empty()
            {
                return Err(cfg(format!(
                    "spec.suppress.{name} requires a non-empty `packages` list (there is no \
                     default package)"
                )));
            }
        }
        Ok(())
    }

    /// Non-fatal advisories (the spec's WARN cases). Currently: a `specific`
    /// window whose `ends` is already before `now` is dead config (additive
    /// prune won't remove it). `now` is `(year, month 1–12, day, hour, min, sec)`.
    ///
    /// NOTE: the window times are interpreted in the OpenNMS *server's* timezone,
    /// but the caller supplies a UTC `now`, so this past-window check is
    /// approximate near the boundary (a few hours of offset). It is advisory
    /// only — never gates the apply — so the imprecision is acceptable.
    pub fn warnings(&self, now: (i32, u8, u8, u8, u8, u8)) -> Vec<String> {
        let mut out = Vec::new();
        if self.spec.schedule.schedule_type == ScheduleType::Specific {
            for (i, t) in self.spec.schedule.times.iter().enumerate() {
                if let Some(end) = parse_specific(&t.ends)
                    && end < now
                {
                    out.push(format!(
                        "spec.schedule.times[{i}]: the `specific` window ends {} which is in the \
                         past — it will be created but never active (delete it to clean up)",
                        t.ends
                    ));
                }
            }
        }
        out
    }
}

fn cfg(m: String) -> Error {
    Error::Config(m)
}

/// Reject a selector value that contains a FIQL metacharacter (`,` `;` `=` `(`
/// `)`) — which would corrupt the `_s` query structure — or the wildcard `*`,
/// which would silently widen an intended exact `==value` match and select more
/// nodes than the operator named.
fn reject_fiql_metachar(field: &str, value: &str) -> Result<()> {
    if let Some(c) = value
        .chars()
        .find(|c| matches!(c, ',' | ';' | '=' | '(' | ')' | '*'))
    {
        return Err(cfg(format!(
            "{field}: value {value:?} contains the disallowed character {c:?} \
             (FIQL metacharacters , ; = ( ) and the wildcard * are not permitted in a selector)"
        )));
    }
    Ok(())
}

/// Validate one time window against its schedule type, returning a message
/// fragment on failure. Enforces the per-type shape and `begins < ends`.
fn validate_time(ty: ScheduleType, t: &TimeWindow) -> std::result::Result<(), String> {
    match ty {
        ScheduleType::Specific => {
            forbid_day(t)?;
            let b = parse_specific(&t.begins)
                .ok_or_else(|| fmt_err("begins", &t.begins, "dd-MMM-yyyy HH:mm:ss"))?;
            let e = parse_specific(&t.ends)
                .ok_or_else(|| fmt_err("ends", &t.ends, "dd-MMM-yyyy HH:mm:ss"))?;
            if b >= e {
                return Err(format!(
                    "begins {:?} must be before ends {:?}",
                    t.begins, t.ends
                ));
            }
        }
        ScheduleType::Daily => {
            forbid_day(t)?;
            order_hms(t)?;
        }
        ScheduleType::Weekly => {
            let day = t
                .day
                .as_deref()
                .ok_or("weekly windows require a `day` (a weekday)")?;
            if !is_weekday(day) {
                return Err(format!("`day` {day:?} is not a weekday (sunday…saturday)"));
            }
            order_hms(t)?;
        }
        ScheduleType::Monthly => {
            let day = t
                .day
                .as_deref()
                .ok_or("monthly windows require a `day` (1–31)")?;
            match day.parse::<u8>() {
                Ok(n) if (1..=31).contains(&n) => {}
                _ => return Err(format!("`day` {day:?} must be a day-of-month in 1–31")),
            }
            order_hms(t)?;
        }
    }
    Ok(())
}

fn forbid_day(t: &TimeWindow) -> std::result::Result<(), String> {
    if t.day.is_some() {
        return Err("`day` is only valid for weekly/monthly windows".into());
    }
    Ok(())
}

/// Validate `begins`/`ends` as `HH:mm:ss` and that begins < ends.
fn order_hms(t: &TimeWindow) -> std::result::Result<(), String> {
    let b = parse_hms(&t.begins).ok_or_else(|| fmt_err("begins", &t.begins, "HH:mm:ss"))?;
    let e = parse_hms(&t.ends).ok_or_else(|| fmt_err("ends", &t.ends, "HH:mm:ss"))?;
    if b >= e {
        return Err(format!(
            "begins {:?} must be before ends {:?}",
            t.begins, t.ends
        ));
    }
    Ok(())
}

fn fmt_err(field: &str, value: &str, fmt: &str) -> String {
    format!("`{field}` {value:?} is not in the expected format `{fmt}`")
}

/// Parse `HH:mm:ss` into a comparable tuple, validating field ranges.
fn parse_hms(s: &str) -> Option<(u8, u8, u8)> {
    let p: Vec<&str> = s.split(':').collect();
    if p.len() != 3 {
        return None;
    }
    let h: u8 = p[0].parse().ok()?;
    let m: u8 = p[1].parse().ok()?;
    let sec: u8 = p[2].parse().ok()?;
    if p[0].len() != 2 || p[1].len() != 2 || p[2].len() != 2 {
        return None;
    }
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    Some((h, m, sec))
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// Parse `dd-MMM-yyyy HH:mm:ss` into a comparable `(year, month, day, h, m, s)`.
fn parse_specific(s: &str) -> Option<(i32, u8, u8, u8, u8, u8)> {
    let (date, time) = s.split_once(' ')?;
    let dparts: Vec<&str> = date.split('-').collect();
    if dparts.len() != 3 {
        return None;
    }
    let day: u8 = dparts[0].parse().ok()?;
    let mon_idx = MONTHS
        .iter()
        .position(|m| m.eq_ignore_ascii_case(dparts[1]))? as u8
        + 1;
    let year: i32 = dparts[2].parse().ok()?;
    if dparts[0].len() != 2 || dparts[2].len() != 4 || !(1..=31).contains(&day) {
        return None;
    }
    let (h, m, sec) = parse_hms(time)?;
    Some((year, mon_idx, day, h, m, sec))
}

fn is_weekday(d: &str) -> bool {
    matches!(
        d.to_ascii_lowercase().as_str(),
        "sunday" | "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> std::result::Result<MaintenanceLocal, serde_norway::Error> {
        serde_norway::from_str(yaml)
    }

    const SPECIFIC: &str = r#"
apiVersion: maintenance.opennms.org/v1
kind: Maintenance
metadata: { name: weekend-patching }
spec:
  schedule:
    type: specific
    times:
      - begins: "20-Jun-2026 22:00:00"
        ends:   "21-Jun-2026 04:00:00"
  devices:
    interfaces: [192.168.8.8]
    nodes:
      - { foreignSource: lab, foreignId: web01 }
  suppress:
    polling: { packages: [prod-poller] }
    notifications: true
"#;

    #[test]
    fn specific_window_parses_and_validates() {
        let doc = parse(SPECIFIC).expect("parses");
        doc.validate().expect("valid");
        assert_eq!(doc.spec.schedule.schedule_type, ScheduleType::Specific);
        assert_eq!(doc.spec.devices.nodes[0].foreign_id, "web01");
        assert!(doc.spec.suppress.notifications);
        assert_eq!(
            doc.spec.suppress.polling.as_ref().unwrap().packages,
            vec!["prod-poller"]
        );
    }

    fn valid_with(schedule: &str, devices: &str, suppress: &str) -> MaintenanceLocal {
        parse(&format!(
            "apiVersion: maintenance.opennms.org/v1\nkind: Maintenance\nmetadata: {{ name: w }}\nspec:\n  schedule:\n{schedule}\n  devices:\n{devices}\n  suppress:\n{suppress}\n"
        ))
        .unwrap()
    }

    #[test]
    fn weekly_requires_weekday_day() {
        let doc = valid_with(
            "    type: weekly\n    times:\n      - { begins: \"22:00:00\", ends: \"23:00:00\" }\n",
            "    interfaces: [match-any]\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("require a `day`")
        );

        let ok = valid_with(
            "    type: weekly\n    times:\n      - { day: Monday, begins: \"22:00:00\", ends: \"23:00:00\" }\n",
            "    interfaces: [match-any]\n",
            "    notifications: true\n",
        );
        ok.validate().expect("weekday accepted");
    }

    #[test]
    fn monthly_day_out_of_range_rejected() {
        let doc = valid_with(
            "    type: monthly\n    times:\n      - { day: \"32\", begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    interfaces: [match-any]\n",
            "    notifications: true\n",
        );
        assert!(doc.validate().unwrap_err().to_string().contains("1–31"));
    }

    #[test]
    fn begins_not_before_ends_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"04:00:00\", ends: \"02:00:00\" }\n",
            "    interfaces: [match-any]\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("must be before")
        );
    }

    #[test]
    fn specific_bad_format_rejected() {
        let doc = valid_with(
            "    type: specific\n    times:\n      - { begins: \"2026-06-20 22:00:00\", ends: \"2026-06-21 04:00:00\" }\n",
            "    interfaces: [match-any]\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("dd-MMM-yyyy")
        );
    }

    #[test]
    fn match_any_with_other_selectors_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    interfaces: [match-any, 10.0.0.1]\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("match-any")
        );
    }

    #[test]
    fn no_device_selector_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    {}\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("at least one selector")
        );
    }

    #[test]
    fn empty_suppress_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    interfaces: [match-any]\n",
            "    {}\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("at least one")
        );
    }

    #[test]
    fn empty_packages_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    interfaces: [match-any]\n",
            "    polling: { packages: [] }\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("non-empty `packages`")
        );
    }

    #[test]
    fn notifications_with_packages_fails_to_parse() {
        // `notifications` is a bool; a map is a type error at parse time.
        let err = parse(
            "apiVersion: maintenance.opennms.org/v1\nkind: Maintenance\nmetadata: { name: w }\nspec:\n  schedule:\n    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n  devices:\n    interfaces: [match-any]\n  suppress:\n    notifications: { packages: [x] }\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn unknown_field_rejected() {
        let err = parse(
            "apiVersion: maintenance.opennms.org/v1\nkind: Maintenance\nmetadata: { name: w }\nspec:\n  schedule: { type: daily, times: [ { begins: \"01:00:00\", ends: \"02:00:00\" } ] }\n  devices: { interfaces: [match-any] }\n  suppress: { notifications: true }\n  bogus: 1\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn categories_and_asset_count_as_selectors() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    categories: [Routers, Core]\n    asset: { field: city, value: Berlin }\n",
            "    notifications: true\n",
        );
        doc.validate()
            .expect("categories/asset are valid selectors");
        assert_eq!(doc.spec.devices.categories, vec!["Routers", "Core"]);
        assert_eq!(doc.spec.devices.asset.as_ref().unwrap().field, "city");
    }

    #[test]
    fn locations_count_as_selector_and_validate() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    locations: [Berlin, Default]\n",
            "    notifications: true\n",
        );
        doc.validate().expect("locations are a valid selector");
        assert_eq!(doc.spec.devices.locations, vec!["Berlin", "Default"]);
    }

    #[test]
    fn empty_location_is_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    locations: [\"\"]\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("must not be empty")
        );
    }

    #[test]
    fn match_any_with_locations_is_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    interfaces: [match-any]\n    locations: [Berlin]\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("match-any")
        );
    }

    #[test]
    fn empty_category_is_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    categories: [\"\"]\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("must not be empty")
        );
    }

    #[test]
    fn fiql_metachar_in_category_is_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    categories: [\"Routers,Core\"]\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("disallowed character")
        );
    }

    #[test]
    fn asset_requires_field_and_value() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    asset: { field: city, value: \"\" }\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("must both be non-empty")
        );
    }

    #[test]
    fn match_any_with_categories_is_rejected() {
        let doc = valid_with(
            "    type: daily\n    times:\n      - { begins: \"01:00:00\", ends: \"02:00:00\" }\n",
            "    interfaces: [match-any]\n    categories: [Routers]\n",
            "    notifications: true\n",
        );
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("match-any")
        );
    }

    #[test]
    fn past_specific_window_warns() {
        let doc = parse(SPECIFIC).unwrap();
        // "now" well after the window's end.
        let w = doc.warnings((2027, 1, 1, 0, 0, 0));
        assert_eq!(w.len(), 1, "a past specific window warns");
        // "now" before the window: no warning.
        assert!(doc.warnings((2026, 1, 1, 0, 0, 0)).is_empty());
    }
}
