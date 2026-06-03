/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end integration tests for the `iam` capability against a live
//! Horizon instance (task 11.4). Each test is `#[ignore]`d so `make test`
//! is unaffected; `make integration` opts in via `--include-ignored`
//! `--test-threads=1` (serial — the lockout test relies on a stable
//! holder-set snapshot).
//!
//! Covers:
//! - imperative user lifecycle: create → list → get → role add → role
//!   remove → delete
//! - `passwordRef`-driven create + `set-password` rotation, verified by the
//!   change in the server-side stored password hash (not by authenticating as
//!   the new user — Horizon's basic-auth realm does not pick up a REST-created
//!   account's credentials immediately, so an auth probe would test the
//!   server's credential cache rather than onmsctl)
//! - the IAM-001 admin-lockout invariant tripping on an intentional bad
//!   apply — constructed **non-destructively**: a throwaway `onmsctl-it-`
//!   user is made the sole holder of an otherwise-unheld protected role, and
//!   the refusal happens before any write. The real admin is never the
//!   target, so this cannot lock out the lab even if the check regressed.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use onmsctl_core::Error;
use onmsctl_iam::api::IamApi;
use onmsctl_iam::apply::multi::{ApplyOptions, apply_users};
use onmsctl_iam::model::local::{FromFileRef, KNOWN_ROLES, PasswordRef, UserLocal};
use onmsctl_iam::resolve_password_ref;
use onmsctl_it::{Harness, harness_or_skip};

/// Initial password for IT-created users. Meets Horizon's "non-empty"
/// requirement; rotated to [`ROTATED_PASSWORD`] in the rotation test.
const IT_PASSWORD: &str = "onmsctl-it-s3cret-A1";
const ROTATED_PASSWORD: &str = "onmsctl-it-s3cret-B2";

/// Parse a `kind: User` YAML document through the real local-model
/// deserializer — the same path `iam apply -f` uses — so fixtures exercise
/// the honeypot / role-dedup / numeric-name guards rather than bypassing
/// them with hand-built structs.
fn parse_user(yaml: &str) -> UserLocal {
    serde_norway::from_str(yaml).expect("test fixture must parse as UserLocal")
}

/// Build a `kind: User` YAML doc for the given username and roles.
fn user_yaml(name: &str, roles: &[&str]) -> String {
    let roles_block: String = roles.iter().map(|r| format!("    - {r}\n")).collect();
    format!(
        "apiVersion: onmsctl.no42.org/v1alpha1\n\
         kind: User\n\
         metadata:\n  name: {name}\n\
         spec:\n  fullName: Integration Test\n  roles:\n{roles_block}"
    )
}

async fn pre_post_cleanup(h: &Harness, when: &str) {
    let n = h
        .cleanup_users()
        .await
        .unwrap_or_else(|e| panic!("{when} user cleanup failed: {e}"));
    if n > 0 {
        eprintln!("{when} cleanup: deleted {n} leftover user(s)");
    }
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn user_lifecycle_create_role_delete() {
    let h = harness_or_skip!();
    pre_post_cleanup(&h, "pre").await;
    let api = IamApi::new(h.client());
    let name = h.unique_name("life");

    // create with ROLE_USER
    let local = parse_user(&user_yaml(&name, &["ROLE_USER"]));
    api.post_user(&local, IT_PASSWORD)
        .await
        .expect("post_user create");

    // list — the user is present
    let list = api.list_users().await.expect("list_users");
    assert!(
        list.users.iter().any(|u| u.user_id == name),
        "created user '{name}' must appear in the list"
    );

    // get — holds ROLE_USER, not ROLE_PROVISION yet
    let got = api
        .get_user(&name)
        .await
        .expect("get_user")
        .expect("user present after create");
    assert!(
        got.roles.iter().any(|r| r == "ROLE_USER"),
        "create role set"
    );
    assert!(
        !got.roles.iter().any(|r| r == "ROLE_PROVISION"),
        "ROLE_PROVISION not yet granted"
    );

    // role add ROLE_PROVISION
    api.put_user_role(&name, "ROLE_PROVISION")
        .await
        .expect("put_user_role add");
    let got = api
        .get_user(&name)
        .await
        .expect("get_user")
        .expect("present");
    assert!(
        got.roles.iter().any(|r| r == "ROLE_PROVISION"),
        "ROLE_PROVISION granted"
    );

    // role remove ROLE_PROVISION
    api.delete_user_role(&name, "ROLE_PROVISION")
        .await
        .expect("delete_user_role remove");
    let got = api
        .get_user(&name)
        .await
        .expect("get_user")
        .expect("present");
    assert!(
        !got.roles.iter().any(|r| r == "ROLE_PROVISION"),
        "ROLE_PROVISION revoked"
    );

    // delete — user is gone
    api.delete_user(&name).await.expect("delete_user");
    assert!(
        api.get_user(&name)
            .await
            .expect("get after delete")
            .is_none(),
        "deleted user '{name}' must be absent"
    );

    pre_post_cleanup(&h, "post").await;
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn passwordref_create_then_set_password_rotate() {
    let h = harness_or_skip!();
    pre_post_cleanup(&h, "pre").await;
    let api = IamApi::new(h.client());
    let name = h.unique_name("pwrot");

    // A mode-0600 password file is the `passwordRef.fromFile` source. The
    // resolver enforces the mode/symlink/size guards before handing back the
    // secret, exactly as the apply create branch does.
    let mut pwfile = tempfile::NamedTempFile::new().expect("create temp pw file");
    write!(pwfile, "{IT_PASSWORD}").expect("write pw");
    pwfile.flush().expect("flush pw");
    std::fs::set_permissions(pwfile.path(), std::fs::Permissions::from_mode(0o600))
        .expect("chmod 600");

    let pref = PasswordRef::FromFile(FromFileRef {
        from_file: pwfile.path().to_path_buf(),
    });
    let secret = resolve_password_ref(&pref).expect("resolve passwordRef.fromFile");

    // Create with the resolved passwordRef secret.
    let local = parse_user(&user_yaml(&name, &["ROLE_USER"]));
    api.post_user(&local, secret.expose())
        .await
        .expect("post_user with resolved passwordRef");

    // Verify the rotation server-side via the stored password hash, NOT by
    // authenticating as the new user: Horizon's basic-auth realm does not
    // pick up a REST-created account's credentials immediately (observed:
    // 401 right after create), so an auth probe would test the server's
    // credential cache rather than onmsctl. The get/list response exposes the
    // hashed `password` (spike 0.3), which is a deterministic, environment-
    // independent witness that `set-password` actually changed the secret.
    let hash_after_create = stored_password_hash(&api, &name).await;
    assert!(
        hash_after_create.is_some(),
        "create via passwordRef must set a password hash"
    );

    // Rotate via set-password, then confirm the stored hash changed.
    api.set_password(&name, ROTATED_PASSWORD)
        .await
        .expect("set_password rotate");
    let hash_after_rotate = stored_password_hash(&api, &name).await;
    assert!(
        hash_after_rotate.is_some(),
        "rotated user must still have a password hash"
    );
    assert_ne!(
        hash_after_create, hash_after_rotate,
        "set-password must change the stored password hash"
    );

    api.delete_user(&name).await.expect("delete_user");
    pre_post_cleanup(&h, "post").await;
}

#[ignore = "live Horizon required (run via `make integration`)"]
#[tokio::test]
async fn admin_lockout_invariant_refuses_bad_apply() {
    let h = harness_or_skip!();
    pre_post_cleanup(&h, "pre").await;
    let api = IamApi::new(h.client());

    // Pick a KNOWN_ROLE currently held by NO server user. A single throwaway
    // user will become its sole holder, so demoting that user empties the
    // protected-role holder set and trips IAM-001 — without ever targeting
    // the real admin (which avoids any chance of a genuine lockout).
    let snapshot = api.list_users().await.expect("list_users (snapshot)");
    let held: BTreeSet<&str> = snapshot
        .users
        .iter()
        .flat_map(|u| u.roles.iter().map(String::as_str))
        .collect();
    // Exclude ROLE_USER — it is the baseline role every fixture carries, so
    // picking it as the protected role would produce a duplicate in the
    // create doc (and isn't a meaningful "admin-like" protected role anyway).
    let protected: &str = match KNOWN_ROLES
        .iter()
        .copied()
        .find(|r| *r != "ROLE_USER" && !held.contains(r))
    {
        Some(r) => r,
        None => {
            eprintln!("SKIP: no unheld KNOWN_ROLE available to isolate the lockout test");
            return;
        }
    };

    // Create the throwaway sole holder of `protected`.
    let name = h.unique_name("lock");
    let local = parse_user(&user_yaml(&name, &["ROLE_USER", protected]));
    api.post_user(&local, IT_PASSWORD)
        .await
        .expect("post_user sole-holder");

    // Sanity: it really is the only holder of `protected`.
    let after = api.list_users().await.expect("list_users (verify holder)");
    let holders: Vec<&str> = after
        .users
        .iter()
        .filter(|u| u.roles.iter().any(|r| r == protected))
        .map(|u| u.user_id.as_str())
        .collect();
    assert_eq!(
        holders,
        vec![name.as_str()],
        "the it-user must be the sole holder of {protected}"
    );

    // Apply a doc that drops `protected` from that user → empties the
    // holder set → IAM-001. No override flag → refuse before any write.
    let demote = parse_user(&user_yaml(&name, &["ROLE_USER"]));
    let opts = ApplyOptions {
        dry_run: false,
        keep_going: false,
        known_roles: KNOWN_ROLES.iter().map(|s| s.to_string()).collect(),
        protected_roles: BTreeSet::from([protected.to_string()]),
        allow_admin_lockout: false,
    };
    let docs = vec![(PathBuf::from(format!("{name}.yaml")), demote)];
    let err = apply_users(&docs, &api, &opts)
        .await
        .expect_err("apply must refuse with IAM-001");
    assert!(
        matches!(err, Error::IamLockout { .. }),
        "expected Error::IamLockout, got {err:?}"
    );

    // Safety: the refusal happened before execution — the user still holds
    // `protected`. Proves the invariant guards rather than rolls back.
    let still = api
        .get_user(&name)
        .await
        .expect("get_user")
        .expect("present");
    assert!(
        still.roles.iter().any(|r| r == protected),
        "an IAM-001 refusal must not have mutated server state"
    );

    api.delete_user(&name).await.expect("delete_user");
    pre_post_cleanup(&h, "post").await;
}

/// Fetch the server-side hashed password for `name`. The get/list response
/// exposes it (spike 0.3); used to witness a `set-password` rotation without
/// authenticating as the user.
async fn stored_password_hash(api: &IamApi<'_>, name: &str) -> Option<String> {
    api.get_user(name)
        .await
        .expect("get_user (password hash)")
        .expect("user present")
        .password
}
