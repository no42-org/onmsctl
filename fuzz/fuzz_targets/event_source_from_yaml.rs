/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Strict EventSource YAML parse and validation, including the guided
//! recovery pass for well-known forbidden `spec.*` keys.

#![no_main]

use libfuzzer_sys::fuzz_target;
use onmsctl_eventconf::apply::local::EventSourceLocal;

fuzz_target!(|data: &[u8]| {
    let _ = EventSourceLocal::from_yaml(data);
});
