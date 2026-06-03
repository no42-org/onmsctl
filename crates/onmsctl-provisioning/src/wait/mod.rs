/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `--wait` poller for asynchronous requisition imports.
//!
//! Horizon's `PUT /rest/requisitions/{fs}/import` is fire-and-forget at
//! the HTTP layer: the server accepts the trigger and runs the import
//! / scan asynchronously. The poller observes completion by watching
//! the requisition's server-managed `last-import` epoch-ms field
//! advance past the pre-trigger snapshot.
//!
//! Scope of this version (paired with task 6.3):
//!
//! - Read-only poll loop over `GET /rest/requisitions/{fs}`.
//! - Convergence: `req.last_import != pre_trigger_last_import_ms`
//!   (i.e. the field changed since the trigger was issued).
//! - Timeout: returns [`Error::WaitTimeout`] (exit 10) when
//!   `--timeout` elapses before convergence.
//! - Failure: returns [`Error::AsyncOpFailed`] (exit 11) only if the
//!   requisition disappears from the server mid-poll (404). Per-import
//!   success / failure classification requires the scan-reports
//!   endpoint and is deferred to a future iteration.
//!
//! Errors from individual poll GETs (transient 5xx, DNS hiccups) are
//! propagated verbatim — no internal retry loop. Operators can re-run
//! the verb if the network was flaky; layering retry on top of
//! polling is the future enhancement.

use std::time::{Duration, Instant};

use onmsctl_core::{AsyncFlags, Error, Result};

use crate::api::ProvisioningApi;

/// Poll the named requisition until its `last-import` timestamp moves
/// past `pre_trigger_last_import_ms`. Returns the new last-import
/// timestamp on success.
///
/// `pre_trigger_last_import_ms` SHOULD be the value observed
/// immediately BEFORE the operator-side trigger that this wait is
/// observing. `None` is the "never imported" case — the poll succeeds
/// when the field becomes `Some(_)`.
pub async fn wait_for_import_completion(
    api: &ProvisioningApi<'_>,
    fs: &str,
    pre_trigger_last_import_ms: Option<i64>,
    flags: &AsyncFlags,
) -> Result<i64> {
    let start = Instant::now();
    loop {
        let req = api.get_requisition(fs).await?.ok_or_else(|| {
            // The requisition disappeared mid-poll. Either an
            // operator deleted it via the UI, or Horizon discarded
            // the in-flight import. Surface as AsyncOpFailed so the
            // exit code (11) signals "the operation we were watching
            // is no longer recoverable" rather than the generic
            // HTTP-404 class.
            Error::AsyncOpFailed {
                handle: fs.to_string(),
                reason: format!("requisition '{fs}' vanished during poll"),
            }
        })?;

        // Convergence: the field must have advanced to a populated
        // value distinct from the snapshot. `Some(x) → Some(y)` and
        // `None → Some(y)` are both real imports we can observe.
        // `Some(x) → None` is a server-side rebuild / rollback that
        // CLEARED the field — that's not an import; keep polling
        // (the timeout below catches us if convergence never happens).
        if let Some(new_ts) = req.last_import
            && Some(new_ts) != pre_trigger_last_import_ms
        {
            // Field advanced — import is observably complete from the
            // requisition's perspective. Per-import success/failure
            // status (the AsyncOpFailed-when-server-reports-failure
            // path) requires the scan-reports endpoint; deferred.
            return Ok(new_ts);
        }

        // Check the timeout AFTER the poll attempt so a generous
        // timeout doesn't waste a sleep cycle when convergence
        // happens on the very first try.
        if start.elapsed() >= flags.timeout {
            return Err(Error::WaitTimeout {
                handle: fs.to_string(),
                timeout: format_duration(flags.timeout),
            });
        }

        tokio::time::sleep(flags.poll_interval).await;
    }
}

/// Format a [`Duration`] as a humanized compound string. Composes
/// non-zero `d`/`h`/`m`/`s` segments left-to-right so error messages
/// read sensibly for any duration. Examples:
/// `90s → "1m30s"`, `7260s → "2h1m"`, `86_401s → "1d1s"`,
/// `Duration::ZERO → "0s"`. Sub-second remainders are truncated —
/// `--timeout` is documented to take integer seconds.
fn format_duration(d: Duration) -> String {
    let mut secs = d.as_secs();
    if secs == 0 {
        return "0s".into();
    }
    const DAY: u64 = 86_400;
    const HOUR: u64 = 3_600;
    const MIN: u64 = 60;
    let mut out = String::new();
    for (size, unit) in [(DAY, 'd'), (HOUR, 'h'), (MIN, 'm')] {
        if secs >= size {
            let n = secs / size;
            secs %= size;
            out.push_str(&format!("{n}{unit}"));
        }
    }
    if secs > 0 {
        out.push_str(&format!("{secs}s"));
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ProvisioningApi;
    use onmsctl_core::{AuthCreds, Context, OnmsClient, OutputFormat, Url};
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_with_client() -> (MockServer, OnmsClient) {
        let server = MockServer::start().await;
        let url = Url::parse(&format!("{}/", server.uri())).unwrap();
        let ctx = Context {
            name: "test".into(),
            url,
            creds: AuthCreds::bearer("t"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        };
        let client = OnmsClient::from_context(&ctx).unwrap();
        (server, client)
    }

    fn fast_flags() -> AsyncFlags {
        AsyncFlags {
            wait: true,
            timeout: Duration::from_millis(500),
            poll_interval: Duration::from_millis(20),
        }
    }

    #[tokio::test]
    async fn returns_new_timestamp_when_last_import_advances() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        // First two GETs return the pre-trigger snapshot; third
        // returns the advanced timestamp. wiremock matchers fire in
        // registration order with respect to up_to_n_times.
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "last-import": 1_000,
                "node": []
            })))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "last-import": 9_999,
                "node": []
            })))
            .mount(&server)
            .await;

        let got = wait_for_import_completion(&api, "acme-prod", Some(1_000), &fast_flags())
            .await
            .unwrap();
        assert_eq!(got, 9_999);
    }

    #[tokio::test]
    async fn returns_wait_timeout_when_field_never_advances() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        // Every poll returns the same pre-trigger snapshot.
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "last-import": 1_000,
                "node": []
            })))
            .mount(&server)
            .await;

        let err = wait_for_import_completion(&api, "acme-prod", Some(1_000), &fast_flags())
            .await
            .unwrap_err();
        match err {
            Error::WaitTimeout { handle, timeout } => {
                assert_eq!(handle, "acme-prod");
                assert!(
                    timeout.ends_with('s') || timeout.ends_with('m'),
                    "timeout {timeout} should be a humanized duration"
                );
            }
            other => panic!("expected WaitTimeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_async_op_failed_when_requisition_vanishes() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let err = wait_for_import_completion(&api, "acme-prod", Some(1_000), &fast_flags())
            .await
            .unwrap_err();
        match err {
            Error::AsyncOpFailed { handle, reason } => {
                assert_eq!(handle, "acme-prod");
                assert!(reason.contains("vanished"));
            }
            other => panic!("expected AsyncOpFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn succeeds_when_pre_snapshot_was_none_and_server_now_reports_value() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "last-import": 5_555,
                "node": []
            })))
            .mount(&server)
            .await;

        let got = wait_for_import_completion(&api, "acme-prod", None, &fast_flags())
            .await
            .unwrap();
        assert_eq!(got, 5_555);
    }

    #[tokio::test]
    async fn some_to_none_regression_is_not_convergence() {
        // Server clears last-import mid-poll (rebuild / rollback).
        // Snapshot was Some(1000); every poll returns None. Loop
        // must NOT treat None as convergence — it should time out.
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme-prod"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme-prod",
                "node": []
            })))
            .mount(&server)
            .await;

        let err = wait_for_import_completion(&api, "acme-prod", Some(1_000), &fast_flags())
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::WaitTimeout { .. }),
            "Some→None should not count as convergence, got {err:?}"
        );
    }

    #[test]
    fn format_duration_composes_compound_units() {
        // Pure-unit cases (largest applicable).
        assert_eq!(format_duration(Duration::from_secs(5)), "5s");
        assert_eq!(format_duration(Duration::from_secs(300)), "5m");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h");
        assert_eq!(format_duration(Duration::from_secs(7200)), "2h");
        assert_eq!(format_duration(Duration::from_secs(86_400)), "1d");
        // Compound cases — the regression the prior implementation hit.
        assert_eq!(format_duration(Duration::from_secs(90)), "1m30s");
        assert_eq!(format_duration(Duration::from_secs(7260)), "2h1m");
        assert_eq!(format_duration(Duration::from_secs(86_401)), "1d1s");
        assert_eq!(format_duration(Duration::from_secs(90_061)), "1d1h1m1s");
        // Boundary.
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        // Sub-second truncates (documented).
        assert_eq!(format_duration(Duration::from_millis(1500)), "1s");
    }
}
