/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Integration tests for the convert pipeline against fixture
//! XML files in `tests/fixtures/convert/`.
//!
//! These exercise the same code paths as the in-module unit tests,
//! but against files on disk and with two-pass determinism checks
//! that pin the contract task 8.6 calls out: "re-renders to
//! identical canonical YAML across two passes".

use onmsctl_provisioning::convert::{
    FindingCode, Severity, convert_directory, convert_requisition_xml,
};
use onmsctl_provisioning::model::RequisitionLocal;

const CLEAN_REQ: &str = include_str!("fixtures/convert/clean-requisition.xml");
const CLEAN_FS: &str = include_str!("fixtures/convert/clean-foreign-source.xml");
const CLEAN_EXPECTED_YAML: &str = include_str!("fixtures/convert/clean-requisition.expected.yaml");
const UNMODELED_REQ: &str = include_str!("fixtures/convert/unmodeled-elements-requisition.xml");
const UNKNOWN_ELEMENT_REQ: &str =
    include_str!("fixtures/convert/truly-unknown-element-requisition.xml");
const MALFORMED_REQ: &str = include_str!("fixtures/convert/malformed-truncated.xml");
const WRONG_ROOT_REQ: &str = include_str!("fixtures/convert/wrong-root-element.xml");

// ---------------------------------------------------------------------------
// 8.6 — round-trip / determinism
// ---------------------------------------------------------------------------

#[test]
fn clean_fixture_with_fs_emits_zero_findings() {
    let r = convert_requisition_xml(CLEAN_REQ, Some(CLEAN_FS), None).unwrap();
    assert_eq!(r.foreign_source, "acme-prod");
    assert!(
        r.findings.is_empty(),
        "expected zero findings on the clean fixture, got: {:#?}",
        r.findings
    );
    assert_eq!(r.exit_code(), 0);
    assert!(r.yaml.is_some(), "clean fixture should emit YAML");
}

#[test]
fn clean_fixture_matches_golden_yaml() {
    // Stronger than the two-pass-self-equality check: pin the
    // converter's output against a committed golden file, so that
    // a serde_norway version bump, HashMap iteration leak, or any
    // platform-dependent emission change shows up as a discrete
    // CI failure rather than slipping through.
    //
    // Re-bless with:
    //   cargo run --example dump_clean_golden -p onmsctl-provisioning \
    //     > crates/onmsctl-provisioning/tests/fixtures/convert/clean-requisition.expected.yaml
    let r = convert_requisition_xml(CLEAN_REQ, Some(CLEAN_FS), None).unwrap();
    let actual = r.yaml.as_ref().expect("yaml emitted");
    assert_eq!(
        actual, CLEAN_EXPECTED_YAML,
        "golden YAML drift — output diverges from clean-requisition.expected.yaml. \
         Re-run the dump_clean_golden example and commit the updated fixture if the \
         change is intentional."
    );
}

#[test]
fn clean_fixture_two_pass_is_byte_identical() {
    // Task 8.6: "re-renders to identical canonical YAML across two
    // passes". Run convert twice on the same input; the YAML output
    // must be byte-equal so callers (CI, diff tools, git) see a
    // stable artifact.
    let pass1 = convert_requisition_xml(CLEAN_REQ, Some(CLEAN_FS), None).unwrap();
    let pass2 = convert_requisition_xml(CLEAN_REQ, Some(CLEAN_FS), None).unwrap();
    assert_eq!(
        pass1.yaml.as_ref().unwrap(),
        pass2.yaml.as_ref().unwrap(),
        "two passes over the same input produced different YAML — determinism violated"
    );
}

#[test]
fn clean_fixture_yaml_round_trips_through_local_model() {
    // The convert output must be valid input for the apply path —
    // parse the emitted YAML back into a RequisitionLocal and assert
    // every modeled field that the fixture set is visible.
    let r = convert_requisition_xml(CLEAN_REQ, Some(CLEAN_FS), None).unwrap();
    let yaml = r.yaml.as_ref().unwrap();
    let local: RequisitionLocal =
        serde_norway::from_str(yaml).expect("emitted YAML re-parses through RequisitionLocal");

    assert_eq!(local.metadata.name, "acme-prod");
    assert_eq!(local.spec.nodes.len(), 3);

    // Spot-check that the fixture's modeled fields survived the
    // round-trip.
    let web = local
        .spec
        .nodes
        .iter()
        .find(|n| n.foreign_id == "web01")
        .unwrap();
    assert_eq!(web.label, "web01.acme.com");
    assert_eq!(web.interfaces.len(), 2);
    assert_eq!(web.categories.len(), 2);
    assert_eq!(web.assets.len(), 3);
    assert_eq!(web.assets.get("city").map(String::as_str), Some("NYC"));

    let fs = local.spec.foreign_source.as_ref().expect("FS present");
    assert_eq!(fs.scan_interval.as_deref(), Some("1d"));
    assert_eq!(fs.detectors.len(), 2);
    assert_eq!(fs.policies.len(), 1);
}

#[test]
fn clean_fixture_without_fs_emits_only_pr004_info() {
    // The same requisition without a matching FS XML must produce
    // ONLY a single PR004 (info). PR004 is informational, so the
    // exit code stays 0.
    let r = convert_requisition_xml(CLEAN_REQ, None, None).unwrap();
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.findings[0].code, FindingCode::Pr004);
    assert_eq!(r.findings[0].severity, Severity::Info);
    assert_eq!(r.exit_code(), 0);
    // YAML still emitted — PR004 doesn't block.
    assert!(r.yaml.is_some());
    // And the YAML correctly omits spec.foreignSource (portable form).
    let yaml = r.yaml.as_ref().unwrap();
    assert!(
        !yaml.contains("foreignSource:"),
        "portable YAML must omit spec.foreignSource:\n{yaml}"
    );
}

// ---------------------------------------------------------------------------
// 8.7 — edge cases
// ---------------------------------------------------------------------------

#[test]
fn unmodeled_elements_fixture_raises_seven_pr001s() {
    // The fixture deliberately uses every known-unmodeled field in
    // a single node + interface + service. Catalog (per
    // flag_unmodeled): @location, @city, node-<meta-data>,
    // interface.@status, interface.@descr, interface-<meta-data>,
    // service-<meta-data> = 7 total.
    let r = convert_requisition_xml(UNMODELED_REQ, None, None).unwrap();
    let pr001s: Vec<_> = r
        .findings
        .iter()
        .filter(|f| f.code == FindingCode::Pr001)
        .collect();
    assert_eq!(
        pr001s.len(),
        7,
        "expected exactly 7 PR001 findings for the catalog of unmodeled fields, got {}: {:#?}",
        pr001s.len(),
        pr001s
    );
    // Warnings present → exit code 1 (PR004 also fires as Info but
    // doesn't bump the exit code further).
    assert_eq!(r.exit_code(), 1);
}

#[test]
fn truly_unknown_element_is_preserved_via_extras_passthrough() {
    // The fixture contains a `<future-extension>` element that no
    // typed XML DTO models. Per the `harden-provisioning-and-
    // eventconf-parity` change (Option B / full passthrough), each
    // XML DTO carries `#[serde(flatten)] extras` so quick_xml routes
    // any unmodeled attribute or child element into the catch-all
    // map. `flag_unmodeled` then emits PR001 and records the data
    // on `metadata.x-onmsctl-unmodeled`.
    let r = convert_requisition_xml(UNKNOWN_ELEMENT_REQ, None, None).unwrap();
    let pr001s: Vec<_> = r
        .findings
        .iter()
        .filter(|f| f.code == FindingCode::Pr001)
        .collect();
    assert!(
        pr001s.iter().any(|f| f.message.contains("future-extension")),
        "expected PR001 for `<future-extension>`; got: {:#?}",
        pr001s
    );
    // PR004 still fires (no FS XML) — that's expected.
    assert!(r.findings.iter().any(|f| f.code == FindingCode::Pr004));

    // And the YAML carries the captured passthrough.
    let yaml = r.yaml.as_ref().expect("yaml emitted");
    let parsed: serde_norway::Value =
        serde_norway::from_str(yaml).expect("emitted yaml round-trips");
    let nodes = parsed
        .get("metadata")
        .and_then(|m| m.get("x-onmsctl-unmodeled"))
        .and_then(|u| u.get("nodes"))
        .expect("nodes entry present on annotation");
    assert!(
        nodes.get("web01").and_then(|n| n.get("future-extension")).is_some(),
        "annotation should carry the unknown element; got: {nodes:#?}"
    );
}

#[test]
fn malformed_xml_returns_err_with_useful_message() {
    let err = convert_requisition_xml(MALFORMED_REQ, None, None).unwrap_err();
    assert!(
        err.contains("parse error"),
        "expected parse-error context in message, got: {err}"
    );
}

#[test]
fn wrong_root_element_returns_err() {
    // <events> instead of <model-import>. quick_xml's serde rejects
    // a root that doesn't match the DTO; convert surfaces the
    // wrapped error rather than panicking.
    let err = convert_requisition_xml(WRONG_ROOT_REQ, None, None).unwrap_err();
    assert!(err.contains("parse error"));
}

// ---------------------------------------------------------------------------
// Cross-cutting: the FS xmlns="http://xmlns.opennms.org/xsd/config/foreign-source"
// must not break parsing. The clean fixture carries the xmlns declaration.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// `convert_directory` integration — the function the CLI actually calls.
// Lays out fixtures in a tempdir and crosses the filesystem boundary.
// ---------------------------------------------------------------------------

#[test]
fn convert_directory_clean_pair_with_fs_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let req_dir = dir.path().join("requisitions");
    let fs_dir = dir.path().join("foreign-sources");
    std::fs::create_dir(&req_dir).unwrap();
    std::fs::create_dir(&fs_dir).unwrap();

    // Pair the clean fixtures by matching basename (per the
    // convention enforced by convert_directory).
    std::fs::write(req_dir.join("acme-prod.xml"), CLEAN_REQ).unwrap();
    std::fs::write(fs_dir.join("acme-prod.xml"), CLEAN_FS).unwrap();

    let results = convert_directory(&req_dir, Some(&fs_dir)).unwrap();
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.foreign_source, "acme-prod");
    assert!(
        r.findings.is_empty(),
        "expected zero findings: {:#?}",
        r.findings
    );
    assert_eq!(r.exit_code(), 0);
    assert!(r.yaml.is_some());
}

#[test]
fn convert_directory_orphan_fs_raises_pr002() {
    let dir = tempfile::tempdir().unwrap();
    let req_dir = dir.path().join("requisitions");
    let fs_dir = dir.path().join("foreign-sources");
    std::fs::create_dir(&req_dir).unwrap();
    std::fs::create_dir(&fs_dir).unwrap();

    // Two FS files but only one matching requisition — the second
    // is orphaned and must surface as PR002.
    std::fs::write(req_dir.join("acme-prod.xml"), CLEAN_REQ).unwrap();
    std::fs::write(fs_dir.join("acme-prod.xml"), CLEAN_FS).unwrap();
    std::fs::write(fs_dir.join("ghost.xml"), CLEAN_FS).unwrap();

    let results = convert_directory(&req_dir, Some(&fs_dir)).unwrap();
    assert_eq!(results.len(), 2);

    // The orphan result has no YAML + PR002.
    let ghost = results
        .iter()
        .find(|r| r.foreign_source == "ghost")
        .expect("orphan ghost result present");
    assert!(ghost.yaml.is_none());
    assert!(
        ghost.findings.iter().any(|f| f.code == FindingCode::Pr002),
        "expected PR002 on orphan: {:#?}",
        ghost.findings
    );
}

#[test]
fn convert_directory_without_fs_dir_pr004_per_requisition() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("acme.xml"), CLEAN_REQ).unwrap();
    std::fs::write(dir.path().join("beta.xml"), CLEAN_REQ).unwrap();

    let results = convert_directory(dir.path(), None).unwrap();
    assert_eq!(results.len(), 2);
    // Every result emits exactly one PR004 (Info) because there's
    // no FS source dir to pair with.
    for r in &results {
        assert!(
            r.findings.iter().any(|f| f.code == FindingCode::Pr004),
            "expected PR004 on {}: {:#?}",
            r.foreign_source,
            r.findings
        );
        assert_eq!(r.exit_code(), 0);
    }
}

// ---------------------------------------------------------------------------
// README example — validate the YAML file we tell operators to use is
// actually parseable by RequisitionLocal. Catches divergence between
// the README example and the local model on every test run.
// ---------------------------------------------------------------------------

#[test]
fn readme_example_acme_prod_parses_through_local_model() {
    const EXAMPLE: &str = include_str!("../../../examples/requisition-acme-prod.yaml");
    let local: RequisitionLocal =
        serde_norway::from_str(EXAMPLE).expect("examples/requisition-acme-prod.yaml parses");
    assert_eq!(local.metadata.name, "acme-prod");
    assert!(
        local.spec.foreign_source.is_some(),
        "example demonstrates the pinned-style YAML (has spec.foreignSource)"
    );
    // Exactly 3 nodes (web/db/cache) — pinned so a regression that
    // silently deletes one of them breaks the test rather than
    // passing as "multiple nodes".
    assert_eq!(
        local.spec.nodes.len(),
        3,
        "the README example must keep exactly 3 nodes (web/db/cache)"
    );
}

#[test]
fn foreign_source_with_default_xmlns_parses() {
    // Already covered by clean_fixture_with_fs_emits_zero_findings,
    // but call it out explicitly so a future quick_xml bump that
    // changes namespace handling shows up as a discrete failure
    // rather than buried in the determinism test.
    let r = convert_requisition_xml(CLEAN_REQ, Some(CLEAN_FS), None).unwrap();
    let yaml = r.yaml.as_ref().unwrap();
    assert!(yaml.contains("scanInterval: 1d"));
    assert!(yaml.contains("IcmpDetector"));
    assert!(yaml.contains("SnmpDetector"));
    assert!(yaml.contains("NodeCategorySettingPolicy"));
}
