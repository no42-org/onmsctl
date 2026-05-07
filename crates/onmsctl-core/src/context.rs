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
}

impl Overrides {
    /// Read the recognised environment variables.
    pub fn from_env() -> Self {
        Self {
            config_path: std::env::var("ONMSCTL_CONFIG").ok().map(PathBuf::from),
            context_name: std::env::var("ONMSCTL_CONTEXT").ok(),
            url: std::env::var("ONMS_URL").ok(),
            user: std::env::var("ONMS_USER").ok(),
            password: std::env::var("ONMS_PASSWORD").ok(),
            token: std::env::var("ONMS_TOKEN").ok(),
            insecure_tls: None, // env-side opt-out is intentionally absent
            output: None,
            verbose: false,
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
        self.verbose = self.verbose || flags.verbose;
        self
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

        // -- Insecure TLS --
        let insecure_skip_tls_verify = overrides
            .insecure_tls
            .unwrap_or(active.server.insecure_skip_tls_verify);

        // -- Auth --
        let creds = resolve_creds(&active.auth, overrides)?;

        Ok(Context {
            url,
            creds,
            insecure_skip_tls_verify,
            output_format: overrides.output.unwrap_or_default(),
            verbose: overrides.verbose,
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
        read_keyring(kr).ok_or_else(|| {
            Error::Auth(format!(
                "basic auth: keyring entry not available (service='{}', account='{}')",
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
        read_keyring(kr).ok_or_else(|| {
            Error::Auth(format!(
                "bearer auth: keyring entry not available (service='{}', account='{}')",
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

fn read_secret_file(path: &std::path::Path) -> std::io::Result<String> {
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

/// Best-effort keyring read. Returns `None` for any failure (entry not found,
/// keyring service unavailable, permission denied) so the resolver can fall
/// through. Keyring backends are platform-dependent (D-Bus on Linux, Keychain
/// on macOS, Credential Manager on Windows) and absent on most servers.
fn read_keyring(r: &KeyringRef) -> Option<String> {
    let entry = keyring::Entry::new(&r.service, &r.account).ok()?;
    entry.get_password().ok()
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
        f.write_all(b"  bearer-token-content  \n").unwrap();
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
