/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Conversion pipeline: XML → `RequisitionLocal` → YAML.
//!
//! Two entry points:
//!
//! - [`convert_requisition_xml`]: single requisition + optional
//!   matching foreign-source. Returns a [`ConversionResult`] with the
//!   emitted YAML (when no error-severity findings) and the structured
//!   findings list.
//! - [`convert_directory`]: walk a directory of requisition XML files,
//!   auto-discover matching foreign-source XML by filename, and emit
//!   one result per requisition. Surfaces directory-level findings
//!   (orphan foreign-sources, missing matching FS files).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::convert::finding::{Finding, FindingCode, Severity};
use crate::convert::xml::{
    Extras, ForeignSourceXml, InterfaceXml, NodeXml, ParameterXml, RequisitionXml,
    parse_foreign_source, parse_requisition,
};
use crate::model::{
    ApiVersion, Detector, ForeignSourceSpec, Interface, Kind, Metadata, Node, Parameter, Policy,
    RequisitionLocal, SnmpPrimary, Spec,
};

/// Outcome of converting a single requisition (with optional FS).
#[derive(Debug, Clone, Serialize)]
pub struct ConversionResult {
    /// The requisition's foreign-source name (taken from the XML
    /// root `foreign-source` attribute). Useful for CLI summaries
    /// when the input path is `None` (stdin not currently supported,
    /// but reserved).
    pub foreign_source: String,
    /// Emitted YAML. `None` when error-severity findings prevented
    /// emission (today no PR code is Error-severity, but reserved
    /// for the matrix to grow).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaml: Option<String>,
    /// Structured findings in discovery order.
    pub findings: Vec<Finding>,
}

impl ConversionResult {
    /// Exit code per design D4 (mirrored from eventconf):
    /// - `0`: no findings, or info-only.
    /// - `1`: warnings present (YAML still emitted).
    /// - `2`: errors present (YAML withheld).
    pub fn exit_code(&self) -> i32 {
        if self.findings.iter().any(|f| f.severity == Severity::Error) {
            2
        } else if self
            .findings
            .iter()
            .any(|f| f.severity == Severity::Warning)
        {
            1
        } else {
            0
        }
    }
}

/// Convert one requisition XML + optional matching foreign-source XML
/// into a `kind: Requisition` YAML document.
///
/// `source_path` is attached to any findings raised during this call
/// so the operator knows which file flagged what.
pub fn convert_requisition_xml(
    req_xml: &str,
    fs_xml: Option<&str>,
    source_path: Option<PathBuf>,
) -> Result<ConversionResult, String> {
    let req =
        parse_requisition(req_xml).map_err(|e| format!("requisition XML parse error: {e}"))?;
    let fs_dto = match fs_xml {
        Some(s) => Some(
            parse_foreign_source(s).map_err(|e| format!("foreign-source XML parse error: {e}"))?,
        ),
        None => None,
    };

    let mut findings: Vec<Finding> = Vec::new();
    let local = build_local(&req, fs_dto.as_ref(), &mut findings, source_path.as_ref());

    // PR004: no foreign-source XML in scope. Informational — operator
    // running portable-style YAML wants exactly this.
    if fs_dto.is_none() {
        findings.push(
            Finding::new(
                FindingCode::Pr004,
                format!(
                    "Requisition/{} has no matching foreign-source XML; emitted YAML inherits Horizon's default-FS",
                    req.foreign_source
                ),
            )
            .opt_source(source_path.as_ref()),
        );
    }

    let yaml =
        serde_norway::to_string(&local).map_err(|e| format!("YAML serialization error: {e}"))?;

    Ok(ConversionResult {
        foreign_source: req.foreign_source.clone(),
        yaml: Some(yaml),
        findings,
    })
}

/// Walk `xml_dir` for `.xml` files, auto-discover matching FS files
/// (either in the same directory or in `fs_dir` if supplied), and
/// emit one [`ConversionResult`] per requisition.
///
/// Filename convention: a requisition `acme-prod.xml` matches a
/// foreign-source `acme-prod.xml` (same basename, in the FS directory).
///
/// Directory-level findings:
/// - **PR002** for any FS file in `fs_dir` whose basename doesn't
///   match a requisition in `xml_dir` (orphan).
pub fn convert_directory(
    xml_dir: &Path,
    fs_dir: Option<&Path>,
) -> Result<Vec<ConversionResult>, String> {
    let req_files = list_xml_files(xml_dir)?;
    let fs_files: BTreeMap<String, PathBuf> = match fs_dir {
        Some(d) => list_xml_files(d)?
            .into_iter()
            .map(|p| (basename_without_xml(&p), p))
            .collect(),
        None => BTreeMap::new(),
    };

    // Auto-discover FS files for each requisition.
    let req_basenames: std::collections::BTreeSet<String> =
        req_files.iter().map(|p| basename_without_xml(p)).collect();

    // PR002: orphan FS files (in fs_dir but no matching requisition).
    let mut orphans: Vec<ConversionResult> = Vec::new();
    for (name, path) in &fs_files {
        if !req_basenames.contains(name) {
            orphans.push(ConversionResult {
                foreign_source: name.clone(),
                yaml: None,
                findings: vec![
                    Finding::new(
                        FindingCode::Pr002,
                        format!("orphan foreign-source XML '{name}'; no matching requisition"),
                    )
                    .with_source(path.clone()),
                ],
            });
        }
    }

    let mut results: Vec<ConversionResult> = Vec::new();
    for req_path in &req_files {
        let req_xml = std::fs::read_to_string(req_path)
            .map_err(|e| format!("read {}: {e}", req_path.display()))?;
        let name = basename_without_xml(req_path);
        let fs_xml = fs_files
            .get(&name)
            .map(std::fs::read_to_string)
            .transpose()
            .map_err(|e| format!("read matching FS for {name}: {e}"))?;
        let result = convert_requisition_xml(&req_xml, fs_xml.as_deref(), Some(req_path.clone()))?;
        results.push(result);
    }

    results.extend(orphans);
    Ok(results)
}

// ---------------------------------------------------------------------------
// XML → local model
// ---------------------------------------------------------------------------

fn build_local(
    req: &RequisitionXml,
    fs: Option<&ForeignSourceXml>,
    findings: &mut Vec<Finding>,
    source_path: Option<&PathBuf>,
) -> RequisitionLocal {
    // PR001 surfaces XML content that exists in the source but has
    // no place in the local model — both the enumerated catalog
    // (node.@location / @city / @status / @descr / <meta-data>) AND
    // arbitrary custom attrs / child elements captured by the
    // `#[serde(flatten)] extras` on each XML DTO. The data is
    // recorded in a nested `serde_norway::Mapping` (no flat dotted
    // keys, so foreign-ids and IPs containing `.` don't collide)
    // and surfaces via the `metadata.x-onmsctl-unmodeled` annotation
    // on the emitted YAML.
    let mut unmodeled: serde_norway::Mapping = serde_norway::Mapping::new();
    flag_unmodeled(req, findings, &mut unmodeled, source_path);

    RequisitionLocal {
        api_version: ApiVersion,
        kind: Kind,
        metadata: Metadata {
            name: req.foreign_source.clone(),
            unmodeled: if unmodeled.is_empty() {
                None
            } else {
                Some(unmodeled)
            },
        },
        spec: Spec {
            foreign_source: fs.map(convert_fs),
            nodes: req.nodes.iter().map(convert_node).collect(),
        },
    }
}

fn convert_node(n: &NodeXml) -> Node {
    let assets: BTreeMap<String, String> = n
        .assets
        .iter()
        .map(|a| (a.name.clone(), a.value.clone()))
        .collect();
    Node {
        foreign_id: n.foreign_id.clone(),
        label: n.node_label.clone(),
        interfaces: n.interfaces.iter().map(convert_interface).collect(),
        categories: n.categories.iter().map(|c| c.name.clone()).collect(),
        assets,
    }
}

fn convert_interface(i: &InterfaceXml) -> Interface {
    Interface {
        ip: i.ip_addr.clone(),
        services: i
            .monitored_services
            .iter()
            .map(|s| s.service_name.clone())
            .collect(),
        snmp_primary: i.snmp_primary.as_deref().and_then(parse_snmp_primary),
    }
}

fn parse_snmp_primary(s: &str) -> Option<SnmpPrimary> {
    match s {
        "P" => Some(SnmpPrimary::P),
        "S" => Some(SnmpPrimary::S),
        "N" => Some(SnmpPrimary::N),
        _ => None,
    }
}

fn convert_fs(f: &ForeignSourceXml) -> ForeignSourceSpec {
    ForeignSourceSpec {
        scan_interval: f.scan_interval.clone(),
        detectors: f
            .detectors
            .detector
            .iter()
            .map(|d| Detector {
                name: d.name.clone(),
                class: Some(d.class.clone()),
                parameters: d.parameter.iter().map(convert_parameter).collect(),
            })
            .collect(),
        policies: f
            .policies
            .policy
            .iter()
            .map(|p| Policy {
                name: p.name.clone(),
                class: p.class.clone(),
                parameters: p.parameter.iter().map(convert_parameter).collect(),
            })
            .collect(),
    }
}

fn convert_parameter(p: &ParameterXml) -> Parameter {
    Parameter {
        key: p.key.clone(),
        value: p.value.clone(),
    }
}

/// Walk the requisition XML for unmodeled content, emit PR001
/// findings, AND record everything into `unmodeled` as a nested
/// `serde_norway::Mapping` so the YAML round-trips it via the
/// `metadata.x-onmsctl-unmodeled` annotation. Two sources of
/// unmodeled content:
///
/// 1. **Enumerated catalog**: known-but-unmodeled fields on the
///    typed XML DTOs (`node.@location`, `@city`, `interface.@status`,
///    `@descr`, all `<meta-data>` elements). PR001 finding text
///    names each.
/// 2. **Extras passthrough**: anything captured by the
///    `#[serde(flatten)] extras` field on each XML DTO — custom
///    vendor attrs, unknown child elements, future Horizon
///    additions. PR001 finding text names each.
///
/// Annotation keys are XML-attribute-prefix-stripped (`@location` →
/// `location`) so operators reading the YAML don't have to know
/// about XML's `@` convention. Catalog keys take precedence over
/// extras when names collide (the catalog is the documented contract;
/// extras would otherwise duplicate it).
fn flag_unmodeled(
    req: &RequisitionXml,
    findings: &mut Vec<Finding>,
    unmodeled: &mut serde_norway::Mapping,
    src: Option<&PathBuf>,
) {
    // Root-level extras (custom attrs / child elements on
    // `<model-import>`). Surface as top-level keys on the
    // annotation alongside `nodes`.
    record_extras(
        &req.extras,
        unmodeled,
        findings,
        || "model-import (root)".to_string(),
        src,
    );

    let mut nodes_map = serde_norway::Mapping::new();
    for n in &req.nodes {
        let mut node_map = serde_norway::Mapping::new();
        if let Some(loc) = n.location.as_deref() {
            findings.push(
                Finding::new(
                    FindingCode::Pr001,
                    format!(
                        "node '{}': 'location' is not modeled in YAML (preserved as annotation)",
                        n.foreign_id
                    ),
                )
                .opt_source(src),
            );
            node_map.insert(
                "location".into(),
                serde_norway::Value::String(loc.to_string()),
            );
        }
        if let Some(city) = n.city.as_deref() {
            findings.push(
                Finding::new(
                    FindingCode::Pr001,
                    format!(
                        "node '{}': 'city' is not modeled in YAML (preserved as annotation — use asset for round-trip)",
                        n.foreign_id
                    ),
                )
                .opt_source(src),
            );
            node_map.insert("city".into(), serde_norway::Value::String(city.to_string()));
        }
        if !n.meta_data.is_empty() {
            findings.push(
                Finding::new(
                    FindingCode::Pr001,
                    format!(
                        "node '{}': <meta-data> elements are not modeled in YAML ({} preserved as annotation)",
                        n.foreign_id,
                        n.meta_data.len()
                    ),
                )
                .opt_source(src),
            );
            node_map.insert("meta-data".into(), meta_data_to_value(&n.meta_data));
        }
        // Node-level extras passthrough.
        record_extras(
            &n.extras,
            &mut node_map,
            findings,
            || format!("node '{}'", n.foreign_id),
            src,
        );

        let mut ifaces_map = serde_norway::Mapping::new();
        for iface in &n.interfaces {
            let mut iface_map = serde_norway::Mapping::new();
            // PR005 (invalid snmp-primary) is a hard drop, not
            // unmodeled — the value is invalid, not just
            // unrepresented. Keep out of the annotation.
            if let Some(val) = iface.snmp_primary.as_deref()
                && parse_snmp_primary(val).is_none()
            {
                findings.push(
                    Finding::new(
                        FindingCode::Pr005,
                        format!(
                            "node '{}' interface {}: snmp-primary='{}' is not P/S/N (dropped)",
                            n.foreign_id, iface.ip_addr, val
                        ),
                    )
                    .opt_source(src),
                );
            }
            if let Some(status) = iface.status.as_deref() {
                findings.push(
                    Finding::new(
                        FindingCode::Pr001,
                        format!(
                            "node '{}' interface {}: 'status' is not modeled in YAML (preserved as annotation)",
                            n.foreign_id, iface.ip_addr
                        ),
                    )
                    .opt_source(src),
                );
                iface_map.insert(
                    "status".into(),
                    serde_norway::Value::String(status.to_string()),
                );
            }
            if let Some(descr) = iface.descr.as_deref() {
                findings.push(
                    Finding::new(
                        FindingCode::Pr001,
                        format!(
                            "node '{}' interface {}: 'descr' is not modeled in YAML (preserved as annotation)",
                            n.foreign_id, iface.ip_addr
                        ),
                    )
                    .opt_source(src),
                );
                iface_map.insert(
                    "descr".into(),
                    serde_norway::Value::String(descr.to_string()),
                );
            }
            if !iface.meta_data.is_empty() {
                findings.push(
                    Finding::new(
                        FindingCode::Pr001,
                        format!(
                            "node '{}' interface {}: <meta-data> elements not modeled ({} preserved as annotation)",
                            n.foreign_id,
                            iface.ip_addr,
                            iface.meta_data.len()
                        ),
                    )
                    .opt_source(src),
                );
                iface_map.insert("meta-data".into(), meta_data_to_value(&iface.meta_data));
            }
            // Interface-level extras passthrough.
            record_extras(
                &iface.extras,
                &mut iface_map,
                findings,
                || format!("node '{}' interface {}", n.foreign_id, iface.ip_addr),
                src,
            );

            // Services with unmodeled content.
            let mut svcs_map = serde_norway::Mapping::new();
            for svc in &iface.monitored_services {
                let mut svc_map = serde_norway::Mapping::new();
                if !svc.meta_data.is_empty() {
                    findings.push(
                        Finding::new(
                            FindingCode::Pr001,
                            format!(
                                "node '{}' interface {} service '{}': <meta-data> elements not modeled ({} preserved as annotation)",
                                n.foreign_id,
                                iface.ip_addr,
                                svc.service_name,
                                svc.meta_data.len()
                            ),
                        )
                        .opt_source(src),
                    );
                    svc_map.insert("meta-data".into(), meta_data_to_value(&svc.meta_data));
                }
                record_extras(
                    &svc.extras,
                    &mut svc_map,
                    findings,
                    || {
                        format!(
                            "node '{}' interface {} service '{}'",
                            n.foreign_id, iface.ip_addr, svc.service_name
                        )
                    },
                    src,
                );
                if !svc_map.is_empty() {
                    svcs_map.insert(
                        svc.service_name.clone().into(),
                        serde_norway::Value::Mapping(svc_map),
                    );
                }
            }
            if !svcs_map.is_empty() {
                iface_map.insert("services".into(), serde_norway::Value::Mapping(svcs_map));
            }
            if !iface_map.is_empty() {
                ifaces_map.insert(
                    iface.ip_addr.clone().into(),
                    serde_norway::Value::Mapping(iface_map),
                );
            }
        }
        if !ifaces_map.is_empty() {
            node_map.insert(
                "interfaces".into(),
                serde_norway::Value::Mapping(ifaces_map),
            );
        }
        if !node_map.is_empty() {
            nodes_map.insert(
                n.foreign_id.clone().into(),
                serde_norway::Value::Mapping(node_map),
            );
        }
    }
    if !nodes_map.is_empty() {
        unmodeled.insert("nodes".into(), serde_norway::Value::Mapping(nodes_map));
    }
}

/// Strip the leading `@` from an XML attribute key (quick-xml's
/// convention) so the operator-facing YAML uses bare names. Child
/// element keys flow through unchanged.
fn strip_at(key: &str) -> &str {
    key.strip_prefix('@').unwrap_or(key)
}

/// Copy unclaimed fields from a `#[serde(flatten)] extras` map into
/// the annotation `target` mapping, stripping `@` prefixes from
/// keys. Emits a PR001 finding per entry — `context_label` builds
/// the human-readable scope string lazily (only called when extras
/// are present).
///
/// Collision policy: if an extras key collides with a catalog key
/// already on `target` (e.g. `<location>NY</location>` child vs
/// `@location="HQ"` attr — both strip to `location`), the catalog
/// value is kept and a distinct PR001 finding is emitted reporting
/// the shadowed child element, so the data-loss is auditable rather
/// than silent.
fn record_extras<L: Fn() -> String>(
    extras: &Extras,
    target: &mut serde_norway::Mapping,
    findings: &mut Vec<Finding>,
    context_label: L,
    src: Option<&PathBuf>,
) {
    if extras.is_empty() {
        return;
    }
    let scope = context_label();
    for (raw_key_value, value) in extras.iter() {
        let Some(raw_key) = raw_key_value.as_str() else {
            continue;
        };
        let stripped = strip_at(raw_key).to_string();
        let key_value = serde_norway::Value::String(stripped.clone());
        if target.contains_key(&key_value) {
            // Catalog already claimed this name — record the
            // shadowed child as an auditable finding instead of
            // dropping it silently.
            findings.push(
                Finding::new(
                    FindingCode::Pr001,
                    format!(
                        "{scope}: '{raw_key}' collides with catalog key '{stripped}' (child shadowed, value not preserved in annotation)"
                    ),
                )
                .opt_source(src),
            );
            continue;
        }
        findings.push(
            Finding::new(
                FindingCode::Pr001,
                format!("{scope}: '{stripped}' is not modeled in YAML (preserved as annotation)"),
            )
            .opt_source(src),
        );
        target.insert(key_value, value.clone());
    }
}

/// Render a sequence of `<meta-data>` elements as a YAML sequence of
/// `{context, key, value}` maps for the unmodeled annotation.
fn meta_data_to_value(items: &[crate::convert::xml::MetaDataXml]) -> serde_norway::Value {
    let seq: Vec<serde_norway::Value> = items
        .iter()
        .map(|m| {
            let mut map = serde_norway::Mapping::new();
            map.insert(
                serde_norway::Value::String("context".into()),
                serde_norway::Value::String(m.context.clone()),
            );
            map.insert(
                serde_norway::Value::String("key".into()),
                serde_norway::Value::String(m.key.clone()),
            );
            map.insert(
                serde_norway::Value::String("value".into()),
                serde_norway::Value::String(m.value.clone()),
            );
            serde_norway::Value::Mapping(map)
        })
        .collect();
    serde_norway::Value::Sequence(seq)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convenience extension on `Finding`: attach an optional source path
/// without forcing the caller to branch.
trait OptSource {
    fn opt_source(self, p: Option<&PathBuf>) -> Self;
}

impl OptSource for Finding {
    fn opt_source(self, p: Option<&PathBuf>) -> Self {
        match p {
            Some(path) => self.with_source(path.clone()),
            None => self,
        }
    }
}

fn list_xml_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("xml"))
        .collect();
    out.sort();
    Ok(out)
}

fn basename_without_xml(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const REQ_BASIC: &str = r#"<?xml version="1.0"?>
<model-import foreign-source="acme-prod">
  <node foreign-id="web01" node-label="web01.acme">
    <interface ip-addr="10.0.0.1" snmp-primary="P">
      <monitored-service service-name="HTTP"/>
    </interface>
    <category name="Production"/>
    <asset name="city" value="NYC"/>
  </node>
</model-import>"#;

    const REQ_WITH_UNMODELED: &str = r#"<?xml version="1.0"?>
<model-import foreign-source="acme-prod">
  <node foreign-id="web01" node-label="web01" location="HQ" city="NYC">
    <interface ip-addr="10.0.0.1" status="1" descr="primary nic">
      <monitored-service service-name="HTTP"/>
      <meta-data context="r" key="k" value="v"/>
    </interface>
    <meta-data context="r" key="owner" value="ops"/>
  </node>
</model-import>"#;

    const FS_BASIC: &str = r#"<?xml version="1.0"?>
<foreign-source name="acme-prod">
  <scan-interval>1d</scan-interval>
  <detectors>
    <detector name="SNMP" class="org.opennms.netmgt.provision.detector.snmp.SnmpDetector"/>
  </detectors>
</foreign-source>"#;

    #[test]
    fn convert_emits_yaml_and_pr004_when_no_fs() {
        let r = convert_requisition_xml(REQ_BASIC, None, None).unwrap();
        assert_eq!(r.foreign_source, "acme-prod");
        let yaml = r.yaml.as_ref().expect("yaml emitted");
        assert!(yaml.contains("apiVersion: provisioning.opennms.org/v1"));
        assert!(yaml.contains("kind: Requisition"));
        assert!(yaml.contains("name: acme-prod"));
        assert!(yaml.contains("foreignId: web01"));
        assert!(yaml.contains("- HTTP"));
        // PR004 because no FS XML was supplied.
        assert!(r.findings.iter().any(|f| f.code == FindingCode::Pr004));
        // Exit code is 0 — PR004 is Info, not Warning/Error.
        assert_eq!(r.exit_code(), 0);
    }

    #[test]
    fn convert_with_fs_emits_yaml_and_no_pr004() {
        let r = convert_requisition_xml(REQ_BASIC, Some(FS_BASIC), None).unwrap();
        let yaml = r.yaml.as_ref().unwrap();
        // foreignSource block is now present.
        assert!(yaml.contains("foreignSource"));
        assert!(yaml.contains("scanInterval: 1d"));
        assert!(yaml.contains("SnmpDetector"));
        // No PR004.
        assert!(!r.findings.iter().any(|f| f.code == FindingCode::Pr004));
    }

    #[test]
    fn unmodeled_attributes_each_raise_pr001() {
        let r = convert_requisition_xml(REQ_WITH_UNMODELED, None, None).unwrap();
        let pr001s: Vec<_> = r
            .findings
            .iter()
            .filter(|f| f.code == FindingCode::Pr001)
            .collect();
        // Expected catalog findings (REQ_WITH_UNMODELED in this file):
        //   1. node 'web01' @location
        //   2. node 'web01' @city
        //   3. node 'web01' <meta-data> (1 entry)
        //   4. interface 10.0.0.1 @status
        //   5. interface 10.0.0.1 @descr
        //   6. interface 10.0.0.1 <meta-data> (1 entry)
        // No service-level <meta-data> in the fixture. Total = 6.
        assert_eq!(
            pr001s.len(),
            6,
            "expected exactly 6 PR001 findings, got {}: {:#?}",
            pr001s.len(),
            pr001s
        );
        // Spot-check the categorization (messages now show stripped
        // YAML keys, not the XML `@` prefix).
        assert!(pr001s.iter().any(|f| f.message.contains("'location'")));
        assert!(pr001s.iter().any(|f| f.message.contains("'city'")));
        assert!(pr001s.iter().any(|f| f.message.contains("'status'")));
        assert!(pr001s.iter().any(|f| f.message.contains("'descr'")));
        // Exit code is 1 because PR001s are Warnings.
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn unmodeled_content_round_trips_via_metadata_annotation() {
        // PR001 unmodeled attributes/elements are no longer silently
        // dropped — they're preserved under
        // `metadata.x-onmsctl-unmodeled` as a NESTED YAML map (not
        // flat dotted keys, so foreign-ids and IPs containing `.`
        // don't collide). Attribute `@` prefixes are stripped from
        // keys for operator-facing readability.
        let r = convert_requisition_xml(REQ_WITH_UNMODELED, None, None).unwrap();
        let yaml = r.yaml.as_ref().expect("yaml emitted");
        // Deserialize back to verify the structure rather than
        // string-match brittle YAML formatter output.
        let parsed: serde_norway::Value =
            serde_norway::from_str(yaml).expect("emitted yaml round-trips");
        let unmodeled = parsed
            .get("metadata")
            .and_then(|m| m.get("x-onmsctl-unmodeled"))
            .expect("x-onmsctl-unmodeled present on metadata");
        let nodes = unmodeled.get("nodes").expect("nodes key present");
        let web01 = nodes.get("web01").expect("web01 node entry present");
        assert_eq!(web01.get("location").and_then(|v| v.as_str()), Some("HQ"));
        assert_eq!(web01.get("city").and_then(|v| v.as_str()), Some("NYC"));
        let node_md = web01
            .get("meta-data")
            .and_then(|v| v.as_sequence())
            .expect("node meta-data is a sequence");
        assert_eq!(node_md.len(), 1);
        assert_eq!(
            node_md[0].get("key").and_then(|v| v.as_str()),
            Some("owner")
        );

        // Interface entries nested under interfaces.<ip>.
        let ifaces = web01
            .get("interfaces")
            .and_then(|v| v.as_mapping())
            .expect("interfaces map present");
        let iface = ifaces
            .get("10.0.0.1")
            .expect("interface 10.0.0.1 present (IP with dots maps cleanly)");
        assert_eq!(iface.get("status").and_then(|v| v.as_str()), Some("1"));
        assert_eq!(
            iface.get("descr").and_then(|v| v.as_str()),
            Some("primary nic")
        );
        let iface_md = iface
            .get("meta-data")
            .and_then(|v| v.as_sequence())
            .expect("iface meta-data is a sequence");
        assert_eq!(iface_md.len(), 1);
        assert_eq!(iface_md[0].get("key").and_then(|v| v.as_str()), Some("k"));
    }

    #[test]
    fn unmodeled_passthrough_captures_custom_vendor_attrs() {
        // Option B / full passthrough: arbitrary unmodeled XML
        // (custom vendor attrs, unknown child elements) flows
        // through `#[serde(flatten)] extras` into the annotation.
        // `@` prefixes are stripped for operator-facing keys.
        const REQ_WITH_CUSTOM: &str = r#"<?xml version="1.0"?>
<model-import foreign-source="acme-prod" some-vendor-attr="vendor-x">
  <node foreign-id="web01.acme.com" node-label="web01"
        legacy-tag="tag-1" location="HQ">
    <interface ip-addr="10.0.0.1" custom-port-mode="trunk"/>
  </node>
</model-import>"#;
        let r = convert_requisition_xml(REQ_WITH_CUSTOM, None, None).unwrap();
        let yaml = r.yaml.as_ref().expect("yaml emitted");
        let parsed: serde_norway::Value = serde_norway::from_str(yaml).unwrap();
        let unmodeled = parsed
            .get("metadata")
            .and_then(|m| m.get("x-onmsctl-unmodeled"))
            .expect("annotation present");

        // Root-level extras captured AS top-level keys (stripped @).
        assert_eq!(
            unmodeled.get("some-vendor-attr").and_then(|v| v.as_str()),
            Some("vendor-x")
        );

        // Node-level extras nested under nodes.<foreign-id>.
        // Note: foreign-id contains dots — keys it as a single key
        // in the nested map, not a dotted path.
        let nodes = unmodeled.get("nodes").and_then(|v| v.as_mapping()).unwrap();
        let web01 = nodes
            .get("web01.acme.com")
            .expect("foreign-id with dots maps cleanly as a single map key");
        assert_eq!(
            web01.get("legacy-tag").and_then(|v| v.as_str()),
            Some("tag-1")
        );
        // Catalog field also preserved (location).
        assert_eq!(web01.get("location").and_then(|v| v.as_str()), Some("HQ"));

        // Interface-level extras passthrough.
        let ifaces = web01
            .get("interfaces")
            .and_then(|v| v.as_mapping())
            .unwrap();
        let iface = ifaces.get("10.0.0.1").unwrap();
        assert_eq!(
            iface.get("custom-port-mode").and_then(|v| v.as_str()),
            Some("trunk")
        );

        // PR001 findings should mention the custom attrs.
        let messages: Vec<&str> = r.findings.iter().map(|f| f.message.as_str()).collect();
        assert!(
            messages.iter().any(|m| m.contains("some-vendor-attr")),
            "expected PR001 for root extras, got: {messages:#?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("legacy-tag")),
            "expected PR001 for node extras, got: {messages:#?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("custom-port-mode")),
            "expected PR001 for interface extras, got: {messages:#?}"
        );
    }

    #[test]
    fn unmodeled_annotation_is_absent_when_xml_has_no_unmodeled_content() {
        // Clean XML — no unmodeled attrs / meta-data → the annotation
        // key should NOT appear in the emitted YAML (serde
        // skip_serializing_if = "Option::is_none").
        let r = convert_requisition_xml(REQ_BASIC, None, None).unwrap();
        let yaml = r.yaml.as_ref().unwrap();
        assert!(
            !yaml.contains("x-onmsctl-unmodeled"),
            "annotation should be absent for clean input, got:\n{yaml}"
        );
    }

    #[test]
    fn snmp_primary_parses_p_s_n() {
        assert_eq!(parse_snmp_primary("P"), Some(SnmpPrimary::P));
        assert_eq!(parse_snmp_primary("S"), Some(SnmpPrimary::S));
        assert_eq!(parse_snmp_primary("N"), Some(SnmpPrimary::N));
        assert_eq!(parse_snmp_primary("X"), None);
        assert_eq!(parse_snmp_primary(""), None);
    }

    #[test]
    fn invalid_snmp_primary_raises_pr005() {
        const REQ_BAD_SNMP: &str = r#"<?xml version="1.0"?>
<model-import foreign-source="acme">
  <node foreign-id="web01" node-label="web01">
    <interface ip-addr="10.0.0.1" snmp-primary="Primary"/>
  </node>
</model-import>"#;
        let r = convert_requisition_xml(REQ_BAD_SNMP, None, None).unwrap();
        let pr005s: Vec<_> = r
            .findings
            .iter()
            .filter(|f| f.code == FindingCode::Pr005)
            .collect();
        assert_eq!(pr005s.len(), 1);
        assert!(pr005s[0].message.contains("Primary"));
        // Value was dropped from the YAML — local model has no snmp-primary key.
        let yaml = r.yaml.as_ref().unwrap();
        assert!(!yaml.contains("snmpPrimary"));
    }

    #[test]
    fn convert_directory_finds_orphan_fs() {
        // Build a tmp dir with one requisition and one orphan FS.
        let dir = tempfile::tempdir().unwrap();
        let xml_dir = dir.path().join("requisitions");
        let fs_dir = dir.path().join("foreign-sources");
        std::fs::create_dir(&xml_dir).unwrap();
        std::fs::create_dir(&fs_dir).unwrap();

        std::fs::write(xml_dir.join("acme-prod.xml"), REQ_BASIC).unwrap();
        std::fs::write(fs_dir.join("acme-prod.xml"), FS_BASIC).unwrap();
        // Orphan: FS with no matching requisition.
        std::fs::write(fs_dir.join("ghost.xml"), FS_BASIC).unwrap();

        let results = convert_directory(&xml_dir, Some(&fs_dir)).unwrap();
        assert_eq!(results.len(), 2);

        // The acme-prod result has YAML + no orphan finding.
        let acme = results
            .iter()
            .find(|r| r.foreign_source == "acme-prod")
            .unwrap();
        assert!(acme.yaml.is_some());
        assert!(!acme.findings.iter().any(|f| f.code == FindingCode::Pr002));

        // The ghost result has no YAML + PR002.
        let ghost = results
            .iter()
            .find(|r| r.foreign_source == "ghost")
            .unwrap();
        assert!(ghost.yaml.is_none());
        assert!(ghost.findings.iter().any(|f| f.code == FindingCode::Pr002));
    }

    #[test]
    fn convert_directory_handles_no_fs_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("acme.xml"), REQ_BASIC).unwrap();
        let results = convert_directory(dir.path(), None).unwrap();
        assert_eq!(results.len(), 1);
        // PR004 fires for every requisition in this mode.
        assert!(
            results[0]
                .findings
                .iter()
                .any(|f| f.code == FindingCode::Pr004)
        );
    }

    #[test]
    fn malformed_xml_returns_err_not_panic() {
        let bad = "<model-import><node foreign-id=\"x\"";
        assert!(convert_requisition_xml(bad, None, None).is_err());
    }

    #[test]
    fn probe_repeated_unknown_siblings_extras_shape() {
        // Diagnostic: how does quick-xml route TWO same-named unknown
        // child elements through #[serde(flatten)] extras: BTreeMap?
        // Two outcomes possible:
        //   (a) BTreeMap last-write-wins → only the second survives
        //       (data loss); we'd need to change the extras shape.
        //   (b) quick-xml aggregates into a Value::Sequence under one
        //       key → both survive; no shape change needed.
        const REQ_DUP: &str = r#"<?xml version="1.0"?>
<model-import foreign-source="acme">
  <node foreign-id="web01" node-label="web01">
    <future-extension key="a"/>
    <future-extension key="b"/>
  </node>
</model-import>"#;
        let parsed = crate::convert::xml::parse_requisition(REQ_DUP).expect("xml parses");
        let node_extras = &parsed.nodes[0].extras;
        let key = serde_norway::Value::String("future-extension".into());
        let value = node_extras.0.get(&key);
        let v = value.expect("future-extension key present");
        match v {
            serde_norway::Value::Sequence(s) => {
                assert_eq!(s.len(), 2, "expected both siblings preserved as sequence");
            }
            other => panic!("expected Value::Sequence preserving both siblings, got {other:#?}"),
        }
    }
}
