/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `apply -f` for `ThresholdGroup` and `ThreshdPackage` (design D6).
//!
//! Both handlers follow the same shape: plan validates every document, runs
//! the plan gates, reads each live object with its ETag and decides
//! `Create`, `Update` or unchanged; execute sends `POST`, `PUT` with
//! `If-Match`, or nothing. Apply never deletes.

pub mod group;
pub mod package;

pub use group::ThresholdGroupHandler;
pub use package::ThreshdPackageHandler;

use std::collections::BTreeMap;

use onmsctl_core::{Action, ApplyOutcome, Error, Result};

const RETRY_HINT: &str = "re-run `onmsctl apply -f` after resolving the error";

/// One planned object: its desired wire body, the planned action, and the
/// ETag read during plan.
pub(crate) struct Planned<D> {
    pub name: String,
    pub desired: D,
    pub action: Action,
    pub etag: Option<String>,
}

/// Refuse the same `metadata.name` in more than one document of a kind.
pub(crate) fn check_unique_names<'a>(
    kind: &str,
    docs: impl Iterator<Item = (String, &'a str)>,
) -> Result<()> {
    let mut by_name: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (label, name) in docs {
        by_name.entry(name).or_default().push(label);
    }
    let dups: Vec<String> = by_name
        .iter()
        .filter(|(_, labels)| labels.len() > 1)
        .map(|(name, labels)| format!("'{name}' ({})", labels.join(", ")))
        .collect();
    if dups.is_empty() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "duplicate {kind} metadata.name across the apply input: {}",
            dups.join("; ")
        )))
    }
}

/// The outcome for a failed write, with the design-D6 messages for the
/// conflict statuses.
pub(crate) fn write_failure(kind: &str, name: &str, action: Action, e: Error) -> ApplyOutcome {
    let (msg, hint) = match &e {
        Error::HttpStatus { status: 412, .. } => (
            format!("{kind} '{name}' changed on the server since plan"),
            "re-run `onmsctl apply -f` to plan against the current server state",
        ),
        Error::HttpStatus { status: 409, .. } => (
            format!("{kind} '{name}' was created on the server since plan"),
            "re-run `onmsctl apply -f` to plan against the current server state",
        ),
        _ => (e.to_string(), RETRY_HINT),
    };
    ApplyOutcome::failed(kind, name, action, msg, hint)
}

/// Join per-document diffs into the bucket's `--diff` text.
pub(crate) fn join_diffs(diffs: Vec<String>) -> Option<String> {
    (!diffs.is_empty()).then(|| diffs.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::kind::DocRef;
    use onmsctl_core::kind::precedence::{RANK_THRESHD_PACKAGE, RANK_THRESHOLD_GROUP};
    use onmsctl_core::{
        ApplyParams, AuthCreds, Context, KindHandler, OutcomeStatus, OutputFormat, Registry, Url,
        apply_documents, parse_documents,
    };
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn ctx(server: &MockServer) -> Context {
        Context {
            name: "test".into(),
            url: Url::parse(&format!("{}/", server.uri())).unwrap(),
            creds: AuthCreds::bearer("t"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        }
    }

    const GROUP: &str = "\
apiVersion: thresholding.opennms.org/v1
kind: ThresholdGroup
metadata: {name: acme-cpu}
spec:
  rrdRepository: /r/
  thresholds:
    - {type: high, dsType: node, dsName: cpuLoad, value: 90, rearm: 80, trigger: 3}
";

    const PACKAGE: &str = "\
apiVersion: thresholding.opennms.org/v1
kind: ThreshdPackage
metadata: {name: acme}
spec:
  filter: IPADDR != '0.0.0.0'
  services:
    - {name: SNMP, interval: 5m, thresholdingGroup: acme-cpu}
";

    /// The group above as the server returns it.
    fn live_group(value: &str) -> serde_json::Value {
        json!({"name": "acme-cpu", "rrdRepository": "/r/", "readOnly": false, "version": "g1",
            "expressions": [],
            "thresholds": [{"type": "high", "dsType": "node", "dsName": "cpuLoad", "value": value,
                "rearm": "80", "trigger": "3", "description": null, "dsLabel": null,
                "exprLabel": null, "triggeredUEI": null, "rearmedUEI": null,
                "filterOperator": "or", "relaxed": false, "resourceFilters": []}]})
    }

    /// The package above as the server returns it, with an outage attached.
    fn live_package(interval: i64) -> serde_json::Value {
        json!({"name": "acme", "filter": "IPADDR != '0.0.0.0'", "specifics": [],
            "includeRanges": [], "excludeRanges": [], "includeUrls": [],
            "outageCalendars": ["weekly-maint"], "version": "p1",
            "services": [{"name": "SNMP", "interval": interval, "userDefined": true, "status": null,
                "parameters": [{"key": "thresholding-group", "value": "acme-cpu"}]}]})
    }

    async fn mount_get(server: &MockServer, p: &str, status: u16, body: Option<serde_json::Value>) {
        let mut r = ResponseTemplate::new(status);
        if let Some(b) = body {
            r = r.insert_header("ETag", "\"e1\"").set_body_json(b);
        }
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(r)
            .mount(server)
            .await;
    }

    async fn base(server: &MockServer) {
        mount_get(
            server,
            "/api/v2/thresholding/metadata",
            200,
            Some(json!({"dsTypes": [{"name": "node"}, {"name": "if"}]})),
        )
        .await;
        mount_get(
            server,
            "/api/v2/thresholding/groups",
            200,
            Some(json!([{"name": "mib2", "readOnly": false}])),
        )
        .await;
    }

    async fn writes(server: &MockServer) -> Vec<Request> {
        server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method.as_str() != "GET")
            .collect()
    }

    fn doc(yaml: &str) -> Vec<onmsctl_core::RawDoc> {
        parse_documents("t.yaml", yaml).unwrap()
    }

    async fn run(
        h: &dyn KindHandler,
        yaml: &str,
        params: &ApplyParams,
        c: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let plan = h.plan(&doc(yaml), params, c).await?;
        h.execute(plan, params, c).await
    }

    fn plan_err<T>(r: Result<T>) -> String {
        match r {
            Err(Error::Config(m)) => m,
            Err(e) => panic!("expected a config error, got {e:?}"),
            Ok(_) => panic!("expected a plan error"),
        }
    }

    // -- groups --

    #[tokio::test]
    async fn group_absent_is_created() {
        let s = MockServer::start().await;
        base(&s).await;
        mount_get(&s, "/api/v2/thresholding/groups/acme-cpu", 404, None).await;
        Mock::given(method("POST"))
            .and(path("/api/v2/thresholding/groups"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&s)
            .await;
        let out = run(
            &ThresholdGroupHandler,
            GROUP,
            &ApplyParams::default(),
            &ctx(&s),
        )
        .await
        .unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Created);
    }

    #[tokio::test]
    async fn group_matching_live_sends_nothing() {
        let s = MockServer::start().await;
        base(&s).await;
        mount_get(
            &s,
            "/api/v2/thresholding/groups/acme-cpu",
            200,
            Some(live_group("90")),
        )
        .await;
        let out = run(
            &ThresholdGroupHandler,
            GROUP,
            &ApplyParams::default(),
            &ctx(&s),
        )
        .await
        .unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Unchanged);
        assert!(writes(&s).await.is_empty(), "every write reloads threshd");
    }

    #[tokio::test]
    async fn group_change_is_put_with_if_match_and_shown_in_the_diff() {
        let s = MockServer::start().await;
        base(&s).await;
        mount_get(
            &s,
            "/api/v2/thresholding/groups/acme-cpu",
            200,
            Some(live_group("85")),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/thresholding/groups/acme-cpu"))
            .and(header("If-Match", "\"e1\""))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
        let c = ctx(&s);
        let plan = ThresholdGroupHandler
            .plan(&doc(GROUP), &ApplyParams::default(), &c)
            .await
            .unwrap();
        let diff = plan.diff.clone().unwrap();
        assert!(
            diff.contains("~ thresholds[0] high node/cpuLoad: value 85 -> 90"),
            "{diff}"
        );
        let out = ThresholdGroupHandler
            .execute(plan, &ApplyParams::default(), &c)
            .await
            .unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Updated);
    }

    #[tokio::test]
    async fn group_412_is_failed_not_forced() {
        let s = MockServer::start().await;
        base(&s).await;
        mount_get(
            &s,
            "/api/v2/thresholding/groups/acme-cpu",
            200,
            Some(live_group("85")),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/thresholding/groups/acme-cpu"))
            .respond_with(ResponseTemplate::new(412))
            .expect(1)
            .mount(&s)
            .await;
        let out = run(
            &ThresholdGroupHandler,
            GROUP,
            &ApplyParams::default(),
            &ctx(&s),
        )
        .await
        .unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Failed);
        assert!(
            out[0].message.contains("changed on the server since plan"),
            "{:?}",
            out[0]
        );
    }

    #[tokio::test]
    async fn group_plan_gates() {
        let s = MockServer::start().await;
        base(&s).await;
        let mut ro = live_group("90");
        ro["readOnly"] = json!(true);
        mount_get(&s, "/api/v2/thresholding/groups/acme-cpu", 200, Some(ro)).await;
        let c = ctx(&s);
        let p = ApplyParams::default();
        let m = plan_err(ThresholdGroupHandler.plan(&doc(GROUP), &p, &c).await);
        assert!(m.contains("contributed by an extension"), "{m}");
        let m = plan_err(
            ThresholdGroupHandler
                .plan(
                    &doc(&GROUP.replace("dsType: node", "dsType: nodee")),
                    &p,
                    &c,
                )
                .await,
        );
        assert!(m.contains("'nodee' is not a datasource type"), "{m}");
        let dup = format!("{GROUP}---\n{GROUP}");
        let m = plan_err(ThresholdGroupHandler.plan(&doc(&dup), &p, &c).await);
        assert!(m.contains("duplicate ThresholdGroup"), "{m}");
        assert!(writes(&s).await.is_empty());
    }

    #[tokio::test]
    async fn missing_api_is_a_version_error() {
        let s = MockServer::start().await;
        mount_get(&s, "/api/v2/thresholding/metadata", 404, None).await;
        let c = ctx(&s);
        for (h, yaml) in [
            (&ThresholdGroupHandler as &dyn KindHandler, GROUP),
            (&ThreshdPackageHandler, PACKAGE),
        ] {
            let m = plan_err(h.plan(&doc(yaml), &ApplyParams::default(), &c).await);
            assert!(m.contains("Horizon 37.0.0"), "{m}");
        }
    }

    // -- packages --

    #[tokio::test]
    async fn package_update_keeps_live_calendars_and_user_defined() {
        let s = MockServer::start().await;
        base(&s).await;
        mount_get(
            &s,
            "/api/v2/threshd/packages/acme",
            200,
            Some(live_package(60_000)),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/threshd/packages/acme"))
            .and(header("If-Match", "\"e1\""))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
        let out = run(
            &ThreshdPackageHandler,
            PACKAGE,
            &params_with_group(),
            &ctx(&s),
        )
        .await
        .unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Updated, "{:?}", out[0]);
        let body: serde_json::Value = serde_json::from_slice(&writes(&s).await[0].body).unwrap();
        assert_eq!(body["outageCalendars"], json!(["weekly-maint"]));
        assert_eq!(body["services"][0]["userDefined"], json!(true));
        assert_eq!(body["services"][0]["interval"], json!(300_000));
    }

    fn params_with_group() -> ApplyParams {
        ApplyParams {
            manifest: vec![DocRef {
                kind: "ThresholdGroup".into(),
                name: Some("acme-cpu".into()),
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn package_matching_live_sends_nothing() {
        let s = MockServer::start().await;
        base(&s).await;
        mount_get(
            &s,
            "/api/v2/threshd/packages/acme",
            200,
            Some(live_package(300_000)),
        )
        .await;
        let out = run(
            &ThreshdPackageHandler,
            PACKAGE,
            &params_with_group(),
            &ctx(&s),
        )
        .await
        .unwrap();
        assert_eq!(
            out[0].status,
            OutcomeStatus::Unchanged,
            "calendars alone must not differ"
        );
        assert!(writes(&s).await.is_empty());
    }

    #[tokio::test]
    async fn package_group_gate() {
        let s = MockServer::start().await;
        base(&s).await;
        let c = ctx(&s);
        let m = plan_err(
            ThreshdPackageHandler
                .plan(&doc(PACKAGE), &ApplyParams::default(), &c)
                .await,
        );
        assert!(m.contains("'acme-cpu' names no threshold group"), "{m}");
        let other_kind = ApplyParams {
            manifest: vec![DocRef {
                kind: "EventSource".into(),
                name: Some("acme-cpu".into()),
            }],
            ..Default::default()
        };
        assert!(
            ThreshdPackageHandler
                .plan(&doc(PACKAGE), &other_kind, &c)
                .await
                .is_err()
        );
        let live = PACKAGE.replace("acme-cpu", "mib2");
        mount_get(&s, "/api/v2/threshd/packages/acme", 404, None).await;
        assert!(
            ThreshdPackageHandler
                .plan(&doc(&live), &ApplyParams::default(), &c)
                .await
                .is_ok(),
            "live group passes"
        );
    }

    #[tokio::test]
    async fn package_409_on_create_is_failed() {
        let s = MockServer::start().await;
        base(&s).await;
        mount_get(&s, "/api/v2/threshd/packages/acme", 404, None).await;
        Mock::given(method("POST"))
            .and(path("/api/v2/threshd/packages"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&s)
            .await;
        let out = run(
            &ThreshdPackageHandler,
            PACKAGE,
            &params_with_group(),
            &ctx(&s),
        )
        .await
        .unwrap();
        assert_eq!(out[0].status, OutcomeStatus::Failed);
        assert!(
            out[0].message.contains("created on the server since plan"),
            "{:?}",
            out[0]
        );
    }

    // -- through the router --

    fn registry() -> Registry {
        let mut reg = Registry::new();
        reg.register(RANK_THRESHOLD_GROUP, Box::new(ThresholdGroupHandler));
        reg.register(RANK_THRESHD_PACKAGE, Box::new(ThreshdPackageHandler));
        reg
    }

    #[tokio::test]
    async fn router_creates_the_group_before_the_package() {
        let s = MockServer::start().await;
        base(&s).await;
        mount_get(&s, "/api/v2/thresholding/groups/acme-cpu", 404, None).await;
        mount_get(&s, "/api/v2/threshd/packages/acme", 404, None).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&s)
            .await;
        let input = format!("{PACKAGE}---\n{GROUP}");
        let out = apply_documents(&registry(), doc(&input), &ApplyParams::default(), &ctx(&s))
            .await
            .unwrap();
        assert!(
            out.iter().all(|o| o.status == OutcomeStatus::Created),
            "{out:?}"
        );
        let paths: Vec<String> = writes(&s)
            .await
            .iter()
            .map(|r| r.url.path().to_string())
            .collect();
        assert_eq!(
            paths,
            ["/api/v2/thresholding/groups", "/api/v2/threshd/packages"]
        );
    }

    #[tokio::test]
    async fn router_aborts_on_a_gate_before_any_write() {
        let s = MockServer::start().await;
        base(&s).await;
        mount_get(&s, "/api/v2/thresholding/groups/acme-cpu", 404, None).await;
        mount_get(&s, "/api/v2/threshd/packages/acme", 404, None).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&s)
            .await;
        let c = ctx(&s);
        for input in [
            format!(
                "{GROUP}---\n{}",
                PACKAGE.replace(
                    "thresholdingGroup: acme-cpu",
                    "thresholdingGroup: acme-cpuu"
                )
            ),
            format!(
                "{}---\n{PACKAGE}",
                GROUP.replace("dsType: node", "dsType: nodee")
            ),
        ] {
            let err = apply_documents(&registry(), doc(&input), &ApplyParams::default(), &c).await;
            assert!(err.is_err(), "gate must abort");
        }
        assert!(writes(&s).await.is_empty(), "no write for any document");
    }
}
