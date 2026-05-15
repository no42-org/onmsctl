/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end integration tests for `onmsctl source apply` against a live
//! Horizon 36 instance. Each test is `#[ignore]`d so `make test` is
//! unaffected; `make integration` opts in via `--include-ignored`.
//!
//! Covers task 5.12: create-from-empty, replace-existing,
//! unchanged-detection, dry-run-with-diff, disabled-state apply.
//!
//! ## Note on the ambiguous-name scenario
//!
//! The 6th case from the task spec — `find_source_by_name` returning
//! `Ambiguous` — cannot be reproduced reliably from the outside: the
//! upload endpoint upserts on basename collision, so the API will not
//! create two sources sharing an exact name. The state is reachable only
//! via direct DB manipulation. Coverage lives in the wiremock-driven
//! unit test `find_source_by_name_returns_ambiguous_for_duplicate_names`
//! in `crates/onmsctl-eventconf/src/api.rs`.

use onmsctl_core::{ApplyOptions, Outcome, run_apply};
use onmsctl_eventconf::EventConfApi;
use onmsctl_eventconf::apply::{
    EventDef, EventSourceLocal, EventSourceSpec, EventSourceTarget, Metadata,
};
use onmsctl_it::{Harness, harness_or_skip};

const SOURCE_NAME_PREFIX: &str = "onmsctl.it"; // vendor-prefix part of the dotted name

/// Build an `EventSourceLocal` with the given name and a UEI per
/// `(severity, label_suffix)` tuple. Validated before return so we
/// fail fast on programming errors in the fixtures.
fn make_local(name: &str, events: &[(&str, &str)], enabled: bool) -> EventSourceLocal {
    let events: Vec<EventDef> = events
        .iter()
        .enumerate()
        .map(|(i, (severity, label))| EventDef {
            uei: format!("uei.opennms.org/it/{name}/{i}"),
            label: (*label).to_string(),
            severity: (*severity).to_string(),
            ..EventDef::default()
        })
        .collect();
    let local = EventSourceLocal {
        api_version: "eventconf.opennms.org/v1".into(),
        kind: "EventSource".into(),
        metadata: Metadata { name: name.into() },
        spec: EventSourceSpec { enabled, events },
    };
    local
        .validate()
        .expect("test fixture must validate against the local schema");
    local
}

/// Generate a server-side source name that is unique to this test and
/// also a legal eventconf source name (vendor-prefixed dotted form).
fn unique_source_name(h: &Harness, slug: &str) -> String {
    // `Harness::unique_name` returns `onmsctl-it-<slug>-<pid>-<ns>-<n>`,
    // which carries the required cleanup prefix but uses `-` as the
    // separator. EventConf source names must split on `.` to derive a
    // vendor — so we rebuild the form, keeping the `onmsctl-it-` prefix
    // intact so the cleanup sweep matches.
    let raw = h.unique_name(slug);
    format!("{raw}.{SOURCE_NAME_PREFIX}")
}

async fn pre_post_cleanup(h: &Harness, when: &str) {
    let n = h
        .cleanup_event_sources()
        .await
        .unwrap_or_else(|e| panic!("{when} cleanup failed: {e}"));
    if n > 0 {
        eprintln!("{when} cleanup: deleted {n} leftover source(s)");
    }
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn apply_creates_source_when_absent() {
    let h = harness_or_skip!();
    pre_post_cleanup(&h, "pre").await;

    let name = unique_source_name(&h, "create");
    let local = make_local(
        &name,
        &[("Warning", "First event"), ("Major", "Second event")],
        true,
    );
    let ctx = h.context(false);
    let opts = ApplyOptions::default();

    let outcome = run_apply::<EventSourceTarget>(local, &opts, &ctx)
        .await
        .expect("apply create");
    assert_eq!(outcome, Outcome::Created, "first apply must Create");

    // Verify the source is actually on the server with the right shape.
    let api = EventConfApi::new(h.client());
    let names = api
        .list_source_names()
        .await
        .expect("list_source_names after create");
    assert!(
        names.iter().any(|n| n == &name),
        "expected '{name}' in server names: {names:?}"
    );

    pre_post_cleanup(&h, "post").await;
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn apply_updates_when_events_change() {
    let h = harness_or_skip!();
    pre_post_cleanup(&h, "pre").await;

    let name = unique_source_name(&h, "update");
    let ctx = h.context(false);
    let opts = ApplyOptions::default();

    // First apply — Created.
    let first = make_local(&name, &[("Warning", "alpha"), ("Major", "beta")], true);
    assert_eq!(
        run_apply::<EventSourceTarget>(first, &opts, &ctx)
            .await
            .expect("apply create"),
        Outcome::Created
    );

    // Second apply, different event set — Updated.
    let second = make_local(&name, &[("Warning", "alpha"), ("Minor", "gamma")], true);
    assert_eq!(
        run_apply::<EventSourceTarget>(second, &opts, &ctx)
            .await
            .expect("apply update"),
        Outcome::Updated
    );

    pre_post_cleanup(&h, "post").await;
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn apply_is_unchanged_when_state_matches() {
    let h = harness_or_skip!();
    pre_post_cleanup(&h, "pre").await;

    let name = unique_source_name(&h, "unchanged");
    let ctx = h.context(false);
    let opts = ApplyOptions::default();

    let local = make_local(&name, &[("Warning", "stable")], true);

    assert_eq!(
        run_apply::<EventSourceTarget>(local.clone(), &opts, &ctx)
            .await
            .expect("apply create"),
        Outcome::Created
    );

    // Re-apply the exact same shape — must Unchanged.
    assert_eq!(
        run_apply::<EventSourceTarget>(local, &opts, &ctx)
            .await
            .expect("apply identical"),
        Outcome::Unchanged
    );

    pre_post_cleanup(&h, "post").await;
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn apply_dry_run_does_not_mutate_and_reports_would_update() {
    let h = harness_or_skip!();
    pre_post_cleanup(&h, "pre").await;

    let name = unique_source_name(&h, "dryrun");
    let ctx = h.context(false);

    // Seed: real create.
    let seed = make_local(&name, &[("Warning", "seed")], true);
    assert_eq!(
        run_apply::<EventSourceTarget>(seed, &ApplyOptions::default(), &ctx)
            .await
            .expect("apply create"),
        Outcome::Created
    );

    // Different shape, dry-run + diff: must report WouldUpdate AND not
    // mutate the server. The diff print to stderr is incidental — the
    // observable side-effect (server state) is what we test.
    let proposed = make_local(&name, &[("Warning", "seed"), ("Minor", "extra")], true);
    let opts = ApplyOptions {
        dry_run: true,
        show_diff: true,
    };
    assert_eq!(
        run_apply::<EventSourceTarget>(proposed, &opts, &ctx)
            .await
            .expect("apply dry-run"),
        Outcome::WouldUpdate
    );

    // Confirm the server is still at the seed shape — one event.
    let api = EventConfApi::new(h.client());
    let pairs = api
        .list_source_names_and_ids()
        .await
        .expect("list_source_names_and_ids");
    let src = pairs
        .into_iter()
        .find(|p| p.name == name)
        .expect("seeded source must still exist");
    let full = api
        .get_source(src.id)
        .await
        .expect("get_source after dry-run");
    assert_eq!(
        full.event_count, 1,
        "dry-run must not have mutated the server's event count"
    );

    pre_post_cleanup(&h, "post").await;
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn apply_disabled_state_results_in_disabled_source() {
    let h = harness_or_skip!();
    pre_post_cleanup(&h, "pre").await;

    let name = unique_source_name(&h, "disabled");
    // verbose=true so the post-update flap warning fires (observed only
    // as a side-effect on stderr; assertion is on the resulting state).
    let ctx = h.context(true);

    // Seed enabled.
    let enabled = make_local(&name, &[("Warning", "seed")], true);
    assert_eq!(
        run_apply::<EventSourceTarget>(enabled, &ApplyOptions::default(), &ctx)
            .await
            .expect("apply create enabled"),
        Outcome::Created
    );

    // Apply same shape but disabled — Updated.
    let disabled = make_local(&name, &[("Warning", "seed")], false);
    assert_eq!(
        run_apply::<EventSourceTarget>(disabled, &ApplyOptions::default(), &ctx)
            .await
            .expect("apply disabled update"),
        Outcome::Updated
    );

    // Final state: source exists and is disabled.
    let api = EventConfApi::new(h.client());
    let pairs = api
        .list_source_names_and_ids()
        .await
        .expect("list_source_names_and_ids");
    let src = pairs
        .into_iter()
        .find(|p| p.name == name)
        .expect("disabled source must exist");
    let full = api
        .get_source(src.id)
        .await
        .expect("get_source after disable");
    assert!(
        !full.enabled,
        "expected source '{name}' to be disabled, got enabled={}",
        full.enabled
    );

    pre_post_cleanup(&h, "post").await;
}
