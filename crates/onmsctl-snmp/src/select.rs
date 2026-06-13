/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Client-side selector matching for `snmp lookup` location discovery.
//!
//! When `lookup` is run without `--location`, we must decide which monitoring
//! locations to query the server for. The set is derived from the stored config:
//! every `definition` whose selector (`specific` / `range` / `ipMatch`) matches
//! the IP contributes its `location`. A definition with no explicit location is
//! the `Default` location.
//!
//! Selector matching mirrors OpenNMS: `specific` is an exact IP, `range` is an
//! inclusive `begin..end` compared numerically, and `ipMatch` is an IPLIKE
//! pattern. IPLIKE is implemented for IPv4 (per-octet `*`, `n`, `a-b`, and
//! comma lists); an IPv6 `ipMatch` is **not** auto-discovered here (callers can
//! still pass `--location` explicitly). A location matched only by a profile
//! `filterExpression` (evaluated server-side) is likewise not discoverable
//! client-side, by design.

use std::net::IpAddr;

use crate::server::{Definition, Range, SnmpConfig};

/// The label used when a definition omits an explicit `location`.
pub const DEFAULT_LOCATION: &str = "Default";

/// Locations (deduped, sorted) whose definition selectors match `ip`.
pub fn locations_for_ip(cfg: &SnmpConfig, ip: IpAddr) -> Vec<String> {
    let mut locs: Vec<String> = cfg
        .definition
        .iter()
        .filter(|d| definition_matches(d, ip))
        .map(|d| {
            d.location
                .clone()
                .unwrap_or_else(|| DEFAULT_LOCATION.to_string())
        })
        .collect();
    locs.sort();
    locs.dedup();
    locs
}

/// `true` when any of the definition's selectors matches `ip`.
fn definition_matches(d: &Definition, ip: IpAddr) -> bool {
    d.specific
        .iter()
        .any(|s| s.parse::<IpAddr>().map(|a| a == ip).unwrap_or(false))
        || d.range.iter().any(|r| range_contains(r, ip))
        || d.ip_match.iter().any(|p| iplike_v4(p, ip))
}

/// Inclusive numeric range check within the same address family.
fn range_contains(r: &Range, ip: IpAddr) -> bool {
    match (r.begin.parse::<IpAddr>(), r.end.parse::<IpAddr>()) {
        (Ok(begin), Ok(end)) => {
            same_family(begin, ip) && same_family(end, ip) && begin <= ip && ip <= end
        }
        _ => false,
    }
}

fn same_family(a: IpAddr, b: IpAddr) -> bool {
    a.is_ipv4() == b.is_ipv4()
}

/// IPv4 IPLIKE match: four dot-separated octet patterns, each `*`, a number, a
/// `a-b` range, or a comma list of those. Non-IPv4 addresses never match here.
fn iplike_v4(pattern: &str, ip: IpAddr) -> bool {
    let IpAddr::V4(v4) = ip else {
        return false;
    };
    let parts: Vec<&str> = pattern.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts
        .iter()
        .zip(v4.octets().iter())
        .all(|(pat, &oct)| octet_matches(pat, oct))
}

/// Match one IPLIKE octet term against an octet value.
fn octet_matches(pat: &str, oct: u8) -> bool {
    if pat == "*" {
        return true;
    }
    pat.split(',').any(|term| match term.split_once('-') {
        Some((a, b)) => match (a.parse::<u8>(), b.parse::<u8>()) {
            (Ok(a), Ok(b)) => oct >= a && oct <= b,
            _ => false,
        },
        None => term.parse::<u8>().map(|n| n == oct).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn cfg(defs: Vec<Definition>) -> SnmpConfig {
        SnmpConfig {
            definition: defs,
            ..Default::default()
        }
    }

    fn def(
        location: Option<&str>,
        specifics: &[&str],
        ranges: &[(&str, &str)],
        ip_match: &[&str],
    ) -> Definition {
        Definition {
            location: location.map(String::from),
            specific: specifics.iter().map(|s| s.to_string()).collect(),
            range: ranges
                .iter()
                .map(|(b, e)| Range {
                    begin: b.to_string(),
                    end: e.to_string(),
                })
                .collect(),
            ip_match: ip_match.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn specific_exact_match_yields_its_location() {
        let c = cfg(vec![def(Some("hq"), &["192.168.8.8"], &[], &[])]);
        assert_eq!(locations_for_ip(&c, ip("192.168.8.8")), vec!["hq"]);
        assert!(locations_for_ip(&c, ip("192.168.8.9")).is_empty());
    }

    #[test]
    fn range_is_inclusive_and_numeric() {
        let c = cfg(vec![def(
            Some("dc"),
            &[],
            &[("10.0.0.1", "10.0.0.254")],
            &[],
        )]);
        assert_eq!(locations_for_ip(&c, ip("10.0.0.1")), vec!["dc"]);
        assert_eq!(locations_for_ip(&c, ip("10.0.0.200")), vec!["dc"]);
        assert_eq!(locations_for_ip(&c, ip("10.0.0.254")), vec!["dc"]);
        assert!(locations_for_ip(&c, ip("10.0.1.1")).is_empty());
    }

    #[test]
    fn iplike_octet_patterns_match() {
        let c = cfg(vec![def(Some("edge"), &[], &[], &["10.*.4-6.1,2"])]);
        assert_eq!(locations_for_ip(&c, ip("10.99.5.2")), vec!["edge"]);
        assert!(locations_for_ip(&c, ip("10.99.7.2")).is_empty()); // octet3 out of 4-6
        assert!(locations_for_ip(&c, ip("10.99.5.3")).is_empty()); // octet4 not in 1,2
    }

    #[test]
    fn multiple_locations_are_deduped_and_sorted() {
        let c = cfg(vec![
            def(Some("nyc-dc"), &["192.168.8.8"], &[], &[]),
            def(
                Some("labmonkeys-hq"),
                &[],
                &[("192.168.8.1", "192.168.8.254")],
                &[],
            ),
            // duplicate location must collapse
            def(Some("nyc-dc"), &[], &[], &["192.168.8.*"]),
        ]);
        assert_eq!(
            locations_for_ip(&c, ip("192.168.8.8")),
            vec!["labmonkeys-hq", "nyc-dc"]
        );
    }

    #[test]
    fn definition_without_location_is_default() {
        let c = cfg(vec![def(None, &["192.168.8.8"], &[], &[])]);
        assert_eq!(
            locations_for_ip(&c, ip("192.168.8.8")),
            vec![DEFAULT_LOCATION]
        );
    }
}
