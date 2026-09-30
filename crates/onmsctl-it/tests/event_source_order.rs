/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Live-Horizon integration test for `kind: EventSourceOrder`.
//!
//! Snapshots the server's source order, applies two throwaway sources plus an
//! order document that lists them, and checks the positions, the no-op
//! re-apply and a reversed list. The throwaway sources are deleted and the
//! snapshot order is restored even when an assertion fails.
//!
//! `#[ignore]`d like the rest of the IT suite; run via `make integration`.
//! Needs a Horizon build that reorders sources through an `eventconf.xml`
//! upload part.

use onmsctl_core::kind::parse_documents;
use onmsctl_core::kind::precedence::{RANK_EVENT_SOURCE, RANK_EVENT_SOURCE_ORDER};
use onmsctl_core::{ApplyOutcome, ApplyParams, OutcomeStatus, Registry, apply_documents};

use onmsctl_eventconf::EventConfApi;
use onmsctl_eventconf::apply::EventSourceHandler;
use onmsctl_eventconf::order::{EventSourceOrderHandler, KIND};

use onmsctl_it::{Harness, harness_or_skip};

fn test_registry() -> Registry {
    let mut reg = Registry::new();
    reg.register(RANK_EVENT_SOURCE, Box::new(EventSourceHandler));
    reg.register(RANK_EVENT_SOURCE_ORDER, Box::new(EventSourceOrderHandler));
    reg
}

fn source_doc(name: &str) -> String {
    format!(
        "apiVersion: eventconf.opennms.org/v1\nkind: EventSource\nmetadata:\n  name: {name}\n\
         spec:\n  events:\n    - uei: uei.opennms.org/it/{name}/0\n      label: First\n      severity: Warning\n"
    )
}

fn order_doc(first: &[&str]) -> String {
    format!(
        "apiVersion: eventconf.opennms.org/v1\nkind: EventSourceOrder\nmetadata:\n  name: default\n\
         spec:\n  first: [{}]\n",
        first.join(", ")
    )
}

/// Apply a multi-document YAML stream and return the order document's outcome.
async fn apply(h: &Harness, yaml: &str) -> Result<ApplyOutcome, String> {
    let docs = parse_documents("it.yaml", yaml).map_err(|e| e.to_string())?;
    let outcomes = apply_documents(
        &test_registry(),
        docs,
        &ApplyParams::default(),
        &h.context(false),
    )
    .await
    .map_err(|e| format!("apply: {e}"))?;
    if let Some(bad) = outcomes.iter().find(|o| o.status.is_failure()) {
        return Err(format!("document failed: {bad:?}"));
    }
    outcomes
        .into_iter()
        .find(|o| o.kind == KIND)
        .ok_or_else(|| "no EventSourceOrder outcome".into())
}

async fn live_order(h: &Harness) -> Result<Vec<String>, String> {
    EventConfApi::new(h.client())
        .list_source_order()
        .await
        .map_err(|e| format!("list_source_order: {e}"))
}

fn expect(cond: bool, msg: impl Into<String>) -> Result<(), String> {
    if cond { Ok(()) } else { Err(msg.into()) }
}

async fn scenario(h: &Harness, a: &str, b: &str) -> Result<(), String> {
    // Create both sources and place them first, in one apply.
    let yaml = format!(
        "{}---\n{}---\n{}",
        source_doc(a),
        source_doc(b),
        order_doc(&[a, b])
    );
    let out = apply(h, &yaml).await?;
    expect(
        out.status == OutcomeStatus::Updated || out.status == OutcomeStatus::Unchanged,
        format!("first apply: {out:?}"),
    )?;
    let order = live_order(h).await?;
    expect(
        order.first().map(String::as_str) == Some(a) && order.get(1).map(String::as_str) == Some(b),
        format!(
            "expected [{a}, {b}] at positions 1 and 2, got {:?}",
            &order[..order.len().min(4)]
        ),
    )?;

    // Re-apply: nothing to do, no upload.
    let out = apply(h, &order_doc(&[a, b])).await?;
    expect(
        out.status == OutcomeStatus::Unchanged,
        format!("re-apply must be unchanged: {out:?}"),
    )?;
    expect(live_order(h).await? == order, "re-apply changed the order")?;

    // Reverse the list: the two swap, everything else keeps its place.
    let out = apply(h, &order_doc(&[b, a])).await?;
    expect(
        out.status == OutcomeStatus::Updated,
        format!("reverse: {out:?}"),
    )?;
    let reversed = live_order(h).await?;
    let mut want = order.clone();
    want.swap(0, 1);
    expect(
        reversed == want,
        format!("reverse must swap only the two listed sources, got {reversed:?}"),
    )
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn event_source_order_places_swaps_and_is_idempotent() {
    let h = harness_or_skip!();
    let _ = h.cleanup_event_sources().await;
    let snapshot = live_order(&h).await.expect("snapshot order");

    let a = h.unique_name("order-a");
    let b = h.unique_name("order-b");
    let result = scenario(&h, &a, &b).await;

    // Teardown runs before any assertion so a failure leaves the server as found.
    let _ = h.cleanup_event_sources().await;
    let api = EventConfApi::new(h.client());
    if live_order(&h).await.ok().as_ref() != Some(&snapshot) {
        api.upload_source_order(&snapshot)
            .await
            .expect("restore snapshot order");
    }
    assert_eq!(
        live_order(&h).await.expect("read order"),
        snapshot,
        "snapshot order restored"
    );

    result.unwrap();
}
