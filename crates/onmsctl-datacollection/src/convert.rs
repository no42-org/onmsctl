/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The asymmetric convert layer (OpenSpec DC4a).
//!
//! - **Write** ([`to_group_xml`]): the local [`DataCollectionSourceLocal`] tree
//!   is serialized to a `datacollection-group` XML document — exactly what the
//!   multipart `…/datacollectionconf/upload` endpoint parses.
//! - **Read/diff** ([`source_unchanged`]): the deployed source is pulled as
//!   JSON (`…/collectsources/{id}/download?format=json`, a [`SourceDownload`])
//!   and both sides are folded into a [`Canonical`] form whose equality is
//!   order-insensitive — so a re-apply that only reorders groups/objects is a
//!   no-op. The two directions are independent mappings of one logical model
//!   and MUST agree, or a re-apply churns (guarded by tests).

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{ClassRef, DataCollectionSourceLocal};
use crate::server::{SourceDownload, WireStrategy};

// ---------------------------------------------------------------------------
// Write: local model -> <datacollection-group> XML
// ---------------------------------------------------------------------------

const XMLNS: &str = "http://xmlns.opennms.org/xsd/config/datacollection";

/// Serialize the desired source as a `datacollection-group` XML document for the
/// multipart upload.
pub fn to_group_xml(local: &DataCollectionSourceLocal) -> String {
    let s = &local.spec;
    let mut out = String::new();
    out.push_str(&format!(
        "<datacollection-group xmlns=\"{XMLNS}\" name=\"{}\">\n",
        attr(&local.metadata.name)
    ));

    for rt in &s.resource_types {
        out.push_str(&format!("  <resourceType name=\"{}\"", attr(&rt.name)));
        if let Some(l) = &rt.label {
            out.push_str(&format!(" label=\"{}\"", attr(l)));
        }
        if let Some(rl) = &rt.resource_label {
            out.push_str(&format!(" resourceLabel=\"{}\"", attr(rl)));
        }
        out.push_str(">\n");
        strategy_xml(
            &mut out,
            "persistenceSelectorStrategy",
            &rt.persistence_selector,
        );
        strategy_xml(&mut out, "storageStrategy", &rt.storage_strategy);
        out.push_str("  </resourceType>\n");
    }

    for g in &s.groups {
        out.push_str(&format!(
            "  <group name=\"{}\" ifType=\"{}\">\n",
            attr(&g.name),
            attr(&g.if_type)
        ));
        for mo in &g.mib_objects {
            out.push_str(&format!(
                "    <mibObj oid=\"{}\" instance=\"{}\" alias=\"{}\" type=\"{}\"",
                attr(&mo.oid),
                attr(&mo.instance),
                attr(&mo.alias),
                attr(&mo.mib_type)
            ));
            if let Some(v) = &mo.maxval {
                out.push_str(&format!(" maxval=\"{}\"", attr(v)));
            }
            if let Some(v) = &mo.minval {
                out.push_str(&format!(" minval=\"{}\"", attr(v)));
            }
            out.push_str("/>\n");
        }
        for inc in &g.include_groups {
            out.push_str(&format!("    <includeGroup>{}</includeGroup>\n", text(inc)));
        }
        out.push_str("  </group>\n");
    }

    for sd in &s.system_defs {
        out.push_str(&format!("  <systemDef name=\"{}\">\n", attr(&sd.name)));
        if let Some(o) = &sd.sysoid {
            out.push_str(&format!("    <sysoid>{}</sysoid>\n", text(o)));
        } else if let Some(m) = &sd.sysoid_mask {
            out.push_str(&format!("    <sysoidMask>{}</sysoidMask>\n", text(m)));
        }
        out.push_str("    <collect>\n");
        for inc in &sd.include_groups {
            out.push_str(&format!(
                "      <includeGroup>{}</includeGroup>\n",
                text(inc)
            ));
        }
        out.push_str("    </collect>\n");
        out.push_str("  </systemDef>\n");
    }

    out.push_str("</datacollection-group>\n");
    out
}

fn strategy_xml(out: &mut String, elem: &str, strat: &Option<ClassRef>) {
    let Some(c) = strat else { return };
    if c.params.is_empty() {
        out.push_str(&format!("    <{elem} class=\"{}\"/>\n", attr(&c.class)));
    } else {
        out.push_str(&format!("    <{elem} class=\"{}\">\n", attr(&c.class)));
        for p in &c.params {
            out.push_str(&format!(
                "      <parameter key=\"{}\" value=\"{}\"/>\n",
                attr(&p.key),
                attr(&p.value)
            ));
        }
        out.push_str(&format!("    </{elem}>\n"));
    }
}

/// Escape a string for use inside an XML attribute value.
fn attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Escape a string for use as XML text content.
fn text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// Diff: normalized, order-insensitive equivalence (DC4b)
// ---------------------------------------------------------------------------

/// A normalized, order-insensitive view of a source tree. Equality of two
/// `Canonical`s means the sources are semantically the same.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Canonical {
    resource_types: BTreeMap<String, CanonRt>,
    groups: BTreeMap<String, CanonGroup>,
    system_defs: BTreeMap<String, CanonSd>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CanonRt {
    label: Option<String>,
    resource_label: Option<String>,
    persistence: Option<CanonStrategy>,
    storage: Option<CanonStrategy>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CanonStrategy {
    class: String,
    /// Parameters are order-SENSITIVE (e.g. sequential `replace-all` rules).
    params: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CanonGroup {
    if_type: String,
    include_groups: BTreeSet<String>,
    mibs: BTreeSet<CanonMib>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct CanonMib {
    oid: String,
    instance: String,
    alias: String,
    mib_type: String,
    maxval: Option<String>,
    minval: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CanonSd {
    sysoid: Option<String>,
    sysoid_mask: Option<String>,
    include_groups: BTreeSet<String>,
}

/// True when the desired local document and the deployed download are
/// semantically equal (so apply is a no-op).
pub fn source_unchanged(local: &DataCollectionSourceLocal, deployed: &SourceDownload) -> bool {
    canon_local(local) == canon_download(deployed)
}

/// Fold the local document into canonical form.
pub fn canon_local(local: &DataCollectionSourceLocal) -> Canonical {
    let s = &local.spec;
    Canonical {
        resource_types: s
            .resource_types
            .iter()
            .map(|rt| {
                (
                    rt.name.clone(),
                    CanonRt {
                        label: rt.label.clone(),
                        resource_label: rt.resource_label.clone(),
                        persistence: rt.persistence_selector.as_ref().map(canon_classref),
                        storage: rt.storage_strategy.as_ref().map(canon_classref),
                    },
                )
            })
            .collect(),
        groups: s
            .groups
            .iter()
            .map(|g| {
                (
                    g.name.clone(),
                    CanonGroup {
                        if_type: g.if_type.clone(),
                        include_groups: g.include_groups.iter().cloned().collect(),
                        mibs: g
                            .mib_objects
                            .iter()
                            .map(|m| CanonMib {
                                oid: m.oid.clone(),
                                instance: m.instance.clone(),
                                alias: m.alias.clone(),
                                mib_type: m.mib_type.to_ascii_lowercase(),
                                maxval: m.maxval.clone(),
                                minval: m.minval.clone(),
                            })
                            .collect(),
                    },
                )
            })
            .collect(),
        system_defs: s
            .system_defs
            .iter()
            .map(|sd| {
                (
                    sd.name.clone(),
                    CanonSd {
                        sysoid: sd.sysoid.clone(),
                        sysoid_mask: sd.sysoid_mask.clone(),
                        include_groups: sd.include_groups.iter().cloned().collect(),
                    },
                )
            })
            .collect(),
    }
}

/// Fold the deployed source download into canonical form.
pub fn canon_download(d: &SourceDownload) -> Canonical {
    Canonical {
        resource_types: d
            .resource_types
            .iter()
            .map(|rt| {
                (
                    rt.name.clone(),
                    CanonRt {
                        label: rt.label.clone(),
                        resource_label: rt.resource_label.clone(),
                        persistence: rt.persistence_selector_strategy.as_ref().map(canon_wire),
                        storage: rt.storage_strategy.as_ref().map(canon_wire),
                    },
                )
            })
            .collect(),
        groups: d
            .groups
            .iter()
            .map(|g| {
                (
                    g.name.clone(),
                    CanonGroup {
                        if_type: g.if_type.clone(),
                        include_groups: g.include_groups.iter().cloned().collect(),
                        mibs: g
                            .mib_objs
                            .iter()
                            .map(|m| CanonMib {
                                oid: m.oid.clone(),
                                instance: m.instance.clone(),
                                alias: m.alias.clone(),
                                mib_type: m.mib_type.to_ascii_lowercase(),
                                maxval: m.maxval.clone(),
                                minval: m.minval.clone(),
                            })
                            .collect(),
                    },
                )
            })
            .collect(),
        system_defs: d
            .system_defs
            .iter()
            .map(|sd| {
                (
                    sd.name.clone(),
                    CanonSd {
                        sysoid: sd.sysoid.clone(),
                        sysoid_mask: sd.sysoid_mask.clone(),
                        include_groups: sd.collect.include_groups.iter().cloned().collect(),
                    },
                )
            })
            .collect(),
    }
}

fn canon_classref(c: &ClassRef) -> CanonStrategy {
    CanonStrategy {
        class: c.class.clone(),
        params: c
            .params
            .iter()
            .map(|p| (p.key.clone(), p.value.clone()))
            .collect(),
    }
}

fn canon_wire(w: &WireStrategy) -> CanonStrategy {
    CanonStrategy {
        class: w.clazz.clone(),
        params: w
            .parameters
            .iter()
            .map(|p| (p.key.clone(), p.value.clone()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> DataCollectionSourceLocal {
        serde_norway::from_str(yaml).expect("parses")
    }

    const SRC: &str = r#"
apiVersion: datacollection.opennms.org/v1
kind: DataCollectionSource
metadata: { name: acme }
spec:
  resourceTypes:
    - name: acmeIdx
      label: Acme Index
      resourceLabel: "${name}"
      persistenceSelector:
        class: org.opennms.netmgt.collection.support.PersistAllSelectorStrategy
      storageStrategy:
        class: org.opennms.netmgt.dao.support.SiblingColumnStorageStrategy
        params:
          - { key: sibling-column-name, value: acmeName }
  groups:
    - name: acme-cpu
      ifType: all
      mibObjects:
        - { oid: .1.3.6.1.4.1.5.1, instance: "0", alias: acmeCpu, type: Gauge }
  systemDefs:
    - name: Acme Box
      sysoid: .1.3.6.1.4.1.5
      includeGroups: [acme-cpu]
"#;

    #[test]
    fn to_group_xml_emits_expected_structure() {
        let xml = to_group_xml(&parse(SRC));
        assert!(xml.contains("<datacollection-group"));
        assert!(xml.contains("name=\"acme\""));
        assert!(xml.contains(
            "<resourceType name=\"acmeIdx\" label=\"Acme Index\" resourceLabel=\"${name}\">"
        ));
        // No-param strategy self-closes; param strategy nests <parameter>.
        assert!(xml.contains("<persistenceSelectorStrategy class=\"org.opennms.netmgt.collection.support.PersistAllSelectorStrategy\"/>"));
        assert!(xml.contains("<storageStrategy class=\"org.opennms.netmgt.dao.support.SiblingColumnStorageStrategy\">"));
        assert!(xml.contains("<parameter key=\"sibling-column-name\" value=\"acmeName\"/>"));
        assert!(xml.contains("<group name=\"acme-cpu\" ifType=\"all\">"));
        assert!(xml.contains(
            "<mibObj oid=\".1.3.6.1.4.1.5.1\" instance=\"0\" alias=\"acmeCpu\" type=\"Gauge\"/>"
        ));
        assert!(xml.contains("<sysoid>.1.3.6.1.4.1.5</sysoid>"));
        assert!(xml.contains("<includeGroup>acme-cpu</includeGroup>"));
    }

    #[test]
    fn attribute_special_chars_escaped() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: a }\nspec:\n  resourceTypes:\n    - name: rt\n      resourceLabel: \"a & b <c> \\\"d\\\"\"\n",
        );
        let xml = to_group_xml(&doc);
        assert!(xml.contains("resourceLabel=\"a &amp; b &lt;c&gt; &quot;d&quot;\""));
    }

    #[test]
    fn reordered_download_compares_unchanged() {
        // The fixture, parsed two ways: itself, and with groups/mibObjs reversed.
        let raw = include_str!("../tests/fixtures/source_download.json");
        let a: SourceDownload = serde_json::from_str(raw).unwrap();
        let mut b = a.clone();
        b.groups.reverse();
        for g in &mut b.groups {
            g.mib_objs.reverse();
        }
        b.resource_types.reverse();
        assert_eq!(
            canon_download(&a),
            canon_download(&b),
            "reordering groups/objects/resourceTypes must not change canonical form"
        );
    }

    #[test]
    fn local_matches_equivalent_download() {
        // A hand-built local equal (ignoring order + type casing) to a small
        // download reads `unchanged`.
        let local = parse(SRC);
        let deployed: SourceDownload = serde_json::from_str(
            r#"{
              "name": "acme",
              "resourceTypes": [ {
                "name": "acmeIdx", "label": "Acme Index", "resourceLabel": "${name}",
                "persistenceSelectorStrategy": { "clazz": "org.opennms.netmgt.collection.support.PersistAllSelectorStrategy", "parameters": [] },
                "storageStrategy": { "clazz": "org.opennms.netmgt.dao.support.SiblingColumnStorageStrategy", "parameters": [ { "key": "sibling-column-name", "value": "acmeName" } ] }
              } ],
              "groups": [ { "name": "acme-cpu", "ifType": "all", "includeGroups": [],
                "mibObjs": [ { "oid": ".1.3.6.1.4.1.5.1", "instance": "0", "alias": "acmeCpu", "type": "gauge", "maxval": null, "minval": null } ] } ],
              "systemDefs": [ { "name": "Acme Box", "sysoid": ".1.3.6.1.4.1.5", "sysoidMask": null,
                "collect": { "includeGroups": [ "acme-cpu" ] } } ]
            }"#,
        )
        .unwrap();
        assert!(
            source_unchanged(&local, &deployed),
            "semantically-equal local/deployed (type Gauge vs gauge) must be unchanged"
        );
    }

    #[test]
    fn changed_mib_object_compares_changed() {
        let local = parse(SRC);
        let mut deployed: SourceDownload = serde_json::from_str(
            r#"{ "name":"acme","resourceTypes":[],"groups":[{"name":"acme-cpu","ifType":"all","includeGroups":[],"mibObjs":[{"oid":".1.3.6.1.4.1.5.1","instance":"0","alias":"acmeCpu","type":"gauge"}]}],"systemDefs":[] }"#,
        ).unwrap();
        // resourceTypes differ (local has one, deployed none) → changed.
        assert!(!source_unchanged(&local, &deployed));
        // Even after matching resourceTypes off, a different alias is changed.
        deployed.groups[0].mib_objs[0].alias = "different".into();
        assert!(!source_unchanged(&local, &deployed));
    }
}
