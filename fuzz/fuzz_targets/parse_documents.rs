/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Multi-document YAML envelope split plus `kind` peek. This is the first
//! thing `apply -f` does with an operator's file, before any handler runs.
//!
//! Beyond "no panic, no hang", a successful split must satisfy:
//! - indexes are contiguous and `label()` reflects them;
//! - no kept document is null;
//! - `peek_kind()` agrees with the raw `kind` field, and fails only when
//!   the document is not a mapping or has no string `kind`;
//! - a kept document that serializes survives its own serialization as
//!   exactly one document with an equal value. `apply` handlers
//!   re-serialize each split document before parsing it into a typed
//!   struct, so a lossy round trip would change what gets applied. The
//!   emitter refuses some values the parser accepts (a mapping used as a
//!   key, `? ? `); the handler reports that as an error, so it is not a
//!   finding here.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onmsctl_core::kind::envelope::parse_documents;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(docs) = parse_documents("fuzz.yaml", text) else {
        return;
    };
    for (i, doc) in docs.iter().enumerate() {
        assert_eq!(doc.index, i, "indexes are contiguous over kept documents");
        assert_eq!(doc.label(), format!("fuzz.yaml#{i}"));
        assert!(!doc.value.is_null(), "null documents are skipped");

        let raw_kind = doc.value.get("kind").and_then(|k| k.as_str());
        match doc.peek_kind() {
            Ok(kind) => assert_eq!(Some(kind), raw_kind, "peek_kind matches the raw field"),
            Err(_) => assert!(
                doc.value.as_mapping().is_none() || raw_kind.is_none(),
                "peek_kind fails only for non-mappings or a missing/non-string kind"
            ),
        }

        let Ok(again) = serde_norway::to_string(&doc.value) else {
            continue;
        };
        let redocs = parse_documents("fuzz.yaml", &again).expect("re-serialized document parses");
        assert_eq!(
            redocs.len(),
            1,
            "one document in, one document out:\n{again}"
        );
        assert_eq!(
            redocs[0].value, doc.value,
            "round trip preserves the value:\n{again}"
        );
    }
});
