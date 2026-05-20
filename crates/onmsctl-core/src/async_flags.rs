/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Shared `--wait` / `--timeout` / `--poll-interval` flag set for any
//! subcommand whose underlying server operation is asynchronous.
//!
//! Capability crates embed this via clap's `#[command(flatten)]` on the
//! variants that trigger async server work (e.g. requisition `apply`,
//! `import`). The polling driver itself is intentionally not provided
//! here — its shape is consumer-specific (scan reports vs job status
//! vs other endpoints) and lives in the capability crate that owns the
//! resource. This module defines only the flag surface and the
//! durations the binary parses on the user's behalf.
//!
//! Per `cli-core` spec defaults: `--timeout` is 30 minutes,
//! `--poll-interval` is 5 seconds. Exit codes for wait failures live on
//! [`crate::Error`]: `WaitTimeout` → exit 10, `AsyncOpFailed` → exit 11.

use std::time::Duration;

use clap::Args;

/// Standard async-flag set for subcommands that trigger an
/// asynchronous server-side operation.
///
/// Without `--wait`, the subcommand SHALL return immediately once the
/// server accepts the request (printing any operation handle to stdout).
/// With `--wait`, the subcommand SHALL poll until convergence, bounded
/// by `--timeout`, at the cadence `--poll-interval`.
#[derive(Args, Debug, Clone)]
pub struct AsyncFlags {
    /// Block until the async server-side operation reaches a terminal
    /// state (success or failure). Without this flag the subcommand
    /// returns as soon as the server accepts the request and prints
    /// the operation handle to stdout for later resumption.
    #[arg(long)]
    pub wait: bool,

    /// Maximum time to wait when `--wait` is set. Accepts a humanized
    /// duration like `30m`, `1h`, `90s`. Defaults to 30 minutes.
    /// Exit code 10 if the timeout fires before convergence.
    #[arg(long, value_parser = parse_duration, default_value = "30m")]
    pub timeout: Duration,

    /// Polling cadence when `--wait` is set. Accepts a humanized
    /// duration like `5s`, `1m`. Defaults to 5 seconds.
    #[arg(long, value_parser = parse_duration, default_value = "5s")]
    pub poll_interval: Duration,
}

impl Default for AsyncFlags {
    fn default() -> Self {
        Self {
            wait: false,
            timeout: Duration::from_secs(30 * 60),
            poll_interval: Duration::from_secs(5),
        }
    }
}

/// Parse a humanized duration string with a single-character unit
/// suffix.
///
/// Accepted units: `s` (seconds), `m` (minutes), `h` (hours), `d` (days).
/// A bare integer is rejected — the unit is required so the meaning is
/// unambiguous. Examples: `5s`, `30m`, `1h`, `7d`. The numeric portion
/// must be a positive integer; fractions are not supported.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".to_string());
    }
    let (num_part, unit_char) = match s.char_indices().last() {
        Some((idx, c)) if c.is_ascii_alphabetic() => (&s[..idx], c),
        _ => return Err(format!("missing unit in duration {s:?} (expected s/m/h/d)")),
    };
    let n: u64 = num_part
        .parse()
        .map_err(|_| format!("invalid number in duration {s:?}"))?;
    let secs = match unit_char.to_ascii_lowercase() {
        's' => n,
        'm' => n.checked_mul(60).ok_or("overflow")?,
        'h' => n.checked_mul(3600).ok_or("overflow")?,
        'd' => n.checked_mul(86400).ok_or("overflow")?,
        other => {
            return Err(format!(
                "unknown duration unit '{other}' (expected s/m/h/d)"
            ));
        }
    };
    Ok(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_each_unit() {
        assert_eq!(parse_duration("5s").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("2d").unwrap(), Duration::from_secs(172_800));
    }

    #[test]
    fn parse_duration_case_insensitive_unit() {
        assert_eq!(parse_duration("5S").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("30M").unwrap(), Duration::from_secs(1800));
    }

    #[test]
    fn parse_duration_rejects_missing_unit() {
        assert!(parse_duration("30").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn parse_duration_rejects_unknown_unit() {
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("1w").is_err());
    }

    #[test]
    fn parse_duration_rejects_negatives_and_fractions() {
        assert!(parse_duration("-5s").is_err());
        assert!(parse_duration("1.5m").is_err());
    }

    #[test]
    fn async_flags_default_matches_spec() {
        let f = AsyncFlags::default();
        assert!(!f.wait);
        assert_eq!(f.timeout, Duration::from_secs(30 * 60));
        assert_eq!(f.poll_interval, Duration::from_secs(5));
    }
}
