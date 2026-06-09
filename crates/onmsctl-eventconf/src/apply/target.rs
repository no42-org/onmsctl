/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `ApplyTarget` impl for the `EventSource` resource.
//!
//! Drives the apply algorithm from `design.md §5.2`:
//!
//!   1. validate local (already done in `EventSourceLocal::from_yaml`).
//!   2. Render local events to eventconf XML.
//!   3. Fetch server state by name. Ambiguous → error.
//!   4. If server exists and (canonical XML matches) AND (enabled flags
//!      match) → no diff → `Unchanged`.
//!   5. Otherwise upload via multipart with the rendered XML; the server
//!      upserts (delete-and-replace events under the source).
//!   6. If `spec.enabled = false`, follow up with PATCH `/sources/status`.
//!      Brief enabled-flap window per design.md §6 limitation 2.

use async_trait::async_trait;
use onmsctl_core::client::MultipartPart;
use onmsctl_core::error::PostUploadLookupKind;
use onmsctl_core::{Context, Diff, Error, OnmsClient, Result};
use serde::Serialize;

use crate::api::{EventConfApi, SourceLookup};
use crate::apply::diff::{EventSetDiff, diff_event_sets, render_diff};
use crate::apply::local::EventSourceLocal;
use crate::dto::Event;
use crate::xml::{parse_events_from_xml, render_eventconf_xml, xml_canonical};

/// Marker type implementing [`onmsctl_core::ApplyTarget`] for
/// `EventSource` documents. Construct via the
/// `onmsctl_core::run_apply::<EventSourceTarget>` driver.
pub struct EventSourceTarget;

/// Server-side state for an `EventSource`. Carries the source row, its
/// parsed events, and the canonical XML form for fast change detection.
#[derive(Clone, Debug, Serialize)]
pub struct EventSourceRemote {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub events: Vec<Event>,
    /// Canonical (lossy-normalized) form of the server's events XML.
    /// Used as the hash-compare anchor in the unchanged check.
    pub(crate) canonical_xml: String,
}

#[async_trait]
impl onmsctl_core::ApplyTarget for EventSourceTarget {
    type Local = EventSourceLocal;
    type Remote = EventSourceRemote;

    fn name(local: &EventSourceLocal) -> &str {
        &local.metadata.name
    }

    async fn fetch(name: &str, ctx: &Context) -> Result<Option<EventSourceRemote>> {
        fetch_remote(name, ctx).await
    }

    async fn create(local: EventSourceLocal, ctx: &Context) -> Result<()> {
        upload_then_optionally_disable(&local, ctx, false).await
    }

    async fn update(
        local: EventSourceLocal,
        _remote: EventSourceRemote,
        ctx: &Context,
    ) -> Result<()> {
        // Update is identical to create at the wire level: the upload
        // endpoint upserts (deletes existing events, inserts new) for
        // any source whose basename already exists. See design.md §3.1.
        upload_then_optionally_disable(&local, ctx, true).await
    }

    fn diff(local: &EventSourceLocal, remote: &EventSourceRemote) -> Diff {
        diff_source(local, remote)
    }
}

/// Fetch server-side `EventSource` state by name. `Ok(None)` when the source is
/// absent; `Err` when the name resolves ambiguously (refuse to apply). Shared by
/// the legacy `ApplyTarget` driver and the kind-router's `EventSourceHandler`.
pub async fn fetch_remote(name: &str, ctx: &Context) -> Result<Option<EventSourceRemote>> {
    let client = OnmsClient::from_context(ctx)?;
    let api = EventConfApi::new(&client);
    match api.find_source_by_name(name).await? {
        SourceLookup::Absent => Ok(None),
        SourceLookup::Ambiguous(ids) => Err(Error::Config(format!(
            "source name '{name}' resolves ambiguously to ids {ids:?}; refuse to apply"
        ))),
        SourceLookup::Found(src) => {
            let xml_bytes = api.download_source_xml(src.id).await?;
            let events = parse_events_from_xml(&xml_bytes)?;
            let canonical_xml = xml_canonical(&xml_bytes)?;
            Ok(Some(EventSourceRemote {
                id: src.id,
                name: src.name,
                enabled: src.enabled,
                events,
                canonical_xml,
            }))
        }
    }
}

/// Structured diff between a local `EventSource` and its server state. Empty
/// when the events canonicalize equal AND the `enabled` flag matches. Shared by
/// the legacy `ApplyTarget` driver and the kind-router's `EventSourceHandler`.
pub fn diff_source(local: &EventSourceLocal, remote: &EventSourceRemote) -> Diff {
    // Fast path: if the rendered local XML normalizes to the same
    // canonical form as the server's stored XML AND the enabled
    // flag matches, there is no diff. Avoids paying for the
    // structured diff computation when state is already synced.
    if local.spec.enabled == remote.enabled
        && let Some(local_canonical) = render_local_canonical_xml(local)
        && local_canonical == remote.canonical_xml
    {
        return Diff::empty();
    }

    // Slow path: build the structured diff (events bucketed by UEI
    // + the source-level enabled-flag change), render to text, wrap.
    let event_diff = compute_diff(local, remote);
    if event_diff.is_empty() && local.spec.enabled == remote.enabled {
        return Diff::empty();
    }
    let mut s = String::new();
    if local.spec.enabled != remote.enabled {
        s.push_str(&format!(
            "spec.enabled: {} → {}    [will sync after upload]\n",
            remote.enabled, local.spec.enabled
        ));
    }
    if !event_diff.is_empty() {
        s.push_str(&render_diff(&event_diff));
    }
    Diff::from_text(s)
}

/// Render local events to canonical XML for the fast-path compare in
/// `diff()`. Returns `None` if rendering or canonicalization fails — in
/// that case we fall through to the structured-diff slow path rather
/// than reporting a spurious match or surfacing a confusing error.
fn render_local_canonical_xml(local: &EventSourceLocal) -> Option<String> {
    let wire_events: Vec<Event> = local.spec.events.iter().map(Event::from).collect();
    let xml = render_eventconf_xml(&wire_events).ok()?;
    xml_canonical(xml.as_bytes()).ok()
}

/// Render the local YAML to eventconf XML, upload via multipart, and
/// follow up with a status PATCH when `spec.enabled = false` (since the
/// upload endpoint forces `enabled = true` server-side; see design.md
/// §3.2).
///
/// `is_update` is true when called from the update path; it gates the
/// `--verbose` enabled-flap warning, which is only meaningful when an
/// already-existing source is being transiently re-enabled by the
/// upload before we PATCH it back to disabled.
pub async fn upload_then_optionally_disable(
    local: &EventSourceLocal,
    ctx: &Context,
    is_update: bool,
) -> Result<()> {
    let client = OnmsClient::from_context(ctx)?;
    let api = EventConfApi::new(&client);

    // Render and upload.
    //
    // Horizon's server-side filename → source-name derivation uses
    // `String.lastIndexOf('.')` to strip the extension — it ONLY removes
    // the final dot-suffix. Uploading `Cisco.events.xml` results in a
    // stored source name of `Cisco.events`, not `Cisco`. Upload with a
    // plain `.xml` suffix so the stored name equals `metadata.name`
    // exactly. Works for every shape:
    //
    //   metadata.name = "Cisco"     → upload "Cisco.xml"     → stored "Cisco"
    //   metadata.name = "cisco.foo" → upload "cisco.foo.xml" → stored "cisco.foo"
    //
    // (Horizon's `stripPathAndExtension` in EventConfRestService.java.)
    let wire_events: Vec<Event> = local.spec.events.iter().map(Event::from).collect();
    let xml = render_eventconf_xml(&wire_events)?;
    let filename = format!("{}.xml", local.metadata.name);
    let parts = vec![MultipartPart::xml(filename, xml.into_bytes())];
    let _ = api.upload(&parts).await?;

    // If the user wants the source disabled, sync after upload. The
    // server unconditionally sets enabled=true on every upload, so this
    // is the only path to a disabled state.
    if !local.spec.enabled {
        let src_id = match api.find_source_by_name(&local.metadata.name).await {
            Ok(SourceLookup::Found(src)) => src.id,
            Ok(SourceLookup::Absent) => {
                return Err(Error::PostUploadLookupFailed {
                    name: local.metadata.name.clone(),
                    kind: PostUploadLookupKind::Absent,
                });
            }
            Ok(SourceLookup::Ambiguous(_)) => {
                return Err(Error::PostUploadLookupFailed {
                    name: local.metadata.name.clone(),
                    kind: PostUploadLookupKind::Ambiguous,
                });
            }
            Err(_) => {
                return Err(Error::PostUploadLookupFailed {
                    name: local.metadata.name.clone(),
                    kind: PostUploadLookupKind::Transport,
                });
            }
        };
        api.set_sources_enabled(&[src_id], false, false).await?;

        // Only warn after the PATCH succeeds and only on the update
        // path: a fresh create cannot have observers seeing an
        // intermediate enabled=true state, so the warning would just
        // be noise.
        if is_update && ctx.verbose {
            eprintln!(
                "warning: spec.enabled=false on update; the source was \
                 enabled for one round-trip duration before being \
                 disabled. For strict avoidance, use the imperative \
                 path."
            );
        }
    }

    Ok(())
}

/// Build the events-only diff. Pulls the structured representation up to
/// where the renderer can consume it.
fn compute_diff(local: &EventSourceLocal, remote: &EventSourceRemote) -> EventSetDiff {
    diff_event_sets(&local.spec.events, &remote.events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::local::{EventDef, EventSourceSpec, Metadata};

    fn local_with(events: Vec<EventDef>, enabled: bool) -> EventSourceLocal {
        EventSourceLocal {
            api_version: "eventconf.opennms.org/v1".into(),
            kind: "EventSource".into(),
            metadata: Metadata {
                name: "cisco.foo".into(),
            },
            spec: EventSourceSpec { enabled, events },
        }
    }

    fn remote_with(events: Vec<Event>, enabled: bool) -> EventSourceRemote {
        EventSourceRemote {
            id: 42,
            name: "cisco.foo".into(),
            enabled,
            events,
            canonical_xml: String::new(),
        }
    }

    fn ev(uei: &str, severity: &str) -> EventDef {
        EventDef {
            uei: uei.into(),
            label: "L".into(),
            severity: severity.into(),
            ..EventDef::default()
        }
    }

    fn rev(uei: &str, severity: &str) -> Event {
        Event {
            uei: Some(uei.into()),
            event_label: Some("L".into()),
            severity: Some(severity.into()),
            ..Event::default()
        }
    }

    #[test]
    fn diff_returns_empty_when_events_and_enabled_match() {
        let l = local_with(vec![ev("uei.a", "Warning")], true);
        let r = remote_with(vec![rev("uei.a", "Warning")], true);
        let d = <EventSourceTarget as onmsctl_core::ApplyTarget>::diff(&l, &r);
        assert!(d.is_empty());
    }

    #[test]
    fn diff_includes_enabled_change_line_when_flags_differ() {
        let l = local_with(vec![ev("uei.a", "Warning")], false);
        let r = remote_with(vec![rev("uei.a", "Warning")], true);
        let d = <EventSourceTarget as onmsctl_core::ApplyTarget>::diff(&l, &r);
        assert!(!d.is_empty());
        let s = d.as_str();
        assert!(s.contains("spec.enabled"));
        assert!(s.contains("true → false"));
    }

    #[test]
    fn diff_includes_event_changes() {
        let l = local_with(vec![ev("uei.a", "Warning"), ev("uei.b", "Major")], true);
        let r = remote_with(vec![rev("uei.a", "Warning"), rev("uei.c", "Cleared")], true);
        let d = <EventSourceTarget as onmsctl_core::ApplyTarget>::diff(&l, &r);
        let s = d.as_str();
        assert!(s.contains("+ uei.b"));
        assert!(s.contains("- uei.c"));
    }

    #[test]
    fn diff_combines_enabled_and_event_changes() {
        let l = local_with(vec![ev("uei.a", "Major")], false);
        let r = remote_with(vec![rev("uei.a", "Warning")], true);
        let d = <EventSourceTarget as onmsctl_core::ApplyTarget>::diff(&l, &r);
        let s = d.as_str();
        assert!(s.contains("spec.enabled"));
        assert!(s.contains("~ uei.a"));
    }

    #[test]
    fn name_is_metadata_name() {
        let l = local_with(vec![], true);
        assert_eq!(
            <EventSourceTarget as onmsctl_core::ApplyTarget>::name(&l),
            "cisco.foo"
        );
    }
}
