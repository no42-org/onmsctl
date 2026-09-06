/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Keeps the eventconf fuzz seeds honest. Each file under
//! `fuzz/seeds/event_source_from_yaml/` and `fuzz/seeds/eventconf_convert/`
//! exists to reach a specific branch; this asserts it still does, on
//! stable, so a seed cannot rot into arbitrary bytes without `make test`
//! noticing. A new seed needs an entry here.

use std::fs;
use std::path::{Path, PathBuf};

use onmsctl_core::Error;
use onmsctl_eventconf::apply::local::EventSourceLocal;
use onmsctl_eventconf::convert::{ConversionResult, ConvertOpts, FindingCode, convert};

fn seed_dir(target: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/seeds")
        .join(target)
}

fn seed(target: &str, name: &str) -> Vec<u8> {
    let path = seed_dir(target).join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn assert_seeds_are(target: &str, known: &[&str]) {
    let mut found: Vec<String> = fs::read_dir(seed_dir(target))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    found.sort_unstable();
    let mut known = known.to_vec();
    known.sort_unstable();
    assert_eq!(
        found, known,
        "{target}: add an expectation for each new seed"
    );
}

fn convert_seed(name: &str) -> ConversionResult {
    let xml = seed("eventconf_convert", name);
    convert(&xml, Path::new(name), &ConvertOpts::default())
}

// -- event_source_from_yaml -------------------------------------------------

#[test]
fn event_source_seeds_have_expectations() {
    assert_seeds_are(
        "event_source_from_yaml",
        &["block-scalar-separator.yaml", "forbidden-spec-keys.yaml"],
    );
}

#[test]
fn forbidden_spec_keys_seed_hits_the_guided_rejection() {
    let err =
        EventSourceLocal::from_yaml(&seed("event_source_from_yaml", "forbidden-spec-keys.yaml"))
            .unwrap_err();
    match err {
        Error::Config(m) => assert!(m.contains("spec.fileOrder"), "guided message, got: {m}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn block_scalar_separator_seed_is_one_accepted_document() {
    let local = EventSourceLocal::from_yaml(&seed(
        "event_source_from_yaml",
        "block-scalar-separator.yaml",
    ))
    .unwrap();
    assert!(!local.spec.enabled);
    let description = local.spec.events[0].description.as_deref().unwrap();
    assert!(
        description.contains("\n---\n"),
        "the `---` stayed inside the scalar: {description:?}"
    );
}

// -- eventconf_convert ------------------------------------------------------

#[test]
fn convert_seeds_have_expectations() {
    assert_seeds_are(
        "eventconf_convert",
        &[
            "duplicate-uei.events.xml",
            "minimal.events.xml",
            "missing-uei.events.xml",
            "modeled-elements.events.xml",
        ],
    );
}

#[test]
fn minimal_seed_converts_cleanly() {
    let result = convert_seed("minimal.events.xml");
    assert_eq!(result.exit_code(), 0);
    EventSourceLocal::from_yaml(result.yaml.as_deref().unwrap().as_bytes()).unwrap();
}

#[test]
fn duplicate_uei_seed_is_a_clean_normalization() {
    let result = convert_seed("duplicate-uei.events.xml");
    assert_eq!(result.exit_code(), 0);
    assert!(result.findings.is_empty(), "{:?}", result.findings);
}

#[test]
fn missing_uei_seed_blocks_emission() {
    let result = convert_seed("missing-uei.events.xml");
    assert_eq!(result.exit_code(), 2);
    assert!(result.yaml.is_none());
}

#[test]
fn modeled_elements_seed_reaches_every_modeled_element_without_ec001() {
    let result = convert_seed("modeled-elements.events.xml");
    assert_eq!(result.exit_code(), 0, "{:?}", result.findings);
    assert!(
        !result.findings.iter().any(|f| f.code == FindingCode::Ec001),
        "prolog, namespace and comment are not unmodeled elements: {:?}",
        result.findings
    );
    let local = EventSourceLocal::from_yaml(result.yaml.as_deref().unwrap().as_bytes()).unwrap();
    let event = &local.spec.events[0];
    assert!(event.snmp.is_some() && event.alarm_data.is_some() && event.mask.is_some());
}
