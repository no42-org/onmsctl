/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Emit the JSON Schema for the `kind: User` YAML document to stdout.
//!
//! Run via `make schema` (which redirects into
//! `schemas/iam-user.schema.json`). The committed schema is the
//! editor-facing artifact; the drift-check test in
//! `tests/schema_drift.rs` fails CI if the type definitions or the
//! annotation table move ahead of the committed file.
//!
//! After the base schema is generated from the `UserLocal` derive, this
//! binary post-processes it to inject the project's `x-onmsctl-list-kind`
//! extension on each list-shaped field per the single-source-of-truth
//! table in [`onmsctl_iam::schema::ANNOTATIONS`]. `set`-kind entries also
//! gain `"uniqueItems": true` so editors catch duplicates at edit-time
//! alongside the parse-time dedup in the model. Mirrors
//! `onmsctl-provisioning`'s generator.

use onmsctl_iam::model::UserLocal;
use onmsctl_iam::schema::ANNOTATIONS;
use serde_json::Value;

fn main() {
    let schema = schemars::schema_for!(UserLocal);
    let mut json: Value = serde_json::to_value(&schema).expect("schema serializes as JSON");

    annotate_list_kinds(&mut json);

    let pretty = serde_json::to_string_pretty(&json).expect("annotated schema serializes as JSON");
    println!("{pretty}");
}

/// Inject `x-onmsctl-list-kind` (and `uniqueItems: true` for `set`-kind
/// entries) on the array fields listed in [`ANNOTATIONS`]. The
/// annotations live on the array-typed field schemas in
/// `$defs/<TypeName>/properties/<field>`.
fn annotate_list_kinds(schema: &mut Value) {
    // schemars 1.x emits definitions under `$defs`.
    let defs = match schema.get_mut("$defs").and_then(Value::as_object_mut) {
        Some(d) => d,
        None => panic!(
            "expected `$defs` in generated schema — schemars layout changed; \
             update gen_schema.rs to match"
        ),
    };

    for (def_name, field, kind) in ANNOTATIONS {
        let prop = defs
            .get_mut(*def_name)
            .and_then(|d| d.get_mut("properties"))
            .and_then(|p| p.get_mut(*field))
            .unwrap_or_else(|| {
                panic!(
                    "expected $defs/{def_name}/properties/{field} in schema — \
                     model layout changed; update gen_schema.rs to match"
                )
            });
        let obj = prop.as_object_mut().unwrap_or_else(|| {
            panic!("$defs/{def_name}/properties/{field} is not an object — cannot annotate")
        });
        obj.insert("x-onmsctl-list-kind".into(), Value::String((*kind).into()));
        if *kind == "set" {
            obj.insert("uniqueItems".into(), Value::Bool(true));
        }
    }
}
