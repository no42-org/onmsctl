/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `onmsctl snmp` subcommand surface.
//!
//! `export` snapshots the deployed snmp-config as a `kind: SnmpConfig` document
//! (the reverse of `apply`); `lookup` reports the effective SNMP parameters
//! OpenNMS would use for one or more IPs (the web UI's SNMP lookup). Both are
//! **Read** verbs — they issue only `GET`s and are permitted under a
//! `read-only` context.
//!
//! Secrets are write-only: `export` emits every secret field as a reference
//! placeholder (never cleartext), so an exported document is safe to commit but
//! must have its secret references wired up before it can be re-applied.

use std::io::{ErrorKind, Write};
use std::net::IpAddr;
use std::path::PathBuf;

use clap::Subcommand;
use onmsctl_core::{
    Classify, CmdKind, Context, Error, OnmsClient, OutputFormat, Result, TableRow, render_list,
};
use serde::Serialize;

use crate::api::SnmpConfigApi;
use crate::convert::from_wire;
use crate::model::SnmpConfigLocal;
use crate::select;
use crate::server::SnmpAgentConfig;

/// Placeholder shown for a masked secret in `lookup` output.
const MASKED: &str = "****";

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
    /// Report the effective SNMP parameters OpenNMS would use for one or more
    /// IPs (the web UI's SNMP lookup).
    Lookup {
        /// One or more IP addresses to resolve.
        #[arg(required = true, value_name = "IP")]
        ips: Vec<String>,
        /// Resolve at this monitoring location only. Without it, every location
        /// whose definition selector (`specific`/`range`/`ipMatches`) matches
        /// the IP is reported, falling back to `Default` when none match. A
        /// location matched only by a profile `filterExpression` (evaluated
        /// server-side) is not auto-discovered — pass `--location` for those.
        #[arg(long)]
        location: Option<String>,
        /// Reveal community strings / passphrases (masked by default).
        #[arg(long)]
        show_secrets: bool,
    },
}

impl Classify for SnmpCmd {
    fn kind(&self) -> CmdKind {
        match self {
            // Both verbs issue only GETs.
            SnmpCmd::Export { .. } | SnmpCmd::Lookup { .. } => CmdKind::Read,
        }
    }
}

impl SnmpCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        match self {
            SnmpCmd::Export { output_file } => run_export(output_file, ctx).await,
            SnmpCmd::Lookup {
                ips,
                location,
                show_secrets,
            } => run_lookup(ips, location, show_secrets, ctx).await,
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

/// One `(ip, location)` lookup result: the queried IP and location plus the
/// effective agent config flattened in, so `-o json|yaml` reads naturally.
#[derive(Debug, Clone, Serialize)]
struct LookupResult {
    ip: String,
    location: String,
    #[serde(flatten)]
    agent: SnmpAgentConfig,
}

impl TableRow for LookupResult {
    fn headers() -> Vec<&'static str> {
        vec![
            "IP",
            "LOCATION",
            "VERSION",
            "PORT",
            "TIMEOUT",
            "RETRIES",
            "CREDENTIAL",
            "PROFILE",
        ]
    }

    fn row(&self) -> Vec<String> {
        let a = &self.agent;
        let dash = || "-".to_string();
        // v3 → security identity; v1/v2c → (already-masked) read community.
        let credential = if a.version_as_string.as_deref() == Some("v3") {
            match (&a.security_name, a.security_level) {
                (Some(name), Some(level)) => format!("{name} (level {level})"),
                (Some(name), None) => name.clone(),
                _ => dash(),
            }
        } else {
            a.read_community.clone().unwrap_or_else(dash)
        };
        vec![
            self.ip.clone(),
            self.location.clone(),
            a.version_as_string.clone().unwrap_or_else(dash),
            a.port.map(|p| p.to_string()).unwrap_or_else(dash),
            a.timeout.map(|t| t.to_string()).unwrap_or_else(dash),
            a.retries.map(|r| r.to_string()).unwrap_or_else(dash),
            credential,
            a.profile_label.clone().unwrap_or_else(dash),
        ]
    }
}

async fn run_lookup(
    ips: Vec<String>,
    location: Option<String>,
    show_secrets: bool,
    ctx: &Context,
) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = SnmpConfigApi::new(&client);
    let results = collect_lookups(&api, &ips, location.as_deref(), show_secrets).await?;
    print!("{}", render_list(&results, ctx.output_format)?);
    Ok(())
}

/// Resolve every `(ip, location)` pair. With an explicit `location`, each IP is
/// queried once there; otherwise the deployed config is fetched once and each
/// IP is resolved at every location its selectors match (or `Default` when
/// none). Secrets are masked unless `show_secrets`.
async fn collect_lookups(
    api: &SnmpConfigApi<'_>,
    ips: &[String],
    location: Option<&str>,
    show_secrets: bool,
) -> Result<Vec<LookupResult>> {
    // Fetch the stored config once, only when we must discover locations.
    let cfg = match location {
        Some(_) => None,
        None => Some(api.get_config().await?),
    };

    let mut results = Vec::new();
    for ip_str in ips {
        let ip: IpAddr = ip_str
            .parse()
            .map_err(|_| Error::Config(format!("invalid IP address {ip_str:?}")))?;
        let locations: Vec<String> = match location {
            Some(loc) => vec![loc.to_string()],
            None => {
                let mut discovered = select::locations_for_ip(cfg.as_ref().unwrap(), ip);
                if discovered.is_empty() {
                    discovered.push(select::DEFAULT_LOCATION.to_string());
                }
                discovered
            }
        };
        for loc in locations {
            let mut agent = api.lookup_for_ip(ip_str, &loc).await?;
            if !show_secrets {
                mask_secrets(&mut agent);
            }
            results.push(LookupResult {
                ip: ip_str.clone(),
                location: loc,
                agent,
            });
        }
    }
    Ok(results)
}

/// Replace any present secret value with a fixed mask, so the cleartext never
/// reaches `-o json|yaml` or the table without `--show-secrets`.
fn mask_secrets(a: &mut SnmpAgentConfig) {
    for field in [
        &mut a.read_community,
        &mut a.write_community,
        &mut a.auth_pass_phrase,
        &mut a.priv_pass_phrase,
    ] {
        if field.is_some() {
            *field = Some(MASKED.to_string());
        }
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
    use onmsctl_core::{AuthCreds, Url};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> OnmsClient {
        let ctx = Context {
            name: "test".into(),
            url: Url::parse(&format!("{}/", server.uri())).unwrap(),
            creds: AuthCreds::basic("admin", "secret"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        };
        OnmsClient::from_context(&ctx).unwrap()
    }

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

    async fn mount_lookup(server: &MockServer, location: &str, community: &str) {
        Mock::given(method("GET"))
            .and(path("/api/v2/snmp-config/lookup"))
            .and(query_param("ipAddress", "192.168.8.8"))
            .and(query_param("location", location))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "address": "192.168.8.8",
                "versionAsString": "v2c",
                "port": 161,
                "readCommunity": community
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn lookup_without_location_discovers_and_masks() {
        let server = MockServer::start().await;
        // Whole config: a definition at `hq` selecting the IP.
        Mock::given(method("GET"))
            .and(path("/api/v2/snmp-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "definition": [{ "location": "hq", "specific": ["192.168.8.8"] }]
            })))
            .mount(&server)
            .await;
        mount_lookup(&server, "hq", "public").await;

        let client = client_for(&server);
        let api = SnmpConfigApi::new(&client);
        let results = collect_lookups(&api, &["192.168.8.8".into()], None, false)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].location, "hq");
        assert_eq!(results[0].agent.version_as_string.as_deref(), Some("v2c"));
        // Masked by default — the cleartext community must not leak.
        assert_eq!(results[0].agent.read_community.as_deref(), Some(MASKED));
    }

    #[tokio::test]
    async fn lookup_with_location_skips_discovery_and_show_secrets_reveals() {
        let server = MockServer::start().await;
        // No whole-config mock: an explicit --location must NOT fetch it.
        mount_lookup(&server, "edge", "s3cr3t").await;

        let client = client_for(&server);
        let api = SnmpConfigApi::new(&client);
        let results = collect_lookups(&api, &["192.168.8.8".into()], Some("edge"), true)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].location, "edge");
        // --show-secrets keeps the cleartext.
        assert_eq!(results[0].agent.read_community.as_deref(), Some("s3cr3t"));
    }

    #[tokio::test]
    async fn lookup_falls_back_to_default_when_no_definition_matches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/snmp-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "definition": []
            })))
            .mount(&server)
            .await;
        mount_lookup(&server, "Default", "public").await;

        let client = client_for(&server);
        let api = SnmpConfigApi::new(&client);
        let results = collect_lookups(&api, &["192.168.8.8".into()], None, false)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].location, "Default");
    }

    #[test]
    fn lookup_is_classified_read() {
        assert_eq!(
            SnmpCmd::Lookup {
                ips: vec!["1.2.3.4".into()],
                location: None,
                show_secrets: false
            }
            .kind(),
            CmdKind::Read
        );
    }
}
