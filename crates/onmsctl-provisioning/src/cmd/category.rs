/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl requisition category` — read-only inspection verb for the
//! requisition's category sub-resource.
//!
//! Categories are scoped within a node (`<fs> <foreign-id>`). The
//! verb issues only `GET` requests. Mutation is declarative: edit the
//! `kind: Requisition` YAML and run `onmsctl apply -f <file>`.

use crate::api::ProvisioningApi;
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
    /// <fs>` for the server's current state. To change categories,
    /// edit the YAML and run `onmsctl apply -f <file>`.
    List {
        /// Foreign-source name.
        #[arg(value_parser = super::nonempty_fs)]
        fs: String,
        /// Foreign-id of the parent node.
        #[arg(value_parser = super::nonempty_string)]
        foreign_id: String,
    },
}

impl Classify for CategoryCmd {
    fn kind(&self) -> CmdKind {
        match self {
            CategoryCmd::List { .. } => CmdKind::Read,
        }
    }
}

impl CategoryCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = ProvisioningApi::new(&client);
        match self {
            CategoryCmd::List { fs, foreign_id } => run_list(&api, &fs, &foreign_id, ctx).await,
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
}
