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
    ForeignSourceXml, InterfaceXml, NodeXml, ParameterXml, RequisitionXml, parse_foreign_source,
    parse_requisition,
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
    let req = parse_requisition(req_xml).map_err(|e| format!("requisition XML parse error: {e}"))?;
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

    let yaml = serde_norway::to_string(&local)
        .map_err(|e| format!("YAML serialization error: {e}"))?;

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
    // PR001 surfaces XML attributes / elements that exist in the
    // source but have no place in the local model. Catalog of
    // currently-unmodeled-but-known: node.@location, node.@city,
    // interface.@status, interface.@descr, all <meta-data> elements.
    flag_unmodeled(req, findings, source_path);

    RequisitionLocal {
        api_version: ApiVersion,
        kind: Kind,
        metadata: Metadata {
            name: req.foreign_source.clone(),
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

/// Walk the requisition XML for known-unmodeled attributes / elements
/// and emit one PR001 finding each. Catalog is enumerated here rather
/// than dynamically discovered so the warning matrix is auditable.
fn flag_unmodeled(req: &RequisitionXml, findings: &mut Vec<Finding>, src: Option<&PathBuf>) {
    for n in &req.nodes {
        if n.location.is_some() {
            findings.push(
                Finding::new(
                    FindingCode::Pr001,
                    format!(
                        "node '{}': @location is not modeled in YAML (dropped)",
                        n.foreign_id
                    ),
                )
                .opt_source(src),
            );
        }
        if n.city.is_some() {
            findings.push(
                Finding::new(
                    FindingCode::Pr001,
                    format!(
                        "node '{}': @city is not modeled in YAML (dropped — use asset instead)",
                        n.foreign_id
                    ),
                )
                .opt_source(src),
            );
        }
        if !n.meta_data.is_empty() {
            findings.push(
                Finding::new(
                    FindingCode::Pr001,
                    format!(
                        "node '{}': <meta-data> elements are not modeled in YAML ({} dropped)",
                        n.foreign_id,
                        n.meta_data.len()
                    ),
                )
                .opt_source(src),
            );
        }
        for iface in &n.interfaces {
            // PR005: snmp-primary value isn't one of P/S/N. The
            // local-model SnmpPrimary enum rejects unknown variants
            // at parse-time, so without this finding the operator
            // sees a silent drop instead of a useful warning.
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
            if iface.status.is_some() {
                findings.push(
                    Finding::new(
                        FindingCode::Pr001,
                        format!(
                            "node '{}' interface {}: @status is not modeled in YAML (dropped)",
                            n.foreign_id, iface.ip_addr
                        ),
                    )
                    .opt_source(src),
                );
            }
            if iface.descr.is_some() {
                findings.push(
                    Finding::new(
                        FindingCode::Pr001,
                        format!(
                            "node '{}' interface {}: @descr is not modeled in YAML (dropped)",
                            n.foreign_id, iface.ip_addr
                        ),
                    )
                    .opt_source(src),
                );
            }
            if !iface.meta_data.is_empty() {
                findings.push(
                    Finding::new(
                        FindingCode::Pr001,
                        format!(
                            "node '{}' interface {}: <meta-data> elements not modeled ({} dropped)",
                            n.foreign_id,
                            iface.ip_addr,
                            iface.meta_data.len()
                        ),
                    )
                    .opt_source(src),
                );
            }
            for svc in &iface.monitored_services {
                if !svc.meta_data.is_empty() {
                    findings.push(
                        Finding::new(
                            FindingCode::Pr001,
                            format!(
                                "node '{}' interface {} service '{}': <meta-data> elements not modeled ({} dropped)",
                                n.foreign_id,
                                iface.ip_addr,
                                svc.service_name,
                                svc.meta_data.len()
                            ),
                        )
                        .opt_source(src),
                    );
                }
            }
        }
    }
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
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
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
        // Expect: location, city, status, descr, interface meta-data,
        // service meta-data ABSENT (no <meta-data> on service in this
        // fixture), node meta-data. Total = 6.
        assert!(
            pr001s.len() >= 5,
            "expected >=5 PR001 findings, got {}: {:#?}",
            pr001s.len(),
            pr001s
        );
        // Spot-check the categorization.
        assert!(pr001s.iter().any(|f| f.message.contains("@location")));
        assert!(pr001s.iter().any(|f| f.message.contains("@city")));
        assert!(pr001s.iter().any(|f| f.message.contains("@status")));
        assert!(pr001s.iter().any(|f| f.message.contains("@descr")));
        // Exit code is 1 because PR001s are Warnings.
        assert_eq!(r.exit_code(), 1);
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
        let acme = results.iter().find(|r| r.foreign_source == "acme-prod").unwrap();
        assert!(acme.yaml.is_some());
        assert!(!acme.findings.iter().any(|f| f.code == FindingCode::Pr002));

        // The ghost result has no YAML + PR002.
        let ghost = results.iter().find(|r| r.foreign_source == "ghost").unwrap();
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
        assert!(results[0]
            .findings
            .iter()
            .any(|f| f.code == FindingCode::Pr004));
    }

    #[test]
    fn malformed_xml_returns_err_not_panic() {
        let bad = "<model-import><node foreign-id=\"x\"";
        assert!(convert_requisition_xml(bad, None, None).is_err());
    }
}
