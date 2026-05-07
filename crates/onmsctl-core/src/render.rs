/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output rendering for `onmsctl` commands.
//!
//! Per cli-core spec "table, json, and yaml output formats" requirement —
//! Table is the default for collection-returning commands; YAML and JSON
//! are pipe-friendly. Capabilities provide a [`TableRow`] impl on any
//! DTO they want to render as a table; YAML and JSON come for free from
//! `serde::Serialize`.

use comfy_table::{ContentArrangement, Table};
use serde::Serialize;

use crate::error::Result;
use crate::format::OutputFormat;

/// DTOs that can be rendered as a table row. Capabilities implement this on
/// the public-facing list shape (e.g. `EventConfSourceDto`).
pub trait TableRow {
    /// Column headers in display order. Static so the implementation is
    /// trivial and the headers consistent across the process.
    fn headers() -> Vec<&'static str>;
    /// One row, indexed parallel to [`Self::headers`].
    fn row(&self) -> Vec<String>;
}

/// Render a collection of items in the requested format.
pub fn render_list<T>(items: &[T], format: OutputFormat) -> Result<String>
where
    T: Serialize + TableRow,
{
    match format {
        OutputFormat::Table => Ok(render_table(items)),
        OutputFormat::Yaml => Ok(serde_norway::to_string(items)?),
        OutputFormat::Json => Ok(serde_json::to_string_pretty(items)?),
    }
}

/// Render a single item in the requested format. For Table output a one-row
/// table is produced.
pub fn render_one<T>(item: &T, format: OutputFormat) -> Result<String>
where
    T: Serialize + TableRow,
{
    match format {
        OutputFormat::Table => Ok(render_table(std::slice::from_ref(item))),
        OutputFormat::Yaml => Ok(serde_norway::to_string(item)?),
        OutputFormat::Json => Ok(serde_json::to_string_pretty(item)?),
    }
}

fn render_table<T: TableRow>(items: &[T]) -> String {
    let mut t = Table::new();
    t.set_content_arrangement(ContentArrangement::DynamicFullWidth)
        .set_header(T::headers());
    for item in items {
        t.add_row(item.row());
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Source {
        id: i64,
        name: String,
        vendor: String,
    }

    impl TableRow for Source {
        fn headers() -> Vec<&'static str> {
            vec!["id", "name", "vendor"]
        }
        fn row(&self) -> Vec<String> {
            vec![self.id.to_string(), self.name.clone(), self.vendor.clone()]
        }
    }

    fn sample() -> Vec<Source> {
        vec![
            Source {
                id: 42,
                name: "cisco.foo".into(),
                vendor: "cisco".into(),
            },
            Source {
                id: 43,
                name: "juniper.bar".into(),
                vendor: "juniper".into(),
            },
        ]
    }

    #[test]
    fn json_output_is_an_array_for_lists() {
        let out = render_list(&sample(), OutputFormat::Json).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v.is_array());
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["name"], "cisco.foo");
    }

    #[test]
    fn yaml_output_round_trips() {
        let out = render_list(&sample(), OutputFormat::Yaml).unwrap();
        assert!(out.contains("name: cisco.foo"));
        assert!(out.contains("vendor: juniper"));
    }

    #[test]
    fn table_output_contains_all_headers_and_rows() {
        let out = render_list(&sample(), OutputFormat::Table).unwrap();
        // Every header appears
        for h in Source::headers() {
            assert!(out.contains(h), "missing header '{h}' in:\n{out}");
        }
        // Every cell value appears
        assert!(out.contains("cisco.foo"));
        assert!(out.contains("juniper.bar"));
        assert!(out.contains("42"));
    }

    #[test]
    fn render_one_produces_single_item_yaml_not_array() {
        let s = Source {
            id: 1,
            name: "x".into(),
            vendor: "v".into(),
        };
        let out = render_one(&s, OutputFormat::Yaml).unwrap();
        // Single-item YAML starts with field names directly, not `- `.
        assert!(!out.trim_start().starts_with('-'));
        assert!(out.contains("name: x"));
    }

    #[test]
    fn render_one_produces_object_json_not_array() {
        let s = Source {
            id: 1,
            name: "x".into(),
            vendor: "v".into(),
        };
        let out = render_one(&s, OutputFormat::Json).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v.is_object());
        assert_eq!(v["name"], "x");
    }

    #[test]
    fn empty_list_renders_table_with_only_headers() {
        let empty: Vec<Source> = vec![];
        let out = render_list(&empty, OutputFormat::Table).unwrap();
        for h in Source::headers() {
            assert!(out.contains(h));
        }
        // Without any rows the table still has the header line, so this is
        // just an existence check; we do not assert about the line count
        // since comfy-table draws the box-frame regardless.
    }

    #[test]
    fn empty_list_renders_empty_json_array() {
        let empty: Vec<Source> = vec![];
        let out = render_list(&empty, OutputFormat::Json).unwrap();
        assert_eq!(out.trim(), "[]");
    }
}
