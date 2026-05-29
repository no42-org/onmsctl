/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Drift check between the committed `schemas/iam-user.schema.json`
//! artifact and the schema regenerated from `UserLocal` (plus the
//! `x-onmsctl-list-kind` / `uniqueItems` post-processing in
//! `examples/gen_schema.rs`) at test time. A failure means the type
//! definitions or the annotation table moved ahead of the committed
//! schema — run `make schema` and commit the result.
//!
//! The annotation table lives in `onmsctl_iam::schema::ANNOTATIONS`
//! and is imported here so the test and the generator cannot diverge.
//! Mirrors `onmsctl-provisioning`'s drift check.

use onmsctl_iam::model::UserLocal;
use onmsctl_iam::schema::ANNOTATIONS;
use serde_json::Value;

#[test]
fn committed_schema_matches_generated() {
    let path = committed_schema_path();
    let committed =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    let schema = schemars::schema_for!(UserLocal);
    let mut json: Value = serde_json::to_value(&schema).expect("schema serializes as JSON");
    inject_annotations(&mut json);

    let mut generated =
        serde_json::to_string_pretty(&json).expect("annotated schema serializes as JSON");
    // `println!` in `gen_schema.rs` appends a trailing newline that the
    // shell-redirected file inherits; mirror that so the comparison is
    // byte-faithful with what `make schema` writes.
    generated.push('\n');

    if committed != generated {
        panic!(
            "schemas/iam-user.schema.json is stale — run `make schema` and commit the result.\n\
             Tip: `diff <(cat schemas/iam-user.schema.json) <(cargo run --quiet --release --example gen_schema -p onmsctl-iam)`"
        );
    }
}

#[test]
fn list_kind_annotations_match_table_exactly() {
    let path = committed_schema_path();
    let committed = std::fs::read_to_string(&path).expect("read committed schema");
    let json: Value = serde_json::from_str(&committed).expect("parse committed schema");

    let defs = json
        .get("$defs")
        .and_then(Value::as_object)
        .expect("committed schema has $defs");

    // 1. Every entry in ANNOTATIONS lands on its expected path with the
    //    expected kind. `set`-kind entries must also carry `uniqueItems: true`.
    for (def_name, field, kind) in ANNOTATIONS {
        let prop = defs
            .get(*def_name)
            .and_then(|d| d.get("properties"))
            .and_then(|p| p.get(*field))
            .unwrap_or_else(|| {
                panic!(
                    "missing $defs/{def_name}/properties/{field} in committed schema; \
                     run `make schema`"
                )
            });
        let actual_kind = prop
            .get("x-onmsctl-list-kind")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "x-onmsctl-list-kind missing on $defs/{def_name}/properties/{field}; \
                     run `make schema`"
                )
            });
        assert_eq!(
            actual_kind, *kind,
            "kind on $defs/{def_name}/properties/{field}"
        );
        if *kind == "set" {
            let unique = prop.get("uniqueItems").and_then(Value::as_bool);
            assert_eq!(
                unique,
                Some(true),
                "set-kind field $defs/{def_name}/properties/{field} must declare uniqueItems: true"
            );
        }
    }

    // 2. No stray `x-onmsctl-list-kind` outside the whitelist. A
    //    hand-edit or future derive that injects the extension elsewhere
    //    would silently land without this guard.
    let mut found: Vec<(String, String)> = Vec::new();
    collect_annotated_paths(&json, &mut found);

    let expected: std::collections::HashSet<(String, String)> = ANNOTATIONS
        .iter()
        .map(|(def, field, _)| ((*def).to_string(), (*field).to_string()))
        .collect();
    let actual: std::collections::HashSet<(String, String)> = found.iter().cloned().collect();

    let unexpected: Vec<_> = actual.difference(&expected).collect();
    assert!(
        unexpected.is_empty(),
        "found x-onmsctl-list-kind on paths not in the ANNOTATIONS table: {unexpected:?}; \
         either add them to the table (and `make schema`) or remove from the schema"
    );
}

fn committed_schema_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::Path::new(manifest_dir)
        .join("..")
        .join("..")
        .join("schemas")
        .join("iam-user.schema.json")
}

fn inject_annotations(schema: &mut Value) {
    let defs = schema
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .expect("schema has $defs");
    for (def_name, field, kind) in ANNOTATIONS {
        let prop = defs
            .get_mut(*def_name)
            .and_then(|d| d.get_mut("properties"))
            .and_then(|p| p.get_mut(*field))
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("expected $defs/{def_name}/properties/{field}"));
        prop.insert("x-onmsctl-list-kind".into(), Value::String((*kind).into()));
        if *kind == "set" {
            prop.insert("uniqueItems".into(), Value::Bool(true));
        }
    }
}

/// Walk the committed schema's `$defs` and collect every path
/// `(definition, property)` that carries an `x-onmsctl-list-kind`
/// annotation. Used to assert no stray annotations exist.
fn collect_annotated_paths(schema: &Value, found: &mut Vec<(String, String)>) {
    let Some(defs) = schema.get("$defs").and_then(Value::as_object) else {
        return;
    };
    for (def_name, def_value) in defs {
        let Some(props) = def_value.get("properties").and_then(Value::as_object) else {
            continue;
        };
        for (prop_name, prop_value) in props {
            if prop_value.get("x-onmsctl-list-kind").is_some() {
                found.push((def_name.clone(), prop_name.clone()));
            }
        }
    }
}
