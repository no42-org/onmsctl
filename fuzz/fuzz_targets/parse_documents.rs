/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Multi-document YAML envelope split plus `kind` peek. This is the first
//! thing `apply -f` does with an operator's file, before any handler runs.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onmsctl_core::kind::envelope::parse_documents;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(docs) = parse_documents("fuzz.yaml", text) {
        for doc in &docs {
            let _ = doc.peek_kind();
            let _ = doc.label();
        }
    }
});
