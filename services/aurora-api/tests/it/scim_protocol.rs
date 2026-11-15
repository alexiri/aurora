#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The `/scim/v2` protocol conformance client (docs/18 §SCIM, RFC 7644 subset of `16 §W15`):
//! discovery documents, CRUD for Users and Groups, the filter/pagination subset, PATCH
//! semantics, error format and media types.

use crate::scim_common::*;
use http::{Method, StatusCode};
use serde_json::{Value, json};

const USERS: &str = "/scim/v2/Users";
const GROUPS: &str = "/scim/v2/Groups";
const USER_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const PATCH_OP: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

async fn patch(fx: &ScimFx, id: &str, ops: Value) -> (u16, Value) {
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{id}"),
            Some(json!({"schemas": [PATCH_OP], "Operations": ops})),
        )
        .await;
    (s.as_u16(), v)
}

#[tokio::test]
async fn discovery_documents() {
    let fx = ScimFx::new().await;
    // ServiceProviderConfig.
    let (s, hd, v) = fx
        .scim(Method::GET, "/scim/v2/ServiceProviderConfig", None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        hd["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/scim+json")
    );
    assert_eq!(
        v["schemas"][0], "urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig",
        "{v}"
    );
    // ResourceTypes: User and Group.
    let (s, _, v) = fx.scim(Method::GET, "/scim/v2/ResourceTypes", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let names: Vec<&str> = v["Resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert_eq!(names, vec!["User", "Group"], "{v}");
    // Schemas: exactly the subset's attributes; the enterprise extension is absent.
    let (s, _, v) = fx.scim(Method::GET, "/scim/v2/Schemas", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let ids: Vec<&str> = v["Resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert_eq!(ids, vec![USER_SCHEMA, GROUP_SCHEMA], "{v}");
    let user_schema = v["Resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == USER_SCHEMA)
        .unwrap()
        .clone();
    let attrs: Vec<&str> = user_schema["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert!(!attrs.iter().any(|a| a.contains("enterprise")), "{v}");
    assert!(
        attrs.contains(&"userName") && attrs.contains(&"active"),
        "{v}"
    );
}

#[tokio::test]
async fn user_crud_and_stable_content_type() {
    let fx = ScimFx::new().await;
    // POST → 201 with Location.
    let (s, hd, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({
                "schemas": [USER_SCHEMA],
                "userName": "u1@x.io",
                "name": {"givenName": "U", "familyName": "One"},
                "emails": [{"value": "u1@x.io", "type": "work", "primary": true}],
            })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert!(
        hd["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/scim+json")
    );
    assert!(hd.get("location").is_some(), "201 carries Location");
    assert_eq!(v["schemas"][0], USER_SCHEMA, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    // GET item.
    let (s, _, v2) = fx.scim(Method::GET, &format!("{USERS}/{id}"), None).await;
    assert_eq!(s, StatusCode::OK, "{v2}");
    assert_eq!(v2["id"], v["id"], "{v2}");
    // PUT replaces.
    let (s, _, v3) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{id}"),
            Some(json!({
                "schemas": [USER_SCHEMA],
                "userName": "u1@x.io",
                "displayName": "Renamed",
                "active": true,
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v3}");
    assert_eq!(v3["displayName"], "Renamed", "{v3}");
    // DELETE → 204, then 404.
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{id}"), None)
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _, _) = fx.scim(Method::GET, &format!("{USERS}/{id}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // DELETE again: idempotent 204.
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{USERS}/{id}"), None)
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn group_crud() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("g1@x.io").await;
    let uid = u["id"].as_str().unwrap().to_owned();
    let (s, hd, v) = fx
        .scim(
            Method::POST,
            GROUPS,
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "team",
                "members": [{"value": uid}],
            })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert!(hd.get("location").is_some());
    let gid = v["id"].as_str().unwrap().to_owned();
    assert_eq!(v["members"][0]["value"], uid.as_str(), "{v}");
    // GET list without `attributes=members`: no member lists.
    let (s, _, v) = fx.scim(Method::GET, GROUPS, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["Resources"][0]["members"].is_null(), "{v}");
    // `attributes=members` adds them.
    let (s, _, v) = fx
        .scim(Method::GET, &format!("{GROUPS}?attributes=members"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        v["Resources"][0]["members"][0]["value"],
        uid.as_str(),
        "{v}"
    );
    // PUT replaces the member list.
    let u2 = fx.provision_ok("g2@x.io").await;
    let u2id = u2["id"].as_str().unwrap().to_owned();
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{GROUPS}/{gid}"),
            Some(json!({
                "schemas": [GROUP_SCHEMA],
                "displayName": "team",
                "members": [{"value": u2id}],
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let members: Vec<&str> = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["value"].as_str())
        .collect();
    assert_eq!(members, vec![u2id.as_str()], "{v}");
    // DELETE.
    let (s, _, _) = fx
        .scim(Method::DELETE, &format!("{GROUPS}/{gid}"), None)
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _, _) = fx.scim(Method::GET, &format!("{GROUPS}/{gid}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn filter_subset_and_invalid_filter() {
    let fx = ScimFx::new().await;
    for (name, mail) in [("Ann", "ann@x.io"), ("Bob", "bob@x.io")] {
        fx.provision(mail, json!({"displayName": name})).await;
    }
    // userName eq
    let (s, _, v) = fx
        .scim(
            Method::GET,
            &format!("{USERS}?filter={}", urlencode("userName eq \"ann@x.io\"")),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], 1, "{v}");
    assert_eq!(v["Resources"][0]["userName"], "ann@x.io", "{v}");
    // emails eq and externalId eq
    let (s, _, v) = fx
        .scim(
            Method::GET,
            &format!("{USERS}?filter={}", urlencode("emails eq \"bob@x.io\"")),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], 1, "{v}");
    // displayName eq (case-insensitive)
    let (s, _, v) = fx
        .scim(
            Method::GET,
            &format!("{USERS}?filter={}", urlencode("displayName eq \"ANN\"")),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], 1, "{v}");
    // Everything else is invalidFilter.
    for f in [
        "userName sw \"a\"",
        "title eq \"x\"",
        "meta.created gt \"2020-01-01\"",
    ] {
        let (s, _, v) = fx
            .scim(
                Method::GET,
                &format!("{USERS}?filter={}", urlencode(f)),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{f}: {v}");
        assert_eq!(scim_type(&v), Some("invalidFilter"), "{f}: {v}");
    }
    // Groups: displayName, externalId and id only.
    let (s, _, v) = fx
        .scim(
            Method::GET,
            &format!("{GROUPS}?filter={}", urlencode("displayName eq \"nope\"")),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], 0, "{v}");
    let (s, _, v) = fx
        .scim(
            Method::GET,
            &format!("{GROUPS}?filter={}", urlencode("userName eq \"x\"")),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(scim_type(&v), Some("invalidFilter"), "{v}");
}

fn urlencode(s: &str) -> String {
    s.replace(' ', "%20").replace('"', "%22")
}

#[tokio::test]
async fn pagination_semantics() {
    let fx = ScimFx::new().await;
    let mut ids = Vec::new();
    for i in 0..5 {
        let v = fx.provision_ok(&format!("p{i}@x.io")).await;
        ids.push(v["id"].as_str().unwrap().to_owned());
    }
    // count=2, startIndex=1: the first two in stable creation order.
    let (s, _, v) = fx
        .scim(Method::GET, &format!("{USERS}?count=2&startIndex=1"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], 5, "{v}");
    assert_eq!(v["itemsPerPage"], 2, "{v}");
    assert_eq!(v["startIndex"], 1, "{v}");
    let page: Vec<&str> = v["Resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert_eq!(page, vec![ids[0].as_str(), ids[1].as_str()], "{v}");
    // startIndex=4, count=2: one item past the end is fine.
    let (s, _, v) = fx
        .scim(Method::GET, &format!("{USERS}?count=2&startIndex=4"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["itemsPerPage"], 2, "{v}");
    // Empty page fully past the end.
    let (s, _, v) = fx
        .scim(Method::GET, &format!("{USERS}?count=2&startIndex=9"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], 5, "{v}");
    assert_eq!(v["itemsPerPage"], 0, "{v}");
    assert_eq!(v["Resources"].as_array().unwrap().len(), 0, "{v}");
    // count=0: metadata only.
    let (s, _, v) = fx
        .scim(Method::GET, &format!("{USERS}?count=0"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], 5, "{v}");
    assert_eq!(v["itemsPerPage"], 0, "{v}");
    // count above the cap clamps to 100 (or the total).
    let (s, _, v) = fx
        .scim(Method::GET, &format!("{USERS}?count=500"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["itemsPerPage"], 5, "{v}");
    // Malformed numbers are 400 invalidSyntax.
    let (s, _, v) = fx
        .scim(Method::GET, &format!("{USERS}?count=abc"), None)
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(scim_type(&v), Some("invalidSyntax"), "{v}");
}

#[tokio::test]
async fn patch_ops_and_case_insensitivity() {
    let fx = ScimFx::new().await;
    let u = fx.provision("pp@x.io", json!({"displayName": "P"})).await;
    let id = u["id"].as_str().unwrap().to_owned();
    // add without path (Okta/Entra object form).
    let (s, v) = patch(
        &fx,
        &id,
        json!([{"op": "add", "value": {"displayName": "P2"}}]),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["displayName"], "P2", "{v}");
    // replace with path; case-insensitive op.
    let (s, v) = patch(
        &fx,
        &id,
        json!([{"op": "REPLACE", "path": "displayName", "value": "P3"}]),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["displayName"], "P3", "{v}");
    // emails[type eq "work"].value.
    let (s, v) = patch(
        &fx,
        &id,
        json!([{"op": "replace", "path": "emails[type eq \"work\"].value", "value": "new@x.io"}]),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["userName"], "new@x.io", "{v}");
    // remove of an optional attribute.
    let (s, v) = patch(&fx, &id, json!([{"op": "remove", "path": "displayName"}])).await;
    assert_eq!(s, 200, "{v}");
    assert!(v["displayName"].is_null(), "{v}");
    // active as boolean and as string "False".
    let (s, v) = patch(
        &fx,
        &id,
        json!([{"op": "replace", "path": "active", "value": true}]),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = patch(
        &fx,
        &id,
        json!([{"op": "replace", "path": "active", "value": "False"}]),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["active"], false, "{v}");
    // One bad operation applies none (atomicity).
    let (s, v) = patch(
        &fx,
        &id,
        json!([
            {"op": "replace", "path": "displayName", "value": "WouldBe"},
            {"op": "replace", "path": "nonsense/path", "value": "x"},
        ]),
    )
    .await;
    assert_eq!(s, 400, "{v}");
    let (s, _, v2) = fx.scim(Method::GET, &format!("{USERS}/{id}"), None).await;
    assert_eq!(s, 200, "{v2}");
    assert!(v2["displayName"].is_null(), "no operation applied: {v2}");
    // Unknown op value.
    let (s, v) = patch(&fx, &id, json!([{"op": "explode", "path": "displayName"}])).await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(scim_type(&v), Some("invalidValue"), "{v}");
    // remove without path.
    let (s, v) = patch(&fx, &id, json!([{"op": "remove"}])).await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn patch_operation_limit_and_unsupported_attributes() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("lim@x.io").await;
    let id = u["id"].as_str().unwrap().to_owned();
    // 101 operations → 400 tooMany.
    let ops: Value = Value::Array(
        (0..101)
            .map(|i| json!({"op": "replace", "path": "displayName", "value": format!("n{i}")}))
            .collect(),
    );
    let (s, v) = patch(&fx, &id, ops).await;
    assert_eq!(s, 400, "{v}");
    assert!(v["detail"].as_str().unwrap().contains("100"), "{v}");
    // `roles` and `password` are refused on POST and PATCH.
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "r@x.io", "password": "hunter2"})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(scim_type(&v), Some("invalidValue"), "{v}");
    let (s, v) = patch(&fx, &id, json!([{"op": "add", "value": {"roles": []}}])).await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn error_format_media_types_and_size() {
    let fx = ScimFx::new().await;
    // Every error is the SCIM error format with the scim+json content type.
    let (s, hd, v) = fx
        .scim(
            Method::GET,
            "/scim/v2/Users/00000000-0000-0000-0000-000000000000",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    assert_scim_error(404, &v);
    assert!(
        hd["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/scim+json")
    );
    // 406: an Accept naming no JSON type.
    let (s, _, v) =
        fx.h.send(
            Method::GET,
            USERS,
            &[
                ("authorization", &format!("Bearer {}", fx.token)),
                ("accept", "text/html"),
            ],
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_ACCEPTABLE, "{v}");
    assert_scim_error(406, &v);
    // 415: a body content type that is neither scim+json nor json.
    let (s, _, v) =
        fx.h.send(
            Method::POST,
            USERS,
            &[
                ("authorization", &format!("Bearer {}", fx.token)),
                ("content-type", "text/csv"),
            ],
            Some(json!({"userName": "x@x.io"})),
        )
        .await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{v}");
    assert_scim_error(415, &v);
    // application/json is accepted too.
    let (s, _, _) =
        fx.h.send(
            Method::POST,
            USERS,
            &[
                ("authorization", &format!("Bearer {}", fx.token)),
                ("content-type", "application/json"),
            ],
            Some(json!({"userName": "j@x.io"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    // 413 above SCIM_BODY_MAX: a body one byte over the cap.
    let big = "x".repeat(aurora_core::limits::SCIM_BODY_MAX + 1);
    let (s, _, v) =
        fx.h.send(
            Method::POST,
            USERS,
            &[("authorization", &format!("Bearer {}", fx.token))],
            Some(json!({"userName": "big@x.io", "displayName": big})),
        )
        .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{v}");
    assert_scim_error(413, &v);
    // Malformed JSON → 400 invalidSyntax.
    let req = http::Request::builder()
        .method(Method::POST)
        .uri(USERS)
        .header("authorization", format!("Bearer {}", fx.token))
        .header("content-type", "application/scim+json")
        .body(axum::body::Body::from("{not json"))
        .unwrap();
    let res = tower::ServiceExt::oneshot(fx.h.app.clone(), req)
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let bytes = http_body_util::BodyExt::collect(res.into_body())
        .await
        .unwrap()
        .to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(scim_type(&v), Some("invalidSyntax"), "{v}");
    // Unknown query keys are ignored (lenient parsing).
    let (s, _, _) = fx
        .scim(Method::GET, &format!("{USERS}?sortBy=userName"), None)
        .await;
    assert_eq!(s, StatusCode::OK);
}

// ---- FIX-081 ---------------------------------------------------------------------------

/// Seed `n` unremoved links of the fixture org straight into the table (a directory bigger than
/// any old in-memory cap), without memberships: only the list endpoints read them.
async fn seed_links(fx: &ScimFx, n: i64) {
    sqlx::query(
        "WITH n AS (SELECT gen_random_uuid() AS id, gen_random_uuid() AS uid, i \
                      FROM generate_series(1, $2::int) i), \
              u AS (INSERT INTO users (id, email, display_name) \
                    SELECT uid, 'bulk' || i || '@x.io', 'b' FROM n RETURNING id) \
         INSERT INTO scim_users (id, org_id, user_id, user_name, display_name, state, created_at) \
         SELECT n.id, $1, n.uid, 'bulk' || n.i || '@x.io', 'Bulk ' || (n.i % 7), 'active', \
                now() + n.i * interval '1 millisecond' FROM n",
    )
    .bind(fx.org)
    .bind(n as i32)
    .execute(fx.h.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn fix081_lists_page_in_sql_and_report_the_true_total() {
    let fx = ScimFx::new().await;
    seed_links(&fx, 10_050).await;
    // The total is the number of rows, not the size of an in-memory window.
    let (s, _, v) = fx
        .scim(Method::GET, &format!("{USERS}?count=0"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], 10_050, "{v}");
    // A page far beyond the old 10 000-row window still has rows.
    let (s, _, v) = fx
        .scim(
            Method::GET,
            &format!("{USERS}?startIndex=10046&count=10"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], 10_050, "{v}");
    assert_eq!(v["Resources"].as_array().unwrap().len(), 5, "{v}");
    assert_eq!(v["startIndex"], 10_046, "{v}");
    // A displayName filter counts and pages in SQL as well (every 7th link is `Bulk 3`).
    let want: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM scim_users WHERE org_id = $1 AND display_name = 'Bulk 3'",
    )
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    let (s, _, v) = fx
        .scim(
            Method::GET,
            &format!("{USERS}?filter=displayName%20eq%20%22bulk%203%22&count=2&startIndex=3"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalResults"], want, "{v}");
    assert_eq!(v["Resources"].as_array().unwrap().len(), 2, "{v}");
}

async fn lock_waiters(fx: &ScimFx) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity \
         WHERE datname = current_database() AND wait_event_type = 'Lock'",
    )
    .fetch_one(fx.h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn fix081_overlapping_patches_on_different_attributes_both_survive() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("lost-update@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    let link: uuid::Uuid = sid.parse().unwrap();
    // The link row is held, so both PATCHes are past whatever they read before the lock and
    // queue on it; they then run one after the other.
    let mut held = fx.h.pool().begin().await.unwrap();
    sqlx::query("SELECT id FROM scim_users WHERE id = $1 FOR UPDATE")
        .bind(link)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let a = patch(
        &fx,
        &sid,
        json!([{"op": "replace", "path": "displayName", "value": "Alpha"}]),
    );
    let b = patch(
        &fx,
        &sid,
        json!([{"op": "replace", "path": "externalId", "value": "ext-beta"}]),
    );
    let release = async {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while lock_waiters(&fx).await < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "both PATCHes should wait for the link row"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        held.commit().await.unwrap();
    };
    let ((sa, va), (sb, vb), ()) = tokio::join!(a, b, release);
    assert_eq!((sa, sb), (200, 200), "{va} {vb}");
    let (s, _, v) = fx.scim(Method::GET, &format!("{USERS}/{sid}"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["displayName"], "Alpha", "no lost update: {v}");
    assert_eq!(v["externalId"], "ext-beta", "no lost update: {v}");
}

#[tokio::test]
async fn fix081_authentication_comes_before_the_body_and_the_id() {
    let fx = ScimFx::new().await;
    // A bearer-less request with a 2 MiB body is the same 401 as any other, answered without
    // reading the body (not a 413).
    let big = "x".repeat(2 * 1024 * 1024);
    let (s, _, v) =
        fx.h.send(
            Method::POST,
            USERS,
            &[("content-type", "application/scim+json")],
            Some(json!({"userName": "big@x.io", "displayName": big})),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    assert_scim_error(401, &v);
    // A dead token with a malformed (here: empty) body gets the standard 401, never a 400.
    let (s, _, v) = fx
        .scim_with(Method::POST, USERS, "Bearer aud_deadtoken", None)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    assert_scim_error(401, &v);
    // A malformed `{id}` is a SCIM error body (404), after authentication; without a valid
    // token it is the 401.
    let (s, hd, v) = fx
        .scim(Method::GET, &format!("{USERS}/not-a-uuid"), None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    assert_scim_error(404, &v);
    assert!(
        hd["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/scim+json")
    );
    let (s, _, v) = fx
        .scim_with(
            Method::DELETE,
            &format!("{GROUPS}/not-a-uuid"),
            "Bearer aud_deadtoken",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    assert_scim_error(401, &v);
    for (m, p) in [
        (Method::PUT, format!("{USERS}/xx")),
        (Method::PATCH, format!("{GROUPS}/xx")),
    ] {
        let (s, _, v) = fx
            .scim(
                m,
                &p,
                Some(json!({"schemas": [PATCH_OP], "Operations": []})),
            )
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
        assert_scim_error(404, &v);
    }
}

#[tokio::test]
async fn fix081_a_filtered_email_path_changes_only_the_matching_entry() {
    let fx = ScimFx::new().await;
    let u = fx.provision_ok("mail@x.io").await;
    let sid = u["id"].as_str().unwrap().to_owned();
    // The stored e-mail is the one primary `work` entry: a `home` target matches nothing and
    // never rewrites `userName`.
    let (s, v) = patch(
        &fx,
        &sid,
        json!([{"op": "replace", "path": "emails[type eq \"home\"].value", "value": "elsewhere@x.io"}]),
    )
    .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(scim_type(&v), Some("noTarget"), "{v}");
    let (_, _, v) = fx.scim(Method::GET, &format!("{USERS}/{sid}"), None).await;
    assert_eq!(v["userName"], "mail@x.io", "{v}");
    // `primary eq false` matches nothing either; an unsupported filter is 400 `invalidPath`.
    let (s, v) = patch(
        &fx,
        &sid,
        json!([{"op": "replace", "path": "emails[primary eq false].value", "value": "p@x.io"}]),
    )
    .await;
    assert_eq!((s, scim_type(&v)), (400, Some("noTarget")), "{v}");
    let (s, v) = patch(
        &fx,
        &sid,
        json!([{"op": "replace", "path": "emails[value co \"x\"].value", "value": "p@x.io"}]),
    )
    .await;
    assert_eq!((s, scim_type(&v)), (400, Some("invalidPath")), "{v}");
    // The matching entries still work.
    for (i, path) in [
        "emails[type eq \"work\"].value",
        "emails[primary eq true].value",
    ]
    .iter()
    .enumerate()
    {
        let new = format!("moved{i}@x.io");
        let (s, v) = patch(
            &fx,
            &sid,
            json!([{"op": "replace", "path": path, "value": new}]),
        )
        .await;
        assert_eq!(s, 200, "{path}: {v}");
        assert_eq!(v["userName"], new, "{path}: {v}");
    }
}

#[tokio::test]
async fn fix081_an_owner_link_is_never_written_by_scim() {
    let fx = ScimFx::new().await;
    // The org owner is adopted into the directory (allowed), and then cannot be changed by it.
    let owner = fx.owner.user_id;
    let (s, _, v) = fx
        .scim(Method::POST, USERS, Some(json!({"userName": "owner@x.io"})))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let sid = v["id"].as_str().unwrap().to_owned();
    let snapshot = |fx: &ScimFx| {
        let pool = fx.h.pool().clone();
        let sid: uuid::Uuid = sid.parse().unwrap();
        async move {
            sqlx::query_as::<_, (String, Option<String>, Option<String>, String, String)>(
                "SELECT user_name::text, display_name, external_id, state, \
                        updated_at::text FROM scim_users WHERE id = $1",
            )
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let before = snapshot(&fx).await;
    for ops in [
        json!([{"op": "replace", "path": "displayName", "value": "Hijacked"}]),
        json!([{"op": "replace", "path": "userName", "value": "owner2@x.io"}]),
        json!([{"op": "replace", "path": "externalId", "value": "e-1"}]),
        json!([{"op": "replace", "path": "active", "value": false}]),
    ] {
        let (s, v) = patch(&fx, &sid, ops.clone()).await;
        assert_eq!(s, 403, "{ops}: {v}");
    }
    let (s, _, v) = fx
        .scim(
            Method::PUT,
            &format!("{USERS}/{sid}"),
            Some(json!({"userName": "owner@x.io"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(snapshot(&fx).await, before, "nothing was written");
    // A request that changes nothing is not a mutation.
    let (s, v) = patch(
        &fx,
        &sid,
        json!([{"op": "replace", "path": "userName", "value": "owner@x.io"}]),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let _ = owner;
}
