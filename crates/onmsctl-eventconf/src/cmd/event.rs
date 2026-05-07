/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl event ...` subcommands.
//!
//! `event list` dispatches to one of three endpoints depending on flags:
//!
//!   - `--source <id>`   → `GET /eventconf/filter/{id}/events`
//!   - `--vendor <name>` (alone) → `GET /eventconf/vendors/{name}/events`
//!   - any combination of `--uei` / `--vendor` / `--source-name` →
//!     `GET /eventconf/filter`
//!
//! Mutations (add / update / delete / enable / disable) take
//! `<sourceId>/<eventId>` references; bulk delete / enable / disable group
//! refs by source-id so the per-source endpoints can be invoked
//! efficiently.

use std::path::PathBuf;

use clap::Subcommand;
use onmsctl_core::{Context, Error, OnmsClient, Result, render_list};

use crate::api::{EventConfApi, EventFilter, EventInSourceFilter};
use crate::dto::{Event, EventConfEventEditRequest};

#[derive(Subcommand, Debug, Clone)]
pub enum EventCmd {
    /// List events. Endpoint chosen by flag combination — see module docs.
    List {
        /// List events for a specific source id.
        #[arg(long)]
        source: Option<i64>,
        /// Cross-source filter on UEI (substring match).
        #[arg(long)]
        uei: Option<String>,
        /// Filter on vendor. Alone (no other filter), uses the
        /// `/vendors/{name}/events` endpoint; combined with other flags,
        /// uses the cross-source `/filter` endpoint.
        #[arg(long)]
        vendor: Option<String>,
        /// Cross-source filter on the source name (substring match).
        #[arg(long = "source-name")]
        source_name: Option<String>,
        /// Per-source filter substring (only used with `--source`).
        #[arg(long = "event-filter")]
        event_filter: Option<String>,
        /// Sort field (only used with `--source`).
        #[arg(long = "sort-by")]
        sort_by: Option<String>,
        /// Sort order: `asc` or `desc`.
        #[arg(long)]
        order: Option<String>,
        /// Pagination offset.
        #[arg(long)]
        offset: Option<i32>,
        /// Pagination limit.
        #[arg(long)]
        limit: Option<i32>,
    },

    /// Add a new event under a source from a YAML/JSON file.
    Add {
        /// Source id under which to create the event.
        #[arg(long)]
        source: i64,
        /// Path to a YAML or JSON file describing the event.
        #[arg(short = 'f', long)]
        file: PathBuf,
    },

    /// Update an event by `<sourceId>/<eventId>` from a YAML/JSON file.
    /// `--enabled` may be passed alongside `-f` to override the enabled
    /// flag; otherwise the file's body is taken as-is and the request's
    /// `enabled` flag defaults to `true`.
    Update {
        /// Reference in `<sourceId>/<eventId>` form.
        reference: String,
        /// Path to a YAML or JSON file describing the event body.
        #[arg(short = 'f', long)]
        file: PathBuf,
        /// Override the event's enabled flag for this update.
        #[arg(long)]
        enabled: Option<bool>,
    },

    /// Delete one or more events. Refs grouped by source-id and one
    /// `DELETE /sources/{id}/events` issued per group.
    Delete {
        /// References in `<sourceId>/<eventId>` form.
        #[arg(required = true)]
        refs: Vec<String>,
    },

    /// Enable one or more events.
    Enable {
        /// References in `<sourceId>/<eventId>` form.
        #[arg(required = true)]
        refs: Vec<String>,
    },

    /// Disable one or more events.
    Disable {
        /// References in `<sourceId>/<eventId>` form.
        #[arg(required = true)]
        refs: Vec<String>,
    },
}

impl EventCmd {
    pub async fn run(self, ctx: &Context) -> Result<()> {
        let client = OnmsClient::from_context(ctx)?;
        let api = EventConfApi::new(&client);
        match self {
            EventCmd::List {
                source,
                uei,
                vendor,
                source_name,
                event_filter,
                sort_by,
                order,
                offset,
                limit,
            } => {
                if let Some(sid) = source {
                    let f = EventInSourceFilter {
                        event_filter,
                        event_sort_by: sort_by,
                        event_order: order,
                        offset,
                        limit,
                    };
                    let page = api.list_events_in_source(sid, &f).await?;
                    let out = render_list(&page.items, ctx.output_format)?;
                    println!("{out}");
                } else if let Some(v) = vendor.as_ref()
                    && uei.is_none()
                    && source_name.is_none()
                {
                    // Vendor-only shortcut.
                    let items = api.get_events_by_vendor(v).await?;
                    let out = render_list(&items, ctx.output_format)?;
                    println!("{out}");
                } else {
                    let f = EventFilter {
                        uei,
                        vendor,
                        source_name,
                        offset,
                        limit,
                    };
                    let items = api.filter_events(&f).await?;
                    let out = render_list(&items, ctx.output_format)?;
                    println!("{out}");
                }
            }
            EventCmd::Add { source, file } => {
                let event: Event = read_event_file(&file)?;
                let id = api.add_event(source, &event).await?;
                eprintln!("created event {id} under source {source}");
            }
            EventCmd::Update {
                reference,
                file,
                enabled,
            } => {
                let (sid, eid) = parse_ref(&reference)?;
                let event: Event = read_event_file(&file)?;
                let req = EventConfEventEditRequest {
                    enabled: enabled.unwrap_or(true),
                    event,
                };
                api.update_event(sid, eid, &req).await?;
                eprintln!("updated event {sid}/{eid}");
            }
            EventCmd::Delete { refs } => {
                run_grouped(&refs, "delete", |sid, eids| {
                    let api = EventConfApi::new(&client);
                    Box::pin(async move { api.delete_events(sid, &eids).await })
                })
                .await?;
            }
            EventCmd::Enable { refs } => {
                run_grouped(&refs, "enable", |sid, eids| {
                    let api = EventConfApi::new(&client);
                    Box::pin(async move { api.set_events_enabled(sid, &eids, true).await })
                })
                .await?;
            }
            EventCmd::Disable { refs } => {
                run_grouped(&refs, "disable", |sid, eids| {
                    let api = EventConfApi::new(&client);
                    Box::pin(async move { api.set_events_enabled(sid, &eids, false).await })
                })
                .await?;
            }
        }
        Ok(())
    }
}

/// Parse `<sourceId>/<eventId>` notation into the typed pair. Used by every
/// event mutation command.
fn parse_ref(s: &str) -> Result<(i64, i64)> {
    let (left, right) = s.split_once('/').ok_or_else(|| {
        Error::Config(format!(
            "event reference '{s}' must be in '<sourceId>/<eventId>' form"
        ))
    })?;
    let sid: i64 = left
        .parse()
        .map_err(|e| Error::Config(format!("event reference '{s}': bad sourceId: {e}")))?;
    let eid: i64 = right
        .parse()
        .map_err(|e| Error::Config(format!("event reference '{s}': bad eventId: {e}")))?;
    Ok((sid, eid))
}

/// Group `<source>/<event>` refs by source-id and call `op` once per
/// source group.
async fn run_grouped<'a, F>(refs: &[String], verb: &str, mut op: F) -> Result<()>
where
    F: FnMut(
        i64,
        Vec<i64>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>,
{
    use std::collections::BTreeMap;
    let mut grouped: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for r in refs {
        let (sid, eid) = parse_ref(r)?;
        grouped.entry(sid).or_default().push(eid);
    }
    let mut total = 0;
    for (sid, eids) in &grouped {
        op(*sid, eids.clone()).await?;
        total += eids.len();
    }
    eprintln!(
        "{verb}: {total} event(s) across {} source(s)",
        grouped.len()
    );
    Ok(())
}

/// Read an Event from a YAML or JSON file. `serde_norway` accepts JSON
/// as a YAML subset so we don't need separate paths.
fn read_event_file(path: &std::path::Path) -> Result<Event> {
    let bytes = std::fs::read(path)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
    serde_norway::from_slice(&bytes).map_err(|e| {
        Error::Config(format!(
            "failed to parse event from {}: {e}",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ref_accepts_simple_pair() {
        let (s, e) = parse_ref("42/108").unwrap();
        assert_eq!(s, 42);
        assert_eq!(e, 108);
    }

    #[test]
    fn parse_ref_rejects_missing_separator() {
        let err = parse_ref("42").unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("must be in")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_ref_rejects_non_numeric_segment() {
        let err = parse_ref("abc/108").unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("bad sourceId")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_ref_rejects_extra_separators() {
        // `42/108/extra` — split_once only sees the first `/`, so the
        // right side parse fails on `108/extra`.
        let err = parse_ref("42/108/extra").unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("bad eventId")),
            other => panic!("unexpected {other:?}"),
        }
    }
}
