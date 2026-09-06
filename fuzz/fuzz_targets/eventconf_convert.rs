/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl event-source convert`: eventconf XML in, YAML plus findings out.
//! Exercises quick-xml deserialization, the EC001 structural scan, the
//! byte-offset to line/column mapping, and then feeds any emitted YAML back
//! through the strict EventSource parser, which is what `apply -f` would do
//! with the converted file. A clean conversion (exit code 0) that the parser
//! then rejects is a converter-vs-parser divergence, so that case panics.

#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use onmsctl_eventconf::apply::local::EventSourceLocal;
use onmsctl_eventconf::convert::{ConvertOpts, convert, render_report_text};

fuzz_target!(|data: &[u8]| {
    let result = convert(data, Path::new("fuzz.events.xml"), &ConvertOpts::default());
    let clean = result.exit_code() == 0;
    let _ = render_report_text(&result);
    if let Some(yaml) = &result.yaml {
        let parsed = EventSourceLocal::from_yaml(yaml.as_bytes());
        if clean {
            parsed.expect("YAML from a clean conversion must parse as an EventSource");
        }
    }
});
