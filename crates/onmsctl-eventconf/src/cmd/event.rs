/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `onmsctl event ...` subcommands.
//!
//! `event list` dispatches to one of three endpoints depending on flags
//! (the flags are clap-level mutually exclusive — the parser rejects
//! conflicting combinations rather than silently picking a winner):
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
use crate::cmd::source::ensure_positive_id;
use crate::dto::{Event, EventConfEventEditRequest};

/// Hard cap on the size of a `-f event.yaml` input file. Events are
/// typically a few hundred bytes; capping at 16 MiB matches the rest of
/// the codebase and prevents accidental OOM on a misdirected path.
const MAX_EVENT_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Subcommand, Debug, Clone)]
pub enum EventCmd {
    /// List events. Endpoint chosen by flag combination — see module docs.
    /// At least one filter (`--source`, `--uei`, `--vendor`, or
    /// `--source-name`) is required. `--source` is mutually exclusive
    /// with the cross-source filter flags.
    List {
        /// List events for a specific source id. Mutually exclusive with
        /// the cross-source filter flags.
        #[arg(
            long,
            conflicts_with_all = ["uei", "vendor", "source_name", "event_filter"]
        )]
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
    /// `--enabled true|false` is **required** so the caller cannot
    /// accidentally flip the event's enabled state by editing the body
    /// alone.
    Update {
        /// Reference in `<sourceId>/<eventId>` form.
        reference: String,
        /// Path to a YAML or JSON file describing the event body.
        #[arg(short = 'f', long)]
        file: PathBuf,
        /// Required explicit enabled flag. Must be `true` or `false`.
        #[arg(
            long,
            action = clap::ArgAction::Set,
            num_args = 1,
            required = true,
            value_name = "BOOL"
        )]
        enabled: bool,
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
                if source.is_none() && uei.is_none() && vendor.is_none() && source_name.is_none() {
                    return Err(Error::Config(
                        "event list: specify at least one filter (--source, --uei, --vendor, or --source-name)"
                            .into(),
                    ));
                }
                if let Some(sid) = source {
                    ensure_positive_id(sid, "source id")?;
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
                ensure_positive_id(source, "source id")?;
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
                let req = EventConfEventEditRequest { enabled, event };
                api.update_event(sid, eid, &req).await?;
                eprintln!("updated event {sid}/{eid} (enabled={})", enabled);
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

/// Parse `<sourceId>/<eventId>` notation into the typed pair. Both halves
/// must be present, non-empty, and non-negative.
fn parse_ref(s: &str) -> Result<(i64, i64)> {
    if s.is_empty() {
        return Err(Error::Config(
            "empty event reference; expected '<sourceId>/<eventId>'".into(),
        ));
    }
    let (left, right) = s.split_once('/').ok_or_else(|| {
        Error::Config(format!(
            "event reference '{s}' must be in '<sourceId>/<eventId>' form"
        ))
    })?;
    if left.is_empty() || right.is_empty() {
        return Err(Error::Config(format!(
            "event reference '{s}' has empty id segment; expected '<sourceId>/<eventId>'"
        )));
    }
    let sid: i64 = left
        .parse()
        .map_err(|e| Error::Config(format!("event reference '{s}': bad sourceId: {e}")))?;
    let eid: i64 = right
        .parse()
        .map_err(|e| Error::Config(format!("event reference '{s}': bad eventId: {e}")))?;
    if sid <= 0 || eid <= 0 {
        return Err(Error::Config(format!(
            "event reference '{s}': ids must be positive (got {sid}/{eid})"
        )));
    }
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
/// as a YAML subset so we don't need separate paths. Caps the file size
/// at [`MAX_EVENT_FILE_BYTES`].
fn read_event_file(path: &std::path::Path) -> Result<Event> {
    let meta = std::fs::metadata(path)
        .map_err(|e| Error::Config(format!("failed to stat {}: {e}", path.display())))?;
    if !meta.is_file() {
        return Err(Error::Config(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if meta.len() > MAX_EVENT_FILE_BYTES {
        return Err(Error::Config(format!(
            "{} is {} bytes, exceeds event-file cap of {} bytes",
            path.display(),
            meta.len(),
            MAX_EVENT_FILE_BYTES
        )));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;

    // Pre-flight: catch the common mistake of passing an EventSource
    // document (top-level apiVersion / kind / spec) to `event add`.
    // `Event`'s fields are all `Option`, so serde would silently parse
    // such a file as a default-everything Event; the empty POST would
    // then 400 server-side with the unhelpful "Event 'uei' is required".
    if looks_like_event_source(&bytes) {
        return Err(Error::Config(format!(
            "{} looks like an EventSource document (top-level `apiVersion` / `kind` / `spec`). \
             `event add` expects a single Event payload (uei, eventLabel, severity, …). \
             Either extract one event from `spec.events` into its own file, \
             or use `source apply -f {}` to upload the whole EventSource.",
            path.display(),
            path.display()
        )));
    }

    serde_norway::from_slice(&bytes).map_err(|e| {
        Error::Config(format!(
            "failed to parse event from {}: {e}",
            path.display()
        ))
    })
}

/// True when the YAML/JSON body parses to a top-level object that carries
/// the EventSource sigil keys. Used by [`read_event_file`] to short-circuit
/// the common "wrong doc shape" mistake before the request hits the wire.
fn looks_like_event_source(bytes: &[u8]) -> bool {
    let Ok(v) = serde_norway::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    let Some(obj) = v.as_object() else {
        return false;
    };
    obj.contains_key("apiVersion") || obj.contains_key("kind") || obj.contains_key("spec")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_event_source_flags_full_fixture() {
        let yaml = r#"
apiVersion: onmsctl.opennms.org/v1
kind: EventSource
metadata:
  name: cisco.foo
spec:
  events:
    - uei: uei.opennms.org/x
      eventLabel: x
      severity: Warning
"#;
        assert!(looks_like_event_source(yaml.as_bytes()));
    }

    #[test]
    fn looks_like_event_source_does_not_flag_single_event() {
        let yaml = r#"
uei: uei.opennms.org/example
eventLabel: Example
severity: Warning
"#;
        assert!(!looks_like_event_source(yaml.as_bytes()));
    }

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

    #[test]
    fn parse_ref_rejects_empty_string() {
        let err = parse_ref("").unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("empty event reference")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_ref_rejects_empty_segments() {
        for bad in ["/108", "42/", "/", " /1", "1/ "] {
            let err = parse_ref(bad).unwrap_err();
            assert!(matches!(err, Error::Config(_)), "for input '{bad}'");
        }
    }

    #[test]
    fn parse_ref_rejects_negative_ids() {
        let err = parse_ref("-1/108").unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("must be positive")),
            other => panic!("unexpected {other:?}"),
        }
        let err = parse_ref("42/-5").unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("must be positive")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_ref_rejects_zero_ids() {
        let err = parse_ref("0/108").unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("must be positive")),
            other => panic!("unexpected {other:?}"),
        }
    }
}
