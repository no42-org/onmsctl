/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Integration-test harness for `onmsctl`.
//!
//! Connects to a live Horizon instance using credentials from
//! `ONMSCTL_TEST_URL`, `ONMSCTL_TEST_USER`, `ONMSCTL_TEST_PASSWORD`.
//! Integration tests should be `#[ignore]`d (so `make test` is unaffected)
//! and call [`harness_or_skip`] at the top of the test body. Tests are
//! run by `make integration` which passes `--include-ignored`.
//!
//! Semantics:
//!
//! - Any of the three env vars unset (or empty) → test prints a `SKIP:`
//!   line and returns. CI without secrets behaves the same as a developer
//!   who forgot to `export` the credentials.
//! - Env vars set but malformed (bad URL, etc.) → test panics. Loud
//!   failure forces investigation rather than silently masking the issue.
//!
//! All resources created by tests SHALL prefix their server-visible
//! `name` with [`RESOURCE_PREFIX`] (use [`Harness::unique_name`]). The
//! harness's cleanup sweep [`Harness::cleanup_event_sources`] uses that
//! prefix to find and delete leftover state from prior runs.

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, anyhow};
use onmsctl_core::{AuthCreds, Context, OnmsClient, OutputFormat, Url};
use onmsctl_eventconf::EventConfApi;

/// Prefix every integration-test-owned resource name SHALL carry. The
/// cleanup sweep matches on this prefix, so naming a resource without
/// it will leak the resource on the server.
pub const RESOURCE_PREFIX: &str = "onmsctl-it-";

pub const ENV_URL: &str = "ONMSCTL_TEST_URL";
pub const ENV_USER: &str = "ONMSCTL_TEST_USER";
pub const ENV_PASSWORD: &str = "ONMSCTL_TEST_PASSWORD";

/// Process-wide counter appended to unique names so multiple
/// `unique_name` calls in the same nanosecond stay distinct.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Outcome of [`Harness::from_env`]. `Skipped` is not an error — it
/// means the harness env is intentionally absent and the test should
/// no-op.
//
// Clippy's `large_enum_variant` lint flags the size delta between
// `Ready(Harness)` and `Skipped(String)`. This enum is constructed
// exactly once per test process, so the size overhead is nil; boxing
// would just add an indirection that the harness macro has to thread
// through.
#[allow(clippy::large_enum_variant)]
pub enum Setup {
    Ready(Harness),
    Skipped(String),
}

pub struct Harness {
    client: OnmsClient,
    url: Url,
    creds: AuthCreds,
}

impl Harness {
    /// Resolve the three env vars and build an [`OnmsClient`].
    ///
    /// Returns `Ok(Skipped)` when any var is unset or empty; returns
    /// `Err` when a var is set but unusable (URL fails to parse, etc.).
    pub fn from_env() -> Result<Setup> {
        let url = match env::var(ENV_URL) {
            Ok(v) if !v.is_empty() => v,
            _ => return Ok(Setup::Skipped(format!("{ENV_URL} unset or empty"))),
        };
        let user = match env::var(ENV_USER) {
            Ok(v) if !v.is_empty() => v,
            _ => return Ok(Setup::Skipped(format!("{ENV_USER} unset or empty"))),
        };
        let password = match env::var(ENV_PASSWORD) {
            Ok(v) if !v.is_empty() => v,
            _ => return Ok(Setup::Skipped(format!("{ENV_PASSWORD} unset or empty"))),
        };

        let parsed =
            Url::parse(&url).with_context(|| format!("{ENV_URL}='{url}' is not a valid URL"))?;
        let creds = AuthCreds::basic(user, password);
        let client = OnmsClient::from_parts(parsed.clone(), creds.clone())
            .map_err(|e| anyhow!("OnmsClient::from_parts failed: {e}"))?;
        Ok(Setup::Ready(Harness {
            client,
            url: parsed,
            creds,
        }))
    }

    pub fn client(&self) -> &OnmsClient {
        &self.client
    }

    /// Build a [`Context`] suitable for `run_apply::<EventSourceTarget>`
    /// and other driver-level entry points that rebuild their own
    /// client from `Context`. `verbose` toggles the stderr warnings
    /// guarded by `ctx.verbose` (notably the disabled-state apply
    /// flap notice).
    pub fn context(&self, verbose: bool) -> Context {
        Context {
            url: self.url.clone(),
            creds: self.creds.clone(),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose,
        }
    }

    /// Build a resource name unique to this test process. The
    /// [`RESOURCE_PREFIX`] is mandatory — the cleanup sweep relies on
    /// it.
    pub fn unique_name(&self, slug: &str) -> String {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{RESOURCE_PREFIX}{slug}-{pid}-{nanos}-{n}")
    }

    /// Delete every EventConf source whose name starts with
    /// [`RESOURCE_PREFIX`]. Tests SHOULD call this both before their
    /// own setup (to clear state from a crashed prior run) and after
    /// their own teardown (so a crash mid-test leaves a smaller mess).
    pub async fn cleanup_event_sources(&self) -> Result<usize> {
        let api = EventConfApi::new(&self.client);
        let names_and_ids = api
            .list_source_names_and_ids()
            .await
            .map_err(|e| anyhow!("list_source_names_and_ids: {e}"))?;
        let ids: Vec<i64> = names_and_ids
            .into_iter()
            .filter(|n| n.name.starts_with(RESOURCE_PREFIX))
            .map(|n| n.id)
            .collect();
        if ids.is_empty() {
            return Ok(0);
        }
        let n = ids.len();
        api.delete_sources(&ids)
            .await
            .map_err(|e| anyhow!("delete_sources: {e}"))?;
        Ok(n)
    }
}

/// Resolve the harness or skip the test. Designed to sit at the top of
/// every `#[tokio::test]` body in `tests/`.
#[macro_export]
macro_rules! harness_or_skip {
    () => {
        match $crate::Harness::from_env() {
            Ok($crate::Setup::Ready(h)) => h,
            Ok($crate::Setup::Skipped(reason)) => {
                eprintln!("SKIP: {reason}");
                return;
            }
            Err(e) => panic!("integration harness setup failed: {e:#}"),
        }
    };
}
