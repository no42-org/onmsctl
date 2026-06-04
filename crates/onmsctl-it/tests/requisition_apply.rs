/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Live-Horizon regression test for `requisition apply`'s create POST
//! (SMOKE-001). Until now the provisioning apply path was only ever exercised
//! by wiremock tests, which accept whatever URL onmsctl sends — so a wrong
//! POST endpoint passed every test yet 405'd against a real Horizon. This
//! drives `ProvisioningApi::post_requisition` against the live server, where
//! the bare-collection `POST /rest/requisitions` is the only accepted shape.
//!
//! `#[ignore]`d like the rest of the IT suite; run via `make integration`.

use onmsctl_it::{Harness, harness_or_skip};
use onmsctl_provisioning::api::ProvisioningApi;
use onmsctl_provisioning::model::server::RequisitionServer;

async fn pre_post_cleanup(h: &Harness, when: &str) {
    let n = h
        .cleanup_requisitions()
        .await
        .unwrap_or_else(|e| panic!("{when} requisition cleanup failed: {e}"));
    if n > 0 {
        eprintln!("{when} cleanup: deleted {n} leftover requisition(s)");
    }
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn post_requisition_succeeds_against_live_horizon() {
    let h = harness_or_skip!();
    pre_post_cleanup(&h, "pre").await;
    let api = ProvisioningApi::new(h.client());

    let name = h.unique_name("req");
    let req = RequisitionServer {
        foreign_source: name.clone(),
        date_stamp: None,
        last_import: None,
        node: vec![],
    };

    // The create POST. Before SMOKE-001 this 405'd (wrong endpoint
    // `/rest/requisitions/{fs}`); it now targets the bare collection and the
    // empty 202 body is drained rather than decoded.
    api.post_requisition(&req)
        .await
        .expect("post_requisition must succeed against a live Horizon");

    // The requisition is really on the server (pending state).
    let got = api
        .get_requisition(&name)
        .await
        .expect("get_requisition")
        .expect("created requisition must be present");
    assert_eq!(got.foreign_source, name);
    assert!(
        api.list_requisition_names()
            .await
            .expect("list_requisition_names")
            .iter()
            .any(|n| n == &name),
        "created requisition '{name}' must appear in the list"
    );

    pre_post_cleanup(&h, "post").await;
}
