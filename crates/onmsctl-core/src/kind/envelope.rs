/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Cheap `kind` discrimination for the router (ADR-001).
//!
//! The router reads only the `{apiVersion, kind}` envelope of each document to
//! select a handler; it never deserializes capability-specific document types.
//! The selected handler performs its own strict parse from the same
//! [`RawDoc::value`].

use std::path::PathBuf;

use serde::Deserialize;

use crate::apply_input::ApplyDispatch;
use crate::error::{Error, Result};

/// One parsed-but-not-validated YAML document plus its provenance. `value`
/// holds the full document so the selected handler can run its own strict
/// deserialization without re-reading the source.
#[derive(Clone, Debug)]
pub struct RawDoc {
    /// File path the document came from, or `"<stdin>"`.
    pub source: String,
    /// Zero-based document index within a multi-document stream.
    pub index: usize,
    /// The full document as an untyped YAML value.
    pub value: serde_norway::Value,
}

impl RawDoc {
    /// Return the document's `kind`, distinguishing the three failure shapes:
    /// the top-level node isn't a mapping, the `kind` field is missing, or it
    /// is present but not a string.
    pub fn peek_kind(&self) -> Result<&str> {
        if self.value.as_mapping().is_none() {
            return Err(Error::Config(format!(
                "{}: document {} is not a YAML mapping (expected an object with apiVersion/kind/metadata)",
                self.source, self.index
            )));
        }
        match self.value.get("kind") {
            None => Err(Error::Config(format!(
                "{}: document {} is missing the `kind` field",
                self.source, self.index
            ))),
            Some(v) => v.as_str().ok_or_else(|| {
                Error::Config(format!(
                    "{}: document {} has a non-string `kind` field",
                    self.source, self.index
                ))
            }),
        }
    }

    /// Human-readable `source#index` label for diagnostics.
    pub fn label(&self) -> String {
        format!("{}#{}", self.source, self.index)
    }
}

/// Strip a leading UTF-8 BOM (`U+FEFF`), which some editors prepend and which
/// the YAML parser would otherwise treat as content on the first line.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Split a YAML stream into one [`RawDoc`] per `---`-separated document. Empty
/// (null) documents are skipped so a trailing `---` or a comment-only document
/// does not produce a phantom entry. The emitted `index` is contiguous over
/// the *kept* documents (skipped null docs do not create gaps). A malformed
/// document fails the whole parse with the source named.
pub fn parse_documents(source: &str, text: &str) -> Result<Vec<RawDoc>> {
    let text = strip_bom(text);
    let mut docs = Vec::new();
    for de in serde_norway::Deserializer::from_str(text) {
        let value = serde_norway::Value::deserialize(de).map_err(|e| {
            // 0-based to match `RawDoc::index` / `peek_kind`'s "document N".
            // `docs.len()` is the index the failing document would receive.
            Error::Config(format!(
                "{source}: invalid YAML in document {}: {e}",
                docs.len()
            ))
        })?;
        if value.is_null() {
            continue;
        }
        let index = docs.len();
        docs.push(RawDoc {
            source: source.to_string(),
            index,
            value,
        });
    }
    Ok(docs)
}

/// Read every file in a resolved [`ApplyDispatch`] into [`RawDoc`]s, reusing
/// `apply_input`'s single/dir/glob resolution (task 1.3 read layer). Files are
/// read in dispatch order; each file may contribute multiple documents.
pub fn load_documents(dispatch: &ApplyDispatch) -> Result<Vec<RawDoc>> {
    // `ApplyDispatch` is `#[non_exhaustive]`, but within the defining crate the
    // two variants are exhaustive; a future variant (e.g. Stdin) will surface
    // here as a compile error to be handled explicitly.
    let paths: Vec<PathBuf> = match dispatch {
        ApplyDispatch::Single(p) => vec![p.clone()],
        ApplyDispatch::Multi(v) => v.clone(),
    };
    let mut docs = Vec::new();
    for p in paths {
        // Name the file on any read failure (permissions, missing, non-UTF-8
        // content), matching `apply_input`'s path-naming convention.
        let text = std::fs::read_to_string(&p)
            .map_err(|e| Error::Config(format!("failed to read {}: {e}", p.display())))?;
        docs.extend(parse_documents(&p.display().to_string(), &text)?);
    }
    Ok(docs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_kind_reads_the_discriminator() {
        let docs =
            parse_documents("a.yaml", "apiVersion: v1\nkind: Requisition\nmetadata: {}").unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].peek_kind().unwrap(), "Requisition");
    }

    #[test]
    fn missing_kind_is_a_config_error_naming_the_source() {
        let docs = parse_documents("b.yaml", "apiVersion: v1\nmetadata: {}").unwrap();
        let err = docs[0].peek_kind().unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("b.yaml"));
    }

    #[test]
    fn multi_document_stream_splits_and_skips_empties() {
        let text = "kind: User\nmetadata: {name: alice}\n---\nkind: EventSource\nmetadata: {name: cisco}\n---\n";
        let docs = parse_documents("multi.yaml", text).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].peek_kind().unwrap(), "User");
        assert_eq!(docs[1].peek_kind().unwrap(), "EventSource");
        assert_eq!(docs[1].index, 1);
    }

    #[test]
    fn load_documents_reads_resolved_files() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "kind: Requisition\nmetadata: {{name: acme}}\n").unwrap();
        let dispatch = ApplyDispatch::Single(f.path().to_path_buf());
        let docs = load_documents(&dispatch).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].peek_kind().unwrap(), "Requisition");
    }

    #[test]
    fn non_mapping_top_level_is_a_clear_error() {
        let docs = parse_documents("c.yaml", "- a\n- b\n").unwrap();
        let err = docs[0].peek_kind().unwrap_err();
        assert!(err.to_string().contains("not a YAML mapping"), "{err}");
    }

    #[test]
    fn non_string_kind_is_distinguished_from_missing() {
        let docs = parse_documents("d.yaml", "kind: 123\nmetadata: {}\n").unwrap();
        let err = docs[0].peek_kind().unwrap_err();
        assert!(err.to_string().contains("non-string"), "{err}");
    }

    #[test]
    fn index_is_contiguous_after_leading_null_documents() {
        let text = "---\n---\nkind: User\nmetadata: {name: a}\n---\nkind: EventSource\nmetadata: {name: b}\n";
        let docs = parse_documents("e.yaml", text).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].index, 0);
        assert_eq!(docs[1].index, 1);
    }

    #[test]
    fn leading_bom_is_stripped() {
        let text = "\u{feff}kind: User\nmetadata: {name: a}\n";
        let docs = parse_documents("f.yaml", text).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].peek_kind().unwrap(), "User");
    }

    #[test]
    fn malformed_document_error_names_the_source() {
        let err = parse_documents("g.yaml", "foo: [1, 2\n").unwrap_err();
        assert!(err.to_string().contains("g.yaml"), "{err}");
    }

    #[test]
    fn load_documents_missing_file_names_the_path() {
        let dispatch = ApplyDispatch::Single(PathBuf::from("/no/such/onmsctl-xyz.yaml"));
        let err = load_documents(&dispatch).unwrap_err();
        assert!(err.to_string().contains("onmsctl-xyz.yaml"), "{err}");
    }
}
