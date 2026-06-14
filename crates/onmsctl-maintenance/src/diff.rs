/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Definition idempotency + the attachment target set.
//!
//! The outage **definition** is readable, so [`definition_unchanged`] is a real
//! diff — normalized (order-insensitive lists, canonical IPs, case-insensitive
//! `type`) so semantically-equal definitions don't churn. The **attachments**
//! are NOT readable from this service, so [`attachment_targets`] just expands the
//! desired `suppress` set; the handler applies it ensure-present.

use crate::model::Suppress;
use crate::server;

/// Which daemon an attachment targets, and the REST path segment it uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Daemon {
    Pollerd,
    Threshd,
    Collectd,
    Notifd,
}

impl Daemon {
    /// The path segment under `/rest/sched-outages/{name}/`.
    pub fn segment(self) -> &'static str {
        match self {
            Daemon::Pollerd => "pollerd",
            Daemon::Threshd => "threshd",
            Daemon::Collectd => "collectd",
            Daemon::Notifd => "notifd",
        }
    }

    /// The friendly `suppress` key (for messages).
    pub fn suppress_key(self) -> &'static str {
        match self {
            Daemon::Pollerd => "polling",
            Daemon::Threshd => "thresholds",
            Daemon::Collectd => "collection",
            Daemon::Notifd => "notifications",
        }
    }
}

/// One attach target: a daemon, and a package for the per-package daemons
/// (`None` for the global `notifd`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachTarget {
    pub daemon: Daemon,
    pub package: Option<String>,
}

/// Expand the desired `suppress` block into the ordered set of attach targets.
pub fn attachment_targets(s: &Suppress) -> Vec<AttachTarget> {
    let mut out = Vec::new();
    let per_pkg = [
        (Daemon::Pollerd, &s.polling),
        (Daemon::Threshd, &s.thresholds),
        (Daemon::Collectd, &s.collection),
    ];
    for (daemon, ds) in per_pkg {
        if let Some(ds) = ds {
            for pkg in &ds.packages {
                out.push(AttachTarget {
                    daemon,
                    package: Some(pkg.clone()),
                });
            }
        }
    }
    if s.notifications {
        out.push(AttachTarget {
            daemon: Daemon::Notifd,
            package: None,
        });
    }
    out
}

/// Canonicalize a wire `Outage` for comparison: lowercase `type`, case-fold the
/// time fields (the month abbreviation in a `specific` date and the weekday
/// `day` are case-insensitive, and the server may echo a different casing), drop
/// the server-assigned `Time.id`, normalize IP spellings, and sort the
/// order-insensitive lists. `name` is the key and is left as-is.
fn canonical(o: &server::Outage) -> server::Outage {
    let mut c = o.clone();
    c.schedule_type = c.schedule_type.to_ascii_lowercase();
    // Normalize each time before sorting: the server assigns `Time.id` (not part
    // of the desired state) and may reformat the case of the month/weekday, so a
    // case-only difference must not register as a change.
    for t in &mut c.time {
        t.id = None;
        t.begins = t.begins.to_ascii_lowercase();
        t.ends = t.ends.to_ascii_lowercase();
        t.day = t.day.as_deref().map(str::to_ascii_lowercase);
    }
    c.time.sort_by(|a, b| {
        (
            a.begins.as_str(),
            a.ends.as_str(),
            a.day.as_deref().unwrap_or(""),
        )
            .cmp(&(
                b.begins.as_str(),
                b.ends.as_str(),
                b.day.as_deref().unwrap_or(""),
            ))
    });
    for i in &mut c.interface {
        if let Ok(ip) = i.address.parse::<std::net::IpAddr>() {
            i.address = ip.to_string();
        }
    }
    c.interface.sort_by(|a, b| a.address.cmp(&b.address));
    c.node.sort_by_key(|n| n.id);
    c
}

/// `true` when the deployed definition already matches the desired one
/// (normalized). Secrets are not a concern here (outages carry none).
pub fn definition_unchanged(desired: &server::Outage, deployed: &server::Outage) -> bool {
    canonical(desired) == canonical(deployed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DaemonSuppress, Suppress};

    fn outage(times: Vec<(&str, &str)>, ifaces: &[&str]) -> server::Outage {
        server::Outage {
            name: "w".into(),
            schedule_type: "daily".into(),
            time: times
                .into_iter()
                .map(|(b, e)| server::Time {
                    begins: b.into(),
                    ends: e.into(),
                    ..Default::default()
                })
                .collect(),
            interface: ifaces
                .iter()
                .map(|a| server::Interface {
                    address: a.to_string(),
                })
                .collect(),
            node: vec![],
        }
    }

    #[test]
    fn reordered_lists_and_case_are_unchanged() {
        let a = outage(
            vec![("01:00:00", "02:00:00"), ("03:00:00", "04:00:00")],
            &["10.0.0.1", "10.0.0.2"],
        );
        let mut b = outage(
            vec![("03:00:00", "04:00:00"), ("01:00:00", "02:00:00")],
            &["10.0.0.2", "10.0.0.1"],
        );
        b.schedule_type = "DAILY".into();
        assert!(definition_unchanged(&a, &b));
    }

    #[test]
    fn server_assigned_time_id_is_ignored() {
        let a = outage(vec![("01:00:00", "02:00:00")], &[]);
        let mut b = outage(vec![("01:00:00", "02:00:00")], &[]);
        b.time[0].id = Some("0".into());
        assert!(definition_unchanged(&a, &b));
    }

    #[test]
    fn day_and_month_case_differences_are_unchanged() {
        // The server may echo a different casing of the weekday / month abbrev;
        // a case-only difference must not churn.
        let mut a = outage(vec![("20-Jun-2026 22:00:00", "21-Jun-2026 04:00:00")], &[]);
        a.schedule_type = "specific".into();
        a.time[0].day = Some("Monday".into());
        let mut b = outage(vec![("20-JUN-2026 22:00:00", "21-jun-2026 04:00:00")], &[]);
        b.schedule_type = "specific".into();
        b.time[0].day = Some("monday".into());
        assert!(definition_unchanged(&a, &b));
    }

    #[test]
    fn changed_time_is_detected() {
        let a = outage(vec![("01:00:00", "02:00:00")], &[]);
        let b = outage(vec![("01:00:00", "05:00:00")], &[]);
        assert!(!definition_unchanged(&a, &b));
    }

    #[test]
    fn attachment_targets_expand_per_package_plus_global_notifd() {
        let s = Suppress {
            polling: Some(DaemonSuppress {
                packages: vec!["p1".into(), "p2".into()],
            }),
            thresholds: None,
            collection: None,
            notifications: true,
        };
        let t = attachment_targets(&s);
        assert_eq!(t.len(), 3);
        assert_eq!(
            t[0],
            AttachTarget {
                daemon: Daemon::Pollerd,
                package: Some("p1".into())
            }
        );
        assert_eq!(
            t[1],
            AttachTarget {
                daemon: Daemon::Pollerd,
                package: Some("p2".into())
            }
        );
        assert_eq!(
            t[2],
            AttachTarget {
                daemon: Daemon::Notifd,
                package: None
            }
        );
    }
}
