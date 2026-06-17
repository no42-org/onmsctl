/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Live-Horizon lifecycle for `kind: BusinessService`: create → reload →
//! status read → delete, with cleanup. `#[ignore]`d so `make test` is
//! unaffected; run via `make integration` (which passes `--include-ignored`)
//! against a 33.x and a develop instance.

use onmsctl_businessservice::api::BusinessServiceApi;
use onmsctl_businessservice::server::{
    BusinessServiceRequest, FunctionDto, ReductionKeyEdgeRequest,
};
use onmsctl_it::harness_or_skip;

#[tokio::test]
#[ignore = "live Horizon required (run via `make integration`)"]
async fn business_service_create_reload_get_delete() {
    let h = harness_or_skip!();
    let api = BusinessServiceApi::new(h.client());

    // Clear any leftover state from a crashed prior run.
    h.cleanup_business_services()
        .await
        .expect("pre-test cleanup");

    let name = h.unique_name("bs");
    let req = BusinessServiceRequest {
        name: name.clone(),
        reduce_function: Some(FunctionDto {
            type_: "HighestSeverity".into(),
            properties: Default::default(),
        }),
        reduction_key_edges: vec![ReductionKeyEdgeRequest {
            reduction_key: format!("uei.opennms.org/onmsctl-it/{name}"),
            weight: 1,
            friendly_name: None,
            map_function: None,
        }],
        ..Default::default()
    };

    // -- create + reload --
    api.create(&req).await.expect("create business service");
    api.reload().await.expect("bsmd reload after create");

    // -- status read: the service is present and named as requested --
    let all = api.fetch_all().await.expect("list business services");
    let (id, found) = all
        .into_iter()
        .find(|(_, r)| r.name == name)
        .expect("created service present in list");
    assert_eq!(found.name, name);

    // -- delete + reload --
    api.delete(id).await.expect("delete business service");
    api.reload().await.expect("bsmd reload after delete");

    // -- gone --
    let after = api.fetch_all().await.expect("list after delete");
    assert!(
        after.iter().all(|(_, r)| r.name != name),
        "service should be deleted"
    );

    // Teardown sweep (best-effort).
    let _ = h.cleanup_business_services().await;
}
