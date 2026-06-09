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
//! `event` is read-only: the event mutators (add / update / delete / enable /
//! disable) were removed in favour of declaring events under an `EventSource`
//! document and reconciling them through the top-level `onmsctl apply -f`.

use clap::Subcommand;
use onmsctl_core::{Context, Error, OnmsClient, Result, render_list};

use crate::api::{EventConfApi, EventFilter, EventInSourceFilter};
use crate::cmd::source::ensure_positive_id;

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
}

impl onmsctl_core::Classify for EventCmd {
    fn kind(&self) -> onmsctl_core::CmdKind {
        use onmsctl_core::CmdKind::Read;
        match self {
            EventCmd::List { .. } => Read,
        }
    }
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
        }
        Ok(())
    }
}
