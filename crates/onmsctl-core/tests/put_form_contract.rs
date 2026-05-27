/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end wire-contract test for `OnmsClient::put_form`. Asserts the
//! exact bytes upstream Horizon's `UserRestService.updateUser` sees on the
//! wire: form-encoded body with percent-escaped special characters, set
//! against the right `Content-Type` header. Mirrors the cli-core delta
//! spec scenario "Form-encoded PUT sets the correct Content-Type".

use onmsctl_core::{AuthCreds, OnmsClient, Url};
use serde::Serialize;
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateForm<'a> {
    full_name: &'a str,
    email: &'a str,
}

#[tokio::test]
async fn put_form_serializes_struct_to_url_encoded_wire_bytes() {
    let mock = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v2/users/alice"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string("fullName=Alice&email=alice%40example.com"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mock)
        .await;

    let client = OnmsClient::from_parts(
        Url::parse(&format!("{}/api/v2/", mock.uri())).unwrap(),
        AuthCreds::bearer("tok"),
    )
    .unwrap();

    client
        .put_form(
            "users/alice",
            &UpdateForm {
                full_name: "Alice",
                email: "alice@example.com",
            },
        )
        .await
        .expect("form-encoded PUT should succeed against the mocked endpoint");
}
