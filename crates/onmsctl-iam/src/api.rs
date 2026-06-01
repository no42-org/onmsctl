/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed HTTP client for Horizon's v1 `UserRestService` (`/rest/users`).
//!
//! Endpoints exposed:
//!
//! | Method | Path                              | Wrapper                          |
//! |--------|-----------------------------------|----------------------------------|
//! | GET    | `rest/users?limit=…`              | [`list_users`](IamApi::list_users) |
//! | GET    | `rest/users/{name}`              | [`get_user`](IamApi::get_user)   |
//! | GET    | `rest/users/whoami`              | [`get_whoami`](IamApi::get_whoami) |
//! | POST   | `rest/users?hashPassword=…`      | [`post_user`](IamApi::post_user) (XML) |
//! | PUT    | `rest/users/{name}`              | [`put_user_form`](IamApi::put_user_form) (form) |
//! | DELETE | `rest/users/{name}`              | [`delete_user`](IamApi::delete_user) |
//! | PUT    | `rest/users/{name}/roles/{role}` | [`put_user_role`](IamApi::put_user_role) |
//! | DELETE | `rest/users/{name}/roles/{role}` | [`delete_user_role`](IamApi::delete_user_role) |
//!
//! Per spike 0.1 (2026-05-29) the body format differs by verb: GET/list/
//! whoami are JSON, POST is XML-only (JSON → 415), PUT is form-encoded.
//! [`crate::model::wire`] documents the split; this module wires each
//! method to the matching `OnmsClient` helper.
//!
//! GET endpoints that may legitimately 404 (`get_user`) surface that as
//! `Ok(None)`. `get_whoami` additionally maps any non-2xx to `Ok(None)`
//! (per design §D6: the self-lockout check treats an unavailable identity
//! as `None` and refuses the apply rather than evaluating against a
//! phantom caller).

use onmsctl_core::{Error, OnmsClient, Result};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use crate::model::local::UserLocal;
use crate::model::wire::{
    OnmsUserListWire, OnmsUserWire, SetPasswordForm, UpdateForm, user_create_xml,
};

/// Characters percent-encoded inside a `{name}` / `{role}` path segment.
/// Mirrors provisioning's `PATH_SEGMENT`: encode everything beyond RFC 3986
/// "unreserved" plus the reserved/sub-delim characters that carry URL
/// meaning, so a username containing `/`, `@`, `?`, etc. can't escape its
/// segment. (Numeric-only usernames are refused at parse time, but other
/// punctuation is legal upstream.)
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b'/')
    .add(b'?')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'=')
    .add(b'+')
    .add(b';')
    .add(b'@')
    .add(b'[')
    .add(b']');

/// REST base under the OnmsClient root URL. The v1 `UserRestService` lives
/// at `/opennms/rest/users`; the client is configured at `/opennms/`.
const BASE: &str = "rest";

/// Upper bound on the single unbounded `GET /users`. Spike 0.4 confirmed the
/// endpoint does not paginate (`count == totalCount` in one call); this
/// guards the pathological case rather than implementing real paging.
const USER_LIST_LIMIT: i64 = 10_000;

/// Typed wrapper over [`OnmsClient`] for the IAM user surface.
pub struct IamApi<'c> {
    client: &'c OnmsClient,
}

impl<'c> IamApi<'c> {
    pub fn new(client: &'c OnmsClient) -> Self {
        Self { client }
    }

    /// `GET /users?limit=10000&offset=0`. Returns the full list wrapper
    /// (`offset`/`count`/`totalCount` + the user array). Spike 0.4 verified a
    /// single unbounded call returns everything; the lockout planner uses
    /// `total_count` to refuse rather than silently truncate if a future
    /// install ever exceeds the limit.
    pub async fn list_users(&self) -> Result<OnmsUserListWire> {
        let path = format!("{BASE}/users");
        self.client
            .get(
                &path,
                &[
                    ("limit", USER_LIST_LIMIT.to_string().as_str()),
                    ("offset", "0"),
                ],
            )
            .await
    }

    /// `GET /users/{name}`. `Ok(None)` on 404 (no such user).
    pub async fn get_user(&self, name: &str) -> Result<Option<OnmsUserWire>> {
        let path = format!("{BASE}/users/{}", encode(name));
        match self.client.get::<OnmsUserWire>(&path, &[]).await {
            Ok(u) => Ok(Some(u)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `GET /users/whoami`. Returns the calling identity, or `Ok(None)` when
    /// the server responds non-2xx (401/403 under token/anonymous auth, or a
    /// `204` — core maps both to [`Error::HttpStatus`]) or returns a 2xx user
    /// with an empty `user-id`.
    ///
    /// **Caveat (deferred to Group 7):** a `200` with an empty or non-JSON
    /// body decodes to [`Error::Transport`], not [`Error::HttpStatus`], so it
    /// currently *propagates* rather than collapsing to `None`. §D6/task 7.2
    /// say "empty body → None"; the precise `IamWhoamiUnavailable` refusal is
    /// enforced in the Group 7 lockout code, which treats both `None` and a
    /// whoami error as "cannot evaluate self-lockout → refuse". The outcome
    /// (apply aborts) is safe either way; only the surfaced error class
    /// differs. Genuine transport errors (DNS, refused, TLS) always propagate.
    pub async fn get_whoami(&self) -> Result<Option<OnmsUserWire>> {
        let path = format!("{BASE}/users/whoami");
        match self.client.get::<OnmsUserWire>(&path, &[]).await {
            Ok(u) if u.user_id.is_empty() => Ok(None),
            Ok(u) => Ok(Some(u)),
            // Non-2xx (incl. the 204-decoded-as-error case) → no usable
            // identity for the self-lockout check.
            Err(Error::HttpStatus { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST /users` with the XML body (spike 0.1: POST is XML-only). The
    /// `password` is **required**: a password-less create returns
    /// `500 'password' cannot be null!` (verified against the live lab
    /// 2026-05-31), so the type makes the invalid state unrepresentable. The
    /// plaintext rides in the body and `?hashPassword=true` is always
    /// appended so the server hashes it and sets `passwordSalt=true`
    /// internally — the client never sends a precomputed hash.
    ///
    /// The "a Create plan must carry a `passwordRef`" policy is enforced one
    /// layer up in the planner (Group 6), which resolves the ref to this
    /// plaintext; callers never reach here without one. An **empty** password
    /// is rejected here defensively (`resolve_password_ref` already refuses
    /// empty secrets upstream) so the contract is enforced rather than merely
    /// asserted, and the operator gets a clear error instead of the server's
    /// `500 'password' cannot be null!`.
    pub async fn post_user(&self, local: &UserLocal, password: &str) -> Result<()> {
        if password.is_empty() {
            return Err(Error::Config(
                "POST /users requires a non-empty password; the resolved passwordRef is empty"
                    .into(),
            ));
        }
        let roles: Vec<String> = local.spec.roles.iter().cloned().collect();
        let duty: Vec<String> = local.spec.duty_schedule.clone().into_iter().collect();
        let xml = user_create_xml(
            &local.metadata.name,
            local.spec.full_name.as_deref(),
            local.spec.comments.as_deref(),
            local.spec.email.as_deref(),
            Some(password),
            &duty,
            &roles,
        )
        .map_err(|e| Error::Config(format!("serialize user XML for POST /users: {e}")))?;
        let path = format!("{BASE}/users?hashPassword=true");
        self.client.post_xml(&path, xml).await
    }

    /// `PUT /users/{name}` with the form-encoded update body (bean-property
    /// keys). The caller (planner) sends only the changed scalar fields.
    pub async fn put_user_form(&self, name: &str, form: &UpdateForm) -> Result<()> {
        let path = format!("{BASE}/users/{}", encode(name));
        self.client.put_form(&path, form).await
    }

    /// Rotate a user's password (task 5.8). Pre-flights with `GET
    /// /users/{name}` (task 4.6) so a missing user yields a clear
    /// [`Error::UserNotFound`] instead of an ambiguous form-PUT 404, then
    /// `PUT /users/{name}` with `password=<plaintext>&hashPassword=true`.
    pub async fn set_password(&self, name: &str, plaintext: &str) -> Result<()> {
        self.require_user(name).await?;
        let path = format!("{BASE}/users/{}", encode(name));
        self.client
            .put_form(&path, &SetPasswordForm::new(plaintext))
            .await
    }

    /// `DELETE /users/{name}`.
    pub async fn delete_user(&self, name: &str) -> Result<()> {
        let path = format!("{BASE}/users/{}", encode(name));
        self.client.delete::<serde_json::Value>(&path, None).await
    }

    /// `PUT /users/{name}/roles/{role}`. Adds a single role to the user.
    pub async fn put_user_role(&self, name: &str, role: &str) -> Result<()> {
        let path = format!("{BASE}/users/{}/roles/{}", encode(name), encode(role));
        // Empty PUT body — the role rides on the path. JSON null body matches
        // the other empty-body PUTs (e.g. provisioning's trigger_import).
        self.client.put_drain(&path, &serde_json::Value::Null).await
    }

    /// `DELETE /users/{name}/roles/{role}`. Removes a single role.
    pub async fn delete_user_role(&self, name: &str, role: &str) -> Result<()> {
        let path = format!("{BASE}/users/{}/roles/{}", encode(name), encode(role));
        self.client.delete::<serde_json::Value>(&path, None).await
    }

    /// Pre-flight existence check for `set-password` (task 4.6): issue
    /// `GET /users/{name}` and map 404 to a clear [`Error::UserNotFound`]
    /// rather than letting the subsequent form PUT 404 ambiguously. Returns
    /// the current server record on success (callers may inspect it).
    pub async fn require_user(&self, name: &str) -> Result<OnmsUserWire> {
        match self.get_user(name).await? {
            Some(u) => Ok(u),
            None => Err(Error::UserNotFound {
                name: name.to_owned(),
            }),
        }
    }
}

/// Percent-encode a `{name}` / `{role}` path segment; preserves unreserved
/// characters (`-`, `_`, `.`, `~`) verbatim for readable URLs.
fn encode(s: &str) -> impl std::fmt::Display + '_ {
    utf8_percent_encode(s, PATH_SEGMENT)
}

fn is_not_found(e: &Error) -> bool {
    matches!(e, Error::HttpStatus { status: 404, .. })
}

// ---------------------------------------------------------------------------
// Tests — wiremock-driven HTTP round-trips
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::local::{ApiVersion, KindUser, Metadata, UserLocal, UserSpec};
    use onmsctl_core::{AuthCreds, Context, OutputFormat, Url};
    use std::collections::BTreeSet;
    use wiremock::matchers::{body_string_contains, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_with_client() -> (MockServer, OnmsClient) {
        let server = MockServer::start().await;
        let url = Url::parse(&format!("{}/", server.uri())).unwrap();
        let ctx = Context {
            name: "test".into(),
            url,
            creds: AuthCreds::basic("admin", "secret"),
            insecure_skip_tls_verify: false,
            output_format: OutputFormat::Table,
            verbose: false,
            read_only: false,
        };
        let client = OnmsClient::from_context(&ctx).unwrap();
        (server, client)
    }

    fn sample_local(name: &str) -> UserLocal {
        UserLocal {
            api_version: ApiVersion,
            kind: KindUser,
            metadata: Metadata {
                name: name.to_owned(),
                unmodeled: None,
            },
            spec: UserSpec {
                full_name: Some("Spike Zero".into()),
                email: Some("spike0@example.com".into()),
                comments: Some("spike".into()),
                duty_schedule: None,
                roles: BTreeSet::from(["ROLE_USER".to_string()]),
                password_ref: None,
            },
        }
    }

    #[tokio::test]
    async fn list_users_sends_limit_and_returns_wrapper() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/users"))
            .and(query_param("limit", "10000"))
            .and(query_param("offset", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "offset": 0, "count": 1, "totalCount": 1,
                "user": [{"user-id": "admin", "role": ["ROLE_ADMIN"]}]
            })))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        let list = api.list_users().await.unwrap();
        assert_eq!(list.total_count, 1);
        assert_eq!(list.users[0].user_id, "admin");
        assert_eq!(list.users[0].roles, vec!["ROLE_ADMIN"]);
    }

    #[tokio::test]
    async fn get_user_returns_some_on_200() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice", "full-name": "Alice", "role": ["ROLE_USER"]
            })))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        let u = api.get_user("alice").await.unwrap().expect("present");
        assert_eq!(u.user_id, "alice");
    }

    #[tokio::test]
    async fn get_user_returns_none_on_404() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/users/ghost"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        assert!(api.get_user("ghost").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn require_user_maps_404_to_user_not_found() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/users/ghost"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        let err = api.require_user("ghost").await.unwrap_err();
        assert!(matches!(err, Error::UserNotFound { name } if name == "ghost"));
    }

    #[tokio::test]
    async fn whoami_returns_identity_on_200() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/users/whoami"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "admin", "full-name": "Administrator", "role": ["ROLE_ADMIN"]
            })))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        let me = api.get_whoami().await.unwrap().expect("identity");
        assert_eq!(me.user_id, "admin");
    }

    #[tokio::test]
    async fn whoami_maps_401_to_none() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/users/whoami"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        assert!(api.get_whoami().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn post_user_sends_xml_body_and_hash_query() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/rest/users"))
            .and(query_param("hashPassword", "true"))
            .and(header("content-type", "application/xml"))
            .and(body_string_contains("<user-id>spike0</user-id>"))
            .and(body_string_contains("<password>s3cr3t</password>"))
            .and(body_string_contains("<role>ROLE_USER</role>"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        api.post_user(&sample_local("spike0"), "s3cr3t")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn post_user_always_appends_hash_query() {
        // Verified live: a password-less POST /users returns 500
        // ("'password' cannot be null!"), so create always carries a
        // password and always requests server-side hashing. A POST without
        // the hashPassword query would land on no mock here and 404.
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/rest/users"))
            .and(query_param("hashPassword", "true"))
            .and(header("content-type", "application/xml"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        api.post_user(&sample_local("any"), "pw").await.unwrap();
    }

    #[tokio::test]
    async fn post_user_rejects_empty_password() {
        // Defensive guard: an empty resolved password fails fast client-side
        // (no HTTP issued) rather than letting the server 500.
        let (_mock, client) = mock_with_client().await;
        let api = IamApi::new(&client);
        let err = api.post_user(&sample_local("x"), "").await.unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
    }

    #[tokio::test]
    async fn put_user_form_uses_form_encoding_and_bean_keys() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("PUT"))
            .and(path("/rest/users/alice"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string_contains("fullName=Alice+Renamed"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        let form = UpdateForm {
            full_name: Some("Alice Renamed".into()),
            email: None,
            comments: None,
        };
        api.put_user_form("alice", &form).await.unwrap();
    }

    #[tokio::test]
    async fn put_user_form_propagates_404() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("PUT"))
            .and(path("/rest/users/ghost"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        let err = api
            .put_user_form("ghost", &UpdateForm::default())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::HttpStatus { status: 404, .. }));
    }

    #[tokio::test]
    async fn set_password_preflights_then_puts_form() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "alice"
            })))
            .mount(&mock)
            .await;
        Mock::given(method("PUT"))
            .and(path("/rest/users/alice"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string_contains("password=n3w-pw"))
            .and(body_string_contains("hashPassword=true"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        api.set_password("alice", "n3w-pw").await.unwrap();
    }

    #[tokio::test]
    async fn set_password_missing_user_is_user_not_found() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/users/ghost"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        let err = api.set_password("ghost", "pw").await.unwrap_err();
        assert!(matches!(err, Error::UserNotFound { name } if name == "ghost"));
    }

    #[tokio::test]
    async fn delete_user_targets_user_path() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("DELETE"))
            .and(path("/rest/users/alice"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        api.delete_user("alice").await.unwrap();
    }

    #[tokio::test]
    async fn put_user_role_targets_roles_subresource() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("PUT"))
            .and(path("/rest/users/alice/roles/ROLE_REST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        api.put_user_role("alice", "ROLE_REST").await.unwrap();
    }

    #[tokio::test]
    async fn delete_user_role_targets_roles_subresource() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("DELETE"))
            .and(path("/rest/users/alice/roles/ROLE_REST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        api.delete_user_role("alice", "ROLE_REST").await.unwrap();
    }

    #[tokio::test]
    async fn post_error_propagates_as_http_status() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("POST"))
            .and(path("/rest/users"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        let err = api.post_user(&sample_local("x"), "pw").await.unwrap_err();
        assert!(matches!(err, Error::HttpStatus { status: 400, .. }));
    }

    #[tokio::test]
    async fn username_with_special_char_is_percent_encoded() {
        let (mock, client) = mock_with_client().await;
        Mock::given(method("GET"))
            .and(path("/rest/users/a%40b"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user-id": "a@b"
            })))
            .mount(&mock)
            .await;
        let api = IamApi::new(&client);
        assert!(api.get_user("a@b").await.unwrap().is_some());
    }
}
