/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Configuration file loader.
//!
//! Loads the kubectl-style configuration described in the `cli-core` spec —
//! XDG-aware path resolution, named contexts, basic/bearer auth — and exposes
//! the parsed shape for [`crate::context::Context::resolve`] to consume.
//!
//! Field naming follows kebab-case on the wire (`current-context`,
//! `password-file`, `insecure-skip-tls-verify`) and snake_case in Rust.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

// -- Path resolution ---------------------------------------------------------

/// Default config-file location.
///
/// 1. `$ONMSCTL_CONFIG` (if set and non-empty), otherwise
/// 2. The platform's standard application-config directory, joined with
///    `config.yaml`. Per the [`directories`] crate's conventions this is:
///    - **Linux:** `$XDG_CONFIG_HOME/onmsctl/config.yaml`
///      (typically `~/.config/onmsctl/config.yaml`)
///    - **macOS:** `~/Library/Application Support/org.no42-org.onmsctl/config.yaml`
///    - **Windows:** `%APPDATA%\no42-org\onmsctl\config\config.yaml`
pub fn default_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("ONMSCTL_CONFIG")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    let dirs = directories::ProjectDirs::from("org", "no42-org", "onmsctl")
        .ok_or_else(|| Error::Config("unable to resolve config directory".into()))?;
    Ok(dirs.config_dir().join("config.yaml"))
}

/// Load and parse the config file at `path`. The caller is responsible for
/// resolving the path via [`default_path`] or an override.
pub fn load(path: &Path) -> Result<ConfigFile> {
    let bytes = std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::Config(format!(
            "config file not found at {} — create one with `onmsctl config use-context` or set $ONMSCTL_CONFIG",
            path.display()
        )),
        _ => Error::Io(e),
    })?;
    let cfg: ConfigFile = serde_norway::from_slice(&bytes)?;
    cfg.validate()?;
    Ok(cfg)
}

// -- File schema -------------------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_context: Option<String>,
    #[serde(default)]
    pub contexts: Vec<NamedContext>,
}

impl ConfigFile {
    pub fn find_context(&self, name: &str) -> Option<&NamedContext> {
        self.contexts.iter().find(|c| c.name == name)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for c in &self.contexts {
            if !seen.insert(c.name.as_str()) {
                return Err(Error::Config(format!(
                    "duplicate context name '{}'",
                    c.name
                )));
            }
            c.auth.validate(&c.name)?;
            c.iam.validate(&c.name)?;
        }
        if let Some(cur) = &self.current_context
            && self.find_context(cur).is_none()
        {
            return Err(Error::Config(format!(
                "current-context '{cur}' is not declared in contexts",
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct NamedContext {
    pub name: String,
    pub server: ServerSpec,
    pub auth: AuthSpec,
    /// When `true`, the CLI refuses any `WriteCmd` invocation against this
    /// context **before issuing any HTTP request**. Defense in depth on top
    /// of the server's own role checks. Defaults to `false`; existing
    /// configs without this field continue to parse.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// Per-context IAM capability settings (design §D8/§D13). Optional and
    /// additive — an absent `iam:` block parses as all-defaults.
    #[serde(default, skip_serializing_if = "IamConfig::is_empty")]
    pub iam: IamConfig,
}

/// Per-context IAM settings consumed by `onmsctl iam apply`. Every field is
/// optional and falls back to a built-in default applied by the command (the
/// `onmsctl-core` crate intentionally does not know the IAM defaults — those
/// live in `onmsctl-iam`). Listing a field here **replaces** the default, it
/// does not extend it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct IamConfig {
    /// Roles whose holder set `iam apply` refuses to empty (IAM-001).
    /// `None` → the built-in default `[ROLE_ADMIN]`; an explicit **empty**
    /// list disables the admin-lockout check entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected_roles: Option<Vec<String>>,
    /// Soft role-validation set (PR-IAM-006 unknown-role warnings). `None` →
    /// the built-in `KNOWN_ROLES`; an explicit list replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_roles: Option<Vec<String>>,
}

impl IamConfig {
    /// `true` when no IAM field is set, so the block serializes away rather
    /// than emitting an empty `iam: {}` in `config view`.
    fn is_empty(&self) -> bool {
        self.protected_roles.is_none() && self.known_roles.is_none()
    }

    fn validate(&self, ctx_name: &str) -> Result<()> {
        for (field, roles) in [
            ("protected-roles", &self.protected_roles),
            ("known-roles", &self.known_roles),
        ] {
            if let Some(roles) = roles
                && roles.iter().any(|r| r.trim().is_empty())
            {
                return Err(Error::Config(format!(
                    "context '{ctx_name}': iam.{field} contains an empty role string"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ServerSpec {
    pub url: String,
    #[serde(default)]
    pub insecure_skip_tls_verify: bool,
}

/// Auth declaration in the config file.
///
/// The kubectl-style YAML shape is `auth: { basic: { ... } }` or
/// `auth: { bearer: { ... } }`. Modeled as a struct with two optional fields
/// (rather than an externally-tagged enum) because `serde_norway` (and
/// `serde_yaml`-family YAML libraries generally) require the YAML `!Tag`
/// syntax for external tagging, which is unergonomic for kubectl-style
/// configs. Exactly one of `basic`/`bearer` must be declared; this is
/// enforced by [`Self::validate`] during config load.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AuthSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<BasicSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer: Option<BearerSpec>,
}

impl AuthSpec {
    fn validate(&self, ctx_name: &str) -> Result<()> {
        match (self.basic.is_some(), self.bearer.is_some()) {
            (true, true) => Err(Error::Config(format!(
                "context '{ctx_name}': auth declares both `basic` and `bearer`; declare exactly one"
            ))),
            (false, false) => Err(Error::Config(format!(
                "context '{ctx_name}': auth declares neither `basic` nor `bearer`; declare exactly one"
            ))),
            _ => {
                if let Some(b) = &self.basic {
                    b.validate(ctx_name)?;
                }
                if let Some(b) = &self.bearer {
                    b.validate(ctx_name)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BasicSpec {
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyring: Option<KeyringRef>,
}

impl BasicSpec {
    fn validate(&self, ctx_name: &str) -> Result<()> {
        if self.username.trim().is_empty() {
            return Err(Error::Config(format!(
                "context '{ctx_name}': basic auth username is empty"
            )));
        }
        if let Some(kr) = &self.keyring {
            kr.validate(ctx_name)?;
        }
        // It is legal to declare none of password/password-file/keyring — the
        // password is then expected via $ONMS_PASSWORD at request time.
        // It is illegal to declare more than one source, since precedence is
        // implicit and ambiguous declarations confuse contributors.
        let n = [
            self.password.is_some(),
            self.password_file.is_some(),
            self.keyring.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if n > 1 {
            return Err(Error::Config(format!(
                "context '{ctx_name}': basic auth declares more than one secret source (password / password-file / keyring); declare exactly one or none (and use $ONMS_PASSWORD)"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BearerSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyring: Option<KeyringRef>,
}

impl BearerSpec {
    fn validate(&self, ctx_name: &str) -> Result<()> {
        if let Some(kr) = &self.keyring {
            kr.validate(ctx_name)?;
        }
        let n = [
            self.token.is_some(),
            self.token_file.is_some(),
            self.keyring.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if n > 1 {
            return Err(Error::Config(format!(
                "context '{ctx_name}': bearer auth declares more than one secret source (token / token-file / keyring); declare exactly one or none (and use $ONMS_TOKEN)"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct KeyringRef {
    pub service: String,
    pub account: String,
}

impl KeyringRef {
    fn validate(&self, ctx_name: &str) -> Result<()> {
        if self.service.trim().is_empty() {
            return Err(Error::Config(format!(
                "context '{ctx_name}': keyring service is empty"
            )));
        }
        if self.account.trim().is_empty() {
            return Err(Error::Config(format!(
                "context '{ctx_name}': keyring account is empty"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_config(yaml: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(yaml.as_bytes()).unwrap();
        f
    }

    #[test]
    fn parses_minimal_basic_context() {
        let yaml = r#"
current-context: dev
contexts:
  - name: dev
    server:
      url: https://horizon.dev.lab/opennms
    auth:
      basic:
        username: admin
        password-file: /run/secrets/dev-onms
"#;
        let f = write_config(yaml);
        let cfg = load(f.path()).unwrap();
        assert_eq!(cfg.current_context.as_deref(), Some("dev"));
        let ctx = cfg.find_context("dev").unwrap();
        assert_eq!(ctx.server.url, "https://horizon.dev.lab/opennms");
        assert!(!ctx.server.insecure_skip_tls_verify);
        let b = ctx.auth.basic.as_ref().expect("expected basic auth");
        assert_eq!(b.username, "admin");
        assert_eq!(
            b.password_file.as_deref(),
            Some(Path::new("/run/secrets/dev-onms"))
        );
        assert!(b.password.is_none() && b.keyring.is_none());
        assert!(ctx.auth.bearer.is_none());
    }

    #[test]
    fn parses_bearer_with_keyring_ref() {
        let yaml = r#"
current-context: prod
contexts:
  - name: prod
    server:
      url: https://prod.example.com/opennms
      insecure-skip-tls-verify: false
    auth:
      bearer:
        keyring:
          service: onmsctl
          account: prod
"#;
        let f = write_config(yaml);
        let cfg = load(f.path()).unwrap();
        let ctx = cfg.find_context("prod").unwrap();
        let b = ctx.auth.bearer.as_ref().expect("expected bearer auth");
        let kr = b.keyring.as_ref().expect("expected keyring ref");
        assert_eq!(kr.service, "onmsctl");
        assert_eq!(kr.account, "prod");
        assert!(ctx.auth.basic.is_none());
    }

    #[test]
    fn rejects_auth_with_both_basic_and_bearer() {
        let yaml = r#"
contexts:
  - name: dev
    server: { url: "http://a" }
    auth:
      basic: { username: u, password: p }
      bearer: { token: t }
"#;
        let f = write_config(yaml);
        let err = load(f.path()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("both"));
                assert!(m.contains("'dev'"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_auth_with_neither_basic_nor_bearer() {
        let yaml = r#"
contexts:
  - name: dev
    server: { url: "http://a" }
    auth: {}
"#;
        let f = write_config(yaml);
        let err = load(f.path()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("neither"));
                assert!(m.contains("'dev'"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r#"
current-context: dev
typo-field: 7
contexts: []
"#;
        let f = write_config(yaml);
        let err = load(f.path()).unwrap_err();
        assert!(matches!(err, Error::Yaml(_)));
    }

    #[test]
    fn rejects_duplicate_context_name() {
        let yaml = r#"
contexts:
  - name: dev
    server: { url: "http://a" }
    auth: { basic: { username: u } }
  - name: dev
    server: { url: "http://b" }
    auth: { basic: { username: u } }
"#;
        let f = write_config(yaml);
        let err = load(f.path()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("duplicate")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_current_context_not_in_contexts() {
        let yaml = r#"
current-context: missing
contexts:
  - name: dev
    server: { url: "http://a" }
    auth: { basic: { username: u } }
"#;
        let f = write_config(yaml);
        let err = load(f.path()).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("missing")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_more_than_one_basic_secret_source() {
        let yaml = r#"
contexts:
  - name: dev
    server: { url: "http://a" }
    auth:
      basic:
        username: u
        password: p
        password-file: /tmp/x
"#;
        let f = write_config(yaml);
        let err = load(f.path()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("more than one"));
                assert!(m.contains("'dev'"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_per_context_iam_block() {
        let yaml = r#"
current-context: prod
contexts:
  - name: prod
    server:
      url: https://prod.example.com/opennms
    auth:
      basic:
        username: admin
        password: p
    iam:
      protected-roles: [ROLE_ADMIN, ROLE_REST]
      known-roles: [ROLE_ADMIN, ROLE_USER]
"#;
        let f = write_config(yaml);
        let cfg = load(f.path()).unwrap();
        let ctx = cfg.find_context("prod").unwrap();
        assert_eq!(
            ctx.iam.protected_roles.as_deref(),
            Some(["ROLE_ADMIN".to_string(), "ROLE_REST".to_string()].as_slice())
        );
        assert_eq!(
            ctx.iam.known_roles.as_deref(),
            Some(["ROLE_ADMIN".to_string(), "ROLE_USER".to_string()].as_slice())
        );
    }

    #[test]
    fn absent_iam_block_defaults_to_empty() {
        // A context without an `iam:` block parses fine (back-compat) and
        // leaves both fields unset so the command applies its built-in
        // defaults.
        let cfg = cfg_with_basic_plaintext_named();
        let ctx = cfg.find_context("dev").unwrap();
        assert!(ctx.iam.protected_roles.is_none());
        assert!(ctx.iam.known_roles.is_none());
        assert!(ctx.iam.is_empty());
    }

    #[test]
    fn rejects_empty_iam_role_string() {
        let yaml = r#"
contexts:
  - name: dev
    server: { url: "http://a" }
    auth: { basic: { username: u, password: p } }
    iam:
      protected-roles: ["ROLE_ADMIN", ""]
"#;
        let f = write_config(yaml);
        let err = load(f.path()).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("iam.protected-roles"));
                assert!(m.contains("empty role"));
                assert!(m.contains("'dev'"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_field_in_iam_block() {
        let yaml = r#"
contexts:
  - name: dev
    server: { url: "http://a" }
    auth: { basic: { username: u, password: p } }
    iam:
      protectd-roles: [ROLE_ADMIN]
"#;
        let f = write_config(yaml);
        let err = load(f.path()).unwrap_err();
        assert!(matches!(err, Error::Yaml(_)));
    }

    fn cfg_with_basic_plaintext_named() -> ConfigFile {
        let yaml = r#"
current-context: dev
contexts:
  - name: dev
    server: { url: "http://a" }
    auth: { basic: { username: u, password: p } }
"#;
        serde_norway::from_str(yaml).unwrap()
    }

    #[test]
    fn missing_file_produces_actionable_error() {
        let path = std::env::temp_dir().join("onmsctl-does-not-exist-xyz.yaml");
        let _ = std::fs::remove_file(&path);
        let err = load(&path).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("not found"));
                assert!(m.contains("ONMSCTL_CONFIG"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn default_path_honours_onmsctl_config_env_var() {
        let prev = std::env::var("ONMSCTL_CONFIG").ok();
        // SAFETY: env mutation in tests is generally racy across threads;
        // we restore the previous value at the end.
        // Tests within a single binary share the env, so we do not run
        // these in parallel. cargo test does run multiple tests in
        // parallel by default, so we use a unique override path and
        // tolerate that other tests might briefly observe it. The
        // assertion only checks our override.
        unsafe {
            std::env::set_var("ONMSCTL_CONFIG", "/tmp/explicit-onmsctl.yaml");
        }
        let p = default_path().unwrap();
        assert_eq!(p, PathBuf::from("/tmp/explicit-onmsctl.yaml"));
        match prev {
            Some(v) => unsafe { std::env::set_var("ONMSCTL_CONFIG", v) },
            None => unsafe { std::env::remove_var("ONMSCTL_CONFIG") },
        }
    }
}
