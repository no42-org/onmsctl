/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Contract tests for the `/eventconf/filter/*` endpoints against a live
//! Horizon instance. Pins three load-bearing wire-format expectations the
//! unit suite cannot verify on its own:
//!
//! 1. `limit` is required by all three filter endpoints.
//! 2. `offset` is required by `/filter/{sourceId}/events` specifically
//!    (server NPEs with 500 if omitted).
//! 3. The items array deserializes under whichever wrapper field name
//!    Horizon currently emits (`eventConfSourceList` today, possibly
//!    renamed later — `Page<T>` carries an alias for both shapes).
//!
//! If Horizon's controller changes its required-params posture or wrapper
//! field name, one of these tests fails and the rename is caught here
//! before users hit it in `event-source list` / `apply`.

use onmsctl_eventconf::{EventConfApi, EventFilter, EventInSourceFilter, SourceFilter};
use onmsctl_it::harness_or_skip;

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn filter_sources_with_defaults_succeeds() {
    let h = harness_or_skip!();
    let api = EventConfApi::new(h.client());
    let page = api
        .filter_sources(&SourceFilter::default())
        .await
        .expect("filter_sources must succeed with API-defaulted limit");
    eprintln!(
        "filter_sources: totalRecords={} items={}",
        page.total_records,
        page.items.len()
    );
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn list_events_in_source_with_defaults_succeeds() {
    let h = harness_or_skip!();
    let api = EventConfApi::new(h.client());

    // Pick any real source id — the listing endpoint is unaffected by the
    // /filter/sources empty-list quirk.
    let names_and_ids = api
        .list_source_names_and_ids()
        .await
        .expect("list_source_names_and_ids must succeed");
    let Some(any) = names_and_ids.first() else {
        eprintln!("SKIP: no event sources on server to probe");
        return;
    };

    let page = api
        .list_events_in_source(any.id, &EventInSourceFilter::default())
        .await
        .unwrap_or_else(|e| {
            panic!(
                "list_events_in_source({}) must succeed with API-defaulted offset/limit: {e}",
                any.id
            )
        });
    eprintln!(
        "list_events_in_source({}): totalRecords={} items={}",
        any.id,
        page.total_records,
        page.items.len()
    );
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn filter_events_with_defaults_succeeds() {
    let h = harness_or_skip!();
    let api = EventConfApi::new(h.client());
    let events = api
        .filter_events(&EventFilter::default())
        .await
        .expect("filter_events must succeed with API-defaulted limit");
    eprintln!("filter_events: events={}", events.len());
}
