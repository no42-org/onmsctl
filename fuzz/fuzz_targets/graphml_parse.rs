/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! GraphML parser for `kind: Graph`. It reads operator files (`graph
//! convert -f`) and whatever a server stores under `rest/graphml/{name}`
//! (plan, `graph get`, `graph export`).
//!
//! Beyond "no panic, no hang", anything the parser accepts must survive
//! the writer: `parse(render(spec)) == spec`, with the rendered container
//! id and no warnings. Plan relies on this. A stored upload that onmsctl
//! wrote has to compare equal to the document it came from, or every
//! apply would plan a spurious update.
//!
//! XML normalizes `\r` in text and `\t`/`\n` in attribute values, so a spec
//! holding those characters cannot round-trip. Such inputs are skipped.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onmsctl_graph::graphml::{parse, render};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(parsed) = parse(text) else {
        return;
    };
    let json = serde_json::to_string(&parsed.spec).expect("the model serializes");
    if json.contains("\\r") || json.contains("\\t") || json.contains("\\n") {
        return;
    }
    let again = render("fuzz", &parsed.spec);
    let back = parse(&again).expect("rendered GraphML parses");
    assert_eq!(
        back.spec, parsed.spec,
        "round trip preserves the spec:\n{again}"
    );
    assert_eq!(back.container_id.as_deref(), Some("fuzz"));
    assert!(back.warnings.is_empty(), "{:?}\n{again}", back.warnings);
});
