/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl iam ...` command surface (Group 8).
//!
//! Subcommand tree: `whoami`, `user` (read verbs `list`/`get`/`export`, the
//! explicit `delete`, and `set-password`), and a `group` stub (out of scope —
//! tracked as a follow-up change). Declarative user mutation moved to the
//! top-level `onmsctl apply -f` (kind `User`); the imperative `iam apply`,
//! `user create`, `user update`, and `user role add`/`remove` verbs were
//! removed. [`IamCmd`] implements [`Classify`] so the binary refuses writes
//! under `--read-only`.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand};
use onmsctl_core::{
    Classify, CmdKind, Context, Error, OnmsClient, Result, render_list, render_one,
};

use crate::api::IamApi;
use crate::model::local::{FromEnvRef, FromFileRef, FromKeyringRef, KeyringRef, PasswordRef};
use crate::model::wire::OnmsUserWire;
use crate::render::UserRow;
use crate::secret::{SecretString, resolve_password_ref};

/// `onmsctl iam` — manage Horizon users and roles.
#[derive(Subcommand, Debug)]
pub enum IamCmd {
    /// Print the calling user (`GET /users/whoami`).
    Whoami,
    /// Manage users (read verbs, explicit delete, password, export).
    #[command(subcommand)]
    User(UserCmd),
    /// Manage groups — not implemented yet (tracked in a follow-up change).
    #[command(subcommand)]
    Group(GroupCmd),
}

#[derive(Subcommand, Debug)]
pub enum UserCmd {
    /// List all users.
    List,
    /// Show one user.
    Get {
        /// Username.
        name: String,
    },
    /// Delete a user.
    Delete {
        /// Username.
        name: String,
        /// Skip the interactive confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Rotate a user's password.
    SetPassword(SetPasswordArgs),
    /// Export users as declarative YAML to stdout.
    Export {
        /// Export only this user (default: all users).
        #[arg(long)]
        name: Option<String>,
    },
}

/// Password-source flags for `set-password`. Exactly one
/// of `--from-file` / `--from-env` / `--from-keyring` / `--password-stdin`
/// may be given (clap enforces mutual exclusion via the `pwsrc` group).
#[derive(Args, Debug, Default)]
pub struct PasswordSource {
    /// Read the password from a local file (mode-checked, ≤4 KiB).
    #[arg(long, group = "pwsrc")]
    from_file: Option<PathBuf>,
    /// Read the password from an environment variable.
    #[arg(long, group = "pwsrc")]
    from_env: Option<String>,
    /// Read the password from the OS keyring, as `<service>/<account>`.
    #[arg(long, group = "pwsrc")]
    from_keyring: Option<String>,
    /// Read the password from stdin (one line).
    #[arg(long, group = "pwsrc")]
    password_stdin: bool,
}

#[derive(Args, Debug)]
pub struct SetPasswordArgs {
    /// Username.
    name: String,
    #[command(flatten)]
    password: PasswordSource,
}

/// Placeholder for the `/groups` surface. Out of scope for this change.
#[derive(Subcommand, Debug)]
pub enum GroupCmd {
    /// List groups (not implemented).
    List,
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

impl Classify for IamCmd {
    fn kind(&self) -> CmdKind {
        match self {
            IamCmd::Whoami => CmdKind::Read,
            IamCmd::User(c) => c.kind(),
            IamCmd::Group(_) => CmdKind::Read, // stub never writes
        }
    }
}

impl Classify for UserCmd {
    fn kind(&self) -> CmdKind {
        match self {
            UserCmd::List | UserCmd::Get { .. } | UserCmd::Export { .. } => CmdKind::Read,
            UserCmd::Delete { .. } | UserCmd::SetPassword(_) => CmdKind::Write,
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

impl IamCmd {
    /// Dispatch the parsed verb against a resolved [`Context`].
    pub async fn run(self, ctx: &Context) -> Result<()> {
        match self {
            IamCmd::Whoami => run_whoami(ctx).await,
            IamCmd::User(cmd) => cmd.run(ctx).await,
            IamCmd::Group(_) => Err(Error::Config(
                "`iam group` is not implemented yet; tracked in a follow-up change".into(),
            )),
        }
    }
}

impl UserCmd {
    async fn run(self, ctx: &Context) -> Result<()> {
        match self {
            UserCmd::List => run_user_list(ctx).await,
            UserCmd::Get { name } => run_user_get(&name, ctx).await,
            UserCmd::Delete { name, yes } => run_user_delete(&name, yes, ctx).await,
            UserCmd::SetPassword(args) => run_set_password(args, ctx).await,
            UserCmd::Export { name } => run_user_export(name, ctx).await,
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn run_whoami(ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    match api.get_whoami().await? {
        Some(u) => {
            print_stdout(&format!("{}\n", u.user_id));
            Ok(())
        }
        None => Err(Error::IamWhoamiUnavailable),
    }
}

async fn run_user_list(ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    let list = api.list_users().await?;
    let rows: Vec<UserRow> = list.users.iter().map(UserRow::from).collect();
    print_stdout(&render_list(&rows, ctx.output_format)?);
    Ok(())
}

async fn run_user_get(name: &str, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    match api.get_user(name).await? {
        Some(u) => {
            let row = UserRow::from(&u);
            print_stdout(&render_one(&row, ctx.output_format)?);
            Ok(())
        }
        None => Err(Error::UserNotFound { name: name.into() }),
    }
}

async fn run_user_delete(name: &str, yes: bool, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    // Pre-confirm GET so the prompt names the blast radius (roles).
    if !yes {
        let blast = match api.get_user(name).await? {
            Some(u) => format!(
                "About to delete user '{name}' (roles: [{}]). This cannot be undone.",
                u.roles.join(", ")
            ),
            None => {
                eprintln!("note: user '{name}' not present (pre-confirm GET 404); nothing to do.");
                return Ok(());
            }
        };
        confirm_with_message(&blast)?;
    }
    // Idempotent delete: a 404 means the user is already absent. The non-`--yes`
    // path short-circuits on the pre-confirm GET; the `--yes` (automation) path
    // skips that GET, so tolerate the 404 here too rather than failing the run.
    match api.delete_user(name).await {
        Ok(()) => eprintln!("deleted user '{name}'"),
        Err(Error::HttpStatus { status: 404, .. }) => {
            eprintln!("note: user '{name}' not present (DELETE 404); nothing to do.");
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

async fn run_set_password(args: SetPasswordArgs, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    let secret = resolve_password(&args.password, "set-password")?;
    api.set_password(&args.name, secret.expose()).await?;
    eprintln!("password updated for '{}'", args.name);
    Ok(())
}

async fn run_user_export(name: Option<String>, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    let wires: Vec<OnmsUserWire> = match name {
        Some(n) => match api.get_user(&n).await? {
            Some(u) => vec![u],
            None => return Err(Error::UserNotFound { name: n }),
        },
        None => api.list_users().await?.users,
    };
    let mut out = String::new();
    for (i, w) in wires.iter().enumerate() {
        if i > 0 {
            out.push_str("---\n");
        }
        let local = crate::model::convert::wire_to_local(w);
        out.push_str(&serde_norway::to_string(&local)?);
    }
    print_stdout(&out);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the password from the selected source. The interactive no-echo
/// prompt (no flag on a TTY) is a planned follow-up; until then a source flag
/// or `--password-stdin` is required, and we refuse clearly.
fn resolve_password(src: &PasswordSource, verb: &str) -> Result<SecretString> {
    if let Some(p) = &src.from_file {
        return resolve_password_ref(&PasswordRef::FromFile(FromFileRef {
            from_file: p.clone(),
        }));
    }
    if let Some(e) = &src.from_env {
        return resolve_password_ref(&PasswordRef::FromEnv(FromEnvRef {
            from_env: e.clone(),
        }));
    }
    if let Some(k) = &src.from_keyring {
        let (service, account) = k.split_once('/').ok_or_else(|| {
            Error::Config(format!(
                "--from-keyring expects '<service>/<account>', got {k:?}"
            ))
        })?;
        return resolve_password_ref(&PasswordRef::FromKeyring(FromKeyringRef {
            from_keyring: KeyringRef {
                service: service.to_owned(),
                account: account.to_owned(),
            },
        }));
    }
    if src.password_stdin {
        // Refuse on a TTY: there is no no-echo handling here, so reading an
        // interactively-typed password would echo the secret to the screen.
        // `--password-stdin` is for piped/redirected input only.
        if std::io::stdin().is_terminal() {
            return Err(Error::Config(
                "--password-stdin reads piped input; it must not be a terminal (the secret would \
                 echo). Pipe the password in, e.g. `printf %s \"$PW\" | onmsctl ... --password-stdin`"
                    .into(),
            ));
        }
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| Error::Config(format!("reading password from stdin: {e}")))?;
        let pw = buf.trim_end_matches(['\n', '\r']).to_string();
        if pw.is_empty() {
            return Err(Error::Config("password from stdin is empty".into()));
        }
        // An internal newline cannot round-trip the form/XML wire paths (same
        // rule as the FromFile resolver) — refuse rather than fold it in.
        if pw.contains(['\n', '\r']) {
            return Err(Error::Config(
                "password from stdin contains an internal newline; provide a single-line password"
                    .into(),
            ));
        }
        return Ok(SecretString::new(pw));
    }
    Err(Error::Config(format!(
        "`iam user {verb}` needs a password source: --from-file, --from-env, \
         --from-keyring <service>/<account>, or --password-stdin (interactive no-echo \
         prompt is a planned enhancement)"
    )))
}

fn confirm_with_message(message: &str) -> Result<()> {
    use std::io::BufRead;
    let stdin_tty = std::io::stdin().is_terminal();
    let stderr_tty = std::io::stderr().is_terminal();
    if !(stdin_tty && stderr_tty) {
        return Err(Error::Config(format!(
            "{message}\nrefusing in a non-interactive context; re-run with --yes / -y to proceed"
        )));
    }
    eprint!("{message}\nType 'yes' or 'y' to confirm (case-insensitive): ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| Error::Config(format!("reading confirmation from stdin: {e}")))?;
    if read == 0 || !is_confirmation(&line) {
        return Err(Error::Config("cancelled by operator".into()));
    }
    Ok(())
}

/// Whether operator input counts as confirmation: `yes`/`y`, case-insensitive,
/// after trimming.
fn is_confirmation(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "yes" | "y")
}

fn print_stdout(s: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[derive(clap::Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: IamCmd,
    }

    fn parse(args: &[&str]) -> std::result::Result<IamCmd, clap::Error> {
        use clap::Parser;
        TestCli::try_parse_from(std::iter::once("onmsctl").chain(args.iter().copied()))
            .map(|c| c.cmd)
    }

    #[test]
    fn clap_tree_is_valid() {
        TestCli::command().debug_assert();
    }

    #[test]
    fn whoami_and_list_are_read() {
        assert_eq!(parse(&["whoami"]).unwrap().kind(), CmdKind::Read);
        assert_eq!(parse(&["user", "list"]).unwrap().kind(), CmdKind::Read);
        assert_eq!(
            parse(&["user", "get", "alice"]).unwrap().kind(),
            CmdKind::Read
        );
        assert_eq!(parse(&["user", "export"]).unwrap().kind(), CmdKind::Read);
    }

    #[test]
    fn mutating_user_verbs_are_write() {
        assert_eq!(
            parse(&["user", "delete", "alice"]).unwrap().kind(),
            CmdKind::Write
        );
        assert_eq!(
            parse(&["user", "set-password", "alice", "--from-env", "PW"])
                .unwrap()
                .kind(),
            CmdKind::Write
        );
    }

    #[test]
    fn delete_yes_is_plumbed() {
        match parse(&["user", "delete", "alice", "--yes"]).unwrap() {
            IamCmd::User(UserCmd::Delete { name, yes }) => {
                assert_eq!(name, "alice");
                assert!(yes);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn password_sources_are_mutually_exclusive() {
        // Two sources at once must be rejected by clap's group.
        assert!(
            parse(&[
                "user",
                "set-password",
                "alice",
                "--from-env",
                "A",
                "--from-file",
                "/x"
            ])
            .is_err()
        );
    }

    #[test]
    fn set_password_accepts_stdin_flag() {
        match parse(&["user", "set-password", "alice", "--password-stdin"]).unwrap() {
            IamCmd::User(UserCmd::SetPassword(a)) => {
                assert!(a.password.password_stdin);
                assert_eq!(a.name, "alice");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn is_confirmation_accepts_yes_and_y_case_insensitive() {
        assert!(is_confirmation("yes"));
        assert!(is_confirmation("YES\n"));
        assert!(is_confirmation(" y "));
        assert!(!is_confirmation("no"));
        assert!(!is_confirmation(""));
    }
}
