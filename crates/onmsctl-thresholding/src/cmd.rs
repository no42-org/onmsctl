/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `onmsctl threshold` subcommand surface (design D7).
//!
//! `group`/`package` `list` and `get`, and `export`, are Read; both
//! `delete` verbs are Write. `get` and `export` emit documents `apply`
//! accepts. Every command runs the version gate first, so an older server
//! gets the Horizon 37.0.0 message instead of a bare 404.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use onmsctl_core::{Classify, CmdKind, Context, Error, OnmsClient, Result, TableRow, render_list};
use serde::Serialize;

use crate::api::{ThreshdApi, ThresholdingApi};
use crate::convert::{group_from_wire, package_from_wire};
use crate::dto::{THRESHOLDING_GROUP_PARAM, ThreshdPackageDto};

/// `onmsctl threshold …` verbs.
#[derive(Subcommand, Debug, Clone)]
pub enum ThresholdCmd {
    /// Threshold groups (thresholds and expressions).
    #[command(subcommand)]
    Group(GroupCmd),
    /// threshd packages (which interfaces and services are thresholded).
    #[command(subcommand)]
    Package(PackageCmd),
    /// Export every group and package as ThresholdGroup / ThreshdPackage
    /// documents. Extension-contributed groups are skipped.
    Export {
        /// Write one file per object into this directory
        /// (`threshold-group-<name>.yaml`, `threshd-package-<name>.yaml`)
        /// instead of a `---`-separated stream on stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Overwrite existing files in the `--out` directory.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum GroupCmd {
    /// List every threshold group, extension groups included.
    List,
    /// Print one group as a ThresholdGroup document.
    Get {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Delete a group. Refused while a package still binds it.
    Delete {
        #[arg(value_name = "NAME")]
        name: String,
        /// Delete even though packages bind the group (they keep a dangling
        /// reference).
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum PackageCmd {
    /// List every threshd package.
    List,
    /// Print one package as a ThreshdPackage document.
    Get {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Delete a package. The server refuses to delete the last one.
    Delete {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

impl Classify for ThresholdCmd {
    fn kind(&self) -> CmdKind {
        match self {
            ThresholdCmd::Group(GroupCmd::Delete { .. })
            | ThresholdCmd::Package(PackageCmd::Delete { .. }) => CmdKind::Write,
            _ => CmdKind::Read,
        }
    }
}

impl ThresholdCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let groups = ThresholdingApi::new(&client);
        let packages = ThreshdApi::new(&client);
        groups.metadata().await?;
        match self {
            ThresholdCmd::Group(GroupCmd::List) => {
                let rows: Vec<GroupRow> = groups
                    .list_groups()
                    .await?
                    .into_iter()
                    .map(|g| GroupRow {
                        name: g.name,
                        rrd_repository: g.rrd_repository,
                        thresholds: g.threshold_count,
                        expressions: g.expression_count,
                        read_only: g.read_only,
                    })
                    .collect();
                print!("{}", render_list(&rows, ctx.output_format)?);
            }
            ThresholdCmd::Group(GroupCmd::Get { name }) => {
                let (dto, _) = groups
                    .get_group(&name)
                    .await?
                    .ok_or_else(|| not_found("threshold group", &name))?;
                print!("{}", yaml(&group_from_wire(&dto))?);
            }
            ThresholdCmd::Group(GroupCmd::Delete { name, force }) => {
                let bound = bound_by(&all_packages(&packages).await?, &name);
                if !bound.is_empty() && !force {
                    return Err(Error::Config(format!(
                        "threshold group '{name}' is bound by package(s) {}; unbind it first or \
                         pass --force to leave them with a dangling reference",
                        bound.join(", ")
                    )));
                }
                let (_, etag) = groups
                    .get_group(&name)
                    .await?
                    .ok_or_else(|| not_found("threshold group", &name))?;
                groups.delete_group(&name, etag.as_deref()).await?;
                eprintln!("Deleted threshold group {name:?}");
            }
            ThresholdCmd::Package(PackageCmd::List) => {
                let rows: Vec<PackageRow> = packages
                    .list_packages()
                    .await?
                    .into_iter()
                    .map(|p| PackageRow {
                        name: p.name,
                        filter: p.filter,
                        services: p.service_count,
                        outage_calendars: p.outage_calendars.join(","),
                    })
                    .collect();
                print!("{}", render_list(&rows, ctx.output_format)?);
            }
            ThresholdCmd::Package(PackageCmd::Get { name }) => {
                let (dto, _) = packages
                    .get_package(&name)
                    .await?
                    .ok_or_else(|| not_found("threshd package", &name))?;
                print!("{}", yaml(&package_from_wire(&dto)?)?);
            }
            ThresholdCmd::Package(PackageCmd::Delete { name }) => {
                let (_, etag) = packages
                    .get_package(&name)
                    .await?
                    .ok_or_else(|| not_found("threshd package", &name))?;
                packages.delete_package(&name, etag.as_deref()).await?;
                eprintln!("Deleted threshd package {name:?}");
            }
            ThresholdCmd::Export { out, force } => {
                let mut files = Vec::new();
                let mut skipped = 0;
                for g in groups.list_groups().await? {
                    if g.read_only {
                        skipped += 1;
                        continue;
                    }
                    if let Some((dto, _)) = groups.get_group(&g.name).await? {
                        files.push((
                            format!("threshold-group-{}.yaml", dto.name),
                            yaml(&group_from_wire(&dto))?,
                        ));
                    }
                }
                for p in all_packages(&packages).await? {
                    files.push((
                        format!("threshd-package-{}.yaml", p.name),
                        yaml(&package_from_wire(&p)?)?,
                    ));
                }
                match out {
                    Some(dir) => {
                        write_files(&dir, &files, force)?;
                        eprintln!("Wrote {} file(s) to {}", files.len(), dir.display());
                    }
                    None => {
                        let docs: Vec<&str> = files.iter().map(|(_, y)| y.as_str()).collect();
                        print!("{}", docs.join("---\n"));
                    }
                }
                if skipped > 0 {
                    eprintln!("Skipped {skipped} extension-contributed group(s) (read-only)");
                }
            }
        }
        Ok(())
    }
}

/// Every package in full (the list endpoint omits services).
async fn all_packages(api: &ThreshdApi<'_>) -> Result<Vec<ThreshdPackageDto>> {
    let mut out = Vec::new();
    for p in api.list_packages().await? {
        if let Some((dto, _)) = api.get_package(&p.name).await? {
            out.push(dto);
        }
    }
    Ok(out)
}

/// Names of the packages with a service bound to `group`.
fn bound_by(packages: &[ThreshdPackageDto], group: &str) -> Vec<String> {
    packages
        .iter()
        .filter(|p| {
            p.services.iter().any(|s| {
                s.parameters
                    .iter()
                    .any(|x| x.key == THRESHOLDING_GROUP_PARAM && x.value == group)
            })
        })
        .map(|p| p.name.clone())
        .collect()
}

/// Write `files` into `dir`, checking every target first (all or nothing).
fn write_files(dir: &Path, files: &[(String, String)], force: bool) -> Result<()> {
    if dir.exists() && !dir.is_dir() {
        return Err(Error::Config(format!(
            "--out path {} exists and is not a directory",
            dir.display()
        )));
    }
    for (name, _) in files {
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(Error::Config(format!(
                "refusing to write '{}/{name}': the object name contains path-unsafe characters \
                 (allowed: alphanumeric, '-', '_', '.')",
                dir.display()
            )));
        }
        let path = dir.join(name);
        if path.exists() && !force {
            return Err(Error::Config(format!(
                "refusing to overwrite existing file {}; pass --force to override",
                path.display()
            )));
        }
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::Config(format!("creating {}: {e}", dir.display())))?;
    for (name, body) in files {
        let path = dir.join(name);
        std::fs::write(&path, body)
            .map_err(|e| Error::Config(format!("write {}: {e}", path.display())))?;
    }
    Ok(())
}

fn yaml<T: Serialize>(v: &T) -> Result<String> {
    serde_norway::to_string(v).map_err(|e| Error::Config(format!("rendering YAML: {e}")))
}

fn not_found(what: &str, name: &str) -> Error {
    Error::Config(format!("no {what} named {name:?}"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupRow {
    name: String,
    rrd_repository: String,
    thresholds: i32,
    expressions: i32,
    read_only: bool,
}

impl TableRow for GroupRow {
    fn headers() -> Vec<&'static str> {
        vec![
            "NAME",
            "RRD REPOSITORY",
            "THRESHOLDS",
            "EXPRESSIONS",
            "READ-ONLY",
        ]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.rrd_repository.clone(),
            self.thresholds.to_string(),
            self.expressions.to_string(),
            if self.read_only { "yes" } else { "" }.to_string(),
        ]
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageRow {
    name: String,
    filter: String,
    services: i32,
    outage_calendars: String,
}

impl TableRow for PackageRow {
    fn headers() -> Vec<&'static str> {
        vec!["NAME", "FILTER", "SERVICES", "OUTAGES"]
    }
    fn row(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.filter.clone(),
            self.services.to_string(),
            self.outage_calendars.clone(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{ParameterDto, ThreshdServiceDto};

    #[test]
    fn classify_read_and_write() {
        let read = [
            ThresholdCmd::Group(GroupCmd::List),
            ThresholdCmd::Group(GroupCmd::Get { name: "g".into() }),
            ThresholdCmd::Package(PackageCmd::List),
            ThresholdCmd::Package(PackageCmd::Get { name: "p".into() }),
            ThresholdCmd::Export {
                out: None,
                force: false,
            },
        ];
        assert!(read.iter().all(|c| c.kind() == CmdKind::Read));
        let write = [
            ThresholdCmd::Group(GroupCmd::Delete {
                name: "g".into(),
                force: false,
            }),
            ThresholdCmd::Package(PackageCmd::Delete { name: "p".into() }),
        ];
        assert!(write.iter().all(|c| c.kind() == CmdKind::Write));
    }

    fn package(name: &str, group: &str) -> ThreshdPackageDto {
        ThreshdPackageDto {
            name: name.into(),
            services: vec![ThreshdServiceDto {
                name: "SNMP".into(),
                interval: 1,
                parameters: vec![ParameterDto {
                    key: THRESHOLDING_GROUP_PARAM.into(),
                    value: group.into(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn bound_by_finds_binding_packages() {
        let pkgs = [
            package("a", "cpu"),
            package("b", "mem"),
            package("c", "cpu"),
        ];
        assert_eq!(bound_by(&pkgs, "cpu"), ["a", "c"]);
        assert!(bound_by(&pkgs, "disk").is_empty());
    }

    #[test]
    fn write_files_is_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("t");
        let files = vec![
            ("threshold-group-a.yaml".to_string(), "a\n".to_string()),
            ("threshd-package-b.yaml".to_string(), "b\n".to_string()),
        ];
        write_files(&out, &files, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("threshd-package-b.yaml")).unwrap(),
            "b\n"
        );
        let err = write_files(&out, &files, false).unwrap_err().to_string();
        assert!(err.contains("refusing to overwrite"), "{err}");
        write_files(&out, &files, true).unwrap();
        let bad = vec![("threshold-group-a/b.yaml".to_string(), String::new())];
        assert!(
            write_files(&out, &bad, true)
                .unwrap_err()
                .to_string()
                .contains("path-unsafe")
        );
    }
}
