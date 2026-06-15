/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Wire (server) DTOs for the v2 `datacollectionconf` REST service.
//!
//! The reconcile baseline is the **source download**
//! (`GET /api/v2/datacollectionconf/collectsources/{id}/download?format=json`),
//! whose JSON mirrors the on-disk `datacollection-group` XML. These DTOs
//! deserialize that download; the write side serializes the same logical tree
//! back to `<datacollection-group>` XML for the multipart upload (see `convert`).
//!
//! Deserialization is **permissive** (unknown fields ignored) so a server that
//! adds fields does not break the read. Shapes confirmed against a live
//! `37.0.0-SNAPSHOT` capture (OpenSpec task 1; fixture `tests/fixtures/`).
//!
//! NOTE on the `parameters`/`clazz` casing and the `ipList`/`systemDefChoice`
//! redundancy: these are JAXB→JSON artifacts of the server's XML model and are
//! mapped to friendly names in [`crate::model`] by the convert layer.

use serde::Deserialize;

/// A whole source as returned by `…/collectsources/{id}/download?format=json`:
/// the `datacollection-group` tree (groups + resource types + system defs).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct SourceDownload {
    pub name: String,
    #[serde(default)]
    pub groups: Vec<WireGroup>,
    #[serde(default, rename = "resourceTypes")]
    pub resource_types: Vec<WireResourceType>,
    #[serde(default, rename = "systemDefs")]
    pub system_defs: Vec<WireSystemDef>,
}

/// A MIB group: `mibObjs` collected for interfaces matching `ifType`. A group
/// may itself `includeGroups` (group composition) and carry mib-object
/// `properties`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WireGroup {
    pub name: String,
    #[serde(default, rename = "ifType")]
    pub if_type: String,
    #[serde(default, rename = "mibObjs")]
    pub mib_objs: Vec<WireMibObj>,
    #[serde(default, rename = "includeGroups")]
    pub include_groups: Vec<String>,
}

/// One collected OID. `maxval`/`minval` are optional bounds (usually null).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WireMibObj {
    pub oid: String,
    pub instance: String,
    pub alias: String,
    #[serde(rename = "type")]
    pub mib_type: String,
    #[serde(default)]
    pub maxval: Option<String>,
    #[serde(default)]
    pub minval: Option<String>,
}

/// A custom resource type with its persistence-selector and storage strategies.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WireResourceType {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, rename = "resourceLabel")]
    pub resource_label: Option<String>,
    #[serde(default, rename = "persistenceSelectorStrategy")]
    pub persistence_selector_strategy: Option<WireStrategy>,
    #[serde(default, rename = "storageStrategy")]
    pub storage_strategy: Option<WireStrategy>,
}

/// A strategy: the Java class (`clazz`) plus `parameters`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WireStrategy {
    /// The server serializes the class under the key `clazz`.
    #[serde(default)]
    pub clazz: String,
    #[serde(default)]
    pub parameters: Vec<WireParam>,
}

/// A `key`/`value` strategy parameter.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WireParam {
    pub key: String,
    pub value: String,
}

/// A system definition: collect `collect.includeGroups` for a matching
/// `sysoid`/`sysoidMask` (the redundant `systemDefChoice` and `ipList` are
/// ignored — the top-level `sysoid`/`sysoidMask` carry the same data).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WireSystemDef {
    pub name: String,
    #[serde(default)]
    pub sysoid: Option<String>,
    #[serde(default, rename = "sysoidMask")]
    pub sysoid_mask: Option<String>,
    #[serde(default)]
    pub collect: WireCollect,
}

/// The `collect` block of a system def: which groups to collect.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WireCollect {
    #[serde(default, rename = "includeGroups")]
    pub include_groups: Vec<String>,
}

/// A source row from `…/collectsources/{id}` or `…/collectsources/names-and-ids`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct SourceSummary {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
}

/// A profile from `…/profiles` (note `rrdRras`, not `rras`; `sourceNames`
/// exposes the source membership — so association is readable, not opaque).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ProfileDto {
    pub id: i64,
    pub name: String,
    #[serde(default, rename = "rrdStep")]
    pub rrd_step: u32,
    #[serde(default, rename = "rrdRras")]
    pub rrd_rras: Vec<String>,
    #[serde(default, rename = "storageFlag")]
    pub storage_flag: String,
    #[serde(default, rename = "sourceNames")]
    pub source_names: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The read DTOs deserialize a real `37.0.0-SNAPSHOT` source download
    /// (the `ejn` group, OpenSpec task 1 capture).
    #[test]
    fn deserializes_live_source_download() {
        let raw = include_str!("../tests/fixtures/source_download.json");
        let d: SourceDownload = serde_json::from_str(raw).expect("download JSON parses");
        assert_eq!(d.name, "ejn");
        assert_eq!(d.groups.len(), 4);
        assert_eq!(d.resource_types.len(), 3);
        assert_eq!(d.system_defs.len(), 1);

        // A group + its first mibObj.
        let g0 = &d.groups[0];
        assert_eq!(g0.name, "ejn-ggsn");
        assert_eq!(g0.if_type, "ignore");
        assert_eq!(g0.mib_objs[0].alias, "ggsnPdpCreateAtmpt");
        assert_eq!(g0.mib_objs[0].mib_type, "counter");
        assert!(g0.mib_objs[0].oid.starts_with(".1.3.6.1.4.1.10923"));

        // A resource type with a multi-parameter storage strategy (clazz key).
        let rt0 = &d.resource_types[0];
        assert_eq!(rt0.name, "ejnGgsnApnIndex");
        let ss = rt0.storage_strategy.as_ref().expect("storage strategy");
        assert!(ss.clazz.ends_with("SiblingColumnStorageStrategy"));
        assert_eq!(ss.parameters[0].key, "sibling-column-name");
        assert_eq!(ss.parameters[0].value, "ApnName");
        let ps = rt0
            .persistence_selector_strategy
            .as_ref()
            .expect("persistence selector");
        assert!(ps.clazz.ends_with("PersistAllSelectorStrategy"));

        // The system def collects four groups by sysoid.
        let sd0 = &d.system_defs[0];
        assert_eq!(sd0.name, "EJN Mobile IP GGSN");
        assert_eq!(sd0.sysoid.as_deref(), Some(".1.3.6.1.4.1.2636.1.1.1.2.18"));
        assert_eq!(sd0.collect.include_groups.len(), 4);
        assert!(sd0.collect.include_groups.contains(&"ejn-ggsn".to_string()));
    }
}
