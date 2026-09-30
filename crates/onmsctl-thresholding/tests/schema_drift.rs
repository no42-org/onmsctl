/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Drift checks between the committed `schemas/threshold-group.schema.json`
//! and `schemas/threshd-package.schema.json` and the schemas generated from
//! the Rust types. A failure means the types moved ahead of the committed
//! files: run `make schema` and commit the result.

use schemars::Schema;

fn check(file: &str, schema: Schema) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .join(file);
    let committed =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    // `println!` in the generators appends the trailing newline the file has.
    let generated = serde_json::to_string_pretty(&schema).expect("schema serializes") + "\n";
    assert!(
        committed == generated,
        "schemas/{file} is stale — run `make schema` and commit the result"
    );
}

#[test]
fn committed_group_schema_matches_generated() {
    check(
        "threshold-group.schema.json",
        schemars::schema_for!(onmsctl_thresholding::model::ThresholdGroupLocal),
    );
}

#[test]
fn committed_package_schema_matches_generated() {
    check(
        "threshd-package.schema.json",
        schemars::schema_for!(onmsctl_thresholding::model::ThreshdPackageLocal),
    );
}
