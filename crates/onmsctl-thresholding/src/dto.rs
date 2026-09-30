/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Wire-format DTOs for `/api/v2/thresholding` and `/api/v2/threshd`,
//! transcribed from the published OpenAPI document of a Horizon build with
//! NMS-19837.
//!
//! Server-managed fields (`version`, `readOnly`) are read but never sent:
//! they are `skip_serializing`. Nullable strings are `Option<String>` and are
//! sent as `null`, the way the server returns them, so a DTO read with `GET`
//! and sent back unchanged is byte-for-byte the same document.

use serde::{Deserialize, Serialize};

// -- thresholding ------------------------------------------------------------

/// A threshold group with its thresholds and expressions, in evaluation order.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdGroupDto {
    pub name: String,
    pub rrd_repository: String,
    #[serde(default)]
    pub thresholds: Vec<ThresholdDto>,
    #[serde(default)]
    pub expressions: Vec<ExpressionDto>,
    /// True for groups contributed by an OSGi extension. Never sent.
    #[serde(default, skip_serializing)]
    pub read_only: bool,
    /// Opaque version, also the ETag. Never sent.
    #[serde(default, skip_serializing)]
    pub version: Option<String>,
}

/// Fields shared by basic thresholds and expressions.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdCommon {
    #[serde(rename = "type")]
    pub kind: String,
    pub ds_type: String,
    pub value: String,
    pub rearm: String,
    pub trigger: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub ds_label: Option<String>,
    #[serde(default)]
    pub expr_label: Option<String>,
    #[serde(default, rename = "triggeredUEI")]
    pub triggered_uei: Option<String>,
    #[serde(default, rename = "rearmedUEI")]
    pub rearmed_uei: Option<String>,
    #[serde(default = "default_filter_operator")]
    pub filter_operator: String,
    #[serde(default)]
    pub relaxed: bool,
    #[serde(default)]
    pub resource_filters: Vec<ResourceFilterDto>,
}

/// A basic threshold on one collected datasource.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdDto {
    #[serde(flatten)]
    pub common: ThresholdCommon,
    pub ds_name: String,
}

/// A threshold on an expression over one or more datasources.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExpressionDto {
    #[serde(flatten)]
    pub common: ThresholdCommon,
    pub expression: String,
}

/// A Java regular expression matched against one resource field.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct ResourceFilterDto {
    pub field: String,
    #[serde(default)]
    pub content: Option<String>,
}

/// One row of `GET /thresholding/groups`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdGroupSummaryDto {
    pub name: String,
    #[serde(default)]
    pub rrd_repository: String,
    #[serde(default)]
    pub threshold_count: i32,
    #[serde(default)]
    pub expression_count: i32,
    #[serde(default)]
    pub read_only: bool,
}

/// `GET /thresholding/metadata`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdingMetadataDto {
    #[serde(default)]
    pub ds_types: Vec<DsTypeDto>,
    #[serde(default)]
    pub filter_operators: Vec<String>,
    #[serde(default)]
    pub threshold_types: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DsTypeDto {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
}

pub(crate) fn default_filter_operator() -> String {
    "or".to_string()
}

// -- threshd -----------------------------------------------------------------

/// A threshd package: a filter selecting interfaces plus the services to
/// threshold on them.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThreshdPackageDto {
    pub name: String,
    pub filter: String,
    #[serde(default)]
    pub specifics: Vec<String>,
    #[serde(default)]
    pub include_ranges: Vec<IpRangeDto>,
    #[serde(default)]
    pub exclude_ranges: Vec<IpRangeDto>,
    #[serde(default)]
    pub include_urls: Vec<String>,
    #[serde(default)]
    pub services: Vec<ThreshdServiceDto>,
    /// Owned by the scheduled-outages API (and `kind: Maintenance`). Sent
    /// back as read, never authored.
    #[serde(default)]
    pub outage_calendars: Vec<String>,
    /// Opaque version, also the ETag. Never sent.
    #[serde(default, skip_serializing)]
    pub version: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThreshdServiceDto {
    pub name: String,
    /// Milliseconds between evaluations.
    pub interval: i64,
    #[serde(default)]
    pub user_defined: bool,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub parameters: Vec<ParameterDto>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct ParameterDto {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase")]
pub struct IpRangeDto {
    pub begin: String,
    pub end: String,
}

/// One row of `GET /threshd/packages`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThreshdPackageSummaryDto {
    pub name: String,
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub service_count: i32,
    #[serde(default)]
    pub outage_calendars: Vec<String>,
}

/// The service parameter that binds a threshold group.
pub const THRESHOLDING_GROUP_PARAM: &str = "thresholding-group";

#[cfg(test)]
mod tests {
    use super::*;

    /// A group and a package as returned by the NMS-19837 build (stock
    /// `mib2`, trimmed to one threshold and one expression).
    const GROUP: &str = r#"{"name":"mib2","rrdRepository":"/opt/opennms/share/rrd/snmp/",
      "thresholds":[{"relaxed":false,"description":"errors","type":"high","dsType":"node",
        "value":"1.0","rearm":"0.0","trigger":"1","dsLabel":null,"exprLabel":null,
        "triggeredUEI":null,"rearmedUEI":null,"filterOperator":"or","resourceFilters":[],
        "dsName":"tcpInErrors"}],
      "expressions":[{"relaxed":false,"description":null,"type":"high","dsType":"if",
        "value":"90.0","rearm":"75.0","trigger":"2","dsLabel":"ifName","exprLabel":null,
        "triggeredUEI":null,"rearmedUEI":null,"filterOperator":"or",
        "resourceFilters":[{"field":"ifHighSpeed","content":"^[1-9]+[0-9]*$"}],
        "expression":"ifHCInOctets * 8 / 1000000 / ifHighSpeed * 100"}],
      "readOnly":false,"version":"1aac"}"#;

    const PACKAGE: &str = r#"{"name":"mib2","filter":"IPADDR != '0.0.0.0'","specifics":[],
      "includeRanges":[{"begin":"1.1.1.1","end":"254.254.254.254"}],"excludeRanges":[],
      "includeUrls":[],"services":[{"name":"SNMP","interval":300000,"userDefined":false,
        "status":"on","parameters":[{"key":"thresholding-group","value":"mib2"}]}],
      "outageCalendars":[],"version":"e65e"}"#;

    fn without(mut v: serde_json::Value, keys: &[&str]) -> serde_json::Value {
        for k in keys {
            v.as_object_mut().unwrap().remove(*k);
        }
        v
    }

    #[test]
    fn group_round_trips_minus_server_managed_fields() {
        let dto: ThresholdGroupDto = serde_json::from_str(GROUP).unwrap();
        assert_eq!(dto.version.as_deref(), Some("1aac"));
        assert_eq!(dto.thresholds[0].ds_name, "tcpInErrors");
        assert_eq!(dto.expressions[0].common.resource_filters.len(), 1);
        let back = serde_json::to_value(&dto).unwrap();
        let wire = without(
            serde_json::from_str(GROUP).unwrap(),
            &["readOnly", "version"],
        );
        assert_eq!(back, wire);
    }

    #[test]
    fn package_round_trips_minus_version() {
        let dto: ThreshdPackageDto = serde_json::from_str(PACKAGE).unwrap();
        assert_eq!(dto.services[0].interval, 300_000);
        let back = serde_json::to_value(&dto).unwrap();
        let wire = without(serde_json::from_str(PACKAGE).unwrap(), &["version"]);
        assert_eq!(back, wire);
    }
}
