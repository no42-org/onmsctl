/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Drift check between the committed `schemas/datacollection.schema.json`
//! artifact and the schema regenerated from `DataCollectionSourceLocal` at test
//! time. A failure means the model moved ahead of the committed schema — run
//! `make schema` and commit the result.

use onmsctl_datacollection::model::DataCollectionSourceLocal;

#[test]
fn committed_schema_matches_generated() {
    let path = committed_schema_path();
    let committed =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    let schema = schemars::schema_for!(DataCollectionSourceLocal);
    let mut generated = serde_json::to_string_pretty(&schema).expect("schema serializes as JSON");
    // `println!` in `gen_schema.rs` appends a trailing newline that the
    // shell-redirected file inherits; mirror it for a byte-faithful compare.
    generated.push('\n');

    if committed != generated {
        panic!(
            "schemas/datacollection.schema.json is stale — run `make schema` and commit the result.\n\
             Tip: `diff <(cat schemas/datacollection.schema.json) <(cargo run --quiet --release --example gen_schema -p onmsctl-datacollection)`"
        );
    }
}

fn committed_schema_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::Path::new(manifest_dir)
        .join("..")
        .join("..")
        .join("schemas")
        .join("datacollection.schema.json")
}
