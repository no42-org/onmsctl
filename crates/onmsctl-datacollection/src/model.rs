/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Local (YAML) model for `kind: DataCollectionSource` — the operator-facing
//! datacollection-group that `onmsctl apply -f` reconciles into an OpenNMS SNMP
//! data-collection source plus (optionally) the snmp-collection profile that
//! includes it.
//!
//! Named, multi-instance: `metadata.name` is the source (datacollection-group)
//! name. The `spec` carries `enabled`, the group tree (`resourceTypes` /
//! `groups` / `systemDefs`), the `profiles` that should include the source, and
//! an optional inline `profileSpec` (the "C+" escape hatch — author/tune a
//! profile from zero). All validation that can be done without the server
//! happens in [`DataCollectionSourceLocal::validate`], before any HTTP request.

use onmsctl_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only accepted `apiVersion`.
pub const API_VERSION: &str = "datacollection.opennms.org/v1";
/// The only accepted `kind`.
pub const KIND: &str = "DataCollectionSource";

/// Allowed `mibObject.type` values (case-insensitive). Covers the SNMP base
/// types OpenNMS persists; a value outside this set is almost always a typo.
const MIB_TYPES: &[&str] = &[
    "counter",
    "counter32",
    "counter64",
    "gauge",
    "gauge32",
    "integer",
    "integer32",
    "unsigned32",
    "timeticks",
    "octetstring",
    "hexstring",
    "string",
    "ipaddress",
    "opaque",
];

/// Allowed `profileSpec.storageFlag` values (case-insensitive), mirroring the
/// server `snmpStorageFlag` (`all` / `select` / `primary`).
const STORAGE_FLAGS: &[&str] = &["all", "select", "primary"];

/// A `kind: DataCollectionSource` document.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DataCollectionSourceLocal {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: Spec,
}

/// Document metadata. `name` is the datacollection-group (source) name.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
}

/// The source body: the group tree plus profile wiring.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spec {
    /// Whether the source is active. Defaults to `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Custom resource types defined by this group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_types: Vec<ResourceType>,
    /// MIB groups: a set of `mibObjects` collected together, scoped by `ifType`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// System definitions: which `groups` to collect for a matching `sysoid`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_defs: Vec<SystemDef>,
    /// Names of snmp-collection profiles that SHALL include this source
    /// (ensure-present association). When `profile_spec` is absent, each name
    /// MUST already exist on the server (checked at apply, not parse).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
    /// Optional inline profile to author/tune from zero (the "C+" escape hatch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_spec: Option<ProfileSpec>,
}

fn default_true() -> bool {
    true
}

/// A custom resource type (indexed/tabular resource) defined by the group.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceType {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_label: Option<String>,
    /// The persistence-selector strategy (`class` + optional `params`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence_selector: Option<ClassRef>,
    /// The storage strategy (`class` + optional `params`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_strategy: Option<ClassRef>,
}

/// A strategy reference: a fully-qualified Java `class` plus optional parameters.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClassRef {
    pub class: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<Param>,
}

/// A `key`/`value` strategy parameter (maps to `<parameter key=.. value=..>`).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Param {
    pub key: String,
    pub value: String,
}

/// A MIB group: the OIDs collected together for interfaces matching `ifType`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Group {
    pub name: String,
    /// The interface-type filter (`all`, `ignore`, or a numeric ifType).
    pub if_type: String,
    pub mib_objects: Vec<MibObject>,
    /// Other groups this group composes in (group-level `includeGroup`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_groups: Vec<String>,
}

/// One collected OID within a group.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MibObject {
    pub oid: String,
    pub instance: String,
    pub alias: String,
    /// SNMP type (see [`MIB_TYPES`]). Field is `type` in YAML.
    #[serde(rename = "type")]
    pub mib_type: String,
    /// Optional collection bounds (rarely set; preserved for round-trip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minval: Option<String>,
}

/// A system definition: collect the named `includeGroups` for a matching sysoid.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemDef {
    pub name: String,
    /// OID prefix match (`sysoidMask`) — every device under this OID subtree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysoid_mask: Option<String>,
    /// Exact OID match (`sysoid`) — alternative to `sysoidMask`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysoid: Option<String>,
    /// Names of `groups` to collect for matching devices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_groups: Vec<String>,
}

/// An inline snmp-collection profile (the "C+" `profileSpec`).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileSpec {
    pub name: String,
    /// RRD step in seconds (must be positive).
    pub rrd_step: u32,
    /// RRA (round-robin archive) definitions (at least one required).
    pub rras: Vec<String>,
    /// Storage flag (`all` / `select` / `primary`).
    pub storage_flag: String,
}

impl DataCollectionSourceLocal {
    /// Validate the document, returning the first user-actionable error. Covers
    /// the API literals, the presence of at least one group-tree member, the
    /// per-member shape (non-empty `mibObjects`, known `type`, resolvable
    /// `includeGroups`), and the optional `profileSpec` (positive `rrdStep`,
    /// ≥1 RRA, valid `storageFlag`). Server-dependent checks (profile existence,
    /// plugin-source guards) happen at apply.
    pub fn validate(&self) -> Result<()> {
        if self.api_version != API_VERSION {
            return Err(cfg(format!(
                "apiVersion must be {API_VERSION:?}, got {:?}",
                self.api_version
            )));
        }
        if self.kind != KIND {
            return Err(cfg(format!("kind must be {KIND:?}, got {:?}", self.kind)));
        }
        if self.metadata.name.trim().is_empty() {
            return Err(cfg("metadata.name must not be empty".into()));
        }
        self.validate_tree()?;
        self.validate_profile_spec()?;
        Ok(())
    }

    fn validate_tree(&self) -> Result<()> {
        let s = &self.spec;
        if s.resource_types.is_empty() && s.groups.is_empty() && s.system_defs.is_empty() {
            return Err(cfg(
                "spec must declare at least one of resourceTypes / groups / systemDefs".into(),
            ));
        }

        for (i, rt) in s.resource_types.iter().enumerate() {
            if rt.name.trim().is_empty() {
                return Err(cfg(format!(
                    "spec.resourceTypes[{i}].name must not be empty"
                )));
            }
            for (field, cref) in [
                ("persistenceSelector", &rt.persistence_selector),
                ("storageStrategy", &rt.storage_strategy),
            ] {
                if let Some(c) = cref
                    && c.class.trim().is_empty()
                {
                    return Err(cfg(format!(
                        "spec.resourceTypes[{i}].{field}.class must not be empty"
                    )));
                }
            }
        }

        for (i, g) in s.groups.iter().enumerate() {
            if g.name.trim().is_empty() {
                return Err(cfg(format!("spec.groups[{i}].name must not be empty")));
            }
            if g.if_type.trim().is_empty() {
                return Err(cfg(format!(
                    "spec.groups[{i}].ifType must not be empty (e.g. `all`, `ignore`, or a numeric ifType)"
                )));
            }
            if g.mib_objects.is_empty() {
                return Err(cfg(format!(
                    "spec.groups[{i}] ({}) must declare at least one mibObject",
                    g.name
                )));
            }
            for (j, mo) in g.mib_objects.iter().enumerate() {
                if mo.oid.trim().is_empty() || mo.alias.trim().is_empty() {
                    return Err(cfg(format!(
                        "spec.groups[{i}].mibObjects[{j}]: oid and alias must both be non-empty"
                    )));
                }
                if !MIB_TYPES
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(&mo.mib_type))
                {
                    return Err(cfg(format!(
                        "spec.groups[{i}].mibObjects[{j}]: type {:?} is not a recognized SNMP type ({})",
                        mo.mib_type,
                        MIB_TYPES.join(", ")
                    )));
                }
            }
        }

        // Every includeGroups entry must name a group defined in THIS document.
        let defined: std::collections::HashSet<&str> =
            s.groups.iter().map(|g| g.name.as_str()).collect();
        for (i, sd) in s.system_defs.iter().enumerate() {
            if sd.name.trim().is_empty() {
                return Err(cfg(format!("spec.systemDefs[{i}].name must not be empty")));
            }
            // `sysoid` (exact) and `sysoidMask` (prefix) are mutually exclusive —
            // the wire model is a choice, and the writer emits only one, so
            // accepting both would diff-churn forever against the server.
            if sd.sysoid.is_some() && sd.sysoid_mask.is_some() {
                return Err(cfg(format!(
                    "spec.systemDefs[{i}] ({}) sets both sysoid and sysoidMask — use exactly one",
                    sd.name
                )));
            }
            for inc in &sd.include_groups {
                if !defined.contains(inc.as_str()) {
                    return Err(cfg(format!(
                        "spec.systemDefs[{i}] ({}) includes group {inc:?}, which is not defined in \
                         this document's spec.groups",
                        sd.name
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_profile_spec(&self) -> Result<()> {
        let Some(p) = &self.spec.profile_spec else {
            return Ok(());
        };
        if p.name.trim().is_empty() {
            return Err(cfg("spec.profileSpec.name must not be empty".into()));
        }
        if p.rrd_step == 0 {
            return Err(cfg(
                "spec.profileSpec.rrdStep must be a positive number of seconds".into(),
            ));
        }
        if p.rras.is_empty() {
            return Err(cfg(
                "spec.profileSpec.rras must declare at least one RRA".into()
            ));
        }
        if !STORAGE_FLAGS
            .iter()
            .any(|f| f.eq_ignore_ascii_case(&p.storage_flag))
        {
            return Err(cfg(format!(
                "spec.profileSpec.storageFlag {:?} must be one of {}",
                p.storage_flag,
                STORAGE_FLAGS.join(" / ")
            )));
        }
        Ok(())
    }

    /// Non-fatal advisories (the spec's WARN cases). Currently: a `profileSpec`
    /// whose `name` is not also listed in `profiles` is reconciled but the
    /// source is not attached to it.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(p) = &self.spec.profile_spec
            && !self.spec.profiles.iter().any(|n| n == &p.name)
        {
            out.push(format!(
                "spec.profileSpec.name {:?} is not listed in spec.profiles — the profile will be \
                 reconciled but this source will NOT be attached to it (add it to `profiles` to \
                 attach)",
                p.name
            ));
        }
        out
    }
}

fn cfg(m: String) -> Error {
    Error::Config(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> std::result::Result<DataCollectionSourceLocal, serde_norway::Error> {
        serde_norway::from_str(yaml)
    }

    const CISCO: &str = r#"
apiVersion: datacollection.opennms.org/v1
kind: DataCollectionSource
metadata: { name: cisco-environment }
spec:
  enabled: true
  profiles: [default]
  resourceTypes:
    - name: ciscoEnvMonTemperatureStatusIndex
      label: Cisco Env Temperature
      resourceLabel: "${index}"
      persistenceSelector:
        class: org.opennms.netmgt.collection.support.PersistAllSelectorStrategy
      storageStrategy:
        class: org.opennms.netmgt.dao.support.IndexStorageStrategy
  groups:
    - name: cisco-temperature
      ifType: all
      mibObjects:
        - oid: .1.3.6.1.4.1.9.9.13.1.3.1.3
          instance: ciscoEnvMonTemperatureStatusIndex
          alias: ciscoEnvMonTempStatusValue
          type: gauge
  systemDefs:
    - name: Cisco Routers
      sysoidMask: .1.3.6.1.4.1.9.1.
      includeGroups: [cisco-temperature]
"#;

    #[test]
    fn full_source_parses_and_validates() {
        let doc = parse(CISCO).expect("parses");
        doc.validate().expect("valid");
        assert!(doc.spec.enabled);
        assert_eq!(doc.spec.profiles, vec!["default"]);
        assert_eq!(doc.spec.groups[0].mib_objects[0].mib_type, "gauge");
        assert_eq!(
            doc.spec.system_defs[0].include_groups,
            vec!["cisco-temperature"]
        );
        assert!(doc.warnings().is_empty());
    }

    #[test]
    fn enabled_defaults_true() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: [ { oid: .1.3.6, instance: '0', alias: x, type: counter } ]\n",
        )
        .unwrap();
        assert!(doc.spec.enabled, "enabled defaults to true when omitted");
        doc.validate().unwrap();
    }

    #[test]
    fn empty_tree_rejected() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec: {}\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("at least one of resourceTypes")
        );
    }

    #[test]
    fn empty_group_rejected() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: []\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("at least one mibObject")
        );
    }

    #[test]
    fn bad_mib_type_rejected() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: [ { oid: .1.3.6, instance: '0', alias: x, type: bogus } ]\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("not a recognized SNMP type")
        );
    }

    #[test]
    fn dangling_include_group_rejected() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: [ { oid: .1.3.6, instance: '0', alias: x, type: counter } ]\n  systemDefs:\n    - name: SD\n      sysoidMask: .1.3.6.1.\n      includeGroups: [missing-group]\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("not defined in this document")
        );
    }

    #[test]
    fn system_def_only_source_is_valid() {
        // A source can ship only systemDefs that reference groups defined here.
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: [ { oid: .1.3.6, instance: '0', alias: x, type: counter } ]\n  systemDefs:\n    - name: SD\n      sysoid: .1.3.6.1.4.1.9.1.1\n      includeGroups: [grp]\n",
        )
        .unwrap();
        doc.validate().expect("self-consistent includeGroups");
    }

    #[test]
    fn system_def_with_both_sysoid_and_mask_rejected() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: [ { oid: .1.3.6, instance: '0', alias: x, type: counter } ]\n  systemDefs:\n    - name: SD\n      sysoid: .1.3.6.1.4.1.9.1.1\n      sysoidMask: .1.3.6.1.4.1.9.\n      includeGroups: [grp]\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("both sysoid and sysoidMask")
        );
    }

    #[test]
    fn profile_spec_invalid_rrd_step_rejected() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: [ { oid: .1.3.6, instance: '0', alias: x, type: counter } ]\n  profileSpec:\n    name: p\n    rrdStep: 0\n    rras: [\"RRA:AVERAGE:0.5:1:2016\"]\n    storageFlag: select\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("rrdStep must be a positive")
        );
    }

    #[test]
    fn profile_spec_bad_storage_flag_rejected() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: [ { oid: .1.3.6, instance: '0', alias: x, type: counter } ]\n  profileSpec:\n    name: p\n    rrdStep: 300\n    rras: [\"RRA:AVERAGE:0.5:1:2016\"]\n    storageFlag: everything\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("storageFlag")
        );
    }

    #[test]
    fn profile_spec_not_in_profiles_warns() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  profiles: [other]\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: [ { oid: .1.3.6, instance: '0', alias: x, type: counter } ]\n  profileSpec:\n    name: p\n    rrdStep: 300\n    rras: [\"RRA:AVERAGE:0.5:1:2016\"]\n    storageFlag: select\n",
        )
        .unwrap();
        doc.validate().expect("valid");
        let w = doc.warnings();
        assert_eq!(w.len(), 1, "profileSpec.name not in profiles warns");
        assert!(w[0].contains("not listed in spec.profiles"));
    }

    #[test]
    fn unknown_field_rejected() {
        let err = parse(
            "apiVersion: datacollection.opennms.org/v1\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups: []\n  bogus: 1\n",
        );
        assert!(err.is_err(), "deny_unknown_fields rejects extra keys");
    }

    #[test]
    fn wrong_api_version_rejected() {
        let doc = parse(
            "apiVersion: datacollection.opennms.org/v2\nkind: DataCollectionSource\nmetadata: { name: g }\nspec:\n  groups:\n    - name: grp\n      ifType: all\n      mibObjects: [ { oid: .1.3.6, instance: '0', alias: x, type: counter } ]\n",
        )
        .unwrap();
        assert!(
            doc.validate()
                .unwrap_err()
                .to_string()
                .contains("apiVersion")
        );
    }
}
