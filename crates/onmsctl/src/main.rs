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

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::builder::TypedValueParser as _;
use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand};
use onmsctl_core::{
    ApplyParams, Classify, CmdKind, Error, OutputFormat, Overrides, apply_documents,
    apply_input::resolve_apply_input,
    config::{ConfigFile, default_path as default_config_path, load as load_config},
    context::Context,
    kind::load_documents,
    render_list,
};

mod registry;

#[derive(Parser, Debug)]
#[command(
    name = "onmsctl",
    // `version` attribute intentionally omitted: clap's auto-generated
    // -V/--version handler emits only the binary's CARGO_PKG_VERSION and
    // ignores the linked capability list. We supply our own --version
    // flag below so `-V` and the `version` subcommand produce the same
    // output (tasks.md 6.3 spec).
    about = "Command-line interface for OpenNMS Horizon",
    long_about = "Command-line interface for OpenNMS Horizon. \
                  See `onmsctl <subcommand> --help` for per-command flags. \
                  Configuration: ~/.config/onmsctl/config.yaml \
                  (XDG; macOS uses ~/Library/Application Support/...).",
    disable_version_flag = true,
)]
struct Cli {
    /// Print binary version and linked capability list, then exit.
    #[arg(short = 'V', long = "version", global = true, action = ArgAction::SetTrue)]
    version_flag: bool,

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

    /// Refuse any `WriteCmd` invocation locally before issuing HTTP.
    /// Overrides the active context's `read-only` field (precedence:
    /// flag > env `ONMSCTL_READ_ONLY` > context > default `false`).
    /// Defense in depth on top of the server's role checks.
    #[arg(long, global = true)]
    read_only: bool,

    #[command(subcommand)]
    command: Option<TopCmd>,
}

/// Top-level resource verbs. Each capability registers its own subcommands
/// here; the binary stitches them together statically.
#[derive(Subcommand, Debug)]
enum TopCmd {
    /// Apply declarative resources from YAML — the canonical mutation entrypoint.
    ///
    /// Reads one or more YAML documents (a single file, a directory, or a glob),
    /// peeks each document's `kind`, and routes it to the registered handler.
    /// Documents apply in a static kind-precedence order through a
    /// plan → gate → execute flow: every document is planned first, the whole
    /// apply aborts before any mutation if any plan fails, then documents
    /// execute in order (stop-on-error by default; `--continue-on-error` to
    /// attempt all). Re-running the same input is the idempotent recovery path.
    Apply(ApplyArgs),
    /// Manage eventconf sources.
    #[command(subcommand, visible_alias = "src")]
    Source(onmsctl_eventconf::cmd::SourceCmd),
    /// Manage eventconf events.
    #[command(subcommand, visible_alias = "evt")]
    Event(onmsctl_eventconf::cmd::EventCmd),
    /// Manage provisioning requisitions (GitOps + lifecycle verbs).
    #[command(subcommand, visible_alias = "req")]
    Requisition(onmsctl_provisioning::RequisitionCmd),
    /// Manage users and roles (IAM).
    #[command(subcommand)]
    Iam(onmsctl_iam::IamCmd),
    /// Print the binary version and linked capability list.
    Version,
    /// Inspect or switch the active configuration.
    #[command(subcommand, visible_alias = "cfg")]
    Config(ConfigCmd),
    /// Generate a shell-completion script.
    ///
    /// Pipe stdout into your shell's completion-loading mechanism, e.g.
    /// `onmsctl completion bash > /etc/bash_completion.d/onmsctl` or
    /// `eval "$(onmsctl completion zsh)"`.
    ///
    /// The generated script targets the literal binary name `onmsctl`.
    /// If you've repackaged or symlinked the binary under a different
    /// name, post-process the output (e.g. `sed -e 's/onmsctl/<name>/g'`).
    Completion {
        /// Target shell.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    // Future capabilities (Node, Alarm, …) extend here.
}

/// Flags for the top-level `apply` command. Kept generic (kubectl-aligned):
/// per-kind knobs (e.g. IAM admin-lockout override) live in context config,
/// not here.
#[derive(Args, Debug)]
struct ApplyArgs {
    /// File, directory, or glob of YAML documents to apply.
    #[arg(short = 'f', long = "filename", value_name = "FILE|DIR|GLOB")]
    filename: PathBuf,

    /// Plan only: classify as a Read command and issue no mutating HTTP.
    #[arg(long)]
    dry_run: bool,

    /// Print each kind-bucket's rendered diff to stderr before reconciling.
    #[arg(long)]
    diff: bool,

    /// Attempt every document instead of halting after the first failure.
    #[arg(long, visible_alias = "keep-going")]
    continue_on_error: bool,

    /// Recurse into subdirectories when the input is a directory (off by default).
    #[arg(short = 'R', long)]
    recursive: bool,
}

#[derive(Subcommand, Debug, Clone)]
enum ConfigCmd {
    /// Print the loaded config with secrets redacted.
    ///
    /// Output is the parsed config re-rendered as YAML; comments and
    /// key ordering from the original file are not preserved. password
    /// / token strings are replaced with `<redacted>`; password-file /
    /// token-file / keyring references are left intact since they are
    /// pointers, not secrets.
    View,
    /// Switch the active context by writing `current-context` back to the
    /// config file. The named context must already exist.
    UseContext {
        /// Name of an existing context.
        name: String,
    },
}

impl Classify for ConfigCmd {
    fn kind(&self) -> CmdKind {
        // Config verbs never issue HTTP. UseContext writes to the local
        // config file but the read-only attribute is about server-side
        // mutation; local config switching is always allowed.
        CmdKind::Read
    }
}

/// Refuse a `WriteCmd` invocation locally when the active context is
/// read-only. Returns `Ok(())` for `Read` or non-read-only contexts.
fn refuse_if_read_only(ctx: &Context, kind: CmdKind) -> Result<(), Error> {
    if ctx.read_only && kind == CmdKind::Write {
        return Err(Error::ReadOnlyRefused {
            context: ctx.name.clone(),
        });
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
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
    // Handle -V / --version BEFORE subcommand resolution so the user can
    // run `onmsctl -V` without specifying a subcommand. Output matches
    // the `version` subcommand exactly (tasks.md 6.3).
    if cli.version_flag {
        return print_version();
    }

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
        // `--read-only` is one-way: passing the flag forces read-only.
        // Omission defers to env / context per precedence.
        read_only: if cli.read_only { Some(true) } else { None },
    };
    let merged = env.with_flags(flags);

    // No subcommand specified — print help and return cleanly.
    let Some(command) = cli.command else {
        let mut cmd = Cli::command();
        cmd.print_help().ok();
        // Trailing newline so the next shell prompt isn't glued to help.
        println!();
        return Ok(());
    };

    // 2. Dispatch. version / completion / config don't need a resolved
    //    Context; capability commands do.
    match command {
        TopCmd::Apply(args) => run_apply(args, &merged).await,
        TopCmd::Version => print_version(),
        TopCmd::Completion { shell } => print_completion(shell),
        TopCmd::Config(cmd) => run_config(cmd, &merged).await,
        TopCmd::Source(cmd) => {
            // Pure-local verbs (`convert`) issue no HTTP — run them without
            // resolving a Context, so an absent config or a locked/unreachable
            // keyring can't block an offline transform (SMOKE-002).
            if cmd.is_local_only() {
                cmd.run_local().await?;
            } else {
                let ctx = resolve_context(&merged)?;
                refuse_if_read_only(&ctx, cmd.kind())?;
                cmd.run(&ctx).await?;
            }
            Ok(())
        }
        TopCmd::Event(cmd) => {
            let ctx = resolve_context(&merged)?;
            refuse_if_read_only(&ctx, cmd.kind())?;
            cmd.run(&ctx).await?;
            Ok(())
        }
        TopCmd::Requisition(cmd) => {
            // Pure-local verbs (`convert`) issue no HTTP — run them without
            // resolving a Context, so an absent config or a locked/unreachable
            // keyring can't block an offline transform (SMOKE-002).
            if cmd.is_local_only() {
                cmd.run_local().await?;
            } else {
                let ctx = resolve_context(&merged)?;
                refuse_if_read_only(&ctx, cmd.kind())?;
                cmd.run(&ctx).await?;
            }
            Ok(())
        }
        TopCmd::Iam(cmd) => {
            let ctx = resolve_context(&merged)?;
            refuse_if_read_only(&ctx, cmd.kind())?;
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
    let s = format!(
        "onmsctl {}\ncapabilities:\n  - {} {}\n  - {} {}\n  - {} {}\n",
        env!("CARGO_PKG_VERSION"),
        onmsctl_eventconf::CAPABILITY_NAME,
        onmsctl_eventconf::VERSION,
        onmsctl_provisioning::CAPABILITY_NAME,
        onmsctl_provisioning::VERSION,
        onmsctl_iam::CAPABILITY_NAME,
        onmsctl_iam::VERSION,
    );
    write_stdout(s.as_bytes())
}

/// `onmsctl completion <shell>` — emits a completion script to stdout via
/// `clap_complete` so the script always matches the actual clap command tree.
fn print_completion(shell: clap_complete::Shell) -> Result<()> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, bin_name, &mut buf);
    write_stdout(&buf)
}

async fn run_config(cmd: ConfigCmd, merged: &Overrides) -> Result<()> {
    let path = config_path_from(merged)?;
    match cmd {
        ConfigCmd::View => {
            let mut cfg = load_config(&path)
                .with_context(|| format!("loading config from {}", path.display()))?;
            redact_secrets(&mut cfg);
            let yaml = serde_norway::to_string(&cfg).context("serializing config")?;
            write_stdout(yaml.as_bytes())
        }
        ConfigCmd::UseContext { name } => {
            // Trim and reject empty/whitespace-only names so users don't
            // get a confusing "context '   ' not found" with invisible
            // whitespace in the message.
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(anyhow::anyhow!(
                    "context name must not be empty or whitespace-only"
                ));
            }
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
            let yaml = serde_norway::to_string(&cfg).context("serializing config")?;
            write_atomic(&path, yaml.as_bytes())?;

            // Verify-by-reload: parse the bytes we just wrote so a future
            // serde-round-trip regression surfaces here, not on the next
            // capability invocation.
            load_config(&path)
                .with_context(|| format!("verifying rewritten config at {}", path.display()))?;

            eprintln!("switched to context '{name}'");
            Ok(())
        }
    }
}

/// `onmsctl apply -f <file|dir|glob>` — the single declarative entrypoint.
///
/// Resolves the input through the shared `apply_input` classifier, loads every
/// YAML document, and drives them through the core kind-router. The verb
/// classifies as `Read` under `--dry-run` (plan only, zero mutating HTTP) and
/// `Write` otherwise; the read-only gate is enforced here, before any HTTP.
async fn run_apply(args: ApplyArgs, merged: &Overrides) -> Result<()> {
    // YAML is the only document format `apply` accepts; both extensions match
    // the per-capability single/dir/glob resolvers used elsewhere.
    const YAML_EXTS: &[&str] = &["yaml", "yml"];

    let ctx = resolve_context(merged)?;

    // Classify + gate BEFORE touching the filesystem or the network: a Write
    // apply in a read-only context must be refused locally. `--dry-run` is a
    // Read and is always permitted.
    let cmd_kind = if args.dry_run {
        CmdKind::Read
    } else {
        CmdKind::Write
    };
    refuse_if_read_only(&ctx, cmd_kind)?;

    // `resolve_apply_input` lists directories non-recursively; `-R` rewrites a
    // directory into a `**/*` glob so the same classifier walks subdirectories
    // (the glob path filters by extension just like directory listing).
    let dispatch = if args.recursive && args.filename.is_dir() {
        let pattern = args.filename.join("**").join("*");
        resolve_apply_input(&pattern, YAML_EXTS)?
    } else {
        // `-R` only applies to a directory. Warn rather than silently no-op so
        // `-R -f file.yaml` / `-R -f 'glob/*'` don't masquerade as recursive.
        if args.recursive {
            eprintln!(
                "note: --recursive ignored — {} is not a directory",
                args.filename.display()
            );
        }
        resolve_apply_input(&args.filename, YAML_EXTS)?
    };

    let docs = load_documents(&dispatch)?;
    if docs.is_empty() {
        // An empty or comment-only input would otherwise be a silent no-op;
        // surface it as a usage error so a mistyped path or stripped file
        // doesn't masquerade as "applied nothing, all good".
        return Err(Error::Config(format!(
            "no YAML documents found in {} (empty or comment-only input)",
            args.filename.display()
        ))
        .into());
    }

    let registry = registry::build();
    let params = ApplyParams {
        dry_run: args.dry_run,
        show_diff: args.diff,
        continue_on_error: args.continue_on_error,
    };

    match apply_documents(&registry, docs, &params, &ctx).await {
        Ok(outcomes) => {
            let rendered = render_list(&outcomes, ctx.output_format)?;
            write_stdout(rendered.as_bytes())?;
            // Table/JSON renders carry no trailing newline; YAML already ends in
            // one. Append exactly one so the shell prompt isn't glued and YAML
            // output doesn't gain a spurious blank line.
            if !rendered.ends_with('\n') {
                write_stdout(b"\n")?;
            }
            // Exit 1 if any document failed — including a `Failed` preview row
            // under `--dry-run` (the status, not the phase, drives the code).
            let failed = outcomes.iter().filter(|o| o.status.is_failure()).count();
            if failed > 0 {
                return Err(Error::PartialSuccess { failed }.into());
            }
            Ok(())
        }
        // A plan-gate `Err` aborts the apply with nothing mutated. Map it to the
        // spec's exit code: a generic config-class gate (unknown kind, dup name)
        // is a document failure → exit 1, NOT a usage error; dedicated gate codes
        // (IAM lockout 13/14/15, transport faults) carry their own meaning and
        // propagate verbatim.
        Err(e) => Err(classify_gate_error(e).into()),
    }
}

/// Map a kind-router plan-gate error to its shell exit-code semantics. A
/// generic config-class gate rejection (`exit_code() == 2`) is re-wrapped as
/// [`Error::ApplyGate`] (exit 1) while preserving its message; any error with a
/// dedicated code is returned unchanged so IAM lockout (13/14/15) and transport
/// faults keep their stable codes.
///
/// CAVEAT: this keys solely on `exit_code() == 2`, so the router's internal
/// preview-integrity violation (an `Error::Config("internal: …")` raised when a
/// handler returns a mismatched preview count) is also demoted from 2 to 1 and
/// labelled `ApplyGate`. That path requires a miswired handler — unreachable in
/// shipped code — and the message still reads "internal: …". The principled fix
/// (a dedicated `Error::Internal` variant that also subsumes the handler
/// `downcast` sites) is tracked as deferred work, not done here.
fn classify_gate_error(e: Error) -> Error {
    if e.exit_code() == 2 {
        Error::ApplyGate(e.to_string())
    } else {
        e
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

/// Atomic config-file write with crash-safety and secret-mode preservation.
///
/// Steps:
///   1. Resolve `path` through any symlinks via `canonicalize` so the
///      upstream file is rewritten rather than replaced by a regular
///      file. (Falls back to the literal path if it doesn't exist yet.)
///   2. Stage the new bytes in a same-directory temp file created via
///      `tempfile::NamedTempFile::new_in` (O_EXCL + random suffix +
///      auto-cleanup-on-drop).
///   3. Apply the original file's permissions on Unix (defaulting to
///      0600 for fresh writes since the config can carry inline secrets).
///   4. `persist()` performs the atomic rename.
///   5. Fsync the parent directory so the rename survives a crash.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    // Step 1: follow symlinks so we write the upstream file.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("creating config directory {}", parent.display()))?;

    // Step 3a: capture the existing file's mode (default 0600 for new files).
    #[cfg(unix)]
    let original_mode: u32 = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&target)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600)
    };

    // Step 2: stage in a sibling temp file with O_EXCL + random suffix.
    let mut tmp = tempfile::NamedTempFile::new_in(&parent)
        .with_context(|| format!("creating temp file in {}", parent.display()))?;
    tmp.write_all(bytes)
        .with_context(|| "writing config bytes to temp file")?;
    tmp.as_file()
        .sync_all()
        .with_context(|| "fsync temp file")?;

    // Step 3b: set permissions BEFORE the rename so the new file
    // appears with the right mode atomically.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(original_mode))
            .with_context(|| {
                format!(
                    "setting permissions {:o} on {}",
                    original_mode,
                    tmp.path().display()
                )
            })?;
    }

    // Step 4: atomic rename.
    tmp.persist(&target)
        .map_err(|e| anyhow::anyhow!("renaming temp file to {}: {}", target.display(), e.error))?;

    // Step 5: fsync the parent dir so the directory entry survives a
    // crash. Skipped on non-Unix; std::fs::File::open on a directory
    // is not portable.
    #[cfg(unix)]
    {
        let dir = std::fs::File::open(&parent)
            .with_context(|| format!("opening {} for fsync", parent.display()))?;
        dir.sync_all()
            .with_context(|| format!("fsyncing {}", parent.display()))?;
    }

    Ok(())
}

/// Write `bytes` to stdout, treating `BrokenPipe` as a clean exit (e.g.
/// when the user pipes our output into `head -c N`). Other I/O errors
/// surface as `Error::Io` so the caller can propagate them through the
/// exit-code mapping.
fn write_stdout(bytes: &[u8]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(bytes) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(anyhow::Error::from(Error::Io(e))),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::config::{
        AuthSpec, BasicSpec, BearerSpec, ConfigFile, NamedContext, ServerSpec,
    };

    fn cfg_with_inline_password(pw: &str) -> ConfigFile {
        ConfigFile {
            current_context: Some("dev".into()),
            contexts: vec![NamedContext {
                name: "dev".into(),
                server: ServerSpec {
                    url: "https://h.example/opennms".into(),
                    insecure_skip_tls_verify: false,
                },
                auth: AuthSpec {
                    basic: Some(BasicSpec {
                        username: "admin".into(),
                        password: Some(pw.into()),
                        password_file: None,
                        keyring: None,
                    }),
                    bearer: None,
                },
                read_only: false,
                iam: Default::default(),
            }],
        }
    }

    fn cfg_with_inline_token(tok: &str) -> ConfigFile {
        ConfigFile {
            current_context: Some("prod".into()),
            contexts: vec![NamedContext {
                name: "prod".into(),
                server: ServerSpec {
                    url: "https://p.example/opennms".into(),
                    insecure_skip_tls_verify: false,
                },
                auth: AuthSpec {
                    basic: None,
                    bearer: Some(BearerSpec {
                        token: Some(tok.into()),
                        token_file: None,
                        keyring: None,
                    }),
                },
                read_only: false,
                iam: Default::default(),
            }],
        }
    }

    #[test]
    fn redact_secrets_replaces_inline_password() {
        let mut cfg = cfg_with_inline_password("supersecret");
        redact_secrets(&mut cfg);
        let pw = cfg.contexts[0]
            .auth
            .basic
            .as_ref()
            .and_then(|b| b.password.as_deref())
            .unwrap();
        assert_eq!(pw, "<redacted>");
    }

    #[test]
    fn redact_secrets_replaces_inline_token() {
        let mut cfg = cfg_with_inline_token("tok-zxcvbnm");
        redact_secrets(&mut cfg);
        let tok = cfg.contexts[0]
            .auth
            .bearer
            .as_ref()
            .and_then(|b| b.token.as_deref())
            .unwrap();
        assert_eq!(tok, "<redacted>");
    }

    #[test]
    fn redact_secrets_leaves_username_and_pointers_intact() {
        let mut cfg = ConfigFile {
            current_context: Some("dev".into()),
            contexts: vec![NamedContext {
                name: "dev".into(),
                server: ServerSpec {
                    url: "https://h.example/opennms".into(),
                    insecure_skip_tls_verify: false,
                },
                auth: AuthSpec {
                    basic: Some(BasicSpec {
                        username: "service-account".into(),
                        password: None,
                        password_file: Some(PathBuf::from("/run/secrets/onms")),
                        keyring: None,
                    }),
                    bearer: None,
                },
                read_only: false,
                iam: Default::default(),
            }],
        };
        redact_secrets(&mut cfg);
        let basic = cfg.contexts[0].auth.basic.as_ref().unwrap();
        assert_eq!(basic.username, "service-account");
        assert!(basic.password.is_none());
        assert_eq!(
            basic.password_file.as_deref(),
            Some(Path::new("/run/secrets/onms"))
        );
    }

    #[test]
    fn write_atomic_writes_bytes_to_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        write_atomic(&path, b"hello\n").unwrap();
        let got = std::fs::read_to_string(&path).unwrap();
        assert_eq!(got, "hello\n");
    }

    #[test]
    fn write_atomic_creates_parent_directory_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/sub/config.yaml");
        write_atomic(&path, b"x").unwrap();
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_existing_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, b"original").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&path, b"updated").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_defaults_to_0600_for_fresh_writes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brand-new.yaml");
        write_atomic(&path, b"x").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_symlink_to_upstream_target() {
        let dir = tempfile::tempdir().unwrap();
        let upstream = dir.path().join("upstream.yaml");
        let link = dir.path().join("config.yaml");
        std::fs::write(&upstream, b"original").unwrap();
        std::os::unix::fs::symlink(&upstream, &link).unwrap();
        write_atomic(&link, b"updated").unwrap();
        // Upstream content updated.
        assert_eq!(std::fs::read_to_string(&upstream).unwrap(), "updated");
        // Link still a symlink pointing to upstream.
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink());
    }
}
