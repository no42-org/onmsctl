/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Drift check between the committed `schemas/event-source.schema.json`
//! artifact and the schema regenerated from `EventSourceLocal` at test
//! time. A failure means the type definitions moved ahead of the
//! committed schema — run `make schema` and commit the result.

use onmsctl_eventconf::apply::local::EventSourceLocal;

#[test]
fn committed_schema_matches_generated() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest_dir)
        .join("..")
        .join("..")
        .join("schemas")
        .join("event-source.schema.json");

    let committed =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    let schema = schemars::schema_for!(EventSourceLocal);
    let mut generated = serde_json::to_string_pretty(&schema).expect("schema serializes as JSON");
    // `println!` in `gen_schema.rs` appends a trailing newline that the
    // shell-redirected file inherits; mirror that so the comparison is
    // byte-faithful with what `make schema` writes.
    generated.push('\n');

    if committed != generated {
        panic!(
            "schemas/event-source.schema.json is stale — run `make schema` and commit the result.\n\
             Tip: `diff <(cat schemas/event-source.schema.json) <(cargo run --example gen_schema -p onmsctl-eventconf)`"
        );
    }
}
