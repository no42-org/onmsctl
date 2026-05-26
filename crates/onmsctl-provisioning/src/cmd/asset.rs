/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl requisition asset` — imperative verbs for the imported-
//! node asset record sub-resource (Group 7 phase 5, task 7.5).
//!
//! **This sub-resource is the misfit of the family.** Per design
//! §D8, every other sub-resource (`node`, `interface`, `service`,
//! `category`) operates on REQUISITION-TIME entries keyed by
//! `<foreign-source> + <foreign-id>` under `/rest/requisitions/...`.
//! `asset` is different: it operates on POST-IMPORT nodes keyed by
//! database node ID (integer), under `/rest/nodes/{db-id}/assetRecord`.
//!
//! Operational consequence: an asset field set via this verb takes
//! effect IMMEDIATELY on the imported inventory — there is no
//! `requisition import` follow-up, no pending state to apply.
//! Requisition-time asset values still flow through `apply -f` via
//! `spec.nodes[].assets`; this verb is the *only* path to mutate
//! the post-import database record.
//!
//! Verb coverage per §D8: `list / get / set` only. Asset records
//! have a fixed schema (~50 named fields like `city`, `serialNumber`,
//! `building`, `rack`); there are no "add" or "remove" operations —
//! every field always exists, you only set its value (empty string
//! clears).

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};

use crate::api::ProvisioningApi;

/// `onmsctl requisition asset ...` subcommands.
///
/// **Misfit alert:** unlike `node`, `interface`, `service`, and
/// `category`, this verb operates on POST-IMPORT nodes keyed by
/// **integer database node ID** (not foreign-id), under
/// `/rest/nodes/{db-id}/assetRecord` (not under
/// `/rest/requisitions/...`). Mutations take effect immediately;
/// there is no `requisition import` follow-up needed. Look up the
/// db-id via the Horizon UI or `GET /opennms/rest/nodes?foreignId=...`
/// before running these verbs.
#[derive(Subcommand, Debug, Clone)]
pub enum AssetCmd {
    /// List every populated asset field on an imported node.
    ///
    /// Issues `GET /rest/nodes/{db-id}/assetRecord` and prints the
    /// full record. Use `-o yaml` or `-o json` for the structured
    /// shape; `-o table` (default) prints `<field>=<value>` lines
    /// for every populated field, alphabetically sorted.
    ///
    /// **Declarative alternative:** assets set at requisition time
    /// live in `spec.nodes[].assets` (key-value pairs); after import
    /// Horizon merges them into the asset record. This verb reads
    /// the merged result, which may include server-side defaults the
    /// requisition didn't carry.
    List {
        /// Database node ID (integer, positive). NOT a foreign-id.
        /// Look up via `onmsctl requisition node list <fs>` first to
        /// find the foreign-id, then resolve to db-id via `GET
        /// /rest/nodes?foreignId={fid}` or the Horizon UI.
        #[arg(value_parser = db_id)]
        db_id: i64,
    },
    /// Print a single asset field's value.
    Get {
        /// Database node ID (integer, positive).
        #[arg(value_parser = db_id)]
        db_id: i64,
        /// Asset field name to read (e.g. `city`, `serialNumber`,
        /// `rack`). Canonical Horizon field names are alphanumeric.
        #[arg(value_parser = asset_field)]
        field: String,
    },
    /// Set a single asset field's value.
    ///
    /// Issues a GET-mutate-PUT cycle: read the current record,
    /// change the target field in memory, and PUT the full record
    /// back. The wire type of the target field is preserved — an
    /// integer field stays integer, a bool field stays bool — so
    /// `set 42 id "7"` against an integer field writes the number
    /// `7`, not the string `"7"`. Type-incompatible input (e.g.
    /// setting a numeric field to a non-numeric string) is rejected
    /// at the cmd layer with a clear error and no PUT.
    ///
    /// **Clear semantics vary by type:**
    /// - String field: empty `""` writes an empty string (clear).
    /// - Numeric or bool field: empty `""` writes `null` (clear).
    /// - Field currently `null`: type can't be inferred from the
    ///   wire — the verb defaults to string and emits a stderr
    ///   warning. To establish a non-string type for a null field,
    ///   use `requisition apply -f` first.
    ///
    /// **Declarative alternative for requisition-time assets:** edit
    /// `spec.nodes[].assets` in the YAML and `requisition apply -f`.
    /// However, requisition-time apply only carries values through
    /// the next import; this verb mutates the imported record
    /// directly, which is the only path for ad-hoc asset edits
    /// outside the GitOps loop.
    Set {
        /// Database node ID (integer, positive).
        #[arg(value_parser = db_id)]
        db_id: i64,
        /// Asset field name to write.
        #[arg(value_parser = asset_field)]
        field: String,
        /// New value for the field. Pass `""` to clear.
        value: String,
    },
}

impl Classify for AssetCmd {
    fn kind(&self) -> CmdKind {
        match self {
            AssetCmd::List { .. } | AssetCmd::Get { .. } => CmdKind::Read,
            AssetCmd::Set { .. } => CmdKind::Write,
        }
    }
}

impl AssetCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        match self {
            AssetCmd::List { db_id } => run_list(&api, db_id, ctx).await,
            AssetCmd::Get { db_id, field } => run_get(&api, db_id, &field, ctx).await,
            AssetCmd::Set {
                db_id,
                field,
                value,
            } => run_set(&api, db_id, &field, &value, ctx).await,
        }
    }
}

async fn run_list(api: &ProvisioningApi<'_>, db_id: i64, ctx: &Context) -> Result<()> {
    let record = api.get_node_asset_record(db_id).await?;

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&record)
                .map_err(|e| Error::Config(format!("serializing asset record to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&record)
                .map_err(|e| Error::Config(format!("serializing asset record to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let map = record.as_object().ok_or_else(|| {
                Error::Config(
                    "asset record GET did not return a JSON object — server returned an \
                     unexpected shape"
                        .into(),
                )
            })?;
            // Print every populated field as `key=value`, alphabetical.
            // "Populated" = not null, not the empty string.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut printed = 0;
            for k in keys {
                if let Some(s) = format_field_for_table(&map[k]) {
                    let line = format!("{k}={s}\n");
                    super::write_stdout(line.as_bytes())?;
                    printed += 1;
                }
            }
            if printed == 0 {
                super::write_stdout(b"(no populated asset fields)\n")?;
            }
        }
    }
    Ok(())
}

async fn run_get(api: &ProvisioningApi<'_>, db_id: i64, field: &str, ctx: &Context) -> Result<()> {
    let record = api.get_node_asset_record(db_id).await?;
    let map = record.as_object().ok_or_else(|| {
        Error::Config(
            "asset record GET did not return a JSON object — server returned an \
             unexpected shape"
                .into(),
        )
    })?;
    let value = map.get(field).ok_or_else(|| {
        Error::Config(format!(
            "asset record on node {db_id} does not carry field {field:?}"
        ))
    })?;

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(value)
                .map_err(|e| Error::Config(format!("serializing asset field to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(value)
                .map_err(|e| Error::Config(format!("serializing asset field to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            // Bare value, no key prefix — when the operator asks for
            // one field by name, the key is already known. Shares
            // `format_field_value` with `run_list` so structured
            // values render with the `(json) ` prefix consistently.
            let line = format!("{}\n", format_field_value(value));
            super::write_stdout(line.as_bytes())?;
        }
    }
    Ok(())
}

async fn run_set(
    api: &ProvisioningApi<'_>,
    db_id: i64,
    field: &str,
    value: &str,
    ctx: &Context,
) -> Result<()> {
    // GET-mutate-PUT, matching the `node set` and `interface set`
    // pattern. Defeats the partial-vs-full-replace PUT ambiguity on
    // Horizon's assetRecord endpoint: by sending the entire current
    // record with only the target field changed, untouched fields
    // stay put regardless of which semantic the server applies.
    let mut record = api.get_node_asset_record(db_id).await?;
    let map = record.as_object_mut().ok_or_else(|| {
        Error::Config(
            "asset record GET did not return a JSON object — server returned an \
             unexpected shape"
                .into(),
        )
    })?;
    // Surface the null-current ambiguity: we can't infer wire type
    // from a null field, so we default to string and the operator
    // may be silently downgrading a numeric/bool column. Warn loudly
    // rather than block — the operator may genuinely want a string
    // (most asset fields are).
    if matches!(map.get(field), Some(serde_json::Value::Null)) {
        eprintln!(
            "warning: field {field:?} is currently null on the server — type cannot be \
             inferred from the wire; defaulting to string. To establish a non-string type, \
             set the field via `requisition apply -f` first."
        );
    }
    let new_value = coerce_value_to_field_type(map.get(field), value)?;
    map.insert(field.to_string(), new_value.clone());
    api.put_node_asset_record(db_id, &record).await?;
    emit_action_outcome(db_id, field, &new_value, ctx)
}

/// Coerce a CLI string value to match the type the field currently
/// carries on the server. Without this, `asset set 42 id "7"` would
/// silently replace the integer `id` field with the string `"7"` in
/// the PUT body — a type mismatch the server may reject or, worse,
/// accept. We probe the GET response's current type and convert the
/// incoming string to match. If the field doesn't exist on the
/// record (or is null), we default to string — matches the most
/// common case (most asset fields are strings).
fn coerce_value_to_field_type(
    current: Option<&serde_json::Value>,
    raw: &str,
) -> Result<serde_json::Value> {
    match current {
        Some(serde_json::Value::Number(n)) => {
            if raw.is_empty() {
                return Ok(serde_json::Value::Null);
            }
            // Preserve the integer-vs-float flavor of the current
            // value. If the field is currently i64, require integer
            // input — a float would silently truncate and change
            // the wire flavor. If the field is currently f64, accept
            // any finite float (integer-shaped input like "7" → 7.0
            // is fine, no precision lost).
            if n.is_i64() {
                raw.parse::<i64>()
                    .map(|i| serde_json::Value::Number(i.into()))
                    .map_err(|_| {
                        Error::Config(format!(
                            "field is integer on the server; cannot set to {raw:?} \
                             (provide an integer, or change the field type via \
                             `requisition apply -f`)"
                        ))
                    })
            } else {
                let f: f64 = raw.parse().map_err(|_| {
                    Error::Config(format!(
                        "field is float on the server; cannot set to {raw:?}"
                    ))
                })?;
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| {
                        Error::Config(format!(
                            "field is float on the server but value {raw:?} is not \
                             a finite number"
                        ))
                    })
            }
        }
        Some(serde_json::Value::Bool(_)) => {
            if raw.is_empty() {
                Ok(serde_json::Value::Null)
            } else {
                match raw.to_ascii_lowercase().as_str() {
                    "true" => Ok(serde_json::Value::Bool(true)),
                    "false" => Ok(serde_json::Value::Bool(false)),
                    _ => Err(Error::Config(format!(
                        "field is boolean on the server; cannot set to {raw:?} \
                         (use 'true' or 'false')"
                    ))),
                }
            }
        }
        // String, Null, Array, Object, or field absent: default to
        // string. The Null case is also flagged by a stderr warning
        // in `run_set` since type inference is impossible there.
        // Arrays / objects are rare on asset records; if operators
        // need structured edits they should use the YAML declarative
        // path.
        _ => Ok(serde_json::Value::String(raw.to_string())),
    }
}

fn emit_action_outcome(
    db_id: i64,
    field: &str,
    value: &serde_json::Value,
    ctx: &Context,
) -> Result<()> {
    // The outcome payload carries the coerced JSON value (not the
    // raw `&str` the operator typed) so JSON/YAML consumers can see
    // the wire type that actually landed — e.g. `value: 42` (number)
    // vs `value: "42"` (string) tells the operator which flavor went
    // out.
    let payload = serde_json::json!({
        "db_id": db_id,
        "field": field,
        "value": value,
        "action": "updated",
    });
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&payload)
                .map_err(|e| Error::Config(format!("serializing asset action to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&payload)
                .map_err(|e| Error::Config(format!("serializing asset action to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let line = format!(
                "Node/{db_id} asset/{field}: updated (value={})\n",
                format_field_value(value)
            );
            super::write_stdout(line.as_bytes())?;
        }
    }
    Ok(())
}

/// Render a JSON value as a single-line string for table output.
/// Scalars render naturally (`NYC` / `42` / `true` / `42.5`); arrays
/// and objects render as a `(json) ...` prefixed JSON literal so the
/// operator can tell at a glance the value is structured. `Value::Null`
/// renders as `<null>` — used by `asset get -o table` when the
/// operator explicitly asked for a single field and that field is null.
fn format_field_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "<null>".to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            format!("(json) {}", serde_json::to_string(v).unwrap_or_default())
        }
    }
}

/// Format a JSON value for the `asset list -o table` view. Returns
/// `None` for fields that should be skipped — `null`, empty strings,
/// and empty arrays/objects — so they don't add noise to the listing.
/// Populated fields delegate to [`format_field_value`] so list and
/// get table output render consistently.
fn format_field_for_table(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) if s.is_empty() => None,
        serde_json::Value::Array(a) if a.is_empty() => None,
        serde_json::Value::Object(o) if o.is_empty() => None,
        _ => Some(format_field_value(v)),
    }
}

/// clap value parser for the `<db-id>` positional. Accepts a positive
/// integer up to `i32::MAX` (Horizon's `node.nodeid` is `INTEGER`,
/// 32-bit signed). Rejects `0`, negatives, non-integer input, and
/// values out of range at parse time.
fn db_id(s: &str) -> std::result::Result<i64, String> {
    let n: i64 = s
        .parse()
        .map_err(|_| format!("db-id must be a positive integer; got {s:?}"))?;
    if n <= 0 {
        return Err(format!("db-id must be positive; got {n}"));
    }
    if n > i32::MAX as i64 {
        return Err(format!(
            "db-id must not exceed i32::MAX ({}); Horizon stores node ID as a 32-bit \
             integer. Got {n}",
            i32::MAX
        ));
    }
    Ok(n)
}

/// clap value parser for asset field names. Whitelists the JSON /
/// Java identifier shape `[A-Za-z_][A-Za-z0-9_]*` — broader than the
/// ASCII-alphanumeric-only first cut so canonical Horizon field
/// names with underscores (and any customer-extended schema sticking
/// to the identifier convention) are accepted. Path-traversal
/// characters, shell-meta, dots, dashes, and whitespace are still
/// rejected at parse time so the value can flow into a JSON key
/// without surprises.
fn asset_field(s: &str) -> std::result::Result<String, String> {
    if s.is_empty() {
        return Err("asset field name must not be empty".into());
    }
    let mut bytes = s.bytes();
    let first = bytes.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return Err(format!(
            "asset field name {s:?} must start with an ASCII letter or '_' \
             (JSON/Java identifier shape)"
        ));
    }
    if !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(format!(
            "asset field name {s:?} contains disallowed characters \
             (allowed: ASCII alphanumeric + '_', identifier shape \
             like 'city', 'serialNumber', 'vendor_phone')"
        ));
    }
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_list_and_get_are_read() {
        let list = AssetCmd::List { db_id: 42 };
        let get = AssetCmd::Get {
            db_id: 42,
            field: "city".into(),
        };
        assert_eq!(list.kind(), CmdKind::Read);
        assert_eq!(get.kind(), CmdKind::Read);
    }

    #[test]
    fn classify_set_is_write() {
        let set = AssetCmd::Set {
            db_id: 42,
            field: "city".into(),
            value: "NYC".into(),
        };
        assert_eq!(set.kind(), CmdKind::Write);
    }

    #[test]
    fn db_id_accepts_positive_integers_up_to_i32_max() {
        assert_eq!(db_id("1").unwrap(), 1);
        assert_eq!(db_id("42").unwrap(), 42);
        assert_eq!(db_id("999999").unwrap(), 999999);
        assert_eq!(db_id(&i32::MAX.to_string()).unwrap(), i32::MAX as i64);
    }

    #[test]
    fn db_id_rejects_zero_negative_non_integer_and_overflow() {
        assert!(db_id("0").is_err());
        assert!(db_id("-1").is_err());
        assert!(db_id("abc").is_err());
        assert!(db_id("42.5").is_err());
        assert!(db_id("1.5").is_err());
        assert!(db_id("").is_err());
        assert!(db_id(" 42 ").is_err());
        // Above i32::MAX (= 2_147_483_647) — Horizon stores node ID
        // as a 32-bit integer.
        assert!(db_id("2147483648").is_err());
        assert!(db_id("9999999999").is_err());
    }

    #[test]
    fn asset_field_accepts_identifier_shape() {
        assert_eq!(asset_field("city").unwrap(), "city");
        assert_eq!(asset_field("serialNumber").unwrap(), "serialNumber");
        assert_eq!(
            asset_field("vendorAssetNumber").unwrap(),
            "vendorAssetNumber"
        );
        assert_eq!(asset_field("rack").unwrap(), "rack");
        // Underscore-containing identifiers — accepted to support
        // customer-extended schemas keeping to the identifier shape.
        assert_eq!(asset_field("vendor_phone").unwrap(), "vendor_phone");
        assert_eq!(asset_field("_private").unwrap(), "_private");
        assert_eq!(asset_field("field_v2").unwrap(), "field_v2");
    }

    // ---- coerce_value_to_field_type (C3 type-coercion fix) ----

    #[test]
    fn coerce_integer_field_accepts_integer_value() {
        let current = serde_json::json!(7);
        let got = coerce_value_to_field_type(Some(&current), "42").unwrap();
        assert_eq!(got, serde_json::json!(42));
        assert!(got.as_i64().is_some());
    }

    #[test]
    fn coerce_integer_field_rejects_float_value() {
        // Flavor preservation: integer field + float input would
        // silently truncate; reject instead.
        let current = serde_json::json!(7);
        assert!(coerce_value_to_field_type(Some(&current), "42.5").is_err());
    }

    #[test]
    fn coerce_float_field_accepts_float_value() {
        let current = serde_json::json!(42.5);
        let got = coerce_value_to_field_type(Some(&current), "40.7128").unwrap();
        assert_eq!(got.as_f64(), Some(40.7128));
        // Confirm float flavor preserved (not coerced to i64).
        assert!(!got.is_i64());
    }

    #[test]
    fn coerce_float_field_accepts_integer_shaped_value_as_float() {
        // Float field + integer-shaped input ("7") writes 7.0 — no
        // precision lost, flavor stays float.
        let current = serde_json::json!(42.5);
        let got = coerce_value_to_field_type(Some(&current), "7").unwrap();
        assert_eq!(got.as_f64(), Some(7.0));
        assert!(!got.is_i64());
    }

    #[test]
    fn coerce_numeric_field_empty_string_clears_to_null() {
        let current = serde_json::json!(7);
        let got = coerce_value_to_field_type(Some(&current), "").unwrap();
        assert_eq!(got, serde_json::Value::Null);
        // Float field too.
        let current = serde_json::json!(42.5);
        let got = coerce_value_to_field_type(Some(&current), "").unwrap();
        assert_eq!(got, serde_json::Value::Null);
    }

    #[test]
    fn coerce_numeric_field_rejects_non_numeric() {
        let current = serde_json::json!(7);
        assert!(coerce_value_to_field_type(Some(&current), "abc").is_err());
        let current = serde_json::json!(42.5);
        assert!(coerce_value_to_field_type(Some(&current), "abc").is_err());
    }

    #[test]
    fn coerce_bool_field_accepts_true_false_case_insensitive() {
        let current = serde_json::json!(true);
        assert_eq!(
            coerce_value_to_field_type(Some(&current), "true").unwrap(),
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            coerce_value_to_field_type(Some(&current), "FALSE").unwrap(),
            serde_json::Value::Bool(false)
        );
    }

    #[test]
    fn coerce_bool_field_rejects_garbage() {
        let current = serde_json::json!(false);
        assert!(coerce_value_to_field_type(Some(&current), "maybe").is_err());
    }

    #[test]
    fn coerce_string_field_keeps_string_type() {
        let current = serde_json::json!("NYC");
        let got = coerce_value_to_field_type(Some(&current), "Brooklyn").unwrap();
        assert_eq!(got, serde_json::Value::String("Brooklyn".into()));
    }

    #[test]
    fn coerce_string_field_empty_stays_empty_string() {
        // Empty-string-as-clear semantic for string fields.
        let current = serde_json::json!("NYC");
        let got = coerce_value_to_field_type(Some(&current), "").unwrap();
        assert_eq!(got, serde_json::Value::String(String::new()));
    }

    #[test]
    fn coerce_absent_field_defaults_to_string() {
        let got = coerce_value_to_field_type(None, "42").unwrap();
        // Even "42" stays a string when the field is absent — we
        // can't infer the type, so we pick the most common asset
        // shape.
        assert_eq!(got, serde_json::Value::String("42".into()));
    }

    #[test]
    fn coerce_null_field_defaults_to_string() {
        // Field exists but is null — same fall-through as absent.
        // The `run_set` caller is responsible for emitting a stderr
        // warning so the operator knows type couldn't be inferred.
        let current = serde_json::Value::Null;
        let got = coerce_value_to_field_type(Some(&current), "42").unwrap();
        assert_eq!(got, serde_json::Value::String("42".into()));
    }

    #[test]
    fn coerce_array_field_defaults_to_string() {
        // Arrays / objects fall through to string — operators who
        // need structured edits use the YAML declarative path.
        let current = serde_json::json!(["a", "b"]);
        let got = coerce_value_to_field_type(Some(&current), "x").unwrap();
        assert_eq!(got, serde_json::Value::String("x".into()));
    }

    #[test]
    fn coerce_object_field_defaults_to_string() {
        let current = serde_json::json!({"k": "v"});
        let got = coerce_value_to_field_type(Some(&current), "x").unwrap();
        assert_eq!(got, serde_json::Value::String("x".into()));
    }

    // ---- format_field_for_table (H6 JSON-literal-leak fix) ----

    #[test]
    fn format_table_skips_null_empty_string_and_empty_containers() {
        assert_eq!(format_field_for_table(&serde_json::Value::Null), None);
        assert_eq!(format_field_for_table(&serde_json::json!("")), None);
        // Empty containers also skipped — they carry no information
        // and would otherwise produce noisy `(json) []` lines.
        assert_eq!(format_field_for_table(&serde_json::json!([])), None);
        assert_eq!(format_field_for_table(&serde_json::json!({})), None);
    }

    #[test]
    fn format_table_renders_scalars_naturally() {
        assert_eq!(
            format_field_for_table(&serde_json::json!("NYC")),
            Some("NYC".into())
        );
        assert_eq!(
            format_field_for_table(&serde_json::json!(7)),
            Some("7".into())
        );
        assert_eq!(
            format_field_for_table(&serde_json::json!(42.5)),
            Some("42.5".into())
        );
        assert_eq!(
            format_field_for_table(&serde_json::json!(true)),
            Some("true".into())
        );
    }

    #[test]
    fn format_table_marks_structured_values_with_json_prefix() {
        let array = format_field_for_table(&serde_json::json!(["a", "b"]));
        assert_eq!(array.as_deref(), Some(r#"(json) ["a","b"]"#));
        let object = format_field_for_table(&serde_json::json!({"k": "v"}));
        assert_eq!(object.as_deref(), Some(r#"(json) {"k":"v"}"#));
    }

    // ---- format_field_value (used by `get -o table` AND `list -o table`) ----

    #[test]
    fn format_field_value_renders_null_as_marker() {
        // `get -o table` explicitly asked for this field; null must
        // be visible (vs. `list` which skips null entirely).
        assert_eq!(format_field_value(&serde_json::Value::Null), "<null>");
    }

    #[test]
    fn format_field_value_renders_structured_with_json_prefix() {
        // Same shape as `list -o table` for non-null structured
        // values — locks the H6-fix consistency between get and list.
        assert_eq!(
            format_field_value(&serde_json::json!(["a", "b"])),
            r#"(json) ["a","b"]"#
        );
        assert_eq!(
            format_field_value(&serde_json::json!({"k": "v"})),
            r#"(json) {"k":"v"}"#
        );
    }

    #[test]
    fn asset_field_rejects_specials_and_traversal() {
        assert!(asset_field("").is_err());
        assert!(asset_field("city.foo").is_err()); // dots not allowed
        assert!(asset_field("city-foo").is_err()); // hyphens not allowed
        assert!(asset_field("../etc").is_err());
        assert!(asset_field("city space").is_err());
        assert!(asset_field("city;rm").is_err());
        assert!(asset_field("city\x00").is_err());
        // Leading digit — JSON/Java identifier shape requires
        // alpha-or-underscore start.
        assert!(asset_field("1city").is_err());
    }
}
