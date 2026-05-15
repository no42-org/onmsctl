/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Harness smoke test. Proves the integration crate compiles, links
//! to the workspace crates, and can hit the live Horizon instance.
//! The actual EventConf integration cases land in task 5.12.

use onmsctl_eventconf::EventConfApi;
use onmsctl_it::harness_or_skip;

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn smoke_list_source_names() {
    let h = harness_or_skip!();
    h.cleanup_event_sources()
        .await
        .expect("pre-test cleanup must succeed");

    let api = EventConfApi::new(h.client());
    let names = api
        .list_source_names()
        .await
        .expect("list_source_names against live instance");
    eprintln!("smoke: server has {} event sources", names.len());

    h.cleanup_event_sources()
        .await
        .expect("post-test cleanup must succeed");
}
