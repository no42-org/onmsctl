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

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::CommandFactory;
use clap::builder::TypedValueParser as _;
use clap::{Parser, Subcommand};
use onmsctl_core::{
    Error, OutputFormat, Overrides,
    config::{ConfigFile, default_path as default_config_path, load as load_config},
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
    #[arg(
        short = 'o',
        long = "output",
        global = true,
        value_parser = clap::builder::PossibleValuesParser::new(["table", "yaml", "json"]).map(|s| s.parse::<OutputFormat>().unwrap()),
    )]
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
    /// Manage eventconf events.
    #[command(subcommand)]
    Event(onmsctl_eventconf::cmd::EventCmd),
    /// Print the binary version and linked capability list.
    Version,
    /// Inspect or switch the active configuration.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Generate a shell-completion script.
    ///
    /// Pipe stdout into your shell's completion-loading mechanism, e.g.
    /// `onmsctl completion bash > /etc/bash_completion.d/onmsctl` or
    /// `eval "$(onmsctl completion zsh)"`.
    Completion {
        /// Target shell.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    // Future capabilities (Node, Alarm, …) extend here.
}

#[derive(Subcommand, Debug, Clone)]
enum ConfigCmd {
    /// Print the loaded config with secrets redacted.
    View,
    /// Switch the active context by writing `current-context` back to the
    /// config file. The named context must already exist.
    UseContext {
        /// Name of an existing context.
        name: String,
    },
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

    // 2. Dispatch. version / completion / config don't need a resolved
    //    Context; capability commands do.
    match cli.command {
        TopCmd::Version => print_version(),
        TopCmd::Completion { shell } => print_completion(shell),
        TopCmd::Config(cmd) => run_config(cmd, &merged).await,
        TopCmd::Source(cmd) => {
            let ctx = resolve_context(&merged)?;
            cmd.run(&ctx).await?;
            Ok(())
        }
        TopCmd::Event(cmd) => {
            let ctx = resolve_context(&merged)?;
            cmd.run(&ctx).await?;
            Ok(())
        }
    }
}

/// Resolve config path → load → resolve a runtime [`Context`] for
/// capability commands. Bypassed by subcommands that don't talk to a server
/// (`version`, `completion`, `config`).
fn resolve_context(merged: &Overrides) -> Result<Context> {
    let config_path = config_path_from(merged)?;
    let cfg = load_config(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;
    let ctx = Context::resolve(&cfg, merged)?;
    Ok(ctx)
}

fn config_path_from(merged: &Overrides) -> Result<PathBuf> {
    match &merged.config_path {
        Some(p) => Ok(p.clone()),
        None => default_config_path().context("resolving default config path"),
    }
}

/// `onmsctl version` — prints the binary version plus each linked
/// capability crate's version. Surface for ops automation that wants to
/// pin a specific binary build.
fn print_version() -> Result<()> {
    println!("onmsctl {}", env!("CARGO_PKG_VERSION"));
    println!("capabilities:");
    println!(
        "  - {} {}",
        onmsctl_eventconf::CAPABILITY_NAME,
        onmsctl_eventconf::VERSION
    );
    Ok(())
}

/// `onmsctl completion <shell>` — emits a completion script to stdout via
/// `clap_complete` so the script always matches the actual clap command tree.
fn print_completion(shell: clap_complete::Shell) -> Result<()> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    let mut stdout = std::io::stdout().lock();
    clap_complete::generate(shell, &mut cmd, bin_name, &mut stdout);
    Ok(())
}

async fn run_config(cmd: ConfigCmd, merged: &Overrides) -> Result<()> {
    let path = config_path_from(merged)?;
    match cmd {
        ConfigCmd::View => {
            let mut cfg = load_config(&path)
                .with_context(|| format!("loading config from {}", path.display()))?;
            redact_secrets(&mut cfg);
            let yaml = serde_norway::to_string(&cfg)
                .map_err(|e| anyhow::anyhow!("serializing config: {e}"))?;
            print!("{yaml}");
            Ok(())
        }
        ConfigCmd::UseContext { name } => {
            let mut cfg = load_config(&path)
                .with_context(|| format!("loading config from {}", path.display()))?;
            if cfg.find_context(&name).is_none() {
                let known = if cfg.contexts.is_empty() {
                    "(none defined)".to_string()
                } else {
                    cfg.contexts
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                return Err(anyhow::anyhow!(
                    "context '{name}' not found in {}; known contexts: {known}",
                    path.display()
                ));
            }
            cfg.current_context = Some(name.clone());
            let yaml = serde_norway::to_string(&cfg)
                .map_err(|e| anyhow::anyhow!("serializing config: {e}"))?;
            write_atomic(&path, yaml.as_bytes())?;
            eprintln!("switched to context '{name}'");
            Ok(())
        }
    }
}

/// Replace inline secret strings with `<redacted>` before printing the
/// config. password-file / token-file / keyring references are left intact
/// since they are pointers, not secrets.
fn redact_secrets(cfg: &mut ConfigFile) {
    for c in &mut cfg.contexts {
        if let Some(b) = c.auth.basic.as_mut()
            && b.password.is_some()
        {
            b.password = Some("<redacted>".into());
        }
        if let Some(b) = c.auth.bearer.as_mut()
            && b.token.is_some()
        {
            b.token = Some("<redacted>".into());
        }
    }
}

/// Write `bytes` to `path` atomically: stage in a sibling temp file, then
/// `rename` over the target. Same-directory rename is POSIX-atomic and
/// avoids a partial config file on crash.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("creating config directory {}", parent.display()))?;
    let stem = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.yaml".to_string());
    let tmp = parent.join(format!(".{stem}.tmp.{}", std::process::id()));
    let mut f = std::fs::File::create(&tmp)
        .with_context(|| format!("creating temp file {}", tmp.display()))?;
    f.write_all(bytes)
        .with_context(|| format!("writing temp file {}", tmp.display()))?;
    f.sync_all()
        .with_context(|| format!("fsync temp file {}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))?;
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
