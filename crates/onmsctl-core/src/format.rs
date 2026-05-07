/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output format selection.
//!
//! Used by [`crate::context::Context`] and the future renderer in
//! `render.rs`. Lives in its own module so the resolver doesn't pull in
//! rendering dependencies.

use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    Table,
    Yaml,
    Json,
}

impl OutputFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Yaml => "yaml",
            Self::Json => "json",
        }
    }
}

impl FromStr for OutputFormat {
    type Err = crate::error::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "table" => Ok(Self::Table),
            "yaml" => Ok(Self::Yaml),
            "json" => Ok(Self::Json),
            other => Err(crate::error::Error::Config(format!(
                "unknown output format '{other}'; expected one of: table, yaml, json"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_table() {
        assert_eq!(OutputFormat::default(), OutputFormat::Table);
    }

    #[test]
    fn parses_known_names_case_insensitive() {
        assert_eq!(
            "table".parse::<OutputFormat>().unwrap(),
            OutputFormat::Table
        );
        assert_eq!("YAML".parse::<OutputFormat>().unwrap(), OutputFormat::Yaml);
        assert_eq!("Json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
    }

    #[test]
    fn unknown_format_errors_with_listing() {
        let err = "xml".parse::<OutputFormat>().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("xml"));
        assert!(msg.contains("table") && msg.contains("yaml") && msg.contains("json"));
    }
}
