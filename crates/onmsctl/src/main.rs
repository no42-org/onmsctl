/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl` binary entry point.
//!
//! Top-level Cli + Capability dispatch. Each capability owns its own
//! subcommand tree; this binary composes them. Adding a future capability
//! is a one-variant addition to `Capability` plus one match arm in
//! [`run`].

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use onmsctl_core::{
    Error, OutputFormat, Overrides,
    config::{default_path as default_config_path, load as load_config},
    context::Context,
};

#[derive(Parser, Debug)]
#[command(
    name = "onmsctl",
    version,
    about = "Command-line interface for OpenNMS Horizon",
    long_about = "Command-line interface for OpenNMS Horizon. \
                  See `onmsctl <subcommand> --help` for per-command flags. \
                  Configuration: ~/.config/onmsctl/config.yaml \
                  (XDG; macOS uses ~/Library/Application Support/...)."
)]
struct Cli {
    /// Override the config file path (also: $ONMSCTL_CONFIG).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Active context name (also: $ONMSCTL_CONTEXT).
    #[arg(long, global = true)]
    context: Option<String>,

    /// Server URL override (also: $ONMS_URL).
    #[arg(long, global = true)]
    url: Option<String>,

    /// Username override for Basic auth (also: $ONMS_USER).
    #[arg(long, global = true)]
    user: Option<String>,

    /// Skip TLS certificate verification (insecure).
    #[arg(long, global = true)]
    insecure_tls: bool,

    /// Output format.
    #[arg(short = 'o', long = "output", global = true, value_parser = parse_output_format)]
    output: Option<OutputFormat>,

    /// Verbose output (full error chains, plus capability-specific diagnostics).
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: TopCmd,
}

/// Top-level resource verbs. Each capability registers its own subcommands
/// here; the binary stitches them together statically.
#[derive(Subcommand, Debug)]
enum TopCmd {
    /// Manage eventconf sources.
    #[command(subcommand)]
    Source(onmsctl_eventconf::cmd::SourceCmd),
    // Phase 4 commit 2: Event(onmsctl_eventconf::cmd::EventCmd),
    // Future capabilities (Node, Alarm, …) extend here.
}

fn parse_output_format(s: &str) -> std::result::Result<OutputFormat, String> {
    s.parse().map_err(|e: Error| e.to_string())
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let verbose = cli.verbose;
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Walk the error chain, printing each link.
            eprintln!("error: {e}");
            if verbose {
                let mut src = e.source();
                while let Some(err) = src {
                    eprintln!("  caused by: {err}");
                    src = err.source();
                }
            }
            ExitCode::from(exit_code_from(&e))
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    // 1. Resolve overrides: env layer + flag layer.
    let env = Overrides::from_env();
    let flags = Overrides {
        config_path: cli.config,
        context_name: cli.context,
        url: cli.url,
        user: cli.user,
        insecure_tls: if cli.insecure_tls { Some(true) } else { None },
        output: cli.output,
        verbose: cli.verbose,
        // Password / token never come from CLI flags; only env or config.
        password: None,
        token: None,
    };
    let merged = env.with_flags(flags);

    // 2. Load config from the resolved path.
    let config_path = match &merged.config_path {
        Some(p) => p.clone(),
        None => default_config_path().context("resolving default config path")?,
    };
    let cfg = load_config(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    // 3. Resolve the active context.
    let ctx = Context::resolve(&cfg, &merged)?;

    // 4. Dispatch to the selected capability.
    match cli.command {
        TopCmd::Source(cmd) => cmd.run(&ctx).await?,
    }
    Ok(())
}

/// Map an error chain to a stable shell exit code. If any link in the chain
/// is an `onmsctl_core::Error`, its `exit_code()` wins; otherwise we exit
/// with the generic `2`.
fn exit_code_from(err: &anyhow::Error) -> u8 {
    if let Some(e) = err.downcast_ref::<Error>() {
        return e.exit_code();
    }
    let mut src = err.source();
    while let Some(err) = src {
        if let Some(e) = err.downcast_ref::<Error>() {
            return e.exit_code();
        }
        src = err.source();
    }
    2
}
