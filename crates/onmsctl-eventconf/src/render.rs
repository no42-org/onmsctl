/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Table-row rendering for eventconf DTOs.
//!
//! Capabilities provide [`onmsctl_core::TableRow`] impls on the public-facing
//! list shapes so the CLI's `-o table` path renders them via comfy-table.
//! YAML and JSON output come for free from `serde::Serialize`.

use onmsctl_core::TableRow;
use serde::Serialize;

use crate::api::{UploadFileError, UploadFileResult};
use crate::dto::{EventConfEventDto, EventConfSourceDto, SourceNameAndId};

/// Single-column wrapper used by the `names` list endpoint so plain
/// strings can flow through the same `render_list` path as the typed
/// DTOs.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct SourceName(pub String);

impl TableRow for SourceName {
    fn headers() -> Vec<&'static str> {
        vec!["name"]
    }
    fn row(&self) -> Vec<String> {
        vec![self.0.clone()]
    }
}

impl TableRow for EventConfSourceDto {
    fn headers() -> Vec<&'static str> {
        vec!["id", "name", "vendor", "fileOrder", "eventCount", "enabled"]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.id.to_string(),
            self.name.clone(),
            self.vendor.clone().unwrap_or_default(),
            self.file_order.to_string(),
            self.event_count.to_string(),
            self.enabled.to_string(),
        ]
    }
}

impl TableRow for EventConfEventDto {
    fn headers() -> Vec<&'static str> {
        vec!["id", "sourceId", "uei", "label", "severity", "enabled"]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.id.to_string(),
            self.source_id.to_string(),
            self.uei.clone(),
            self.event_label.clone(),
            self.severity.clone(),
            self.enabled.to_string(),
        ]
    }
}

impl TableRow for SourceNameAndId {
    fn headers() -> Vec<&'static str> {
        vec!["id", "name"]
    }
    fn row(&self) -> Vec<String> {
        vec![self.id.to_string(), self.name.clone()]
    }
}

impl TableRow for UploadFileResult {
    fn headers() -> Vec<&'static str> {
        vec!["file", "status", "events", "vendor"]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.file.clone(),
            "success".to_string(),
            self.event_count.to_string(),
            self.vendor.clone().unwrap_or_default(),
        ]
    }
}

impl TableRow for UploadFileError {
    fn headers() -> Vec<&'static str> {
        vec!["file", "status", "error"]
    }
    fn row(&self) -> Vec<String> {
        vec![self.file.clone(), "error".to_string(), self.error.clone()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_dto_row_aligns_with_headers() {
        let s = EventConfSourceDto {
            id: 42,
            name: "cisco.foo".into(),
            vendor: Some("cisco".into()),
            description: None,
            file_order: 50,
            event_count: 17,
            enabled: true,
            created_time: None,
            last_modified: None,
            uploaded_by: None,
        };
        let row = s.row();
        assert_eq!(row.len(), EventConfSourceDto::headers().len());
        assert_eq!(row[0], "42");
        assert_eq!(row[1], "cisco.foo");
        assert_eq!(row[2], "cisco");
        assert_eq!(row[3], "50");
        assert_eq!(row[4], "17");
        assert_eq!(row[5], "true");
    }

    #[test]
    fn event_dto_row_aligns_with_headers() {
        let e = EventConfEventDto {
            id: 108,
            source_id: 42,
            uei: "uei.opennms.org/test".into(),
            event_label: "Test event".into(),
            severity: "Warning".into(),
            description: None,
            enabled: true,
            vendor: None,
            source_name: None,
            created_time: None,
            last_modified: None,
        };
        let row = e.row();
        assert_eq!(row.len(), EventConfEventDto::headers().len());
        assert_eq!(row[0], "108");
        assert_eq!(row[2], "uei.opennms.org/test");
    }

    #[test]
    fn source_dto_with_no_vendor_renders_empty_cell() {
        let s = EventConfSourceDto {
            id: 1,
            name: "n".into(),
            vendor: None,
            description: None,
            file_order: 0,
            event_count: 0,
            enabled: false,
            created_time: None,
            last_modified: None,
            uploaded_by: None,
        };
        assert_eq!(s.row()[2], "");
    }
}
