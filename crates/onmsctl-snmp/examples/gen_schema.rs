/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Emit the JSON Schema for the `kind: SnmpConfig` YAML document to stdout.
//!
//! Run via `make schema` (which redirects into
//! `schemas/snmp-config.schema.json`). The committed schema is the
//! editor-facing artifact; the drift-check test in `tests/schema_drift.rs`
//! fails CI if the model moves ahead of the committed file.
//!
//! Unlike the provisioning / iam generators, `SnmpConfig` is reconciled by
//! whole-config replace — there are no per-list merge keys, so there is no
//! `x-onmsctl-list-kind` annotation table to inject. The schema is emitted
//! straight from the `SnmpConfigLocal` derive.

use onmsctl_snmp::model::SnmpConfigLocal;

fn main() {
    let schema = schemars::schema_for!(SnmpConfigLocal);
    let pretty = serde_json::to_string_pretty(&schema).expect("schema serializes as JSON");
    println!("{pretty}");
}
