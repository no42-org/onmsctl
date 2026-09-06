/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Keeps the `parse_documents` fuzz seeds honest. Each file under
//! `fuzz/seeds/parse_documents/` exists to reach a specific branch; this
//! asserts it still does, on stable, so a seed cannot rot into arbitrary
//! bytes without `make test` noticing. A new seed needs an entry here.

use std::fs;
use std::path::PathBuf;

use onmsctl_core::kind::envelope::parse_documents;

fn seed_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/parse_documents")
}

fn seed(name: &str) -> String {
    let path = seed_dir().join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

const KNOWN: &[&str] = &["multi-document.yaml", "not-a-mapping.yaml"];

#[test]
fn every_seed_has_an_expectation() {
    let mut found: Vec<String> = fs::read_dir(seed_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    found.sort_unstable();
    let mut known: Vec<&str> = KNOWN.to_vec();
    known.sort_unstable();
    assert_eq!(found, known, "add an expectation for each new seed");
}

#[test]
fn multi_document_keeps_two_real_documents() {
    let docs = parse_documents("multi-document.yaml", &seed("multi-document.yaml")).unwrap();
    assert_eq!(
        docs.len(),
        2,
        "null documents around and between are skipped"
    );
    assert_eq!(docs[0].peek_kind().unwrap(), "EventSource");
    assert_eq!(docs[1].peek_kind().unwrap(), "Requisition");
    assert_eq!(
        docs[1].index, 1,
        "indexes are contiguous over kept documents"
    );
}

#[test]
fn not_a_mapping_documents_split_but_have_no_kind() {
    let docs = parse_documents("not-a-mapping.yaml", &seed("not-a-mapping.yaml")).unwrap();
    assert_eq!(docs.len(), 2);
    for doc in &docs {
        assert!(
            doc.peek_kind().is_err(),
            "{}: a sequence or scalar has no kind",
            doc.label()
        );
    }
}
