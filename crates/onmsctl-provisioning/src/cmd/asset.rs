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
    /// Issues `PUT /rest/nodes/{db-id}/assetRecord` with a JSON body
    /// containing only the named field, relying on Horizon's
    /// partial-update semantic: every other asset field stays put.
    ///
    /// Pass an empty string to clear an existing value:
    /// `onmsctl requisition asset set 42 city ""`.
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
                let v = &map[k];
                let s = match v {
                    serde_json::Value::Null => continue,
                    serde_json::Value::String(s) if s.is_empty() => continue,
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let line = format!("{k}={s}\n");
                super::write_stdout(line.as_bytes())?;
                printed += 1;
            }
            if printed == 0 {
                super::write_stdout(b"(no populated asset fields)\n")?;
            }
        }
    }
    Ok(())
}

async fn run_get(
    api: &ProvisioningApi<'_>,
    db_id: i64,
    field: &str,
    ctx: &Context,
) -> Result<()> {
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
            let json = serde_json::to_string_pretty(value).map_err(|e| {
                Error::Config(format!("serializing asset field to JSON: {e}"))
            })?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(value).map_err(|e| {
                Error::Config(format!("serializing asset field to YAML: {e}"))
            })?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            // Bare value, no key prefix — when the user asks for one
            // field by name, the key is already known.
            let s = match value {
                serde_json::Value::Null => String::from("<null>"),
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let line = format!("{s}\n");
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
    map.insert(field.to_string(), serde_json::Value::String(value.to_string()));
    api.put_node_asset_record(db_id, &record).await?;
    emit_action_outcome(db_id, field, value, ctx)
}

fn emit_action_outcome(db_id: i64, field: &str, value: &str, ctx: &Context) -> Result<()> {
    let payload = serde_json::json!({
        "db_id": db_id,
        "field": field,
        "value": value,
        "action": "updated",
    });
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&payload).map_err(|e| {
                Error::Config(format!("serializing asset action to JSON: {e}"))
            })?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&payload).map_err(|e| {
                Error::Config(format!("serializing asset action to YAML: {e}"))
            })?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let line = format!("Node/{db_id} asset/{field}: updated (value={value:?})\n");
            super::write_stdout(line.as_bytes())?;
        }
    }
    Ok(())
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
        assert!(db_id("3.14").is_err());
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
        assert_eq!(asset_field("vendorAssetNumber").unwrap(), "vendorAssetNumber");
        assert_eq!(asset_field("rack").unwrap(), "rack");
        // Underscore-containing identifiers — accepted to support
        // customer-extended schemas keeping to the identifier shape.
        assert_eq!(asset_field("vendor_phone").unwrap(), "vendor_phone");
        assert_eq!(asset_field("_private").unwrap(), "_private");
        assert_eq!(asset_field("field_v2").unwrap(), "field_v2");
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
