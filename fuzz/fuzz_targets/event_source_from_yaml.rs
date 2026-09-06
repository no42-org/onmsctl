/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Strict EventSource YAML parse and validation, including the guided
//! recovery pass for well-known forbidden `spec.*` keys.
//!
//! An accepted document must also survive its own serialization: what
//! `event-source get -o yaml` prints, `apply -f` must read back as the
//! same EventSource. A parse-serialize-parse mismatch is a divergence
//! between the serde derive and `validate()`, so it panics.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onmsctl_eventconf::apply::local::EventSourceLocal;

fuzz_target!(|data: &[u8]| {
    let Ok(local) = EventSourceLocal::from_yaml(data) else {
        return;
    };
    let out = serde_norway::to_string(&local).expect("a validated EventSource serializes");
    let again = EventSourceLocal::from_yaml(out.as_bytes())
        .unwrap_or_else(|e| panic!("serialized EventSource must parse again: {e}\n{out}"));
    assert_eq!(again, local, "round trip preserves the EventSource:\n{out}");
});
