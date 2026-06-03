/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Declarative apply driver for config-as-file capabilities.
//!
//! Per cli-core spec "ApplyTarget driver supporting --dry-run and --diff" —
//! each capability that wants `apply -f` ergonomics implements
//! [`ApplyTarget`]; this module owns the shared driver, the [`Outcome`]
//! enum, and the opaque [`Diff`] type that capability impls produce.
//!
//! The [`Diff`] type intentionally avoids prescribing a structured shape:
//! capability-specific diff algorithms (e.g. EventConf's UEI bucketing,
//! design.md §5.3) live where the data is understood, and this module
//! treats their output as opaque text for display.

use std::fmt;

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::context::Context;
use crate::error::Result;

/// Outcome of an [`run_apply`] call. Stable, observable, suitable for
/// reporting to operators or for branching in scripts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Resource did not exist; it was created.
    Created,
    /// Resource existed and differed from local; it was updated.
    Updated,
    /// Resource existed and was canonically identical to local; no
    /// HTTP mutation was issued.
    Unchanged,
    /// `--dry-run` and the resource did not exist on the server.
    WouldCreate,
    /// `--dry-run` and the resource existed but differed from local.
    WouldUpdate,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Outcome::Created => "created",
            Outcome::Updated => "updated",
            Outcome::Unchanged => "unchanged",
            Outcome::WouldCreate => "would-create",
            Outcome::WouldUpdate => "would-update",
        };
        f.write_str(s)
    }
}

/// Knobs for [`run_apply`]. Map straight onto `--dry-run` and `--diff` CLI
/// flags.
#[derive(Clone, Copy, Debug, Default)]
pub struct ApplyOptions {
    /// When set, no mutating HTTP calls are made; only `WouldCreate`,
    /// `WouldUpdate`, and `Unchanged` are reachable.
    pub dry_run: bool,
    /// When set, the structured diff is printed to stderr before applying
    /// (or in dry-run mode, before reporting `WouldUpdate`).
    pub show_diff: bool,
}

/// Opaque rendered diff. Capability impls construct one of these from their
/// domain-specific diff algorithm; the driver only knows how to ask whether
/// it is empty and how to print it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff(String);

impl Diff {
    pub fn empty() -> Self {
        Self(String::new())
    }
    pub fn from_text(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Diff {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Capabilities that support declarative `apply -f` implement this trait.
///
/// `Local` is the YAML shape the user authors. `Remote` is the server-state
/// shape used for change detection. Both are typically capability-defined
/// DTOs.
#[async_trait]
pub trait ApplyTarget: Sized {
    type Local: DeserializeOwned + Send;
    type Remote: Serialize + Send;

    /// Resource name extracted from a local document. Used for the
    /// fetch-by-name lookup that drives create-vs-update routing.
    fn name(local: &Self::Local) -> &str;

    /// Fetch the current server state for the named resource. `Ok(None)`
    /// means the resource does not exist.
    async fn fetch(name: &str, ctx: &Context) -> Result<Option<Self::Remote>>;

    /// Create the resource. Called only when [`Self::fetch`] returns
    /// `Ok(None)` and `dry_run` is false.
    async fn create(local: Self::Local, ctx: &Context) -> Result<()>;

    /// Update the resource. Called only when [`Self::fetch`] returns
    /// `Ok(Some(_))`, [`Self::diff`] reports a non-empty diff, and
    /// `dry_run` is false.
    async fn update(local: Self::Local, remote: Self::Remote, ctx: &Context) -> Result<()>;

    /// Compute the diff between local and remote shapes. An empty diff
    /// means the resource is canonically identical to local; no mutation
    /// will be issued. Capability impls choose the algorithm
    /// (see e.g. EventConf's UEI-bucket diff in design.md §5.3).
    fn diff(local: &Self::Local, remote: &Self::Remote) -> Diff;
}

/// Drive an apply against the given resource type. The flow is fixed:
/// fetch → classify → mutate (or dry-run-report).
pub async fn run_apply<T>(local: T::Local, opts: &ApplyOptions, ctx: &Context) -> Result<Outcome>
where
    T: ApplyTarget,
{
    let name = T::name(&local).to_string();
    let remote = T::fetch(&name, ctx).await?;
    match remote {
        None => {
            if opts.dry_run {
                return Ok(Outcome::WouldCreate);
            }
            T::create(local, ctx).await?;
            Ok(Outcome::Created)
        }
        Some(r) => {
            let diff = T::diff(&local, &r);
            if diff.is_empty() {
                return Ok(Outcome::Unchanged);
            }
            // Print the diff to stderr before any update — both for dry-run
            // (so the user sees what WOULD change) and for the real-update
            // path (so the user sees what IS about to change). Doc comment
            // on `show_diff` is the load-bearing contract here.
            if opts.show_diff {
                eprintln!("{diff}");
            }
            if opts.dry_run {
                return Ok(Outcome::WouldUpdate);
            }
            T::update(local, r, ctx).await?;
            Ok(Outcome::Updated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthCreds;
    use crate::format::OutputFormat;
    use std::sync::{Arc, Mutex};

    /// Thin fake target. Backed by a shared in-memory store so tests can
    /// pre-populate "remote state" and assert on calls.
    struct Fake;

    #[derive(Clone, Default)]
    struct Store {
        remote: Option<RemoteSrc>,
        creates: usize,
        updates: usize,
    }

    type SharedStore = Arc<Mutex<Store>>;

    // Local and Remote types — kept small. The thread_local store stand-in is
    // injected via a context-stash hack: we pass a SharedStore through the
    // Context's url path. Hack-y but keeps the test self-contained.
    #[derive(serde::Deserialize, Clone, Debug)]
    struct LocalSrc {
        name: String,
        body: String,
    }

    #[derive(serde::Serialize, Clone, Debug)]
    struct RemoteSrc {
        name: String,
        body: String,
    }

    // Smuggle a `SharedStore` through tests. The trait methods on `Fake` are
    // associated functions with no `self` and no test-fixture parameter, so
    // we route per-test state through a `thread_local!`. Tests are pinned to
    // the current-thread runtime via `#[tokio::test(flavor = "current_thread")]`
    // so awaits cannot resume on a different thread where the thread_local
    // is unset. (For real capabilities the "store" is the remote server,
    // accessed via `OnmsClient`; this dance is test-only.)
    thread_local! {
        static FAKE_STORE: std::cell::RefCell<Option<SharedStore>> = const { std::cell::RefCell::new(None) };
    }

    fn install_store(s: SharedStore) {
        FAKE_STORE.with(|cell| *cell.borrow_mut() = Some(s));
    }

    fn current_store() -> SharedStore {
        FAKE_STORE.with(|cell| cell.borrow().as_ref().expect("store installed").clone())
    }

    #[async_trait]
    impl ApplyTarget for Fake {
        type Local = LocalSrc;
        type Remote = RemoteSrc;

        fn name(local: &LocalSrc) -> &str {
            &local.name
        }

        async fn fetch(_name: &str, _ctx: &Context) -> Result<Option<RemoteSrc>> {
            Ok(current_store().lock().unwrap().remote.clone())
        }

        async fn create(local: LocalSrc, _ctx: &Context) -> Result<()> {
            let store = current_store();
            let mut s = store.lock().unwrap();
            s.remote = Some(RemoteSrc {
                name: local.name,
                body: local.body,
            });
            s.creates += 1;
            Ok(())
        }

        async fn update(local: LocalSrc, _remote: RemoteSrc, _ctx: &Context) -> Result<()> {
            let store = current_store();
            let mut s = store.lock().unwrap();
            s.remote = Some(RemoteSrc {
                name: local.name,
                body: local.body,
            });
            s.updates += 1;
            Ok(())
        }

        fn diff(local: &LocalSrc, remote: &RemoteSrc) -> Diff {
            if local.body == remote.body {
                Diff::empty()
            } else {
                Diff::from_text(format!("- {}\n+ {}", remote.body, local.body))
            }
        }
    }

    fn test_ctx() -> Context {
        Context {
            name: "test".into(),
            url: reqwest::Url::parse("http://unused/").unwrap(),
            creds: AuthCreds::bearer("t"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
            iam: Default::default(),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn create_path_when_remote_absent() {
        let store: SharedStore = Arc::new(Mutex::new(Store::default()));
        install_store(store.clone());
        let local = LocalSrc {
            name: "cisco.foo".into(),
            body: "v1".into(),
        };
        let outcome = run_apply::<Fake>(local, &ApplyOptions::default(), &test_ctx())
            .await
            .unwrap();
        assert_eq!(outcome, Outcome::Created);
        let s = store.lock().unwrap();
        assert_eq!(s.creates, 1);
        assert_eq!(s.updates, 0);
        assert!(s.remote.is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unchanged_when_remote_matches_local() {
        let store: SharedStore = Arc::new(Mutex::new(Store {
            remote: Some(RemoteSrc {
                name: "cisco.foo".into(),
                body: "v1".into(),
            }),
            ..Default::default()
        }));
        install_store(store.clone());
        let local = LocalSrc {
            name: "cisco.foo".into(),
            body: "v1".into(),
        };
        let outcome = run_apply::<Fake>(local, &ApplyOptions::default(), &test_ctx())
            .await
            .unwrap();
        assert_eq!(outcome, Outcome::Unchanged);
        let s = store.lock().unwrap();
        assert_eq!(s.creates, 0);
        assert_eq!(s.updates, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn update_path_when_remote_differs() {
        let store: SharedStore = Arc::new(Mutex::new(Store {
            remote: Some(RemoteSrc {
                name: "cisco.foo".into(),
                body: "v1".into(),
            }),
            ..Default::default()
        }));
        install_store(store.clone());
        let local = LocalSrc {
            name: "cisco.foo".into(),
            body: "v2".into(),
        };
        let outcome = run_apply::<Fake>(local, &ApplyOptions::default(), &test_ctx())
            .await
            .unwrap();
        assert_eq!(outcome, Outcome::Updated);
        let s = store.lock().unwrap();
        assert_eq!(s.updates, 1);
        assert_eq!(s.remote.as_ref().unwrap().body, "v2");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dry_run_does_not_mutate_create_path() {
        let store: SharedStore = Arc::new(Mutex::new(Store::default()));
        install_store(store.clone());
        let local = LocalSrc {
            name: "x".into(),
            body: "v1".into(),
        };
        let opts = ApplyOptions {
            dry_run: true,
            show_diff: false,
        };
        let outcome = run_apply::<Fake>(local, &opts, &test_ctx()).await.unwrap();
        assert_eq!(outcome, Outcome::WouldCreate);
        let s = store.lock().unwrap();
        assert_eq!(s.creates, 0);
        assert!(s.remote.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dry_run_does_not_mutate_update_path() {
        let store: SharedStore = Arc::new(Mutex::new(Store {
            remote: Some(RemoteSrc {
                name: "x".into(),
                body: "v1".into(),
            }),
            ..Default::default()
        }));
        install_store(store.clone());
        let local = LocalSrc {
            name: "x".into(),
            body: "v2".into(),
        };
        let opts = ApplyOptions {
            dry_run: true,
            show_diff: false,
        };
        let outcome = run_apply::<Fake>(local, &opts, &test_ctx()).await.unwrap();
        assert_eq!(outcome, Outcome::WouldUpdate);
        let s = store.lock().unwrap();
        assert_eq!(s.updates, 0);
        assert_eq!(s.remote.as_ref().unwrap().body, "v1");
    }

    #[test]
    fn diff_empty_check() {
        assert!(Diff::empty().is_empty());
        assert!(!Diff::from_text("changed").is_empty());
    }

    #[test]
    fn outcome_display_is_stable() {
        assert_eq!(Outcome::Created.to_string(), "created");
        assert_eq!(Outcome::WouldUpdate.to_string(), "would-update");
        assert_eq!(Outcome::Unchanged.to_string(), "unchanged");
    }
}
