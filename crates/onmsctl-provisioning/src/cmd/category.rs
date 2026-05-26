/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl requisition category` — imperative escape-hatch verbs for
//! the requisition's category sub-resource (Group 7 phase 4,
//! task 7.4).
//!
//! Categories are scoped within a node (`<fs> <foreign-id>
//! <category>`). Verb coverage per design.md §D8: `list / add /
//! remove` only — categories are a tag-like resource (just a name);
//! there's nothing to mutate beyond add/remove, and `get` would
//! surface no information `list` doesn't.

use crate::api::ProvisioningApi;
use crate::model::server::CategoryRef;
use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};

/// `onmsctl requisition category ...` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum CategoryCmd {
    /// List every category attached to a given node (projected from
    /// the node's existing GET — no new endpoint hit).
    ///
    /// **Declarative alternative:** read the `spec.nodes[].categories`
    /// block from the local YAML, or `onmsctl requisition export
    /// <fs>` for the server's current state.
    List {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
    },
    /// Attach a category to an existing node.
    ///
    /// Idempotent on the wire: `CategoryRef` carries only `name`, so
    /// POSTing the same category twice is effectively a no-op —
    /// unlike interface / service `add`, there are no nested
    /// collections to clobber.
    ///
    /// **Declarative alternative:** add the entry to
    /// `spec.nodes[].categories` in the YAML and `requisition apply
    /// -f`. Apply will diff the change and re-import; this verb
    /// skips both (operator runs `requisition import <fs>` to take
    /// effect).
    Add {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// Category name (e.g. `Production`, `Production Servers`,
        /// `Web`, `Database`). Allowed characters: ASCII alphanumeric,
        /// `.`, `_`, `-`, space, `(`, `)`. Must contain at least one
        /// alphanumeric character.
        #[arg(value_parser = category_name)]
        category: String,
    },
    /// Detach a category from the node's pending state.
    ///
    /// **Declarative alternative:** delete the entry from
    /// `spec.nodes[].categories` and `requisition apply -f`.
    Remove {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
        /// Category name to remove. Allowed characters: ASCII
        /// alphanumeric, `.`, `_`, `-`, space, `(`, `)`. Must contain
        /// at least one alphanumeric character.
        #[arg(value_parser = category_name)]
        category: String,
    },
}

impl Classify for CategoryCmd {
    fn kind(&self) -> CmdKind {
        match self {
            CategoryCmd::List { .. } => CmdKind::Read,
            CategoryCmd::Add { .. } | CategoryCmd::Remove { .. } => CmdKind::Write,
        }
    }
}

impl CategoryCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        match self {
            CategoryCmd::List { fs, foreign_id } => run_list(&api, &fs, &foreign_id, ctx).await,
            CategoryCmd::Add {
                fs,
                foreign_id,
                category,
            } => run_add(&api, &fs, &foreign_id, &category, ctx).await,
            CategoryCmd::Remove {
                fs,
                foreign_id,
                category,
            } => run_remove(&api, &fs, &foreign_id, &category, ctx).await,
        }
    }
}

async fn run_list(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    ctx: &Context,
) -> Result<()> {
    // Deliberate: project from the node's existing GET. Categories
    // are embedded in the node payload, so one round-trip is cheaper
    // than N. Tradeoff: a concurrent mutation between this GET and a
    // sibling per-category verb can surface a stale snapshot — fine
    // for a read-only listing.
    let node = api
        .get_requisition_node(fs, foreign_id)
        .await?
        .ok_or_else(|| {
            // The node GET 404s when EITHER the requisition or the
            // node is missing — Horizon doesn't distinguish.
            Error::Config(format!(
                "GET returned 404 — one of requisition '{fs}' or node '{foreign_id}' does not exist"
            ))
        })?;
    let names: Vec<&str> = node.category.iter().map(|c| c.name.as_str()).collect();

    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&names)
                .map_err(|e| Error::Config(format!("serializing category list to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&names)
                .map_err(|e| Error::Config(format!("serializing category list to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            if names.is_empty() {
                super::write_stdout(b"(no categories)\n")?;
            } else {
                for n in &names {
                    let line = format!("{n}\n");
                    super::write_stdout(line.as_bytes())?;
                }
            }
        }
    }
    Ok(())
}

async fn run_add(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    category: &str,
    ctx: &Context,
) -> Result<()> {
    let cat = CategoryRef {
        name: category.to_string(),
    };
    api.post_requisition_category(fs, foreign_id, &cat).await?;
    emit_action_outcome(fs, foreign_id, category, "added", ctx)
}

async fn run_remove(
    api: &ProvisioningApi<'_>,
    fs: &str,
    foreign_id: &str,
    category: &str,
    ctx: &Context,
) -> Result<()> {
    api.delete_requisition_category(fs, foreign_id, category)
        .await?;
    emit_action_outcome(fs, foreign_id, category, "removed", ctx)
}

fn emit_action_outcome(
    fs: &str,
    foreign_id: &str,
    category: &str,
    action: &str,
    ctx: &Context,
) -> Result<()> {
    let payload = serde_json::json!({
        "foreign_source": fs,
        "foreign_id": foreign_id,
        "category": category,
        "action": action,
    });
    match ctx.output_format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&payload)
                .map_err(|e| Error::Config(format!("serializing category action to JSON: {e}")))?;
            super::write_stdout_line(json.as_bytes())?;
        }
        OutputFormat::Yaml => {
            let yaml = serde_norway::to_string(&payload)
                .map_err(|e| Error::Config(format!("serializing category action to YAML: {e}")))?;
            super::write_stdout(yaml.as_bytes())?;
        }
        OutputFormat::Table => {
            let line =
                format!("Requisition/{fs} node/{foreign_id} category/{category}: {action}\n");
            super::write_stdout(line.as_bytes())?;
        }
    }
    Ok(())
}

/// clap value parser for category-name positionals. Whitelists ASCII
/// alphanumeric plus `.`, `_`, `-`, space, `(`, `)` — broader than
/// `service_name` because real-world Horizon deployments ship
/// multi-word categories like `Production Servers` and `Network
/// Interfaces`. Rejects path-traversal (`/`, `..`), shell-meta
/// (`;`, `|`, backtick, `<`, `>`, `"`), and control characters at
/// parse time. Requires at least one alphanumeric character so `.`,
/// `..`, `   `, or `(())` can't sneak through as path segments.
fn category_name(s: &str) -> std::result::Result<String, String> {
    if s.is_empty() {
        return Err("category name must not be empty".into());
    }
    if !s.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || b == b'.'
            || b == b'_'
            || b == b'-'
            || b == b' '
            || b == b'('
            || b == b')'
    }) {
        return Err(format!(
            "category name {s:?} contains disallowed characters \
             (allowed: ASCII alphanumeric, '.', '_', '-', space, '(', ')')"
        ));
    }
    if !s.bytes().any(|b| b.is_ascii_alphanumeric()) {
        return Err(format!(
            "category name {s:?} must contain at least one alphanumeric character"
        ));
    }
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_list_is_read() {
        let list = CategoryCmd::List {
            fs: "acme".into(),
            foreign_id: "web01".into(),
        };
        assert_eq!(list.kind(), CmdKind::Read);
    }

    #[test]
    fn classify_add_and_remove_are_write() {
        let add = CategoryCmd::Add {
            fs: "acme".into(),
            foreign_id: "web01".into(),
            category: "Production".into(),
        };
        let remove = CategoryCmd::Remove {
            fs: "acme".into(),
            foreign_id: "web01".into(),
            category: "Production".into(),
        };
        assert_eq!(add.kind(), CmdKind::Write);
        assert_eq!(remove.kind(), CmdKind::Write);
    }

    #[test]
    fn category_name_accepts_canonical_and_multiword_values() {
        assert_eq!(category_name("Production").unwrap(), "Production");
        assert_eq!(category_name("Web").unwrap(), "Web");
        assert_eq!(category_name("Database-1").unwrap(), "Database-1");
        assert_eq!(category_name("app_v2").unwrap(), "app_v2");
        assert_eq!(category_name("env.prod").unwrap(), "env.prod");
        // Multi-word with spaces — Horizon's built-in categories use
        // this shape (e.g. `Production Servers`, `Network Interfaces`).
        assert_eq!(
            category_name("Production Servers").unwrap(),
            "Production Servers"
        );
        assert_eq!(category_name("App (prod)").unwrap(), "App (prod)");
    }

    #[test]
    fn category_name_rejects_path_traversal_and_specials() {
        assert!(category_name("").is_err());
        assert!(category_name("..").is_err());
        assert!(category_name("   ").is_err()); // whitespace-only, no alphanumeric
        assert!(category_name("Prod/foo").is_err());
        assert!(category_name("prod;rm").is_err());
        assert!(category_name("a|b").is_err());
        assert!(category_name("a<b").is_err());
        assert!(category_name("a\x00b").is_err());
    }
}
