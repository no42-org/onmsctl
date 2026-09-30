/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed REST client for `/api/v2/thresholding` and `/api/v2/threshd`.
//!
//! Reads that can miss return `Ok(None)` on 404. Writes take the ETag read
//! earlier as `If-Match`, so a concurrent change answers 412 instead of being
//! overwritten.

use onmsctl_core::{Error, OnmsClient, Result};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use crate::dto::{
    ThreshdPackageDto, ThreshdPackageSummaryDto, ThresholdGroupDto, ThresholdGroupSummaryDto,
    ThresholdingMetadataDto,
};

const THRESHOLDING: &str = "api/v2/thresholding";
const THRESHD: &str = "api/v2/threshd";

/// The version-gate message for a server without the threshold API.
pub const THRESHOLDING_UNSUPPORTED: &str = "this OpenNMS server does not expose the threshold \
     REST API (/api/v2/thresholding, /api/v2/threshd); ThresholdGroup and ThreshdPackage \
     require Horizon 37.0.0 or later (NMS-19837)";

/// Characters percent-encoded in a path segment (same set as `onmsctl-eventconf`).
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

fn seg(name: &str) -> String {
    utf8_percent_encode(name, PATH_SEGMENT).to_string()
}

/// `Ok(None)` for a 404, the value otherwise.
fn found<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(Error::HttpStatus { status: 404, .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The threshold-group endpoints.
pub struct ThresholdingApi<'c> {
    client: &'c OnmsClient,
}

impl<'c> ThresholdingApi<'c> {
    pub fn new(client: &'c OnmsClient) -> Self {
        Self { client }
    }

    /// `GET /thresholding/metadata`. Doubles as the version gate: a 404
    /// becomes [`THRESHOLDING_UNSUPPORTED`].
    pub async fn metadata(&self) -> Result<ThresholdingMetadataDto> {
        found(
            self.client
                .get(&format!("{THRESHOLDING}/metadata"), &[])
                .await,
        )?
        .ok_or_else(|| Error::Config(THRESHOLDING_UNSUPPORTED.into()))
    }

    /// `GET /thresholding/groups`: every group, extension groups included.
    pub async fn list_groups(&self) -> Result<Vec<ThresholdGroupSummaryDto>> {
        self.client
            .get(&format!("{THRESHOLDING}/groups"), &[])
            .await
    }

    /// `GET /thresholding/groups/{name}` with its ETag, or `None`.
    pub async fn get_group(
        &self,
        name: &str,
    ) -> Result<Option<(ThresholdGroupDto, Option<String>)>> {
        found(
            self.client
                .get_with_etag(&format!("{THRESHOLDING}/groups/{}", seg(name)))
                .await,
        )
    }

    /// `POST /thresholding/groups`.
    pub async fn create_group(&self, dto: &ThresholdGroupDto) -> Result<()> {
        self.client
            .post_drain(&format!("{THRESHOLDING}/groups"), dto)
            .await
    }

    /// `PUT /thresholding/groups/{name}` with `If-Match`.
    pub async fn update_group(
        &self,
        name: &str,
        dto: &ThresholdGroupDto,
        etag: Option<&str>,
    ) -> Result<()> {
        self.client
            .put_drain_if_match(&format!("{THRESHOLDING}/groups/{}", seg(name)), dto, etag)
            .await
    }

    /// `DELETE /thresholding/groups/{name}` with `If-Match`.
    pub async fn delete_group(&self, name: &str, etag: Option<&str>) -> Result<()> {
        self.client
            .delete_if_match(&format!("{THRESHOLDING}/groups/{}", seg(name)), etag)
            .await
    }
}

/// The threshd package endpoints.
pub struct ThreshdApi<'c> {
    client: &'c OnmsClient,
}

impl<'c> ThreshdApi<'c> {
    pub fn new(client: &'c OnmsClient) -> Self {
        Self { client }
    }

    /// `GET /threshd/packages`.
    pub async fn list_packages(&self) -> Result<Vec<ThreshdPackageSummaryDto>> {
        self.client.get(&format!("{THRESHD}/packages"), &[]).await
    }

    /// `GET /threshd/packages/{name}` with its ETag, or `None`.
    pub async fn get_package(
        &self,
        name: &str,
    ) -> Result<Option<(ThreshdPackageDto, Option<String>)>> {
        found(
            self.client
                .get_with_etag(&format!("{THRESHD}/packages/{}", seg(name)))
                .await,
        )
    }

    /// `POST /threshd/packages`.
    pub async fn create_package(&self, dto: &ThreshdPackageDto) -> Result<()> {
        self.client
            .post_drain(&format!("{THRESHD}/packages"), dto)
            .await
    }

    /// `PUT /threshd/packages/{name}` with `If-Match`.
    pub async fn update_package(
        &self,
        name: &str,
        dto: &ThreshdPackageDto,
        etag: Option<&str>,
    ) -> Result<()> {
        self.client
            .put_drain_if_match(&format!("{THRESHD}/packages/{}", seg(name)), dto, etag)
            .await
    }

    /// `DELETE /threshd/packages/{name}` with `If-Match`.
    pub async fn delete_package(&self, name: &str, etag: Option<&str>) -> Result<()> {
        self.client
            .delete_if_match(&format!("{THRESHD}/packages/{}", seg(name)), etag)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onmsctl_core::{AuthCreds, Url};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn server() -> (MockServer, OnmsClient) {
        let mock = MockServer::start().await;
        let client = OnmsClient::from_parts(
            Url::parse(&format!("{}/", mock.uri())).unwrap(),
            AuthCreds::bearer("t"),
        )
        .unwrap();
        (mock, client)
    }

    fn group() -> ThresholdGroupDto {
        ThresholdGroupDto {
            name: "a b".into(),
            rrd_repository: "/r/".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn metadata_404_is_the_version_gate() {
        let (mock, client) = server().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/thresholding/metadata"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let err = ThresholdingApi::new(&client).metadata().await.unwrap_err();
        assert!(err.to_string().contains("Horizon 37.0.0"), "{err}");
    }

    #[tokio::test]
    async fn get_group_encodes_the_name_and_returns_etag_or_none() {
        let (mock, client) = server().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/thresholding/groups/a%20b"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"g1\"")
                    .set_body_json(serde_json::json!({"name": "a b", "rrdRepository": "/r/"})),
            )
            .mount(&mock)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/thresholding/groups/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = ThresholdingApi::new(&client);
        let (dto, etag) = api.get_group("a b").await.unwrap().unwrap();
        assert_eq!(
            (dto.name.as_str(), etag.as_deref()),
            ("a b", Some("\"g1\""))
        );
        assert!(api.get_group("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn writes_hit_the_right_paths_with_if_match() {
        let (mock, client) = server().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/thresholding/groups"))
            .and(body_json(serde_json::to_value(group()).unwrap()))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&mock)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/thresholding/groups/a%20b"))
            .and(header("If-Match", "\"g1\""))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&mock)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/v2/threshd/packages/p"))
            .and(header("If-Match", "\"p1\""))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&mock)
            .await;
        let api = ThresholdingApi::new(&client);
        api.create_group(&group()).await.unwrap();
        api.update_group("a b", &group(), Some("\"g1\""))
            .await
            .unwrap();
        ThreshdApi::new(&client)
            .delete_package("p", Some("\"p1\""))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn conflicts_surface_their_status() {
        let (mock, client) = server().await;
        Mock::given(method("PUT"))
            .and(path("/api/v2/threshd/packages/p"))
            .respond_with(ResponseTemplate::new(412).set_body_string("changed since it was read"))
            .mount(&mock)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v2/threshd/packages"))
            .respond_with(ResponseTemplate::new(409).set_body_string("already exists"))
            .mount(&mock)
            .await;
        let api = ThreshdApi::new(&client);
        let dto = ThreshdPackageDto {
            name: "p".into(),
            filter: "f".into(),
            ..Default::default()
        };
        let e = api
            .update_package("p", &dto, Some("\"old\""))
            .await
            .unwrap_err();
        assert!(matches!(e, Error::HttpStatus { status: 412, .. }), "{e:?}");
        let e = api.create_package(&dto).await.unwrap_err();
        assert!(matches!(e, Error::HttpStatus { status: 409, .. }), "{e:?}");
    }
}
