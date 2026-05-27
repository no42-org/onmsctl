/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Secret resolution for the IAM capability.
//!
//! [`resolve_password_ref`] is the single source of truth for materializing
//! a [`crate::model::PasswordRef`] into a cleartext value. Both the apply
//! pipeline (Group 6, Create plans only) and the imperative
//! `iam user set-password` verb call it. See design.md §D5 for the
//! create-only-on-apply policy.
//!
//! The returned [`SecretString`] zeroizes on drop and redacts on `Debug` /
//! `Display` formatting.

use std::io::Read;
use std::path::Path;

use onmsctl_core::error::{Error, Result};
use zeroize::Zeroizing;

use crate::model::{FromEnvRef, FromFileRef, FromKeyringRef, PasswordRef};

/// Read cap for `fromFile` secret bodies. Realistic passwords (even long
/// passphrases) sit well under 1 KiB; this bounds pathological allocation
/// from misconfigured paths pointing at giant files.
const MAX_SECRET_FILE_BYTES: usize = 4096;

/// Cleartext secret. Backed by `zeroize::Zeroizing<String>` so the heap
/// buffer is wiped on drop. `Debug` redacts the contents; there is no
/// `Display` impl on purpose — callers must reach for `.expose()`
/// explicitly when they actually need the cleartext, so `format!("{s}")`
/// never compiles as a stealth way to print a secret.
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn new(s: String) -> Self {
        Self(Zeroizing::new(s))
    }

    /// Expose the cleartext. Call at the moment the secret is needed
    /// (e.g. building the HTTP request body) — do not stash the `&str`.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

/// Materialize a `PasswordRef` into a cleartext `SecretString`. Single
/// source of truth — used by both the apply Create path and the imperative
/// `set-password` verb (see design.md §D5).
pub fn resolve_password_ref(r: &PasswordRef) -> Result<SecretString> {
    match r {
        PasswordRef::FromFile(s) => resolve_from_file(s),
        PasswordRef::FromEnv(s) => resolve_from_env(s),
        PasswordRef::FromKeyring(s) => resolve_from_keyring(s),
    }
}

fn resolve_from_file(spec: &FromFileRef) -> Result<SecretString> {
    let path = &spec.from_file;

    // Symlink refusal happens before open: the target's mode cannot be
    // enforced atomically across a symlink hop. Operators using symlinks
    // for secrets should pass the resolved path.
    #[cfg(unix)]
    refuse_symlink(path)?;

    // Open once. All subsequent metadata + read calls work off the same
    // FD so the mode check and the read see the same file (closes the
    // stat-then-read TOCTOU window).
    let f = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| {
            Error::Config(format!(
                "passwordRef.fromFile: cannot open {}: {e}",
                path.display()
            ))
        })?;

    #[cfg(unix)]
    check_unix_mode_on_fd(&f, path)?;

    // Enforce size cap before allocating. `metadata().len()` is 0 for
    // some non-regular files (sockets, FIFOs) which would also fail the
    // mode check above on regular operating systems, so falling through
    // to a bounded read is safe.
    let declared = f.metadata().map(|m| m.len()).map_err(|e| {
        Error::Config(format!(
            "passwordRef.fromFile: cannot stat opened {}: {e}",
            path.display()
        ))
    })?;
    if declared > MAX_SECRET_FILE_BYTES as u64 {
        return Err(Error::Config(format!(
            "passwordRef.fromFile: {} is {} bytes; cap is {} bytes — verify the path \
             points at a secret file, not e.g. a key bundle",
            path.display(),
            declared,
            MAX_SECRET_FILE_BYTES
        )));
    }

    // Read into a Zeroizing buffer so the heap allocation is wiped on
    // drop, not just the final SecretString.
    let mut buf = Zeroizing::new(String::with_capacity(
        declared.min(MAX_SECRET_FILE_BYTES as u64) as usize,
    ));
    f.take(MAX_SECRET_FILE_BYTES as u64 + 1)
        .read_to_string(&mut buf)
        .map_err(|e| {
            Error::Config(format!(
                "passwordRef.fromFile: cannot read {}: {e}",
                path.display()
            ))
        })?;
    if buf.len() > MAX_SECRET_FILE_BYTES {
        // Should be unreachable thanks to the declared-length check, but
        // guard the streaming-read path too in case `declared` was 0
        // (sparse files, special FS).
        return Err(Error::Config(format!(
            "passwordRef.fromFile: {} exceeded {} byte cap during read",
            path.display(),
            MAX_SECRET_FILE_BYTES
        )));
    }

    let trimmed = strip_trailing_lf(&buf);
    if trimmed.is_empty() {
        return Err(Error::Config(format!(
            "passwordRef.fromFile: {} is empty after trimming a trailing newline",
            path.display()
        )));
    }

    // Internal newlines (after the one trailing strip) are a hard refuse:
    // a literal `\n` in a password cannot round-trip the form-encoded PUT
    // or the XML POST. Catching it here gives the operator a clear local
    // error instead of a confusing server-side 400 hours later. Leading /
    // trailing whitespace stays a warning (could be a deliberate
    // passphrase like "correct horse battery staple ").
    if trimmed.contains('\n') {
        return Err(Error::Config(format!(
            "passwordRef.fromFile: {} contains an internal newline; passwords cannot \
             carry literal `\\n` through the form-encoded PUT or XML POST that Horizon \
             accepts. Trim the file to exactly one password line.",
            path.display()
        )));
    }
    if has_outer_whitespace(trimmed) {
        eprintln!(
            "warning: passwordRef.fromFile: {} has leading or trailing whitespace; \
             the full content is used verbatim. Verify this is the intended secret.",
            path.display()
        );
    }

    // .to_string() allocates a fresh buffer; wrap it in SecretString so
    // that buffer is also zeroized on drop. The original `buf` (also a
    // Zeroizing<String>) is zeroized when this function returns.
    Ok(SecretString::new(trimmed.to_string()))
}

fn resolve_from_env(spec: &FromEnvRef) -> Result<SecretString> {
    let name = &spec.from_env;
    match std::env::var(name) {
        Ok(s) if s.is_empty() => Err(Error::Config(format!(
            "passwordRef.fromEnv: env var {name} is set but empty"
        ))),
        Ok(s) => Ok(SecretString::new(s)),
        Err(std::env::VarError::NotPresent) => Err(Error::Config(format!(
            "passwordRef.fromEnv: env var {name} is not set"
        ))),
        Err(std::env::VarError::NotUnicode(_)) => Err(Error::Config(format!(
            "passwordRef.fromEnv: env var {name} contains invalid UTF-8"
        ))),
    }
}

fn resolve_from_keyring(spec: &FromKeyringRef) -> Result<SecretString> {
    let k = &spec.from_keyring;
    let secret = onmsctl_core::auth::read_keyring_secret(&k.service, &k.account).map_err(|e| {
        Error::Config(format!(
            "passwordRef.fromKeyring (service={:?}, account={:?}): {e}",
            k.service, k.account
        ))
    })?;
    if secret.is_empty() {
        return Err(Error::Config(format!(
            "passwordRef.fromKeyring (service={:?}, account={:?}): keyring entry is empty",
            k.service, k.account
        )));
    }
    Ok(SecretString::new(secret))
}

#[cfg(unix)]
fn refuse_symlink(path: &Path) -> Result<()> {
    let sym = std::fs::symlink_metadata(path).map_err(|e| {
        Error::Config(format!(
            "passwordRef.fromFile: cannot stat {}: {e}",
            path.display()
        ))
    })?;
    if sym.file_type().is_symlink() {
        return Err(Error::Config(format!(
            "passwordRef.fromFile: {} is a symlink; refusing to follow because the \
             target's mode cannot be enforced atomically. Pass the resolved path \
             directly.",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn check_unix_mode_on_fd(f: &std::fs::File, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = f.metadata().map_err(|e| {
        Error::Config(format!(
            "passwordRef.fromFile: cannot stat opened {}: {e}",
            path.display()
        ))
    })?;
    let mode = meta.permissions().mode() & 0o777;
    // Write bits: group OR world writable both indicate an attacker can
    // tamper with the secret. Refuse both.
    if mode & 0o022 != 0 {
        return Err(Error::Config(format!(
            "passwordRef.fromFile: {} is writable beyond the owner (mode {mode:#o}); \
             refusing to read a tampered-secret file. Restrict via `chmod 600`.",
            path.display()
        )));
    }
    // Read bits: group OR world readable both leak the secret. Warn.
    if mode & 0o044 != 0 {
        eprintln!(
            "warning: passwordRef.fromFile: {} is readable beyond the owner \
             (mode {mode:#o}); restrict to 0600",
            path.display()
        );
    }
    Ok(())
}

fn strip_trailing_lf(s: &str) -> &str {
    if let Some(rest) = s.strip_suffix("\r\n") {
        rest
    } else if let Some(rest) = s.strip_suffix('\n') {
        rest
    } else {
        s
    }
}

/// `true` if the trimmed content has leading or trailing non-newline
/// whitespace. Triggers a stderr warning but the value is still passed
/// through verbatim — intent could be a multi-word passphrase. Internal
/// newlines are handled separately (hard refuse — see `resolve_from_file`).
fn has_outer_whitespace(s: &str) -> bool {
    s.starts_with(|c: char| c.is_whitespace()) || s.ends_with(|c: char| c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::KeyringRef;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn file_ref(path: PathBuf) -> PasswordRef {
        PasswordRef::FromFile(FromFileRef { from_file: path })
    }
    fn env_ref(name: &str) -> PasswordRef {
        PasswordRef::FromEnv(FromEnvRef {
            from_env: name.into(),
        })
    }
    fn keyring_ref(service: &str, account: &str) -> PasswordRef {
        PasswordRef::FromKeyring(FromKeyringRef {
            from_keyring: KeyringRef {
                service: service.into(),
                account: account.into(),
            },
        })
    }

    #[test]
    fn secret_string_debug_redacts_and_expose_round_trips() {
        let s = SecretString::new("supersecret".into());
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("supersecret"), "Debug leaked: {dbg}");
        assert!(dbg.contains("redacted"));
        assert_eq!(s.expose(), "supersecret");
        // SecretString has no Display impl on purpose — `format!("{s}")`
        // does not compile. (This is enforced at the type level; we can't
        // assert "does not compile" inside a runtime test without a UI
        // test harness, but the absence is intentional and documented on
        // the type.)
    }

    #[test]
    fn from_file_happy_path_trims_single_trailing_newline() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "hunter2").unwrap(); // adds one '\n'
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let r = resolve_password_ref(&file_ref(f.path().to_owned())).unwrap();
        assert_eq!(r.expose(), "hunter2");
    }

    #[test]
    fn from_file_handles_crlf_line_ending() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hunter2\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let r = resolve_password_ref(&file_ref(f.path().to_owned())).unwrap();
        assert_eq!(r.expose(), "hunter2");
    }

    #[test]
    fn from_file_missing_path_errors() {
        let bogus = PathBuf::from("/tmp/onmsctl-iam-this-file-does-not-exist-xyz-9921");
        let err = resolve_password_ref(&file_ref(bogus)).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("cannot")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn from_file_empty_after_trim_errors() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f).unwrap(); // file contains just "\n"
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let err = resolve_password_ref(&file_ref(f.path().to_owned())).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("empty")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn from_file_world_writable_refused() {
        use std::os::unix::fs::PermissionsExt;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hunter2\n").unwrap();
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o666)).unwrap();
        let err = resolve_password_ref(&file_ref(f.path().to_owned())).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("writable beyond the owner"), "got: {m}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn from_file_internal_newline_is_refused() {
        // Wire-incompatibility: a literal \n cannot round-trip the
        // form-encoded PUT or the XML POST that Horizon accepts. Refusing
        // here gives the operator a clear local error rather than a
        // confusing server-side 400 hours later.
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hunter2\nextra\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let err = resolve_password_ref(&file_ref(f.path().to_owned())).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("internal newline"), "got: {m}");
                assert!(m.contains("password"), "got: {m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn from_file_symlink_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.pw");
        std::fs::write(&target, b"hunter2\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("link.pw");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = resolve_password_ref(&file_ref(link)).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("symlink"), "got: {m}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn from_file_group_writable_refused() {
        use std::os::unix::fs::PermissionsExt;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hunter2\n").unwrap();
        // 0o620 = owner rw, group w (no group r). World bits zero.
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o620)).unwrap();
        let err = resolve_password_ref(&file_ref(f.path().to_owned())).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("writable beyond the owner"), "got: {m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn from_file_size_cap_refused() {
        let mut f = NamedTempFile::new().unwrap();
        // One byte over the 4 KiB cap.
        let big = vec![b'a'; MAX_SECRET_FILE_BYTES + 1];
        f.write_all(&big).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let err = resolve_password_ref(&file_ref(f.path().to_owned())).unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("cap"), "got: {m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn from_env_present_value_returned() {
        let key = "ONMSCTL_IAM_TEST_FROM_ENV_PRESENT";
        // SAFETY: `set_var` is only marked unsafe in Rust 2024 because env
        // mutation races with other-threaded `getenv`. cargo test runs each
        // test in parallel but each test uses a unique env-var name so they
        // do not collide. We do not spawn threads that read this var.
        unsafe { std::env::set_var(key, "frompath") };
        let r = resolve_password_ref(&env_ref(key)).unwrap();
        assert_eq!(r.expose(), "frompath");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn from_env_unset_errors() {
        let key = "ONMSCTL_IAM_TEST_FROM_ENV_UNSET";
        unsafe { std::env::remove_var(key) };
        let err = resolve_password_ref(&env_ref(key)).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("not set")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn from_env_empty_errors() {
        let key = "ONMSCTL_IAM_TEST_FROM_ENV_EMPTY";
        unsafe { std::env::set_var(key, "") };
        let err = resolve_password_ref(&env_ref(key)).unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("empty")),
            other => panic!("unexpected {other:?}"),
        }
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn from_keyring_nonexistent_service_errors() {
        // No assertion on the exact backend message — varies by platform.
        // The contract is: a non-existent entry surfaces as Error::Config
        // wrapping the keyring backend's reason.
        let r = resolve_password_ref(&keyring_ref(
            "onmsctl-iam-test-service-no-such-thing-xyz",
            "onmsctl-iam-test-account-no-such-thing-xyz",
        ));
        assert!(r.is_err());
        match r.unwrap_err() {
            Error::Config(m) => assert!(m.contains("fromKeyring"), "got: {m}"),
            other => panic!("unexpected {other:?}"),
        }
    }
}
