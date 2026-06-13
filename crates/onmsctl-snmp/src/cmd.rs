/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `onmsctl snmp` subcommand surface.
//!
//! `export` snapshots the deployed snmp-config as a `kind: SnmpConfig` document
//! (the reverse of `apply`); `lookup` reports the effective parameters for an IP
//! (added in a later increment). Both are **Read** verbs — they issue only
//! `GET`s and are permitted under a `read-only` context.
//!
//! Secrets are write-only: `export` emits every secret field as a reference
//! placeholder (never cleartext), so an exported document is safe to commit but
//! must have its secret references wired up before it can be re-applied.

use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result};

use crate::api::SnmpConfigApi;
use crate::convert::from_wire;
use crate::model::SnmpConfigLocal;

/// `onmsctl snmp …` verbs.
#[derive(Subcommand, Debug, Clone)]
pub enum SnmpCmd {
    /// Snapshot the deployed SNMP configuration as a `kind: SnmpConfig`
    /// document (secret fields emitted as reference placeholders).
    Export {
        /// Write the document to this file instead of stdout.
        #[arg(short = 'O', long = "output-file")]
        output_file: Option<PathBuf>,
    },
}

impl Classify for SnmpCmd {
    fn kind(&self) -> CmdKind {
        match self {
            SnmpCmd::Export { .. } => CmdKind::Read,
        }
    }
}

impl SnmpCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        match self {
            SnmpCmd::Export { output_file } => run_export(output_file, ctx).await,
        }
    }
}

async fn run_export(output_file: Option<PathBuf>, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = SnmpConfigApi::new(&client);
    let wire = api.get_config().await?;
    let local = from_wire(&wire);
    let rendered = render_doc(&local, ctx.output_format)?;

    match output_file {
        Some(path) => {
            std::fs::write(&path, rendered.as_bytes())
                .map_err(|e| Error::Config(format!("write {}: {e}", path.display())))?;
            eprintln!("Exported snmp-config to {}", path.display());
            Ok(())
        }
        None => write_stdout(rendered.as_bytes()),
    }
}

/// Render the exported document in the requested format. The `kind: SnmpConfig`
/// YAML is the canonical form (and what `apply` re-consumes); `-o json` emits
/// the same model as pretty JSON. `table` has no meaningful tabular shape for a
/// whole config, so it falls back to YAML.
fn render_doc(local: &SnmpConfigLocal, format: OutputFormat) -> Result<String> {
    match format {
        OutputFormat::Json => Ok(serde_json::to_string_pretty(local)
            .map_err(|e| Error::Config(format!("serializing snmp-config to JSON: {e}")))?
            + "\n"),
        OutputFormat::Yaml | OutputFormat::Table => Ok(serde_norway::to_string(local)?),
    }
}

/// Write `bytes` to stdout, treating `BrokenPipe` as a clean exit (e.g. piping
/// into `head`). Mirrors the other capabilities' local helper.
fn write_stdout(bytes: &[u8]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(bytes) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server;

    fn sample_wire() -> server::SnmpConfig {
        server::SnmpConfig {
            defaults: server::Configuration {
                version: Some("v2c".into()),
                read_community: Some("public-cleartext".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn export_yaml_is_a_snmpconfig_doc_without_cleartext_secrets() {
        let local = from_wire(&sample_wire());
        let yaml = render_doc(&local, OutputFormat::Yaml).unwrap();
        assert!(yaml.contains("kind: SnmpConfig"));
        assert!(yaml.contains("apiVersion: snmp.opennms.org/v1"));
        // The deployed cleartext secret is NOT carried into the export.
        assert!(!yaml.contains("public-cleartext"));
        // A reference placeholder is emitted instead.
        assert!(yaml.contains("fromEnv"));
        // The document round-trips back into the model.
        let reparsed: SnmpConfigLocal = serde_norway::from_str(&yaml).unwrap();
        reparsed.validate().expect("exported doc is valid");
    }

    #[test]
    fn export_json_matches_the_model() {
        let local = from_wire(&sample_wire());
        let json = render_doc(&local, OutputFormat::Json).unwrap();
        let reparsed: SnmpConfigLocal = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed, local);
    }

    #[test]
    fn export_is_classified_read() {
        assert_eq!(SnmpCmd::Export { output_file: None }.kind(), CmdKind::Read);
    }
}
