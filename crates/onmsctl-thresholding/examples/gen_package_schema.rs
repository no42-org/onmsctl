/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Emit the JSON Schema for the `ThreshdPackage` YAML document to stdout.
//!
//! Run via `make schema` (which redirects into
//! `schemas/threshd-package.schema.json`). The drift-check test in
//! `tests/schema_drift.rs` fails CI if the types move ahead of the
//! committed file.

use onmsctl_thresholding::model::ThreshdPackageLocal;

fn main() {
    let schema = schemars::schema_for!(ThreshdPackageLocal);
    let pretty = serde_json::to_string_pretty(&schema).expect("schema serializes as JSON");
    println!("{pretty}");
}
