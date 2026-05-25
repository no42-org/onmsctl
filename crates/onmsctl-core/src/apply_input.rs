/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Shared `apply -f <path>` dispatch for capabilities that declare
//! YAML-driven verbs.
//!
//! Every `apply -f` verb across `onmsctl-*` capabilities accepts the
//! same three input shapes: a single file, a directory, or a glob
//! pattern. This module owns the classification logic so per-
//! capability copies can't drift. Provisioning's `requisition apply
//! -f` and eventconf's `source apply -f` both call into
//! [`resolve_apply_input`] with their extension filter.
//!
//! See `openspec/changes/harden-provisioning-and-eventconf-parity/design.md`
//! §D4 for the design rationale.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Result of classifying an `apply -f` argument: a single resolved
/// file goes through the per-capability single-file fast-path; a
/// multi-file dispatch goes through the per-capability orchestrator.
/// `Single` carries the resolved path so glob patterns that happen
/// to match exactly one file still get single-file-only flags like
/// `--diff`.
///
/// `#[non_exhaustive]` so a future variant (e.g. `Stdin`) is
/// additive — downstream `match`es must include a catch-all arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum ApplyDispatch {
    Single(PathBuf),
    Multi(Vec<PathBuf>),
}

/// Detect whether a string contains glob metacharacters per the
/// `glob` crate's pattern language. `*`, `?`, and `[` are the three
/// triggers. Backslash escapes are NOT considered here — operators
/// who pass a literal `*` should quote it differently or rely on
/// the literal-glob-in-filename safety net in [`resolve_apply_input`].
pub(crate) fn looks_like_glob(s: &str) -> bool {
    s.bytes().any(|b| matches!(b, b'*' | b'?' | b'['))
}

/// Resolve a CLI `-f` argument into either a single-file or a
/// multi-file dispatch decision.
///
/// `ext_filter` is the list of file-extension strings (without the
/// leading `.`) that count as in-scope files for this capability —
/// e.g. `&["yaml", "yml"]`. Files with other extensions are skipped
/// during directory listing and glob expansion.
///
/// The classification rules:
///
/// 1. Path is non-UTF-8 → `Error::Config` (we can't pass it to the
///    glob crate or echo it cleanly in errors).
/// 2. Path contains glob metacharacters AND does NOT literally
///    exist as a regular file → expand via [`glob::glob_with`] with
///    `require_literal_leading_dot: true` so `*.yaml` doesn't match
///    `.hidden.yaml`. Single-match-glob collapses to `Single` so
///    single-file-only flags still apply.
/// 3. Path exists literally (whether or not it contains glob chars)
///    → `Single(file)` for regular files; `Multi(dir-listing)` for
///    directories.
/// 4. Otherwise → error.
///
/// Glob expansion filters out non-`ext_filter` matches and non-
/// regular entries (with a stderr count when entries are dropped,
/// so the operator isn't surprised). Empty match-sets raise a
/// config error.
pub fn resolve_apply_input(file: &Path, ext_filter: &[&str]) -> Result<ApplyDispatch> {
    if ext_filter.is_empty() {
        return Err(Error::Config(
            "resolve_apply_input called with empty ext_filter — capability must \
             declare which file extensions it accepts (programmer error)"
                .into(),
        ));
    }
    let raw = file.to_str().ok_or_else(|| {
        Error::Config(format!(
            "path {:?} is not valid UTF-8 — pass a UTF-8 file path or glob pattern",
            file.display()
        ))
    })?;

    if looks_like_glob(raw) {
        if file.is_file() {
            return Ok(ApplyDispatch::Single(file.to_path_buf()));
        }
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: true,
        };
        let entries = glob::glob_with(raw, opts)
            .map_err(|e| Error::Config(format!("invalid glob pattern {raw:?}: {e}")))?;
        let mut total_files = 0usize;
        let mut out: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let p = entry.map_err(|e| Error::Config(format!("glob match error: {e}")))?;
            if !p.is_file() {
                continue;
            }
            total_files += 1;
            if !extension_matches(&p, ext_filter) {
                continue;
            }
            out.push(p);
        }
        out.sort();
        if out.is_empty() {
            if total_files > 0 {
                return Err(Error::Config(format!(
                    "glob pattern {raw:?} matched {total_files} file(s), none with \
                     {} extension",
                    format_ext_list(ext_filter)
                )));
            }
            return Err(Error::Config(format!(
                "glob pattern {raw:?} matched no files"
            )));
        }
        let dropped = total_files - out.len();
        if dropped > 0 {
            eprintln!(
                "note: glob {raw:?} matched {total_files} file(s); {dropped} entries \
                 skipped (extensions outside {})",
                format_ext_list(ext_filter)
            );
        }
        if out.len() == 1 {
            return Ok(ApplyDispatch::Single(out.into_iter().next().unwrap()));
        }
        return Ok(ApplyDispatch::Multi(out));
    }

    let meta = std::fs::metadata(file)
        .map_err(|e| Error::Config(format!("failed to stat {}: {e}", file.display())))?;
    if meta.is_dir() {
        let files = list_matching_files(file, ext_filter)?;
        if files.is_empty() {
            return Err(Error::Config(format!(
                "{} contains no {} files",
                file.display(),
                format_ext_list(ext_filter)
            )));
        }
        return Ok(ApplyDispatch::Multi(files));
    }
    Ok(ApplyDispatch::Single(file.to_path_buf()))
}

/// Walk `dir` for regular files matching `ext_filter` (case-
/// sensitive on the extension comparison, non-recursive — capabilities
/// that want recursion pass a glob pattern with `**` instead). Sorted
/// output. Internal helper — callers should go through
/// [`resolve_apply_input`] so the empty-filter guard and the dispatch
/// semantics apply uniformly.
pub(crate) fn list_matching_files(dir: &Path, ext_filter: &[&str]) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| Error::Config(format!("read_dir {}: {e}", dir.display())))?;
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| extension_matches(p, ext_filter))
        .collect();
    out.sort();
    Ok(out)
}

fn extension_matches(p: &Path, ext_filter: &[&str]) -> bool {
    let Some(ext) = p.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    ext_filter.contains(&ext)
}

fn format_ext_list(ext_filter: &[&str]) -> String {
    if ext_filter.is_empty() {
        return "<any>".to_string();
    }
    ext_filter
        .iter()
        .map(|e| format!(".{e}"))
        .collect::<Vec<_>>()
        .join(" / ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML_EXTS: &[&str] = &["yaml", "yml"];

    // ---- looks_like_glob ----

    #[test]
    fn looks_like_glob_detects_metacharacters() {
        assert!(looks_like_glob("*.yaml"));
        assert!(looks_like_glob("requisitions/*.yaml"));
        assert!(looks_like_glob("requisitions/**/*.yaml"));
        assert!(looks_like_glob("acme-?.yaml"));
        assert!(looks_like_glob("[ab]cme.yaml"));
    }

    #[test]
    fn looks_like_glob_passes_plain_paths() {
        assert!(!looks_like_glob("acme.yaml"));
        assert!(!looks_like_glob("requisitions/"));
        assert!(!looks_like_glob("requisitions/acme-prod.yaml"));
        assert!(!looks_like_glob(""));
    }

    // ---- resolve_apply_input ----

    #[test]
    fn resolve_apply_input_routes_single_file_to_single() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("acme.yaml");
        std::fs::write(&path, "apiVersion: v1\n").unwrap();
        match resolve_apply_input(&path, YAML_EXTS).unwrap() {
            ApplyDispatch::Single(resolved) => assert_eq!(resolved, path),
            ApplyDispatch::Multi(_) => panic!("single file should resolve to Single"),
        }
    }

    #[test]
    fn resolve_apply_input_routes_directory_to_multi() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.yaml"), "x").unwrap();
        std::fs::write(tmp.path().join("b.yml"), "x").unwrap();
        std::fs::write(tmp.path().join("README.md"), "x").unwrap();
        match resolve_apply_input(tmp.path(), YAML_EXTS).unwrap() {
            ApplyDispatch::Multi(files) => {
                assert_eq!(files.len(), 2, "non-yaml files filtered out");
                assert!(files[0].ends_with("a.yaml"));
                assert!(files[1].ends_with("b.yml"));
            }
            ApplyDispatch::Single(_) => panic!("directory should resolve to Multi"),
        }
    }

    #[test]
    fn resolve_apply_input_expands_glob_non_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.yaml"), "x").unwrap();
        std::fs::write(tmp.path().join("b.yaml"), "x").unwrap();
        let subdir = tmp.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(subdir.join("nested.yaml"), "x").unwrap();
        let pattern = tmp.path().join("*.yaml");
        match resolve_apply_input(&pattern, YAML_EXTS).unwrap() {
            ApplyDispatch::Multi(files) => {
                assert_eq!(files.len(), 2);
                assert!(files.iter().all(|f| !f.ends_with("nested.yaml")));
            }
            ApplyDispatch::Single(_) => panic!("glob should resolve to Multi"),
        }
    }

    #[test]
    fn resolve_apply_input_expands_recursive_glob_with_double_star() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.yaml"), "x").unwrap();
        let subdir = tmp.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(subdir.join("nested.yaml"), "x").unwrap();
        let pattern = tmp.path().join("**/*.yaml");
        match resolve_apply_input(&pattern, YAML_EXTS).unwrap() {
            ApplyDispatch::Multi(files) => {
                assert!(files.len() >= 2);
                let names: Vec<String> = files
                    .iter()
                    .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                    .collect();
                assert!(names.contains(&"a.yaml".to_string()));
                assert!(names.contains(&"nested.yaml".to_string()));
            }
            ApplyDispatch::Single(_) => panic!("recursive glob should resolve to Multi"),
        }
    }

    #[test]
    fn resolve_apply_input_glob_with_no_matches_is_a_config_error() {
        let tmp = tempfile::tempdir().unwrap();
        let pattern = tmp.path().join("nonexistent-*.yaml");
        assert!(resolve_apply_input(&pattern, YAML_EXTS).is_err());
    }

    #[test]
    fn resolve_apply_input_glob_matching_exactly_one_file_collapses_to_single() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("acme-prod.yaml"), "x").unwrap();
        std::fs::write(tmp.path().join("README.md"), "x").unwrap();
        let pattern = tmp.path().join("acme-*.yaml");
        match resolve_apply_input(&pattern, YAML_EXTS).unwrap() {
            ApplyDispatch::Single(p) => assert!(p.ends_with("acme-prod.yaml")),
            ApplyDispatch::Multi(_) => {
                panic!("single-file glob match should collapse to Single")
            }
        }
    }

    #[test]
    fn resolve_apply_input_literal_glob_in_filename_routes_to_single() {
        let tmp = tempfile::tempdir().unwrap();
        let weird = tmp.path().join("weird[1].yaml");
        std::fs::write(&weird, "x").unwrap();
        match resolve_apply_input(&weird, YAML_EXTS).unwrap() {
            ApplyDispatch::Single(resolved) => assert_eq!(resolved, weird),
            ApplyDispatch::Multi(_) => {
                panic!("literal filename with glob-like chars should route to Single")
            }
        }
    }

    #[test]
    fn resolve_apply_input_glob_excludes_dotfiles() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.yaml"), "x").unwrap();
        std::fs::write(tmp.path().join(".hidden.yaml"), "x").unwrap();
        let pattern = tmp.path().join("*.yaml");
        match resolve_apply_input(&pattern, YAML_EXTS).unwrap() {
            ApplyDispatch::Single(p) => {
                assert!(p.ends_with("a.yaml"));
            }
            ApplyDispatch::Multi(files) => {
                assert!(files.iter().all(|f| !f.ends_with(".hidden.yaml")));
            }
        }
    }

    #[test]
    fn resolve_apply_input_glob_matches_yml_extension_too() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.yaml"), "x").unwrap();
        std::fs::write(tmp.path().join("b.yml"), "x").unwrap();
        let pattern = tmp.path().join("*");
        match resolve_apply_input(&pattern, YAML_EXTS).unwrap() {
            ApplyDispatch::Multi(files) => {
                assert_eq!(files.len(), 2);
            }
            ApplyDispatch::Single(_) => {
                panic!("two yaml files should resolve to Multi")
            }
        }
    }

    #[test]
    fn resolve_apply_input_rejects_empty_ext_filter() {
        // Empty filter is programmer-error: it would silently match
        // nothing and produce "matched no <any> files" — actively
        // misleading. Surface the misuse loudly.
        let tmp = tempfile::tempdir().unwrap();
        let err = resolve_apply_input(tmp.path(), &[]).unwrap_err();
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("empty ext_filter"), "msg was: {msg}");
            }
            other => panic!("expected Config, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolve_apply_input_rejects_non_utf8_path() {
        // Classification rule #1 from the spec: non-UTF-8 paths
        // can't flow into the glob crate without lossy conversion,
        // so we reject them upfront. Unix-only — Windows uses UTF-16
        // and can't construct a non-UTF-8 OsString without unsafe.
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let invalid = PathBuf::from(OsStr::from_bytes(&[0xff, 0xfe, 0x80]));
        let err = resolve_apply_input(&invalid, YAML_EXTS).unwrap_err();
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("UTF-8"), "msg was: {msg}");
            }
            other => panic!("expected Config, got {other:?}"),
        }
    }

    #[test]
    fn resolve_apply_input_honors_custom_ext_filter() {
        // Capabilities other than provisioning / eventconf might
        // accept .json — verify the parameter actually flows through.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.json"), "x").unwrap();
        std::fs::write(tmp.path().join("b.yaml"), "x").unwrap();
        let json_filter: &[&str] = &["json"];
        match resolve_apply_input(tmp.path(), json_filter).unwrap() {
            ApplyDispatch::Multi(files) => {
                assert_eq!(files.len(), 1);
                assert!(files[0].ends_with("a.json"));
            }
            ApplyDispatch::Single(p) => {
                // dir with single matching file might collapse — but
                // directory mode doesn't collapse today, it stays
                // Multi. Adjust if behavior changes.
                assert!(p.ends_with("a.json"));
            }
        }
    }
}
