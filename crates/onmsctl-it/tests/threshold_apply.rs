/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Live-Horizon integration tests for `kind: ThresholdGroup` and
//! `kind: ThreshdPackage`.
//!
//! Needs a Horizon build with the NMS-19837 threshold API. The tests skip
//! themselves when `/api/v2/thresholding/metadata` is absent, so
//! `make integration` still passes against older servers. `#[ignore]`d like
//! the rest of the IT suite; run via `make integration`.

use onmsctl_core::kind::parse_documents;
use onmsctl_core::kind::precedence::{RANK_THRESHD_PACKAGE, RANK_THRESHOLD_GROUP};
use onmsctl_core::{
    ApplyOutcome, ApplyParams, OnmsClient, OutcomeStatus, Registry, apply_documents,
};

use onmsctl_thresholding::api::{ThreshdApi, ThresholdingApi};
use onmsctl_thresholding::apply::{ThreshdPackageHandler, ThresholdGroupHandler};
use onmsctl_thresholding::convert::{group_from_wire, package_from_wire};

use onmsctl_it::Harness;

fn registry() -> Registry {
    let mut reg = Registry::new();
    reg.register(RANK_THRESHOLD_GROUP, Box::new(ThresholdGroupHandler));
    reg.register(RANK_THRESHD_PACKAGE, Box::new(ThreshdPackageHandler));
    reg
}

fn group_doc(name: &str, value: u32) -> String {
    format!(
        "apiVersion: thresholding.opennms.org/v1\nkind: ThresholdGroup\nmetadata:\n  name: {name}\n\
         spec:\n  rrdRepository: /opt/opennms/share/rrd/snmp/\n  thresholds:\n    - type: high\n      \
         dsType: node\n      dsName: itCpu\n      value: {value}\n      rearm: 80\n      trigger: 3\n"
    )
}

fn package_doc(name: &str, group: &str, filter: &str) -> String {
    format!(
        "apiVersion: thresholding.opennms.org/v1\nkind: ThreshdPackage\nmetadata:\n  name: {name}\n\
         spec:\n  filter: \"{filter}\"\n  includeRanges:\n    - begin: 192.0.2.1\n      end: 192.0.2.254\n  \
         services:\n    - name: SNMP\n      interval: 5m\n      status: \"on\"\n      thresholdingGroup: {group}\n"
    )
}

async fn apply(h: &Harness, yaml: &str, dry_run: bool) -> Vec<ApplyOutcome> {
    let params = ApplyParams {
        dry_run,
        ..Default::default()
    };
    apply_documents(
        &registry(),
        parse_documents("it.yaml", yaml).unwrap(),
        &params,
        &h.context(false),
    )
    .await
    .expect("apply must pass the plan gate")
}

fn statuses(out: &[ApplyOutcome]) -> Vec<OutcomeStatus> {
    out.iter().map(|o| o.status).collect()
}

/// Highest `reloadDaemonConfig` event id, to detect writes by their reloads.
async fn last_reload_event(client: &OnmsClient) -> i64 {
    let v: serde_json::Value = client
        .get(
            "rest/events",
            &[
                ("eventUei", "uei.opennms.org/internal/reloadDaemonConfig"),
                ("orderBy", "id"),
                ("order", "desc"),
                ("limit", "1"),
            ],
        )
        .await
        .unwrap_or(serde_json::Value::Null);
    v["event"][0]["id"].as_i64().unwrap_or(0)
}

/// Skip unless the server has the threshold API; clean up leftovers.
async fn ready() -> Option<Harness> {
    let h = match onmsctl_it::Harness::from_env() {
        Ok(onmsctl_it::Setup::Ready(h)) => h,
        Ok(onmsctl_it::Setup::Skipped(reason)) => {
            eprintln!("SKIP: {reason}");
            return None;
        }
        Err(e) => panic!("integration harness setup failed: {e:#}"),
    };
    if !h.thresholds_supported().await {
        eprintln!("SKIP: server has no threshold API (NMS-19837)");
        return None;
    }
    h.cleanup_thresholds().await.expect("pre-cleanup");
    Some(h)
}

#[ignore = "live Horizon with NMS-19837 required (run via `make integration`)"]
#[tokio::test]
async fn group_and_package_create_then_reapply_is_a_no_op() {
    let Some(h) = ready().await else { return };
    let (g, p) = (h.unique_name("grp"), h.unique_name("pkg"));
    let yaml = format!(
        "{}---\n{}",
        package_doc(&p, &g, "IPADDR != '0.0.0.0'"),
        group_doc(&g, 90)
    );

    let out = apply(&h, &yaml, false).await;
    assert_eq!(
        statuses(&out),
        [OutcomeStatus::Created, OutcomeStatus::Created],
        "{out:?}"
    );
    assert!(
        ThresholdingApi::new(h.client())
            .get_group(&g)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        ThreshdApi::new(h.client())
            .get_package(&p)
            .await
            .unwrap()
            .is_some()
    );

    let before = last_reload_event(h.client()).await;
    let out = apply(&h, &yaml, false).await;
    assert_eq!(
        statuses(&out),
        [OutcomeStatus::Unchanged, OutcomeStatus::Unchanged],
        "{out:?}"
    );
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(
        last_reload_event(h.client()).await,
        before,
        "a no-op apply must not reload threshd"
    );

    let out = apply(&h, &group_doc(&g, 95), false).await;
    assert_eq!(statuses(&out), [OutcomeStatus::Updated], "{out:?}");

    h.cleanup_thresholds().await.expect("post-cleanup");
}

#[ignore = "live Horizon with NMS-19837 required (run via `make integration`)"]
#[tokio::test]
async fn package_update_keeps_an_attached_outage() {
    let Some(h) = ready().await else { return };
    let (g, p, outage) = (
        h.unique_name("grp"),
        h.unique_name("pkg"),
        h.unique_name("outage"),
    );
    let base = format!(
        "{}---\n{}",
        group_doc(&g, 90),
        package_doc(&p, &g, "IPADDR != '0.0.0.0'")
    );
    apply(&h, &base, false).await;

    let c = h.client();
    let body = serde_json::json!({"name": outage, "type": "weekly", "node": [], "interface": [],
        "time": [{"day": "monday", "begins": "00:00:00", "ends": "01:00:00"}]});
    c.post_drain("rest/sched-outages", &body)
        .await
        .expect("create outage");
    c.put_empty(&format!("rest/sched-outages/{outage}/threshd/{p}"))
        .await
        .expect("attach outage");

    let changed = format!(
        "{}---\n{}",
        group_doc(&g, 90),
        package_doc(&p, &g, "IPADDR != '0.0.0.1'")
    );
    let out = apply(&h, &changed, false).await;
    let calendars = ThreshdApi::new(c)
        .get_package(&p)
        .await
        .unwrap()
        .unwrap()
        .0
        .outage_calendars;

    let _ = c
        .delete::<()>(&format!("rest/sched-outages/{outage}/threshd/{p}"), None)
        .await;
    let _ = c
        .delete::<()>(&format!("rest/sched-outages/{outage}"), None)
        .await;
    h.cleanup_thresholds().await.expect("post-cleanup");

    assert_eq!(
        statuses(&out),
        [OutcomeStatus::Unchanged, OutcomeStatus::Updated],
        "{out:?}"
    );
    assert_eq!(
        calendars,
        [outage],
        "apply must not detach the Maintenance outage"
    );
}

#[ignore = "live Horizon with NMS-19837 required (run via `make integration`)"]
#[tokio::test]
async fn export_of_every_object_applies_back_unchanged() {
    let Some(h) = ready().await else { return };
    let (groups, packages) = (
        ThresholdingApi::new(h.client()),
        ThreshdApi::new(h.client()),
    );
    let mut docs = Vec::new();
    for g in groups
        .list_groups()
        .await
        .unwrap()
        .into_iter()
        .filter(|g| !g.read_only)
    {
        let (dto, _) = groups.get_group(&g.name).await.unwrap().unwrap();
        docs.push(serde_norway::to_string(&group_from_wire(&dto)).unwrap());
    }
    for p in packages.list_packages().await.unwrap() {
        let (dto, _) = packages.get_package(&p.name).await.unwrap().unwrap();
        docs.push(serde_norway::to_string(&package_from_wire(&dto).unwrap()).unwrap());
    }
    assert!(!docs.is_empty(), "a stock install has groups and packages");

    let out = apply(&h, &docs.join("---\n"), true).await;
    let changed: Vec<_> = out
        .iter()
        .filter(|o| o.status != OutcomeStatus::Unchanged)
        .collect();
    assert!(
        changed.is_empty(),
        "exported objects must apply back unchanged: {changed:?}"
    );
}
