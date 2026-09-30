/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Conversions between the local model and the wire DTOs (design D4, D5).
//!
//! - [`group_to_wire`] / [`package_to_wire`] build the request body with the
//!   server's documented defaults applied, so an applied document compares
//!   equal to what the server returns.
//! - [`canonical_group`] / [`canonical_package`] normalize a DTO for
//!   comparison: order-insensitive lists sorted, server-managed fields cleared.
//! - [`group_from_wire`] / [`package_from_wire`] are the export mapping: they
//!   drop defaults and unowned fields and keep values as the server's strings.

use onmsctl_core::{Error, Result};

use crate::dto::{
    ExpressionDto, IpRangeDto, ParameterDto, ResourceFilterDto, THRESHOLDING_GROUP_PARAM,
    ThreshdPackageDto, ThreshdServiceDto, ThresholdCommon, ThresholdDto, ThresholdGroupDto,
    default_filter_operator,
};
use crate::model::{
    API_VERSION, CommonRef, ExpressionDef, GROUP_KIND, Interval, IpRangeDef, Metadata,
    PACKAGE_KIND, ParameterDef, ResourceFilterDef, Scalar, ServiceDef, ThreshdPackageLocal,
    ThreshdPackageSpec, ThresholdDef, ThresholdGroupLocal, ThresholdGroupSpec,
};

/// Threshold types for which the server stores `rearmedUEI` as null.
const NO_REARM_UEI: &[&str] = &["relativeChange", "absoluteChange"];

// -- local -> wire ------------------------------------------------------------

pub fn group_to_wire(local: &ThresholdGroupLocal) -> ThresholdGroupDto {
    ThresholdGroupDto {
        name: local.metadata.name.clone(),
        rrd_repository: local.spec.rrd_repository.clone(),
        thresholds: local
            .spec
            .thresholds
            .iter()
            .map(|t| ThresholdDto {
                common: common_to_wire(&t.common()),
                ds_name: t.ds_name.clone(),
            })
            .collect(),
        expressions: local
            .spec
            .expressions
            .iter()
            .map(|e| ExpressionDto {
                common: common_to_wire(&e.common()),
                expression: e.expression.clone(),
            })
            .collect(),
        read_only: false,
        version: None,
    }
}

fn common_to_wire(c: &CommonRef<'_>) -> ThresholdCommon {
    let rearmed_uei = if NO_REARM_UEI.contains(&c.kind) {
        None
    } else {
        c.rearmed_uei.clone()
    };
    ThresholdCommon {
        kind: c.kind.to_string(),
        ds_type: c.ds_type.to_string(),
        value: c.value.to_wire(),
        rearm: c.rearm.to_wire(),
        trigger: c.trigger.to_wire(),
        description: c.description.clone(),
        ds_label: c.ds_label.clone(),
        expr_label: c.expr_label.clone(),
        triggered_uei: c.triggered_uei.clone(),
        rearmed_uei,
        filter_operator: c
            .filter_operator
            .clone()
            .unwrap_or_else(default_filter_operator),
        relaxed: c.relaxed.unwrap_or(false),
        resource_filters: c
            .resource_filters
            .iter()
            .map(|f| ResourceFilterDto {
                field: f.field.clone(),
                content: f.content.clone(),
            })
            .collect(),
    }
}

/// The request body for `local`. `live` is the package as the server holds
/// it, if any: its `outageCalendars` and each service's `userDefined` (by
/// service name) are kept, so apply never detaches a scheduled outage.
pub fn package_to_wire(
    local: &ThreshdPackageLocal,
    live: Option<&ThreshdPackageDto>,
) -> ThreshdPackageDto {
    let live_user_defined = |name: &str| {
        live.and_then(|p| p.services.iter().find(|s| s.name == name))
            .is_some_and(|s| s.user_defined)
    };
    let range = |r: &IpRangeDef| IpRangeDto {
        begin: r.begin.clone(),
        end: r.end.clone(),
    };
    ThreshdPackageDto {
        name: local.metadata.name.clone(),
        filter: local.spec.filter.clone(),
        specifics: local.spec.specifics.clone(),
        include_ranges: local.spec.include_ranges.iter().map(range).collect(),
        exclude_ranges: local.spec.exclude_ranges.iter().map(range).collect(),
        include_urls: local.spec.include_urls.clone(),
        services: local
            .spec
            .services
            .iter()
            .map(|s| ThreshdServiceDto {
                name: s.name.clone(),
                // Validated before conversion.
                interval: s.interval.millis().unwrap_or_default(),
                user_defined: live_user_defined(&s.name),
                status: s.status.clone(),
                parameters: s
                    .thresholding_group
                    .iter()
                    .map(|g| ParameterDto {
                        key: THRESHOLDING_GROUP_PARAM.to_string(),
                        value: g.clone(),
                    })
                    .chain(s.parameters.iter().map(|p| ParameterDto {
                        key: p.key.clone(),
                        value: p.value.clone(),
                    }))
                    .collect(),
            })
            .collect(),
        outage_calendars: live.map(|p| p.outage_calendars.clone()).unwrap_or_default(),
        version: None,
    }
}

// -- canonical form -----------------------------------------------------------

/// `dto` with server-managed fields cleared. Thresholds, expressions and
/// resource filters keep their order: it is evaluation order.
pub fn canonical_group(dto: &ThresholdGroupDto) -> ThresholdGroupDto {
    ThresholdGroupDto {
        read_only: false,
        version: None,
        ..dto.clone()
    }
}

/// `dto` with server-managed fields cleared and order-insensitive lists
/// sorted: ranges, specifics, include URLs, services (by name) and each
/// service's parameters.
pub fn canonical_package(dto: &ThreshdPackageDto) -> ThreshdPackageDto {
    let mut p = ThreshdPackageDto {
        version: None,
        ..dto.clone()
    };
    p.specifics.sort();
    p.include_ranges.sort();
    p.exclude_ranges.sort();
    p.include_urls.sort();
    p.outage_calendars.sort();
    p.services.sort_by(|a, b| a.name.cmp(&b.name));
    for s in &mut p.services {
        s.parameters.sort();
    }
    p
}

// -- wire -> local (export) ---------------------------------------------------

pub fn group_from_wire(dto: &ThresholdGroupDto) -> ThresholdGroupLocal {
    ThresholdGroupLocal {
        api_version: API_VERSION.to_string(),
        kind: GROUP_KIND.to_string(),
        metadata: Metadata {
            name: dto.name.clone(),
        },
        spec: ThresholdGroupSpec {
            rrd_repository: dto.rrd_repository.clone(),
            thresholds: dto
                .thresholds
                .iter()
                .map(|t| {
                    let c = CommonOut::from(&t.common);
                    ThresholdDef {
                        kind: c.kind,
                        ds_type: c.ds_type,
                        ds_name: t.ds_name.clone(),
                        value: c.value,
                        rearm: c.rearm,
                        trigger: c.trigger,
                        description: c.description,
                        ds_label: c.ds_label,
                        expr_label: c.expr_label,
                        triggered_uei: c.triggered_uei,
                        rearmed_uei: c.rearmed_uei,
                        filter_operator: c.filter_operator,
                        relaxed: c.relaxed,
                        resource_filters: c.resource_filters,
                    }
                })
                .collect(),
            expressions: dto
                .expressions
                .iter()
                .map(|e| {
                    let c = CommonOut::from(&e.common);
                    ExpressionDef {
                        kind: c.kind,
                        ds_type: c.ds_type,
                        expression: e.expression.clone(),
                        value: c.value,
                        rearm: c.rearm,
                        trigger: c.trigger,
                        description: c.description,
                        ds_label: c.ds_label,
                        expr_label: c.expr_label,
                        triggered_uei: c.triggered_uei,
                        rearmed_uei: c.rearmed_uei,
                        filter_operator: c.filter_operator,
                        relaxed: c.relaxed,
                        resource_filters: c.resource_filters,
                    }
                })
                .collect(),
        },
    }
}

/// The shared threshold fields in export form.
struct CommonOut {
    kind: String,
    ds_type: String,
    value: Scalar,
    rearm: Scalar,
    trigger: Scalar,
    description: Option<String>,
    ds_label: Option<String>,
    expr_label: Option<String>,
    triggered_uei: Option<String>,
    rearmed_uei: Option<String>,
    filter_operator: Option<String>,
    relaxed: Option<bool>,
    resource_filters: Vec<ResourceFilterDef>,
}

impl From<&ThresholdCommon> for CommonOut {
    fn from(c: &ThresholdCommon) -> Self {
        CommonOut {
            kind: c.kind.clone(),
            ds_type: c.ds_type.clone(),
            // The server's strings, verbatim: `"1.0"` must not become `1`.
            value: Scalar::Str(c.value.clone()),
            rearm: Scalar::Str(c.rearm.clone()),
            trigger: Scalar::Str(c.trigger.clone()),
            description: c.description.clone(),
            ds_label: c.ds_label.clone(),
            expr_label: c.expr_label.clone(),
            triggered_uei: c.triggered_uei.clone(),
            rearmed_uei: c.rearmed_uei.clone(),
            filter_operator: (c.filter_operator != default_filter_operator())
                .then(|| c.filter_operator.clone()),
            relaxed: c.relaxed.then_some(true),
            resource_filters: c
                .resource_filters
                .iter()
                .map(|f| ResourceFilterDef {
                    field: f.field.clone(),
                    content: f.content.clone(),
                })
                .collect(),
        }
    }
}

/// Export mapping for a package. Fails when a service carries more than one
/// `thresholding-group` parameter, which the local model cannot express.
pub fn package_from_wire(dto: &ThreshdPackageDto) -> Result<ThreshdPackageLocal> {
    let range = |r: &IpRangeDto| IpRangeDef {
        begin: r.begin.clone(),
        end: r.end.clone(),
    };
    let mut services = Vec::with_capacity(dto.services.len());
    for s in &dto.services {
        let (groups, others): (Vec<_>, Vec<_>) = s
            .parameters
            .iter()
            .partition(|p| p.key == THRESHOLDING_GROUP_PARAM);
        if groups.len() > 1 {
            return Err(Error::Config(format!(
                "package '{}' service '{}' has {} `{THRESHOLDING_GROUP_PARAM}` parameters; \
                 a ThreshdPackage document can bind one group per service",
                dto.name,
                s.name,
                groups.len()
            )));
        }
        services.push(ServiceDef {
            name: s.name.clone(),
            interval: Interval::from_millis(s.interval),
            status: s.status.clone(),
            thresholding_group: groups.first().map(|p| p.value.clone()),
            parameters: others
                .into_iter()
                .map(|p| ParameterDef {
                    key: p.key.clone(),
                    value: p.value.clone(),
                })
                .collect(),
        });
    }
    Ok(ThreshdPackageLocal {
        api_version: API_VERSION.to_string(),
        kind: PACKAGE_KIND.to_string(),
        metadata: Metadata {
            name: dto.name.clone(),
        },
        spec: ThreshdPackageSpec {
            filter: dto.filter.clone(),
            specifics: dto.specifics.clone(),
            include_ranges: dto.include_ranges.iter().map(range).collect(),
            exclude_ranges: dto.exclude_ranges.iter().map(range).collect(),
            include_urls: dto.include_urls.clone(),
            services,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::parse_documents;

    fn common(kind: &str) -> ThresholdCommon {
        ThresholdCommon {
            kind: kind.into(),
            ds_type: "if".into(),
            value: "1.0".into(),
            rearm: "0.0".into(),
            trigger: "${requisition:trigger|2}".into(),
            description: Some("errors".into()),
            ds_label: Some("ifName".into()),
            expr_label: None,
            triggered_uei: Some("uei.opennms.org/acme/high".into()),
            rearmed_uei: Some("uei.opennms.org/acme/rearm".into()),
            filter_operator: "and".into(),
            relaxed: true,
            resource_filters: vec![ResourceFilterDto {
                field: "ifName".into(),
                content: Some("^ge-.*".into()),
            }],
        }
    }

    /// A server-shaped group exercising non-default values everywhere.
    fn group_dto() -> ThresholdGroupDto {
        let mut plain = common("low");
        plain.filter_operator = "or".into();
        plain.relaxed = false;
        plain.resource_filters.clear();
        plain.description = None;
        let mut change = common("relativeChange");
        change.rearmed_uei = None;
        ThresholdGroupDto {
            name: "acme".into(),
            rrd_repository: "/opt/opennms/share/rrd/snmp/".into(),
            thresholds: vec![
                ThresholdDto {
                    common: common("high"),
                    ds_name: "ifInErrors".into(),
                },
                ThresholdDto {
                    common: plain,
                    ds_name: "ifInDiscards".into(),
                },
            ],
            expressions: vec![ExpressionDto {
                common: change,
                expression: "ifHCInOctets * 8".into(),
            }],
            read_only: false,
            version: Some("v1".into()),
        }
    }

    fn package_dto() -> ThreshdPackageDto {
        ThreshdPackageDto {
            name: "acme".into(),
            filter: "IPADDR != '0.0.0.0'".into(),
            specifics: vec!["10.0.0.9".into(), "10.0.0.7".into()],
            include_ranges: vec![
                IpRangeDto {
                    begin: "10.1.0.1".into(),
                    end: "10.1.0.9".into(),
                },
                IpRangeDto {
                    begin: "10.0.0.1".into(),
                    end: "10.0.0.9".into(),
                },
            ],
            exclude_ranges: vec![],
            include_urls: vec!["file:/opt/opennms/etc/include".into()],
            services: vec![
                ThreshdServiceDto {
                    name: "SNMP".into(),
                    interval: 300_000,
                    user_defined: true,
                    status: Some("on".into()),
                    parameters: vec![
                        ParameterDto {
                            key: THRESHOLDING_GROUP_PARAM.into(),
                            value: "acme".into(),
                        },
                        ParameterDto {
                            key: "extra".into(),
                            value: "x".into(),
                        },
                    ],
                },
                ThreshdServiceDto {
                    name: "JMX".into(),
                    interval: 1_500,
                    user_defined: false,
                    status: None,
                    parameters: vec![],
                },
            ],
            outage_calendars: vec!["weekly-maint".into()],
            version: Some("v7".into()),
        }
    }

    #[test]
    fn exported_group_converts_back_to_the_same_wire_form() {
        let dto = group_dto();
        let local = group_from_wire(&dto);
        local.validate().expect("exported group validates");
        assert_eq!(
            canonical_group(&group_to_wire(&local)),
            canonical_group(&dto)
        );
    }

    #[test]
    fn exported_package_converts_back_to_the_same_wire_form() {
        let dto = package_dto();
        let local = package_from_wire(&dto).unwrap();
        local.validate().expect("exported package validates");
        let back = package_to_wire(&local, Some(&dto));
        assert_eq!(canonical_package(&back), canonical_package(&dto));
    }

    #[test]
    fn export_survives_a_yaml_round_trip() {
        let local = group_from_wire(&group_dto());
        let yaml = serde_norway::to_string(&local).unwrap();
        assert!(
            yaml.contains("value: '1.0'") || yaml.contains("value: \"1.0\""),
            "{yaml}"
        );
        let doc = &parse_documents("g.yaml", &yaml).unwrap()[0];
        assert_eq!(ThresholdGroupLocal::from_raw(doc).unwrap(), local);

        let local = package_from_wire(&package_dto()).unwrap();
        let yaml = serde_norway::to_string(&local).unwrap();
        assert!(
            yaml.contains("interval: 5m") && yaml.contains("interval: 1500"),
            "{yaml}"
        );
        let doc = &parse_documents("p.yaml", &yaml).unwrap()[0];
        assert_eq!(ThreshdPackageLocal::from_raw(doc).unwrap(), local);
    }

    #[test]
    fn export_drops_defaults_and_unowned_fields() {
        let local = group_from_wire(&group_dto());
        let plain = &local.spec.thresholds[1];
        assert_eq!(plain.filter_operator, None, "`or` is the default");
        assert_eq!(plain.relaxed, None, "`false` is the default");
        assert_eq!(
            local.spec.thresholds[0].filter_operator.as_deref(),
            Some("and")
        );
        let yaml = serde_norway::to_string(&package_from_wire(&package_dto()).unwrap()).unwrap();
        for absent in [
            "outageCalendars",
            "userDefined",
            "version",
            THRESHOLDING_GROUP_PARAM,
        ] {
            assert!(
                !yaml.contains(absent),
                "{absent} leaked into export: {yaml}"
            );
        }
        assert!(yaml.contains("thresholdingGroup: acme"), "{yaml}");
    }

    #[test]
    fn to_wire_applies_server_defaults() {
        let yaml = "\
apiVersion: thresholding.opennms.org/v1
kind: ThresholdGroup
metadata: {name: g}
spec:
  rrdRepository: /r/
  thresholds:
    - {type: absoluteChange, dsType: node, dsName: x, value: 90, rearm: 0.5, trigger: 3, rearmedUEI: uei.x}
";
        let local = ThresholdGroupLocal::from_raw(&parse_documents("g", yaml).unwrap()[0]).unwrap();
        let t = &group_to_wire(&local).thresholds[0].common;
        assert_eq!(t.filter_operator, "or");
        assert!(!t.relaxed);
        assert_eq!(t.rearmed_uei, None, "stored as null for absoluteChange");
        assert_eq!(
            (t.value.as_str(), t.rearm.as_str(), t.trigger.as_str()),
            ("90", "0.5", "3")
        );
    }

    #[test]
    fn package_to_wire_keeps_live_calendars_and_user_defined() {
        let live = package_dto();
        let mut local = package_from_wire(&live).unwrap();
        local.spec.filter = "IPADDR != '0.0.0.0' & catincAcme".into();
        local.spec.services.push(ServiceDef {
            name: "NEW".into(),
            interval: Interval::Duration("1m".into()),
            status: None,
            thresholding_group: None,
            parameters: vec![],
        });
        let wire = package_to_wire(&local, Some(&live));
        assert_eq!(wire.outage_calendars, vec!["weekly-maint"]);
        let ud: Vec<_> = wire
            .services
            .iter()
            .map(|s| (s.name.as_str(), s.user_defined))
            .collect();
        assert_eq!(ud, [("SNMP", true), ("JMX", false), ("NEW", false)]);
        assert!(package_to_wire(&local, None).outage_calendars.is_empty());
    }

    #[test]
    fn canonical_package_ignores_list_order_but_not_values() {
        let a = package_dto();
        let mut b = a.clone();
        b.services.reverse();
        b.specifics.reverse();
        b.include_ranges.reverse();
        b.services[1].parameters.reverse();
        b.version = Some("other".into());
        assert_eq!(canonical_package(&a), canonical_package(&b));
        b.services[0].interval += 1;
        assert_ne!(canonical_package(&a), canonical_package(&b));
    }

    #[test]
    fn canonical_group_keeps_evaluation_order() {
        let a = group_dto();
        let mut b = a.clone();
        b.thresholds.reverse();
        assert_ne!(canonical_group(&a), canonical_group(&b));
        b.thresholds.reverse();
        b.read_only = true;
        b.version = None;
        assert_eq!(canonical_group(&a), canonical_group(&b));
    }

    #[test]
    fn two_group_parameters_cannot_be_exported() {
        let mut dto = package_dto();
        dto.services[0].parameters.push(ParameterDto {
            key: THRESHOLDING_GROUP_PARAM.into(),
            value: "other".into(),
        });
        let err = package_from_wire(&dto).unwrap_err().to_string();
        assert!(err.contains("2 `thresholding-group` parameters"), "{err}");
    }
}
