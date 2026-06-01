/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl iam ...` command surface (Group 8).
//!
//! Subcommand tree: `whoami`, `apply` (declarative), `user` (imperative +
//! `role`, `set-password`, `export`), and a `group` stub (out of scope —
//! tracked as a follow-up change). [`IamCmd`] implements [`Classify`] so the
//! binary refuses writes under `--read-only`; per design §D8 (F16) an
//! `apply --dry-run` classifies as **Read** so review workflows run in
//! read-only contexts — a deliberate divergence from provisioning's
//! "apply is always Write" choice.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand};
use onmsctl_core::{
    Classify, CmdKind, Context, Error, OnmsClient, Result, render_list, render_one,
};

use crate::api::IamApi;
use crate::apply::multi::{ApplyOptions, ApplyState, UserResult, apply_users};
use crate::model::local::{
    FromEnvRef, FromFileRef, FromKeyringRef, KNOWN_ROLES, KeyringRef, PasswordRef, UserLocal,
};
use crate::model::wire::OnmsUserWire;
use crate::render::{UserRow, render_apply_report};
use crate::secret::{SecretString, resolve_password_ref};

/// `onmsctl iam` — manage Horizon users and roles.
#[derive(Subcommand, Debug)]
pub enum IamCmd {
    /// Print the calling user (`GET /users/whoami`).
    Whoami,
    /// Reconcile declared users from YAML against the server (declarative).
    Apply(ApplyArgs),
    /// Manage users (imperative verbs, roles, password, export).
    #[command(subcommand)]
    User(UserCmd),
    /// Manage groups — not implemented yet (tracked in a follow-up change).
    #[command(subcommand)]
    Group(GroupCmd),
}

/// Arguments for `iam apply`. Mirrors `requisition apply`'s flag shape.
#[derive(Args, Debug)]
pub struct ApplyArgs {
    /// User YAML file(s) or directories (each `-f` may name a file or a dir
    /// of `*.yaml` / `*.yml`).
    #[arg(short = 'f', long = "file", required = true)]
    files: Vec<PathBuf>,
    /// Plan and render without issuing any write.
    #[arg(long)]
    dry_run: bool,
    /// Show the per-user planned actions (the per-user diff). Without this or
    /// `--dry-run`, apply prints only the one-line summary + findings.
    #[arg(long)]
    diff: bool,
    /// Continue past a per-user Phase-2 failure instead of stopping. Does NOT
    /// bypass plan-phase refusals (IAM-001/002, PR-IAM-002/003/005).
    #[arg(long)]
    keep_going: bool,
    /// Permit emptying a protected role's holder set (IAM-001). Requires
    /// `--yes`.
    #[arg(long, requires = "yes")]
    allow_admin_lockout: bool,
    /// Confirm destructive / override actions (required with
    /// `--allow-admin-lockout`).
    #[arg(short = 'y', long)]
    yes: bool,
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
    /// Create a user (imperative). Requires a password source.
    Create(CreateArgs),
    /// Update a user's scalar fields (imperative).
    Update(UpdateArgs),
    /// Delete a user.
    Delete {
        /// Username.
        name: String,
        /// Skip the interactive confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Add or remove a single role on a user.
    #[command(subcommand)]
    Role(UserRoleCmd),
    /// Rotate a user's password.
    SetPassword(SetPasswordArgs),
    /// Export users as declarative YAML to stdout.
    Export {
        /// Export only this user (default: all users).
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum UserRoleCmd {
    /// Grant a role.
    Add {
        /// Username.
        name: String,
        /// Role, e.g. `ROLE_USER`.
        role: String,
    },
    /// Revoke a role.
    Remove {
        /// Username.
        name: String,
        /// Role to revoke.
        role: String,
        /// Skip the interactive confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

/// Password-source flags shared by `create` and `set-password`. Exactly one
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
pub struct CreateArgs {
    /// Username.
    name: String,
    #[arg(long)]
    full_name: Option<String>,
    #[arg(long)]
    email: Option<String>,
    #[arg(long)]
    comments: Option<String>,
    /// Role to grant (repeatable).
    #[arg(long = "role")]
    roles: Vec<String>,
    #[arg(long)]
    duty_schedule: Option<String>,
    #[command(flatten)]
    password: PasswordSource,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Username.
    name: String,
    #[arg(long)]
    full_name: Option<String>,
    #[arg(long)]
    email: Option<String>,
    #[arg(long)]
    comments: Option<String>,
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
            // F16 / task 8.8: a dry-run apply issues only GETs, so read-only
            // contexts may run it for review. A real apply is Write.
            IamCmd::Apply(a) if a.dry_run => CmdKind::Read,
            IamCmd::Apply(_) => CmdKind::Write,
            IamCmd::User(c) => c.kind(),
            IamCmd::Group(_) => CmdKind::Read, // stub never writes
        }
    }
}

impl Classify for UserCmd {
    fn kind(&self) -> CmdKind {
        match self {
            UserCmd::List | UserCmd::Get { .. } | UserCmd::Export { .. } => CmdKind::Read,
            UserCmd::Create(_)
            | UserCmd::Update(_)
            | UserCmd::Delete { .. }
            | UserCmd::SetPassword(_) => CmdKind::Write,
            UserCmd::Role(c) => c.kind(),
        }
    }
}

impl Classify for UserRoleCmd {
    fn kind(&self) -> CmdKind {
        CmdKind::Write
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
            IamCmd::Apply(args) => run_apply(args, ctx).await,
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
            UserCmd::Create(args) => run_user_create(args, ctx).await,
            UserCmd::Update(args) => run_user_update(args, ctx).await,
            UserCmd::Delete { name, yes } => run_user_delete(&name, yes, ctx).await,
            UserCmd::Role(UserRoleCmd::Add { name, role }) => run_role_add(&name, &role, ctx).await,
            UserCmd::Role(UserRoleCmd::Remove { name, role, yes }) => {
                run_role_remove(&name, &role, yes, ctx).await
            }
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

async fn run_user_create(args: CreateArgs, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    let secret = resolve_password(&args.password, "create")?;
    for role in &args.roles {
        warn_if_unknown_role(role);
    }
    let spec = crate::model::local::UserSpec {
        full_name: args.full_name,
        email: args.email,
        comments: args.comments,
        duty_schedule: args.duty_schedule,
        roles: args.roles.into_iter().collect(),
        password_ref: None,
    };
    let local = build_local(&args.name, spec)?;
    api.post_user(&local, secret.expose()).await?;
    eprintln!("created user '{}'", args.name);
    Ok(())
}

async fn run_user_update(args: UpdateArgs, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    // Pre-flight existence so update never silently creates.
    api.require_user(&args.name).await?;
    let form = crate::model::wire::UpdateForm {
        full_name: args.full_name,
        email: args.email,
        comments: args.comments,
    };
    if form.is_empty() {
        eprintln!("nothing to update for '{}'", args.name);
        return Ok(());
    }
    api.put_user_form(&args.name, &form).await?;
    eprintln!("updated user '{}'", args.name);
    Ok(())
}

async fn run_role_add(name: &str, role: &str, ctx: &Context) -> Result<()> {
    if role.is_empty() {
        return Err(Error::Config("role must not be empty".into()));
    }
    warn_if_unknown_role(role);
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    api.put_user_role(name, role).await?;
    eprintln!("granted '{role}' to '{name}'");
    Ok(())
}

async fn run_role_remove(name: &str, role: &str, yes: bool, ctx: &Context) -> Result<()> {
    if role.is_empty() {
        return Err(Error::Config("role must not be empty".into()));
    }
    confirm_destructive(yes, &format!("revoke role '{role}' from user '{name}'"))?;
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);
    api.delete_user_role(name, role).await?;
    eprintln!("revoked '{role}' from '{name}'");
    Ok(())
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

async fn run_apply(args: ApplyArgs, ctx: &Context) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = IamApi::new(&client);

    let docs = load_documents(&args.files)?;
    if docs.is_empty() {
        return Err(Error::Config(
            "no user documents found in the given -f path(s)".into(),
        ));
    }

    let opts = ApplyOptions {
        dry_run: args.dry_run,
        keep_going: args.keep_going,
        known_roles: KNOWN_ROLES.iter().map(|s| s.to_string()).collect(),
        protected_roles: std::collections::BTreeSet::from(["ROLE_ADMIN".to_string()]),
        // The IAM-001 override needs both flags (task 7.3). clap's
        // `requires = "yes"` already rejects `--allow-admin-lockout` alone, so
        // this `&& yes` is belt-and-suspenders.
        allow_admin_lockout: args.allow_admin_lockout && args.yes,
    };

    let report = apply_users(&docs, &api, &opts).await?;
    // Per-user actions are the "diff": shown for `--dry-run` (review) or when
    // explicitly requested with `--diff`. A plain apply prints only the
    // summary line + findings. (spec.md §"`--diff` SHALL render per-user
    // diffs in the plan phase".)
    render_apply_report(&report, args.dry_run || args.diff);

    match report.state {
        ApplyState::AbortedInput => Err(Error::Config(
            "apply aborted: duplicate metadata.name across input documents (PR-IAM-002)".into(),
        )),
        _ => {
            let failed = report
                .users
                .iter()
                .filter(|u| matches!(u.result, UserResult::Failed | UserResult::PlanFailed))
                .count();
            if failed > 0 {
                Err(Error::PartialSuccess { failed })
            } else {
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a validated [`UserLocal`] from imperative parts by round-tripping
/// through YAML so the same parse-time guards (numeric/empty name, role
/// validation) apply as on the declarative path.
fn build_local(name: &str, spec: crate::model::local::UserSpec) -> Result<UserLocal> {
    let doc = UserLocal {
        api_version: crate::model::local::ApiVersion,
        kind: crate::model::local::KindUser,
        metadata: crate::model::local::Metadata {
            name: name.to_owned(),
            unmodeled: None,
        },
        spec,
    };
    let yaml = serde_norway::to_string(&doc)?;
    Ok(serde_norway::from_str(&yaml)?)
}

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

/// Read every YAML document under the given file / directory paths into
/// `(path, UserLocal)` pairs. Directories contribute their `*.yaml` / `*.yml`
/// entries (non-recursive). One document per file.
fn load_documents(paths: &[PathBuf]) -> Result<Vec<(PathBuf, UserLocal)>> {
    let mut files: Vec<PathBuf> = Vec::new();
    for p in paths {
        if p.is_dir() {
            for entry in std::fs::read_dir(p)
                .map_err(|e| Error::Config(format!("reading directory {}: {e}", p.display())))?
            {
                let entry = entry.map_err(|e| Error::Config(format!("reading dir entry: {e}")))?;
                let path = entry.path();
                // Only regular files with a (case-insensitive) yaml/yml
                // extension — skips subdirectories (incl. one named `*.yaml`)
                // and tolerates `*.YAML` / `*.Yml`.
                let is_yaml = path.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                    x.eq_ignore_ascii_case("yaml") || x.eq_ignore_ascii_case("yml")
                });
                if is_yaml && path.is_file() {
                    files.push(path);
                }
            }
        } else {
            files.push(p.clone());
        }
    }
    files.sort();
    files.dedup();

    let mut docs = Vec::with_capacity(files.len());
    for path in files {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::Config(format!("reading {}: {e}", path.display())))?;
        let local: UserLocal = serde_norway::from_str(&text)
            .map_err(|e| Error::Config(format!("parsing {}: {e}", path.display())))?;
        docs.push((path, local));
    }
    Ok(docs)
}

/// Confirm a destructive imperative action. `--yes` skips; otherwise requires
/// an interactive TTY and a `yes`/`y` answer; non-TTY without `--yes` refuses.
fn confirm_destructive(yes: bool, action: &str) -> Result<()> {
    if yes {
        return Ok(());
    }
    confirm_with_message(&format!("About to {action}. This cannot be undone."))
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

/// Emit the `PR-IAM-006` soft-validation warning for a role outside the
/// built-in known set, so the imperative `create` / `role add` paths warn on
/// a typo just like the declarative `apply` planner does.
fn warn_if_unknown_role(role: &str) {
    if !KNOWN_ROLES.contains(&role) {
        eprintln!(
            "[PR-IAM-006] warning: role {role:?} is not in the known-roles set; applying anyway"
        );
    }
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
    fn apply_dry_run_is_read_other_apply_is_write() {
        let dry = parse(&["apply", "-f", "u.yaml", "--dry-run"]).unwrap();
        assert_eq!(dry.kind(), CmdKind::Read);
        let real = parse(&["apply", "-f", "u.yaml"]).unwrap();
        assert_eq!(real.kind(), CmdKind::Write);
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
            parse(&["user", "role", "add", "alice", "ROLE_USER"])
                .unwrap()
                .kind(),
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
    fn apply_allow_admin_lockout_and_keep_going_plumbed() {
        match parse(&[
            "apply",
            "-f",
            "u.yaml",
            "--allow-admin-lockout",
            "--yes",
            "--keep-going",
        ])
        .unwrap()
        {
            IamCmd::Apply(a) => {
                assert!(a.allow_admin_lockout);
                assert!(a.yes);
                assert!(a.keep_going);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn allow_admin_lockout_requires_yes() {
        // Passing the override without --yes is a parse error, not a silent
        // downgrade.
        assert!(parse(&["apply", "-f", "u.yaml", "--allow-admin-lockout"]).is_err());
        // With --yes it parses.
        assert!(parse(&["apply", "-f", "u.yaml", "--allow-admin-lockout", "--yes"]).is_ok());
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
