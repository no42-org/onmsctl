/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Emit the JSON Schema for the `EventSourceOrder` YAML document to stdout.
//!
//! Run via `make schema` (which redirects into
//! `schemas/event-source-order.schema.json`). The drift-check test in
//! `tests/schema_drift.rs` fails CI if the type definitions move ahead of
//! the committed file.

use onmsctl_eventconf::order::EventSourceOrderLocal;

fn main() {
    let schema = schemars::schema_for!(EventSourceOrderLocal);
    let pretty = serde_json::to_string_pretty(&schema).expect("schema serializes as JSON");
    println!("{pretty}");
}
