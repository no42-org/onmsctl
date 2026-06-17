/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Emit the JSON Schema for the `kind: BusinessService` YAML document to stdout.
//!
//! Run via `make schema` (which redirects into
//! `schemas/business-service.schema.json`). The committed schema is the
//! editor-facing artifact; the drift-check test fails CI if the model moves
//! ahead of it.

use onmsctl_businessservice::model::BusinessServiceLocal;

fn main() {
    let schema = schemars::schema_for!(BusinessServiceLocal);
    let pretty = serde_json::to_string_pretty(&schema).expect("schema serializes as JSON");
    println!("{pretty}");
}
