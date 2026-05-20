/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resolved per-process context.
//!
//! Implements the precedence rule from `cli-core` spec
//! "override precedence" requirement:
//!
//! ```text
//!   flags > environment > active context > defaults
//! ```
//!
//! And the credential-resolution chain from design.md §4.4:
//!
//! ```text
//!   1. explicit env var (ONMS_PASSWORD / ONMS_TOKEN)
//!   2. keyring entry (if config references it)
//!   3. password-file or token-file
//!   4. plaintext literal in config (warned about, once per process)
//! ```

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::auth::AuthCreds;
use crate::config::{AuthSpec, BasicSpec, BearerSpec, ConfigFile, KeyringRef};
use crate::error::{Error, Result};
use crate::format::OutputFormat;

/// CLI-flag and environment-variable inputs to the resolver. Keeping flags
/// and env in a single shape makes the precedence rule explicit at the call
/// site.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    pub config_path: Option<PathBuf>,
    pub context_name: Option<String>,
    pub url: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
    pub insecure_tls: Option<bool>,
    pub output: Option<OutputFormat>,
    pub verbose: bool,
    /// `Some(true)` forces read-only regardless of context; `Some(false)`
    /// forces writable regardless of context; `None` defers to the
    /// resolved context's `read-only` field. Precedence: flag > env >
    /// context > default `false`.
    pub read_only: Option<bool>,
}

impl Overrides {
    /// Read the recognised environment variables. Empty-string values are
    /// treated as unset so e.g. `ONMS_PASSWORD=""` does not produce an
    /// empty-credential request that bypasses keyring/file sources.
    pub fn from_env() -> Self {
        Self {
            config_path: var_nonempty("ONMSCTL_CONFIG").map(PathBuf::from),
            context_name: var_nonempty("ONMSCTL_CONTEXT"),
            url: var_nonempty("ONMS_URL"),
            user: var_nonempty("ONMS_USER"),
            password: var_nonempty("ONMS_PASSWORD"),
            token: var_nonempty("ONMS_TOKEN"),
            insecure_tls: None, // env-side opt-out is intentionally absent
            output: None,
            verbose: false,
            read_only: env_bool("ONMSCTL_READ_ONLY"),
        }
    }

    /// Layer flags over env. Flag wins when both are set.
    pub fn with_flags(mut self, flags: Overrides) -> Self {
        if flags.config_path.is_some() {
            self.config_path = flags.config_path;
        }
        if flags.context_name.is_some() {
            self.context_name = flags.context_name;
        }
        if flags.url.is_some() {
            self.url = flags.url;
        }
        if flags.user.is_some() {
            self.user = flags.user;
        }
        if flags.password.is_some() {
            self.password = flags.password;
        }
        if flags.token.is_some() {
            self.token = flags.token;
        }
        if flags.insecure_tls.is_some() {
            self.insecure_tls = flags.insecure_tls;
        }
        if flags.output.is_some() {
            self.output = flags.output;
        }
        if flags.read_only.is_some() {
            self.read_only = flags.read_only;
        }
        self.verbose = self.verbose || flags.verbose;
        self
    }
}

/// Parse a boolean-shaped env var. Accepts `1`, `true`, `yes`, `on`
/// (case-insensitive) as `Some(true)`; `0`, `false`, `no`, `off` as
/// `Some(false)`; missing or empty as `None`. Anything else is treated as
/// unset (silent — the resolver falls back to the context's declared
/// value).
fn env_bool(key: &str) -> Option<bool> {
    let v = var_nonempty(key)?;
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Resolved per-process state. Carries everything an HTTP request needs and
/// the output preference, with no further config lookups required.
#[derive(Clone, Debug)]
pub struct Context {
    pub url: reqwest::Url,
    pub creds: AuthCreds,
    pub insecure_skip_tls_verify: bool,
    pub output_format: OutputFormat,
    pub verbose: bool,
    /// Refuse `WriteCmd` invocations against this context. Resolved with
    /// precedence: flag > env > context's `read-only` field > default
    /// `false`. The binary's dispatch layer checks this before running
    /// any classified [`crate::CmdKind::Write`] command and refuses
    /// locally without issuing HTTP.
    pub read_only: bool,
}

impl Context {
    /// Resolve a [`Context`] from a parsed [`ConfigFile`] and the merged
    /// [`Overrides`].
    pub fn resolve(file: &ConfigFile, overrides: &Overrides) -> Result<Self> {
        let active_name = overrides
            .context_name
            .clone()
            .or_else(|| file.current_context.clone())
            .ok_or(Error::NoContext)?;

        let active = file
            .find_context(&active_name)
            .ok_or_else(|| Error::UnknownContext(active_name.clone()))?;

        // -- URL --
        let raw_url = overrides
            .url
            .clone()
            .unwrap_or_else(|| active.server.url.clone());
        let url = reqwest::Url::parse(&raw_url)
            .map_err(|e| Error::Config(format!("invalid server URL '{raw_url}': {e}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Config(format!(
                "unsupported URL scheme '{}' in server URL '{raw_url}'; expected http or https",
                url.scheme()
            )));
        }

        // -- Insecure TLS --
        let insecure_skip_tls_verify = overrides
            .insecure_tls
            .unwrap_or(active.server.insecure_skip_tls_verify);

        // -- Auth --
        let creds = resolve_creds(&active.auth, overrides)?;

        // -- Read-only -- precedence: flag/env (Overrides) > context > default false.
        let read_only = overrides.read_only.unwrap_or(active.read_only);

        Ok(Context {
            url,
            creds,
            insecure_skip_tls_verify,
            output_format: overrides.output.unwrap_or_default(),
            verbose: overrides.verbose,
            read_only,
        })
    }
}

fn resolve_creds(spec: &AuthSpec, overrides: &Overrides) -> Result<AuthCreds> {
    if let Some(b) = &spec.basic {
        return resolve_basic(b, overrides);
    }
    if let Some(b) = &spec.bearer {
        return resolve_bearer(b, overrides);
    }
    // Defensive — config-load validation should have rejected this earlier.
    Err(Error::Config(
        "auth: declare exactly one of `basic` or `bearer`".into(),
    ))
}

fn resolve_basic(spec: &BasicSpec, overrides: &Overrides) -> Result<AuthCreds> {
    let username = overrides
        .user
        .clone()
        .unwrap_or_else(|| spec.username.clone());
    let password = if let Some(p) = overrides.password.clone() {
        p
    } else if let Some(kr) = &spec.keyring {
        read_keyring(kr).map_err(|e| {
            Error::Auth(format!(
                "basic auth: cannot read keyring (service='{}', account='{}'): {e}",
                kr.service, kr.account
            ))
        })?
    } else if let Some(path) = &spec.password_file {
        read_secret_file(path).map_err(|e| {
            Error::Auth(format!(
                "basic auth: failed to read password-file {}: {e}",
                path.display()
            ))
        })?
    } else if let Some(p) = spec.password.clone() {
        warn_plaintext_once();
        p
    } else {
        return Err(Error::Auth(
            "basic auth: no password source — set $ONMS_PASSWORD or declare password / password-file / keyring in the context"
                .into(),
        ));
    };
    Ok(AuthCreds::basic(username, password))
}

fn resolve_bearer(spec: &BearerSpec, overrides: &Overrides) -> Result<AuthCreds> {
    let token = if let Some(t) = overrides.token.clone() {
        t
    } else if let Some(kr) = &spec.keyring {
        read_keyring(kr).map_err(|e| {
            Error::Auth(format!(
                "bearer auth: cannot read keyring (service='{}', account='{}'): {e}",
                kr.service, kr.account
            ))
        })?
    } else if let Some(path) = &spec.token_file {
        read_secret_file(path).map_err(|e| {
            Error::Auth(format!(
                "bearer auth: failed to read token-file {}: {e}",
                path.display()
            ))
        })?
    } else if let Some(t) = spec.token.clone() {
        warn_plaintext_once();
        t
    } else {
        return Err(Error::Auth(
            "bearer auth: no token source — set $ONMS_TOKEN or declare token / token-file / keyring in the context"
                .into(),
        ));
    };
    Ok(AuthCreds::bearer(token))
}

/// Read an env var only if it is set AND non-empty. An empty string from
/// the shell otherwise becomes a valid-but-meaningless override
/// (e.g. `ONMS_PASSWORD=""` → empty Basic auth password).
fn var_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn read_secret_file(path: &std::path::Path) -> std::io::Result<String> {
    // Strip only trailing CR/LF — preserves any internal or trailing
    // whitespace that is part of the credential. `.trim()` would silently
    // change a credential whose intended form ends with a space.
    Ok(std::fs::read_to_string(path)?
        .trim_end_matches(['\n', '\r'])
        .to_string())
}

/// Read a keyring entry, returning the cleartext on success. Returns the
/// underlying error verbatim so the caller can include it in the user-facing
/// `Auth` error — operators need to distinguish "entry not found" from
/// "backend unavailable" (D-Bus down on a headless Linux box, keychain
/// locked, etc.).
fn read_keyring(r: &KeyringRef) -> Result<String> {
    let entry = keyring::Entry::new(&r.service, &r.account)
        .map_err(|e| Error::Auth(format!("keyring entry construction failed: {e}")))?;
    entry
        .get_password()
        .map_err(|e| Error::Auth(format!("keyring read failed: {e}")))
}

static PLAINTEXT_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

fn warn_plaintext_once() {
    if !PLAINTEXT_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "warning: plaintext password/token in config — consider keyring or password-file"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NamedContext, ServerSpec};
    use std::io::Write;

    fn basic_auth(username: &str, b: BasicSpec) -> AuthSpec {
        let _ = username; // BasicSpec carries username; kept here for API symmetry
        AuthSpec {
            basic: Some(b),
            bearer: None,
        }
    }

    fn bearer_auth(b: BearerSpec) -> AuthSpec {
        AuthSpec {
            basic: None,
            bearer: Some(b),
        }
    }

    fn cfg_with_basic_password_file(path: &std::path::Path) -> ConfigFile {
        ConfigFile {
            current_context: Some("dev".into()),
            contexts: vec![NamedContext {
                name: "dev".into(),
                server: ServerSpec {
                    url: "https://horizon.dev.lab/opennms".into(),
                    insecure_skip_tls_verify: false,
                },
                auth: basic_auth(
                    "admin",
                    BasicSpec {
                        username: "admin".into(),
                        password: None,
                        password_file: Some(path.to_path_buf()),
                        keyring: None,
                    },
                ),
                read_only: false,
            }],
        }
    }

    fn cfg_with_basic_plaintext(password: &str) -> ConfigFile {
        ConfigFile {
            current_context: Some("dev".into()),
            contexts: vec![NamedContext {
                name: "dev".into(),
                server: ServerSpec {
                    url: "https://horizon.dev.lab/opennms".into(),
                    insecure_skip_tls_verify: false,
                },
                auth: basic_auth(
                    "admin",
                    BasicSpec {
                        username: "admin".into(),
                        password: Some(password.into()),
                        password_file: None,
                        keyring: None,
                    },
                ),
                read_only: false,
            }],
        }
    }

    #[test]
    fn resolves_basic_with_password_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"secret-from-file\n").unwrap();
        let cfg = cfg_with_basic_password_file(f.path());
        let ctx = Context::resolve(&cfg, &Overrides::default()).unwrap();
        assert_eq!(ctx.url.as_str(), "https://horizon.dev.lab/opennms");
        assert_eq!(ctx.creds, AuthCreds::basic("admin", "secret-from-file"));
        assert_eq!(ctx.output_format, OutputFormat::Table);
    }

    #[test]
    fn env_password_overrides_password_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"file-secret").unwrap();
        let cfg = cfg_with_basic_password_file(f.path());
        let o = Overrides {
            password: Some("env-secret".into()),
            ..Default::default()
        };
        let ctx = Context::resolve(&cfg, &o).unwrap();
        assert_eq!(ctx.creds, AuthCreds::basic("admin", "env-secret"));
    }

    #[test]
    fn flag_url_overrides_context_url() {
        let cfg = cfg_with_basic_plaintext("p");
        let o = Overrides {
            url: Some("https://staging.lab/opennms".into()),
            ..Default::default()
        };
        let ctx = Context::resolve(&cfg, &o).unwrap();
        assert_eq!(ctx.url.as_str(), "https://staging.lab/opennms");
    }

    #[test]
    fn flag_user_overrides_context_username() {
        let cfg = cfg_with_basic_plaintext("p");
        let o = Overrides {
            user: Some("operator".into()),
            ..Default::default()
        };
        let ctx = Context::resolve(&cfg, &o).unwrap();
        match ctx.creds {
            AuthCreds::Basic { username, .. } => assert_eq!(username, "operator"),
            _ => panic!("expected basic"),
        }
    }

    #[test]
    fn flag_context_picks_named_context() {
        let cfg = ConfigFile {
            current_context: Some("dev".into()),
            contexts: vec![
                NamedContext {
                    name: "dev".into(),
                    server: ServerSpec {
                        url: "https://dev.lab/opennms".into(),
                        insecure_skip_tls_verify: false,
                    },
                    auth: basic_auth(
                        "u",
                        BasicSpec {
                            username: "u".into(),
                            password: Some("p".into()),
                            password_file: None,
                            keyring: None,
                        },
                    ),
                    read_only: false,
                },
                NamedContext {
                    name: "prod".into(),
                    server: ServerSpec {
                        url: "https://prod.example.com/opennms".into(),
                        insecure_skip_tls_verify: false,
                    },
                    auth: basic_auth(
                        "operator",
                        BasicSpec {
                            username: "operator".into(),
                            password: Some("p".into()),
                            password_file: None,
                            keyring: None,
                        },
                    ),
                    read_only: false,
                },
            ],
        };
        let o = Overrides {
            context_name: Some("prod".into()),
            ..Default::default()
        };
        let ctx = Context::resolve(&cfg, &o).unwrap();
        assert_eq!(ctx.url.as_str(), "https://prod.example.com/opennms");
    }

    #[test]
    fn no_context_resolved_yields_specific_error() {
        let cfg = ConfigFile {
            current_context: None,
            contexts: vec![],
        };
        let err = Context::resolve(&cfg, &Overrides::default()).unwrap_err();
        assert!(matches!(err, Error::NoContext));
    }

    #[test]
    fn unknown_context_yields_specific_error() {
        let cfg = ConfigFile {
            current_context: Some("dev".into()),
            contexts: vec![NamedContext {
                name: "dev".into(),
                server: ServerSpec {
                    url: "https://dev.lab/opennms".into(),
                    insecure_skip_tls_verify: false,
                },
                auth: basic_auth(
                    "u",
                    BasicSpec {
                        username: "u".into(),
                        password: Some("p".into()),
                        password_file: None,
                        keyring: None,
                    },
                ),
                read_only: false,
            }],
        };
        let o = Overrides {
            context_name: Some("staging".into()),
            ..Default::default()
        };
        let err = Context::resolve(&cfg, &o).unwrap_err();
        match err {
            Error::UnknownContext(n) => assert_eq!(n, "staging"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn bearer_token_file_is_resolved_at_request_time() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        // Trailing newline is stripped (typical for `echo > token-file`),
        // but internal/leading whitespace is preserved by design — see
        // `read_secret_file` in this module for the rationale.
        f.write_all(b"bearer-token-content\n").unwrap();
        let cfg = ConfigFile {
            current_context: Some("dev".into()),
            contexts: vec![NamedContext {
                name: "dev".into(),
                server: ServerSpec {
                    url: "https://dev.lab/opennms".into(),
                    insecure_skip_tls_verify: false,
                },
                auth: bearer_auth(BearerSpec {
                    token: None,
                    token_file: Some(f.path().to_path_buf()),
                    keyring: None,
                }),
                read_only: false,
            }],
        };
        let ctx = Context::resolve(&cfg, &Overrides::default()).unwrap();
        // trim() removes both leading/trailing whitespace and the trailing newline
        assert_eq!(ctx.creds, AuthCreds::bearer("bearer-token-content"));
    }

    #[test]
    fn invalid_url_in_override_produces_actionable_error() {
        let cfg = cfg_with_basic_plaintext("p");
        let o = Overrides {
            url: Some("not a url".into()),
            ..Default::default()
        };
        let err = Context::resolve(&cfg, &o).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("invalid server URL")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn output_format_default_is_table() {
        let cfg = cfg_with_basic_plaintext("p");
        let ctx = Context::resolve(&cfg, &Overrides::default()).unwrap();
        assert_eq!(ctx.output_format, OutputFormat::Table);
    }

    #[test]
    fn flag_output_format_overrides_default() {
        let cfg = cfg_with_basic_plaintext("p");
        let o = Overrides {
            output: Some(OutputFormat::Yaml),
            ..Default::default()
        };
        let ctx = Context::resolve(&cfg, &o).unwrap();
        assert_eq!(ctx.output_format, OutputFormat::Yaml);
    }
}
