/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed wrapper around Horizon's `/eventconf/*` REST surface.
//!
//! Capabilities consume `EventConfApi<'_>(&OnmsClient)` and never see the
//! HTTP transport directly. Wire-format inconsistencies (`enabled` vs
//! `enable`, `sourceIds` vs `eventsIds`, the unusual
//! `POST /eventconf/sources/eventConfSource` path) are absorbed here; the
//! public method names are uniform.

use onmsctl_core::client::MultipartPart;
use onmsctl_core::{Error, OnmsClient, Result};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::dto::*;

/// Characters that need percent-encoding when interpolated into a URL path
/// segment. Equivalent to RFC 3986's complement of `pchar` minus `/`. We
/// keep alphanumerics, `-._~`, and `:@` (allowed in path segments)
/// unencoded.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'%')
    .add(b'/')
    .add(b'\\')
    .add(b'+')
    .add(b'&')
    .add(b'=');

/// Default page size applied to filter endpoints when the caller passes no
/// explicit `limit`. Horizon's eventconf filter endpoints reject the
/// request with `400 "Invalid offset/limit values"` (and in one case 500
/// "offset is null") when `limit` is missing — `offset` is server-defaulted
/// on `/filter/sources` and `/filter`, but is required on
/// `/filter/{id}/events`. Real eventconf installs have far fewer than 1000
/// sources or events-per-source, so a single page of 1000 is effectively
/// "show everything" without paginating.
const DEFAULT_PAGE_LIMIT: i32 = 1000;

/// Hard cap on the page size used by `find_source_by_name` so a degenerate
/// duplicate-name pair is detected even if the server's default limit is
/// small. Real eventconf installs have far fewer than 1000 sources.
const FIND_SOURCE_LIMIT: i32 = DEFAULT_PAGE_LIMIT;

/// Base path for all eventconf endpoints. Joined onto the configured
/// `OnmsClient` base URL.
const BASE: &str = "api/v2/eventconf";

/// Typed wrapper around the EventConf REST surface.
pub struct EventConfApi<'c> {
    client: &'c OnmsClient,
}

impl<'c> EventConfApi<'c> {
    pub fn new(client: &'c OnmsClient) -> Self {
        Self { client }
    }
}

// -- Filter parameter shapes ------------------------------------------------

/// Query parameters for `GET /eventconf/filter/sources`.
#[derive(Clone, Debug, Default)]
pub struct SourceFilter {
    pub filter: Option<String>,
    pub sort_by: Option<String>,
    pub order: Option<String>,
    pub offset: Option<i32>,
    pub limit: Option<i32>,
}

impl SourceFilter {
    pub fn name_eq(name: impl Into<String>) -> Self {
        Self {
            filter: Some(name.into()),
            ..Self::default()
        }
    }
}

/// Query parameters for `GET /eventconf/filter/{sourceId}/events`.
#[derive(Clone, Debug, Default)]
pub struct EventInSourceFilter {
    pub event_filter: Option<String>,
    pub event_sort_by: Option<String>,
    pub event_order: Option<String>,
    pub offset: Option<i32>,
    pub limit: Option<i32>,
}

/// Query parameters for `GET /eventconf/filter`.
#[derive(Clone, Debug, Default)]
pub struct EventFilter {
    pub uei: Option<String>,
    pub vendor: Option<String>,
    pub source_name: Option<String>,
    pub offset: Option<i32>,
    pub limit: Option<i32>,
}

// -- Misc response types ----------------------------------------------------

/// Result of `POST /eventconf/sources/eventConfSource`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreatedSource {
    pub id: i64,
    pub name: String,
    pub file_order: i32,
}

/// Result of `POST /eventconf/sources/{sourceId}/events`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreatedEvent {
    pub id: i64,
}

/// Aggregated result of `POST /eventconf/upload`. Per the upload handler
/// (design.md §3.1), the server returns one entry per attached file in
/// either `success` or `errors`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct UploadResult {
    pub success: Vec<UploadFileResult>,
    pub errors: Vec<UploadFileError>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadFileResult {
    pub file: String,
    pub event_count: i32,
    #[serde(default)]
    pub vendor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UploadFileError {
    pub file: String,
    pub error: String,
}

/// Outcome of `find_source_by_name`. The filter endpoint can return zero,
/// one, or many matches; ambiguity is its own variant so apply-style flows
/// can refuse to mutate (per design.md §7 "find_source_by_name race").
#[derive(Clone, Debug)]
pub enum SourceLookup {
    Absent,
    Found(EventConfSourceDto),
    Ambiguous(Vec<i64>),
}

// -- Source endpoints --------------------------------------------------------

impl EventConfApi<'_> {
    /// `GET /eventconf/sources/{id}`.
    pub async fn get_source(&self, source_id: i64) -> Result<EventConfSourceDto> {
        let path = format!("{BASE}/sources/{source_id}");
        self.client.get(&path, &[]).await
    }

    /// `POST /eventconf/sources/eventConfSource` — note the unusual path.
    pub async fn create_source(&self, req: &AddEventConfSourceRequest) -> Result<CreatedSource> {
        let path = format!("{BASE}/sources/eventConfSource");
        self.client.post(&path, req).await
    }

    /// `DELETE /eventconf/sources` with body `{ sourceIds: [...] }`.
    /// Empty slice is refused client-side so the server cannot interpret
    /// `{sourceIds: []}` as "delete all".
    pub async fn delete_sources(&self, source_ids: &[i64]) -> Result<()> {
        ensure_non_empty(source_ids, "delete_sources", "source ids")?;
        let path = format!("{BASE}/sources");
        let payload = EventConfSourceDeletePayload {
            source_ids: source_ids.to_vec(),
        };
        self.client.delete(&path, Some(&payload)).await
    }

    /// `PATCH /eventconf/sources/status`. Empty slice refused client-side.
    pub async fn set_sources_enabled(
        &self,
        source_ids: &[i64],
        enabled: bool,
        cascade_to_events: bool,
    ) -> Result<()> {
        ensure_non_empty(source_ids, "set_sources_enabled", "source ids")?;
        let path = format!("{BASE}/sources/status");
        let payload = EventConfSrcEnableDisablePayload {
            enabled,
            cascade_to_events,
            source_ids: source_ids.to_vec(),
        };
        let _: serde_json::Value = self.client.patch(&path, &payload).await?;
        Ok(())
    }

    /// `GET /eventconf/sources/names`.
    pub async fn list_source_names(&self) -> Result<Vec<String>> {
        let path = format!("{BASE}/sources/names");
        self.client.get(&path, &[]).await
    }

    /// `GET /eventconf/sources/names-and-ids`.
    pub async fn list_source_names_and_ids(&self) -> Result<Vec<SourceNameAndId>> {
        let path = format!("{BASE}/sources/names-and-ids");
        self.client.get(&path, &[]).await
    }

    /// `GET /eventconf/sources/{id}/events/download` — raw eventconf XML.
    pub async fn download_source_xml(&self, source_id: i64) -> Result<Vec<u8>> {
        let path = format!("{BASE}/sources/{source_id}/events/download");
        self.client.get_bytes(&path).await
    }

    /// `GET /eventconf/filter/sources` with `SourceFilter` parameters.
    ///
    /// Horizon requires `limit` on this endpoint; `offset` is optional and
    /// server-defaulted. When the caller omits `limit` we apply
    /// [`DEFAULT_PAGE_LIMIT`] so the request does not 400.
    pub async fn filter_sources(&self, filter: &SourceFilter) -> Result<Page<EventConfSourceDto>> {
        validate_paging(filter.offset, filter.limit, "filter_sources")?;
        let path = format!("{BASE}/filter/sources");
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(f) = &filter.filter {
            q.push(("filter", f.clone()));
        }
        if let Some(s) = &filter.sort_by {
            q.push(("sortBy", s.clone()));
        }
        if let Some(o) = &filter.order {
            q.push(("order", o.clone()));
        }
        if let Some(off) = filter.offset {
            q.push(("offset", off.to_string()));
        }
        let limit = filter.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
        q.push(("limit", limit.to_string()));
        // 204 No Content is normal when the filter matches nothing — turn
        // that into an empty page so callers always see a typed response.
        let result: Result<Page<EventConfSourceDto>> =
            self.client.get(&path, &borrow_pairs(&q)).await;
        match result {
            Ok(p) => Ok(p),
            Err(Error::HttpStatus { status: 204, .. }) => Ok(Page {
                total_records: 0,
                items: Vec::new(),
            }),
            Err(e) => Err(e),
        }
    }

    /// `POST /eventconf/upload` — multipart file upload.
    pub async fn upload(&self, parts: &[MultipartPart]) -> Result<UploadResult> {
        let path = format!("{BASE}/upload");
        self.client.multipart(&path, parts).await
    }

    /// Look up a source by its exact name. Wraps `filter_sources` and
    /// post-filters (the `filter` query parameter is substring-matching;
    /// we require an exact name match).
    ///
    /// Uses an explicit page limit of [`FIND_SOURCE_LIMIT`] so a degenerate
    /// duplicate-name pair on a hypothetical page 2 cannot be silently
    /// missed. If `total_records` exceeds the limit the lookup errors
    /// rather than risk reporting `Found` for a name that may also live
    /// on an unread page.
    pub async fn find_source_by_name(&self, name: &str) -> Result<SourceLookup> {
        let mut filter = SourceFilter::name_eq(name);
        filter.limit = Some(FIND_SOURCE_LIMIT);
        let page = self.filter_sources(&filter).await?;
        if page.total_records > FIND_SOURCE_LIMIT as i64 {
            return Err(Error::Config(format!(
                "find_source_by_name('{name}') returned {} substring matches; \
                 cannot guarantee exact-name disambiguation. Use a more specific name.",
                page.total_records
            )));
        }
        let exact: Vec<EventConfSourceDto> =
            page.items.into_iter().filter(|s| s.name == name).collect();
        match exact.len() {
            0 => Ok(SourceLookup::Absent),
            1 => Ok(SourceLookup::Found(exact.into_iter().next().unwrap())),
            _ => Ok(SourceLookup::Ambiguous(
                exact.iter().map(|s| s.id).collect(),
            )),
        }
    }
}

// -- Event endpoints ---------------------------------------------------------

impl EventConfApi<'_> {
    /// `GET /eventconf/filter/{sourceId}/events`.
    ///
    /// Horizon requires BOTH `offset` and `limit` on this endpoint —
    /// omitting `offset` triggers a 500 NPE (`offset is null`), not a 400.
    /// When the caller omits either, we apply `offset=0` /
    /// [`DEFAULT_PAGE_LIMIT`] so the request does not crash the server.
    pub async fn list_events_in_source(
        &self,
        source_id: i64,
        filter: &EventInSourceFilter,
    ) -> Result<Page<EventConfEventDto>> {
        validate_paging(filter.offset, filter.limit, "list_events_in_source")?;
        let path = format!("{BASE}/filter/{source_id}/events");
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(f) = &filter.event_filter {
            q.push(("eventFilter", f.clone()));
        }
        if let Some(s) = &filter.event_sort_by {
            q.push(("eventSortBy", s.clone()));
        }
        if let Some(o) = &filter.event_order {
            q.push(("eventOrder", o.clone()));
        }
        let offset = filter.offset.unwrap_or(0);
        q.push(("offset", offset.to_string()));
        let limit = filter.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
        q.push(("limit", limit.to_string()));
        let result: Result<Page<EventConfEventDto>> =
            self.client.get(&path, &borrow_pairs(&q)).await;
        match result {
            Ok(p) => Ok(p),
            Err(Error::HttpStatus { status: 204, .. }) => Ok(Page {
                total_records: 0,
                items: Vec::new(),
            }),
            Err(e) => Err(e),
        }
    }

    /// `GET /eventconf/filter` — cross-source event filter.
    ///
    /// Horizon requires `limit` on this endpoint; `offset` is optional and
    /// server-defaulted. Apply [`DEFAULT_PAGE_LIMIT`] when caller omits it.
    pub async fn filter_events(&self, filter: &EventFilter) -> Result<Vec<EventConfEventDto>> {
        validate_paging(filter.offset, filter.limit, "filter_events")?;
        let path = format!("{BASE}/filter");
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(u) = &filter.uei {
            q.push(("uei", u.clone()));
        }
        if let Some(v) = &filter.vendor {
            q.push(("vendor", v.clone()));
        }
        if let Some(n) = &filter.source_name {
            q.push(("sourceName", n.clone()));
        }
        if let Some(off) = filter.offset {
            q.push(("offset", off.to_string()));
        }
        let limit = filter.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
        q.push(("limit", limit.to_string()));
        let result: Result<Vec<EventConfEventDto>> =
            self.client.get(&path, &borrow_pairs(&q)).await;
        match result {
            Ok(items) => Ok(items),
            Err(Error::HttpStatus { status: 204, .. }) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// `GET /eventconf/vendors/{vendorName}/events`. The vendor name is
    /// percent-encoded so names with `&`, ` `, `'`, `/` etc. don't break
    /// the URL routing.
    pub async fn get_events_by_vendor(&self, vendor: &str) -> Result<Vec<EventConfEventDto>> {
        let encoded = utf8_percent_encode(vendor, PATH_SEGMENT).to_string();
        let path = format!("{BASE}/vendors/{encoded}/events");
        self.client.get(&path, &[]).await
    }

    /// `POST /eventconf/sources/{sourceId}/events`.
    pub async fn add_event(&self, source_id: i64, event: &Event) -> Result<i64> {
        let path = format!("{BASE}/sources/{source_id}/events");
        let created: CreatedEvent = self.client.post(&path, event).await?;
        Ok(created.id)
    }

    /// `PUT /eventconf/sources/{sourceId}/events/{eventId}`.
    pub async fn update_event(
        &self,
        source_id: i64,
        event_id: i64,
        req: &EventConfEventEditRequest,
    ) -> Result<()> {
        let path = format!("{BASE}/sources/{source_id}/events/{event_id}");
        let _: serde_json::Value = self.client.put(&path, req).await?;
        Ok(())
    }

    /// `DELETE /eventconf/sources/{sourceId}/events` with `{eventIds: [...]}`.
    /// Empty slice is refused client-side.
    pub async fn delete_events(&self, source_id: i64, event_ids: &[i64]) -> Result<()> {
        ensure_non_empty(event_ids, "delete_events", "event ids")?;
        let path = format!("{BASE}/sources/{source_id}/events");
        let payload = EventConfEventDeletePayload {
            event_ids: event_ids.to_vec(),
        };
        self.client.delete(&path, Some(&payload)).await
    }

    /// `PATCH /eventconf/sources/{sourceId}/events/status`. Empty slice
    /// is refused client-side.
    pub async fn set_events_enabled(
        &self,
        source_id: i64,
        event_ids: &[i64],
        enable: bool,
    ) -> Result<()> {
        ensure_non_empty(event_ids, "set_events_enabled", "event ids")?;
        let path = format!("{BASE}/sources/{source_id}/events/status");
        let payload = EnableDisableConfSourceEventsPayload {
            enable,
            events_ids: event_ids.to_vec(),
        };
        let _: serde_json::Value = self.client.patch(&path, &payload).await?;
        Ok(())
    }
}

/// Adapt a `Vec<(&str, String)>` to the `&[(&str, &str)]` shape the
/// underlying `OnmsClient::get` expects.
fn borrow_pairs<'a>(pairs: &'a [(&'a str, String)]) -> Vec<(&'a str, &'a str)> {
    pairs.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

/// Reject an empty id slice client-side. Some Horizon mutation endpoints
/// have undefined semantics for empty payloads (potentially "delete all");
/// we refuse to issue the request rather than gamble on server behavior.
fn ensure_non_empty(ids: &[i64], method: &str, kind: &str) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::Config(format!(
            "{method}: refusing to call with empty {kind} slice; pass at least one id"
        )));
    }
    Ok(())
}

/// Validate that paging parameters, when supplied, are non-negative.
/// Negative values would either be rejected server-side with an opaque 400
/// or, worse, silently produce undefined paging behaviour.
fn validate_paging(offset: Option<i32>, limit: Option<i32>, method: &str) -> Result<()> {
    if let Some(off) = offset
        && off < 0
    {
        return Err(Error::Config(format!(
            "{method}: offset must be non-negative, got {off}"
        )));
    }
    if let Some(lim) = limit
        && lim < 0
    {
        return Err(Error::Config(format!(
            "{method}: limit must be non-negative, got {lim}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::{AuthCreds, Url};
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Standard fixture: spawn a wiremock server, build an `OnmsClient`
    /// pointing at it. Tests construct `EventConfApi::new(&client)`.
    async fn mock_with_client() -> (MockServer, OnmsClient) {
        let mock = MockServer::start().await;
        let url = format!("{}/", mock.uri());
        let client =
            OnmsClient::from_parts(Url::parse(&url).unwrap(), AuthCreds::bearer("t")).unwrap();
        (mock, client)
    }

    #[tokio::test]
    async fn get_source_round_trips_dto() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/sources/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 42,
                "name": "cisco.foo",
                "vendor": "cisco",
                "fileOrder": 50,
                "eventCount": 17,
                "enabled": true
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let s = api.get_source(42).await.unwrap();
        assert_eq!(s.id, 42);
        assert_eq!(s.name, "cisco.foo");
        assert_eq!(s.file_order, 50);
        assert_eq!(s.event_count, 17);
    }

    #[tokio::test]
    async fn create_source_uses_unusual_path() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/eventconf/sources/eventConfSource"))
            .and(body_json(serde_json::json!({"name": "cisco.foo"})))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 42, "name": "cisco.foo", "fileOrder": 50
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let req = AddEventConfSourceRequest {
            name: "cisco.foo".into(),
            description: None,
            vendor: None,
        };
        let created = api.create_source(&req).await.unwrap();
        assert_eq!(created.id, 42);
        assert_eq!(created.file_order, 50);
    }

    #[tokio::test]
    async fn delete_sources_sends_payload() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("DELETE"))
            .and(path("/api/v2/eventconf/sources"))
            .and(body_json(serde_json::json!({"sourceIds": [42, 43]})))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        api.delete_sources(&[42, 43]).await.unwrap();
    }

    #[tokio::test]
    async fn set_sources_enabled_uses_patch_and_camelcase_payload() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("PATCH"))
            .and(path("/api/v2/eventconf/sources/status"))
            .and(body_json(serde_json::json!({
                "enabled": true,
                "cascadeToEvents": true,
                "sourceIds": [42, 43]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        api.set_sources_enabled(&[42, 43], true, true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn filter_sources_passes_query_params() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .and(query_param("filter", "cisco"))
            .and(query_param("limit", "10"))
            .and(query_param("offset", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 0,
                "items": []
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let p = api
            .filter_sources(&SourceFilter {
                filter: Some("cisco".into()),
                offset: Some(0),
                limit: Some(10),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(p.total_records, 0);
    }

    #[tokio::test]
    async fn filter_sources_204_yields_empty_page() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let p = api.filter_sources(&SourceFilter::default()).await.unwrap();
        assert_eq!(p.total_records, 0);
        assert!(p.items.is_empty());
    }

    #[tokio::test]
    async fn find_source_by_name_returns_found_for_exact_match() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .and(query_param("filter", "cisco.foo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 1,
                "items": [
                    {"id": 42, "name": "cisco.foo", "fileOrder": 50,
                     "eventCount": 17, "enabled": true}
                ]
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        match api.find_source_by_name("cisco.foo").await.unwrap() {
            SourceLookup::Found(s) => assert_eq!(s.id, 42),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn find_source_by_name_filters_substring_matches_to_exact() {
        // The filter endpoint substring-matches; we filter to exact name.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 2,
                "items": [
                    {"id": 42, "name": "cisco.foo", "fileOrder": 50,
                     "eventCount": 17, "enabled": true},
                    {"id": 99, "name": "cisco.foobar", "fileOrder": 60,
                     "eventCount": 0, "enabled": true}
                ]
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        match api.find_source_by_name("cisco.foo").await.unwrap() {
            SourceLookup::Found(s) => assert_eq!(s.id, 42),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn find_source_by_name_returns_absent_when_no_match() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 0,
                "items": []
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        match api.find_source_by_name("nope").await.unwrap() {
            SourceLookup::Absent => {}
            other => panic!("expected Absent, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn find_source_by_name_returns_ambiguous_for_duplicate_names() {
        // Degenerate state — two sources sharing a name. apply must refuse.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 2,
                "items": [
                    {"id": 42, "name": "dup", "fileOrder": 50,
                     "eventCount": 0, "enabled": true},
                    {"id": 43, "name": "dup", "fileOrder": 60,
                     "eventCount": 0, "enabled": true}
                ]
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        match api.find_source_by_name("dup").await.unwrap() {
            SourceLookup::Ambiguous(ids) => {
                assert_eq!(ids.len(), 2);
                assert!(ids.contains(&42));
                assert!(ids.contains(&43));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn add_event_returns_id() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/eventconf/sources/42/events"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": 108})))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let e = Event {
            uei: Some("uei.opennms.org/test".into()),
            severity: Some("Warning".into()),
            ..Event::default()
        };
        let id = api.add_event(42, &e).await.unwrap();
        assert_eq!(id, 108);
    }

    #[tokio::test]
    async fn update_event_uses_put() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/eventconf/sources/42/events/108"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let req = EventConfEventEditRequest {
            enabled: true,
            event: Event::default(),
        };
        api.update_event(42, 108, &req).await.unwrap();
    }

    #[tokio::test]
    async fn delete_events_sends_eventids_payload() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("DELETE"))
            .and(path("/api/v2/eventconf/sources/42/events"))
            .and(body_json(serde_json::json!({"eventIds": [108, 109]})))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        api.delete_events(42, &[108, 109]).await.unwrap();
    }

    #[tokio::test]
    async fn set_events_enabled_sends_typo_field_names() {
        let (mock, client) = mock_with_client().await;
        // Wire format uses `enable` and `eventsIds` (with the trailing s).
        Mock::given(method("PATCH"))
            .and(path("/api/v2/eventconf/sources/42/events/status"))
            .and(body_json(serde_json::json!({
                "enable": true,
                "eventsIds": [108, 109]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        api.set_events_enabled(42, &[108, 109], true).await.unwrap();
    }

    #[tokio::test]
    async fn list_source_names_returns_string_array() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/sources/names"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!(["cisco.foo", "juniper.bar"])),
            )
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let names = api.list_source_names().await.unwrap();
        assert_eq!(
            names,
            vec!["cisco.foo".to_string(), "juniper.bar".to_string()]
        );
    }

    #[tokio::test]
    async fn list_source_names_and_ids_returns_typed_pairs() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/sources/names-and-ids"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": 42, "name": "a"},
                {"id": 43, "name": "b"}
            ])))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let pairs = api.list_source_names_and_ids().await.unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].id, 42);
    }

    #[tokio::test]
    async fn download_source_xml_returns_raw_bytes() {
        let (mock, client) = mock_with_client().await;
        let xml = b"<events><event><uei>uei.test</uei></event></events>";
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/sources/42/events/download"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(xml.as_slice()))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let got = api.download_source_xml(42).await.unwrap();
        assert_eq!(got, xml);
    }

    #[tokio::test]
    async fn upload_returns_success_and_errors_buckets() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/eventconf/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": [{"file": "foo", "eventCount": 17, "vendor": "cisco"}],
                "errors": [{"file": "bad", "error": "ParseException: bad XML"}]
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let parts = vec![MultipartPart::xml("foo.events.xml", b"<events/>".to_vec())];
        let r = api.upload(&parts).await.unwrap();
        assert_eq!(r.success.len(), 1);
        assert_eq!(r.success[0].event_count, 17);
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].file, "bad");
    }

    #[tokio::test]
    async fn filter_events_passes_uei_query() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter"))
            .and(query_param("uei", "restart"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let _ = api
            .filter_events(&EventFilter {
                uei: Some("restart".into()),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn get_events_by_vendor_passes_path_segment() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/vendors/cisco/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let _ = api.get_events_by_vendor("cisco").await.unwrap();
    }

    #[tokio::test]
    async fn delete_sources_rejects_empty_slice() {
        let (_mock, client) = mock_with_client().await;
        let api = EventConfApi::new(&client);
        let err = api.delete_sources(&[]).await.unwrap_err();
        match err {
            Error::Config(m) => {
                assert!(m.contains("delete_sources"));
                assert!(m.contains("empty"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_events_rejects_empty_slice() {
        let (_mock, client) = mock_with_client().await;
        let api = EventConfApi::new(&client);
        let err = api.delete_events(42, &[]).await.unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("empty")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_sources_enabled_rejects_empty_slice() {
        let (_mock, client) = mock_with_client().await;
        let api = EventConfApi::new(&client);
        let err = api.set_sources_enabled(&[], true, false).await.unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("empty")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn negative_offset_is_rejected_client_side() {
        let (_mock, client) = mock_with_client().await;
        let api = EventConfApi::new(&client);
        let err = api
            .filter_sources(&SourceFilter {
                offset: Some(-1),
                ..Default::default()
            })
            .await
            .unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("offset must be non-negative")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn negative_limit_is_rejected_client_side() {
        let (_mock, client) = mock_with_client().await;
        let api = EventConfApi::new(&client);
        let err = api
            .filter_events(&EventFilter {
                limit: Some(-5),
                ..Default::default()
            })
            .await
            .unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("limit must be non-negative")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn vendor_name_is_percent_encoded_in_path() {
        // A vendor name containing `&`, ` `, and `/` would otherwise break
        // URL routing (the `/` would split into a new path segment).
        // Encoded form: Schneider%20Electric%20%26%20Co%2FBrands
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v2/eventconf/vendors/Schneider%20Electric%20%26%20Co%2FBrands/events",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let _ = api
            .get_events_by_vendor("Schneider Electric & Co/Brands")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn find_source_by_name_errors_when_total_exceeds_page_limit() {
        // Server reports more substring matches than fit in a single
        // page — the lookup cannot guarantee exact-name disambiguation.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 1500,
                "items": []
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let err = api.find_source_by_name("common-prefix").await.unwrap_err();
        match err {
            Error::Config(m) => assert!(m.contains("more specific")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_events_in_source_passes_event_filter_query() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/42/events"))
            .and(query_param("eventFilter", "restart"))
            .and(query_param("eventSortBy", "uei"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 0,
                "items": []
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let _ = api
            .list_events_in_source(
                42,
                &EventInSourceFilter {
                    event_filter: Some("restart".into()),
                    event_sort_by: Some("uei".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    // -- contract regression: Horizon wrapper-field-name + required paging --

    #[tokio::test]
    async fn filter_sources_accepts_eventconfsourcelist_wrapper() {
        // Horizon emits items under `eventConfSourceList`, not `items`.
        // The serde alias on `Page<T>.items` is what makes this work; without
        // it, deserialization silently returned an empty page even when
        // records existed.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 1,
                "eventConfSourceList": [
                    {"id": 7, "name": "horizon-shape", "fileOrder": 5,
                     "eventCount": 0, "enabled": true}
                ]
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let p = api.filter_sources(&SourceFilter::default()).await.unwrap();
        assert_eq!(p.total_records, 1);
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].name, "horizon-shape");
    }

    #[tokio::test]
    async fn list_events_in_source_accepts_eventconfsourcelist_wrapper() {
        // The per-source endpoint reuses the same wrapper field name as
        // /filter/sources, despite carrying event payloads — a Horizon
        // controller copy-paste quirk we must absorb.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/42/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 1,
                "eventConfSourceList": [
                    {"id": 108, "sourceId": 42, "uei": "uei.opennms.org/x",
                     "eventLabel": "x", "severity": "Warning", "enabled": true}
                ]
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let p = api
            .list_events_in_source(42, &EventInSourceFilter::default())
            .await
            .unwrap();
        assert_eq!(p.total_records, 1);
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].uei, "uei.opennms.org/x");
    }

    #[tokio::test]
    async fn filter_sources_defaults_limit_when_caller_omits_it() {
        // Server requires `limit` on /filter/sources; without it the
        // request 400s. Caller passes no limit — we apply DEFAULT_PAGE_LIMIT.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/sources"))
            .and(query_param("limit", "1000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 0,
                "eventConfSourceList": []
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        api.filter_sources(&SourceFilter::default()).await.unwrap();
    }

    #[tokio::test]
    async fn list_events_in_source_defaults_offset_and_limit() {
        // Server NPEs (500) on this endpoint when `offset` is missing,
        // even if `limit` is present. We default both.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/42/events"))
            .and(query_param("offset", "0"))
            .and(query_param("limit", "1000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 0,
                "eventConfSourceList": []
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        api.list_events_in_source(42, &EventInSourceFilter::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn get_source_decodes_real_horizon_shape() {
        // Captured from a live Horizon /eventconf/sources/{id} response:
        // `createdTime` and `lastModified` are epoch-millis integers, not
        // ISO strings. `uploadedBy` is a plain string.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/sources/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "name": "opennms.snmp.trap.translator.events",
                "description": "",
                "vendor": "opennms",
                "fileOrder": 23,
                "enabled": true,
                "eventCount": 8,
                "createdTime": 1778799087519_i64,
                "lastModified": 1778799087519_i64,
                "uploadedBy": "system-migration"
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let s = api
            .get_source(1)
            .await
            .expect("must decode the real Horizon source shape");
        assert_eq!(s.id, 1);
        assert_eq!(s.file_order, 23);
        assert_eq!(s.created_time, Some(1778799087519));
        assert_eq!(s.last_modified, Some(1778799087519));
        assert_eq!(s.uploaded_by.as_deref(), Some("system-migration"));
    }

    #[tokio::test]
    async fn list_events_in_source_decodes_real_horizon_shape() {
        // Captured from a live Horizon /eventconf/filter/{id}/events
        // response: `sourceId` is absent (implicit in path), `createdTime`
        // and `lastModified` are epoch-millis integers, and unmodeled
        // fields like `xmlContent` / `modifiedBy` / `fileOrder` are
        // present alongside.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter/23/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "totalRecords": 1,
                "eventConfSourceList": [
                    {
                        "id": 154,
                        "uei": "MATCH-ANY-UEI",
                        "eventLabel": "OpenNMS-defined event: MATCH-ANY-UEI",
                        "description": "<p>x</p>",
                        "enabled": true,
                        "xmlContent": "<event/>",
                        "createdTime": 1778799087558_i64,
                        "lastModified": 1778799087558_i64,
                        "modifiedBy": "system-migration",
                        "sourceName": "opennms.catch-all.events",
                        "vendor": "opennms",
                        "fileOrder": 1,
                        "severity": "Indeterminate"
                    }
                ]
            })))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        let p = api
            .list_events_in_source(23, &EventInSourceFilter::default())
            .await
            .expect("must decode the real Horizon response shape");
        assert_eq!(p.total_records, 1);
        assert_eq!(p.items.len(), 1);
        let e = &p.items[0];
        assert_eq!(e.id, 154);
        assert!(e.source_id.is_none());
        assert_eq!(e.severity, "Indeterminate");
        assert_eq!(e.created_time, Some(1778799087558));
        assert_eq!(e.last_modified, Some(1778799087558));
    }

    #[tokio::test]
    async fn filter_events_defaults_limit_when_caller_omits_it() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/eventconf/filter"))
            .and(query_param("limit", "1000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&mock)
            .await;
        let api = EventConfApi::new(&client);
        api.filter_events(&EventFilter::default()).await.unwrap();
    }
}
