/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Reverse of the `apply` path: read deployed Horizon state back into
//! `kind: Requisition` YAML for git-managed sync (tasks 5.13–5.14).
//!
//! Two functions:
//!
//! - [`export_requisition`] — single foreign-source. Fetches the
//!   requisition + custom foreign-source (if any) and emits a YAML
//!   document. With `include_defaults` true, when the server has no
//!   custom FS we GET `/foreignSources/default` and inline it with a
//!   snapshot-timestamp YAML header comment so the operator can see
//!   what defaults the requisition would inherit at apply time.
//! - [`export_all_requisitions`] — every requisition the server lists
//!   via `list_requisition_names`. Calls `export_requisition` for each.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::api::ProvisioningApi;
use crate::model::server::ForeignSourceServer;
use crate::model::{RequisitionLocal, requisition_from_wire};
use onmsctl_core::{Error, Result};

/// Per-requisition export outcome. Carries the rendered YAML, the
/// foreign-source name (for sorting / file naming), and a flag noting
/// whether `spec.foreignSource` was synthesized from the default-FS
/// (so callers can attribute the inlined block in their output).
#[derive(Debug, Clone, Serialize)]
pub struct ExportOutcome {
    pub foreign_source: String,
    /// Serialized YAML document. Includes a leading `# yaml-language-
    /// server: ...` directive and, when default-FS was inlined, a
    /// snapshot-timestamp comment naming the inlining.
    pub yaml: String,
    /// `true` when the YAML's `spec.foreignSource` block came from
    /// Horizon's default-FS rather than a custom per-requisition FS.
    /// `false` when the server had a custom FS (or when
    /// `include_defaults` was false — in which case the YAML omits
    /// `spec.foreignSource` entirely per design D1's portable style).
    pub default_fs_inlined: bool,
}

/// Export a single requisition (with optional FS) to YAML.
///
/// - 404 on the requisition surfaces as `Error::Config` ("not found")
///   so the CLI exit code stays consistent with the `status` verb's
///   404 handling.
/// - 404 on the custom FS is the portable-style case; the emitted
///   YAML omits `spec.foreignSource` unless `include_defaults` is
///   true (in which case the default-FS is inlined with a snapshot
///   comment).
pub async fn export_requisition(
    api: &ProvisioningApi<'_>,
    fs: &str,
    include_defaults: bool,
) -> Result<ExportOutcome> {
    // Single-fs entry point: fetch the default-FS on demand inside
    // the helper. Bulk callers should use `export_all_requisitions`
    // which caches the default-FS across iterations.
    export_requisition_with_default(api, fs, include_defaults, None).await
}

/// Internal helper that lets `export_all_requisitions` pass a
/// pre-fetched default-FS, avoiding N identical GETs in bulk mode.
/// When `cached_default` is `None` and `include_defaults` is true,
/// this fetches the default on demand (matching the single-fs path).
async fn export_requisition_with_default(
    api: &ProvisioningApi<'_>,
    fs: &str,
    include_defaults: bool,
    cached_default: Option<&ForeignSourceServer>,
) -> Result<ExportOutcome> {
    let req = api.get_requisition(fs).await?.ok_or_else(|| {
        Error::Config(format!(
            "no requisition '{fs}' on the server (GET /rest/requisitions/{fs} returned 404)"
        ))
    })?;
    let custom_fs = api.get_foreign_source(fs).await?;

    // Decide which FS to attach. The expensive case is "no custom
    // FS, --include-defaults set, no cached default" — that's the
    // only path that issues an HTTP GET for the default. Bulk
    // callers feed a `cached_default` to skip the per-fs GET.
    let default_fs_inlined = custom_fs.is_none() && include_defaults;
    let default_fetched: Option<ForeignSourceServer> =
        if default_fs_inlined && cached_default.is_none() {
            Some(api.get_default_foreign_source().await?)
        } else {
            None
        };
    let fs_to_attach = custom_fs
        .as_ref()
        .or(cached_default)
        .or(default_fetched.as_ref());

    let local = requisition_from_wire(&req, fs_to_attach);
    let yaml = render_yaml(&local, default_fs_inlined)?;

    Ok(ExportOutcome {
        foreign_source: req.foreign_source.clone(),
        yaml,
        default_fs_inlined,
    })
}

/// Export every requisition on the server. Calls
/// `list_requisition_names` once, then `export_requisition` per name
/// in sorted order. Failures abort with the first error — partial
/// export is more confusing than no export.
///
/// When `include_defaults` is set, fetches `/foreignSources/default`
/// exactly once at the top and reuses the cached value across every
/// portable-style requisition. A server with 50 portable
/// requisitions saves 49 round-trips compared to per-fs fetches.
pub async fn export_all_requisitions(
    api: &ProvisioningApi<'_>,
    include_defaults: bool,
) -> Result<Vec<ExportOutcome>> {
    let names = api.list_requisition_names().await?;
    // Cache the default-FS once if any portable-style requisition
    // might need it. Fetched eagerly when include_defaults is set
    // (the alternative — lazy fetch on first miss — would need
    // mutable state threaded through the loop, which buys little).
    let cached_default = if include_defaults {
        Some(api.get_default_foreign_source().await?)
    } else {
        None
    };
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let outcome = export_requisition_with_default(
            api,
            &name,
            include_defaults,
            cached_default.as_ref(),
        )
        .await?;
        out.push(outcome);
    }
    Ok(out)
}

/// Render a `RequisitionLocal` to YAML with the standard header
/// directive. When `default_fs_inlined` is true, prepend a snapshot-
/// timestamp comment so the operator knows the `spec.foreignSource`
/// block came from Horizon's default at a specific moment (not the
/// requisition's own per-FS overrides).
fn render_yaml(local: &RequisitionLocal, default_fs_inlined: bool) -> Result<String> {
    let body = serde_norway::to_string(local)
        .map_err(|e| Error::Config(format!("serializing exported requisition: {e}")))?;

    let mut out = String::new();
    out.push_str(
        "# yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/requisition.schema.json\n",
    );
    if default_fs_inlined {
        let now = format_unix_ts(SystemTime::now());
        out.push_str(&format!(
            "# spec.foreignSource was inlined from Horizon's default \
             foreign-source at {now}.\n\
             # Drop the spec.foreignSource block to revert to portable \
             style (Horizon's default-FS is\n\
             # re-applied at every `apply` — the inlined snapshot does \
             not stay in sync after this export).\n",
        ));
    } else if local.spec.foreign_source.is_none() {
        // Portable style: spec.foreignSource is omitted, so the
        // requisition inherits Horizon's default-FS at apply time.
        // Spec.md mandates the inherit-default annotation comment so
        // a future reader of the YAML doesn't assume detectors /
        // policies are intentionally absent — they're inherited
        // from the server's current default.
        out.push_str(
            "# Portable style: spec.foreignSource is omitted; this requisition inherits\n\
             # Horizon's default foreign-source (detectors + policies) at apply time.\n\
             # Run `requisition export <fs> --include-defaults` to snapshot the\n\
             # current default into spec.foreignSource if you need a pinned record.\n",
        );
    }
    out.push_str(&body);
    Ok(out)
}

/// Format a SystemTime as `YYYY-MM-DDTHH:MM:SSZ` UTC without pulling
/// in chrono. Computes Gregorian Y/M/D from epoch seconds using the
/// standard algorithm (Howard Hinnant's date library, public domain).
fn format_unix_ts(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400);
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;

    // Howard Hinnant's days_from_civil inverse.
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Convert days since 1970-01-01 to (year, month, day) in proleptic
/// Gregorian calendar. Algorithm: Howard Hinnant's `civil_from_days`,
/// adapted from his date library (public domain, well-tested).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { (mp + 3) as u32 } else { (mp - 9) as u32 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
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
        };
        let client = OnmsClient::from_context(&ctx).unwrap();
        (server, client)
    }

    #[tokio::test]
    async fn export_single_with_custom_fs_includes_foreignsource_block() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme",
                "node": [{
                    "foreign-id": "web01",
                    "node-label": "web01.acme",
                    "interface": [],
                    "category": [],
                    "asset": [],
                    "meta-data": []
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "acme",
                "scan-interval": "30m",
                "detectors": [],
                "policies": []
            })))
            .mount(&server)
            .await;

        let out = export_requisition(&api, "acme", false).await.unwrap();
        assert_eq!(out.foreign_source, "acme");
        assert!(!out.default_fs_inlined);
        assert!(out.yaml.contains("kind: Requisition"));
        assert!(out.yaml.contains("name: acme"));
        assert!(out.yaml.contains("foreignSource:"));
        assert!(out.yaml.contains("scanInterval: 30m"));
    }

    #[tokio::test]
    async fn export_single_no_custom_fs_omits_foreignsource_block() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme",
                "node": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let out = export_requisition(&api, "acme", false).await.unwrap();
        assert!(!out.default_fs_inlined);
        assert!(!out.yaml.contains("foreignSource:"));
        assert!(!out.yaml.contains("inlined from Horizon's default"));
        // Portable-style annotation comment — spec.md mandates this
        // so future readers don't assume detectors/policies are
        // intentionally absent.
        assert!(out.yaml.contains("Portable style: spec.foreignSource is omitted"));
        assert!(out.yaml.contains("inherits"));
    }

    #[tokio::test]
    async fn export_single_with_include_defaults_inlines_default_fs() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "acme",
                "node": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/acme"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "default",
                "scan-interval": "1d",
                "detectors": [{
                    "name": "ICMP",
                    "class": "org.opennms.netmgt.provision.detector.icmp.IcmpDetector"
                }],
                "policies": []
            })))
            .mount(&server)
            .await;

        let out = export_requisition(&api, "acme", true).await.unwrap();
        assert!(out.default_fs_inlined);
        assert!(out.yaml.contains("foreignSource:"));
        assert!(out.yaml.contains("scanInterval: 1d"));
        assert!(out.yaml.contains("IcmpDetector"));
        // Snapshot comment appears with an ISO-8601-shaped timestamp.
        // A regex like `\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z` would be
        // cleanest, but pulling the regex crate for one assertion is
        // overkill — do a manual shape check that catches the failure
        // mode (broken timestamp, e.g. epoch-0 or empty string).
        let marker = "inlined from Horizon's default foreign-source at ";
        let idx = out
            .yaml
            .find(marker)
            .expect("snapshot marker present in comment");
        let ts_start = idx + marker.len();
        let ts = &out.yaml[ts_start..ts_start + 20]; // YYYY-MM-DDTHH:MM:SSZ = 20 chars
        assert_eq!(ts.len(), 20, "timestamp must be 20 chars (YYYY-MM-DDTHH:MM:SSZ), got {ts:?}");
        assert!(
            ts.ends_with('Z'),
            "timestamp must end with Z (UTC marker), got {ts:?}"
        );
        let bytes = ts.as_bytes();
        // Manual shape check: digit positions (0..4, 5..7, 8..10,
        // 11..13, 14..16, 17..19), separator positions (4='-', 7='-',
        // 10='T', 13=':', 16=':').
        for pos in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
            assert!(
                bytes[pos].is_ascii_digit(),
                "byte {pos} of timestamp {ts:?} must be a digit"
            );
        }
        assert_eq!(bytes[4], b'-', "byte 4 must be '-' in {ts:?}");
        assert_eq!(bytes[7], b'-', "byte 7 must be '-' in {ts:?}");
        assert_eq!(bytes[10], b'T', "byte 10 must be 'T' in {ts:?}");
        assert_eq!(bytes[13], b':', "byte 13 must be ':' in {ts:?}");
        assert_eq!(bytes[16], b':', "byte 16 must be ':' in {ts:?}");
        // Sanity check: the year is at least 1970 (no clock-skew
        // degradation to epoch-0 1970-01-01 in a real environment).
        let year: u16 = ts[0..4].parse().expect("year parses");
        assert!(year >= 1970, "year {year} suspiciously low — clock skew?");
    }

    #[tokio::test]
    async fn export_404_on_requisition_surfaces_as_config_error() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitions/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let err = export_requisition(&api, "missing", false).await.unwrap_err();
        assert!(err.to_string().contains("no requisition 'missing'"));
    }

    #[tokio::test]
    async fn export_all_iterates_listed_names() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitionNames"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "count": 2,
                "foreign-source": ["alpha", "zebra"]
            })))
            .mount(&server)
            .await;
        for name in ["alpha", "zebra"] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/requisitions/{name}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "foreign-source": name,
                    "node": []
                })))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/rest/foreignSources/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
        }

        let outs = export_all_requisitions(&api, false).await.unwrap();
        assert_eq!(outs.len(), 2);
        // list_requisition_names sorts ascending.
        assert_eq!(outs[0].foreign_source, "alpha");
        assert_eq!(outs[1].foreign_source, "zebra");
    }

    #[tokio::test]
    async fn export_all_aborts_on_first_per_fs_error() {
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitionNames"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "count": 2,
                "foreign-source": ["alpha", "zebra"]
            })))
            .mount(&server)
            .await;
        // alpha 200, zebra 404 → error on second iteration.
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/alpha"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "foreign-source": "alpha",
                "node": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/alpha"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/requisitions/zebra"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let err = export_all_requisitions(&api, false).await.unwrap_err();
        assert!(err.to_string().contains("no requisition 'zebra'"));
    }

    #[tokio::test]
    async fn export_all_caches_default_fs_across_iterations() {
        // Three portable requisitions + --include-defaults must only
        // GET /foreignSources/default ONCE (via the cache in
        // export_all_requisitions). `.expect(1)` on the mock pins the
        // contract; without the cache the count would be 3.
        let (server, client) = mock_with_client().await;
        let api = ProvisioningApi::new(&client);

        Mock::given(method("GET"))
            .and(path("/rest/requisitionNames"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "count": 3,
                "foreign-source": ["alpha", "beta", "gamma"]
            })))
            .mount(&server)
            .await;
        for name in ["alpha", "beta", "gamma"] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/requisitions/{name}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "foreign-source": name,
                    "node": []
                })))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/rest/foreignSources/{name}")))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
        }
        // Exactly one GET to the default-FS endpoint, regardless of
        // how many portable requisitions consume it.
        Mock::given(method("GET"))
            .and(path("/rest/foreignSources/default"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "default",
                "scan-interval": "1d",
                "detectors": [],
                "policies": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let outs = export_all_requisitions(&api, true).await.unwrap();
        assert_eq!(outs.len(), 3);
        // Every result has the default inlined.
        assert!(outs.iter().all(|o| o.default_fs_inlined));
        // wiremock verifies .expect(1) on drop.
    }

    #[test]
    fn format_unix_ts_known_values() {
        // Epoch.
        assert_eq!(format_unix_ts(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // 2026-05-22 12:00:00 UTC = 1_779_451_200.
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_779_451_200);
        assert_eq!(format_unix_ts(t), "2026-05-22T12:00:00Z");
        // Leap year handling: 2024-02-29 00:00:00 UTC = 1_709_164_800.
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800);
        assert_eq!(format_unix_ts(t), "2024-02-29T00:00:00Z");
    }
}
