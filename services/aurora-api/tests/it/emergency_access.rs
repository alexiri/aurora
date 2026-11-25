#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-236 (D-478, C-1164, C-1165, SEC-154): the proving tests for the emergency read access.
//!
//! * owner-only, session-only: a member, an org admin, an instance admin who is not an org
//!   owner and every API key (`auo_` org key included) are refused on open; a key can never
//!   *use* a grant either;
//! * bounded: 1–24 hours (default 4), one grant per workspace and person (the 409 and the
//!   partial unique index), never extendable (a second open while one is active is 409);
//! * one workspace: the grant opens nothing outside it;
//! * announced: e-mail and Web Push to the workspace's people in their own right and the org's
//!   other owners, never the opener, display names never e-mail addresses; the banner fields on
//!   the workspace; a personal workspace tells its person;
//! * recorded: every content read writes `conversation.emergency_view` before the body (a
//!   failed write refuses the read with 503, deduplicated per actor, conversation and hour);
//!   the export writes `conversation.export` with `via: "emergency"` too;
//! * read-only: the open, list and end responses carry metadata only;
//! * ends: on `expires_at` evaluated at use (before the sweep), by the opener or another org
//!   owner (idempotent, critical audit row), with `access_lost` when the opener stops being an
//!   owner, by the minute sweep (which also prunes rows past the 400-day retention).
//!
//! The route-level walk (every C-1165 read answers, every other workspace route refuses) is
//! `route_inventory.rs::emergency_reader_reads_the_readauth_routes_and_is_refused_everywhere_else`.

use crate::common::*;
use crate::web_push::{HOST, SUBJECT};
use aurora_testkit::push::MockPush;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

const MARKER: &str = "EMERGENCY-message-content";

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    ws2: Uuid,
    conv: Uuid,
    person: Uuid,
    person_tok: String,
    opener: Uuid,
    opener_tok: String,
    other: Uuid,
    other_tok: String,
    admin_tok: String,
    member_tok: String,
}

async fn world() -> W {
    world_with(|_| ()).await
}

/// The world, with the harness config mutated (the push test turns Web Push on).
async fn world_with(mutate: impl FnOnce(&mut aurora_config::ApiConfig)) -> W {
    let h = Harness::with_config(mutate).await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let ws2 = h.workspace(org).await;
    let agent = h.byo_agent(org, ws, "bot", "Bot").await;
    let person = h.user("p@x.io", false).await;
    let opener = h.user("opener@x.io", false).await;
    let other = h.user("oo@x.io", false).await;
    let admin = h.user("ad@x.io", false).await;
    let member = h.user("m@x.io", false).await; // The person is an org member too: the access law resolves workspace access through an
    // active org membership, so a workspace-bound non-member has no API access at all.
    h.member(org, person, "member", "active").await;
    h.member(org, opener, "owner", "active").await;
    h.member(org, other, "owner", "active").await;
    h.member(org, admin, "admin", "active").await;
    h.member(org, member, "member", "active").await;
    h.bind_ws(ws, person, "member").await;
    h.bind_ws(ws2, person, "member").await;
    // Distinct display names: the notices and the banner label people by name, never e-mail.
    for (id, name) in [
        (person, "Pat Person"),
        (opener, "Olive Owner"),
        (other, "Oscar Owner"),
    ] {
        sqlx::query("UPDATE users SET display_name = $1 WHERE id = $2")
            .bind(name)
            .bind(id)
            .execute(h.pool())
            .await
            .unwrap();
    }
    let (_, person_tok) = h.good_session(person).await;
    let (_, opener_tok) = h.good_session(opener).await;
    let (_, other_tok) = h.good_session(other).await;
    let (_, admin_tok) = h.good_session(admin).await;
    let (_, member_tok) = h.good_session(member).await;

    // The person's direct conversation with one message: the content the emergency reader may
    // read, and must record.
    let (s, v) = h
        .call_tok(
            &person_tok,
            Method::POST,
            &format!("/v1/agents/{agent}/conversations/direct"),
            None,
        )
        .await;
    assert!((200..300).contains(&s), "{v}");
    let conv: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = h
        .call_tok(
            &person_tok,
            Method::POST,
            &format!("/v1/conversations/{conv}/messages"),
            Some(json!({"text": MARKER})),
        )
        .await;
    assert!((200..300).contains(&s), "{v}");

    W {
        h,
        org,
        ws,
        ws2,
        conv,
        person,
        person_tok,
        opener,
        opener_tok,
        other,
        other_tok,
        admin_tok,
        member_tok,
    }
}

impl W {
    async fn call(&self, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        self.h.call_tok(tok, m, uri, body).await
    }

    async fn open(&self, tok: &str, body: Value) -> (u16, Value) {
        self.call(
            tok,
            Method::POST,
            &format!("/v1/workspaces/{}/emergency-access", self.ws),
            Some(body),
        )
        .await
    }

    /// The newest grant row (id, hours it was opened for, end reason).
    async fn row(&self) -> Option<(Uuid, f64, Option<String>)> {
        sqlx::query_as(
            "SELECT id, EXTRACT(EPOCH FROM (expires_at - opened_at))::float8 / 3600.0, end_reason \
             FROM workspace_emergency_access ORDER BY opened_at DESC, id DESC LIMIT 1",
        )
        .fetch_optional(self.h.pool())
        .await
        .unwrap()
    }

    /// The one open grant's id (the tests keep at most one).
    async fn grant_id(&self) -> Uuid {
        sqlx::query_scalar(
            "SELECT id FROM workspace_emergency_access WHERE ended_at IS NULL \
             ORDER BY opened_at DESC LIMIT 1",
        )
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }

    async fn events(&self, tok: &str) -> (u16, Value) {
        self.call(
            tok,
            Method::GET,
            &format!("/v1/conversations/{}/events", self.conv),
            None,
        )
        .await
    }
}

/// A raw request with arbitrary bytes (the multipart upload, the attachment download).
async fn raw(
    h: &Harness,
    m: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    let mut b = Request::builder().method(m).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let res = h
        .app
        .clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (p, body) = res.into_parts();
    (
        p.status.as_u16(),
        body.collect().await.unwrap().to_bytes().to_vec(),
    )
}

/// An API key row whose `Bearer <token>` authenticates (hash = sha256 of the token bytes).
async fn seed_key(h: &Harness, org: Uuid, ws: Option<Uuid>, user: Uuid, token: &str) {
    sqlx::query(
        "INSERT INTO api_keys (id, org_id, workspace_id, user_id, kind, name, token_hash, display_prefix) \
         VALUES ($1,$2,$3,$4,'user','k',sha256($5::bytea),$6)",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(ws)
    .bind(user)
    .bind(token.as_bytes())
    .bind(format!("{token}_x"))
    .execute(h.pool())
    .await
    .unwrap();
}

// ---------------------------------------------------------------- open: who, how much

#[tokio::test]
async fn open_is_owner_only_session_only_and_bounded() {
    let w = world().await;
    // A plain member and an org admin: 403 `org_owner_required`.
    for tok in [&w.member_tok, &w.admin_tok] {
        let (s, v) = w
            .open(
                tok,
                json!({"reason": "incident review of the reported messages"}),
            )
            .await;
        assert_eq!(s, 403, "{v}");
        assert_eq!(code(&v), "org_owner_required", "{v}");
    }
    // An instance admin who is not an org owner: still 403.
    let ia = w.h.user("ia@x.io", true).await;
    w.h.member(w.org, ia, "member", "active").await;
    let (_, ia_tok) = w.h.good_session(ia).await;
    let (s, v) = w
        .open(
            &ia_tok,
            json!({"reason": "incident review of the reported messages"}),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(code(&v), "org_owner_required", "{v}");

    // Any API key can never open it: 403 `session_required` (a workspace key and an `auo_`
    // org key both).
    seed_key(&w.h, w.org, Some(w.ws), w.person, "auk_wskey-token").await;
    seed_key(&w.h, w.org, None, w.opener, "auo_orgkey-token").await;
    for t in ["auk_wskey-token", "auo_orgkey-token"] {
        let (s, _, v) =
            w.h.send(
                Method::POST,
                &format!("/v1/workspaces/{}/emergency-access", w.ws),
                &[("authorization", &bearer(t))],
                Some(json!({"reason": "incident review of the reported messages"})),
            )
            .await;
        assert_eq!(s.as_u16(), 403, "{v}");
        assert_eq!(code(&v), "session_required", "{v}");
    }

    // Validation: a short reason and out-of-range hours are 422.
    let (s, v) = w.open(&w.opener_tok, json!({"reason": "too short"})).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(code(&v), "reason_invalid", "{v}");
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "incident review of the reported messages", "hours": 25}),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(code(&v), "hours_invalid", "{v}");
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "incident review of the reported messages", "hours": 0}),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(code(&v), "hours_invalid", "{v}");

    // The owner opens it: 201, the default is 4 hours, the row is active.
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "incident review of the reported messages"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["active"], json!(true), "{v}");
    assert_eq!(
        v["by"]["display_name"],
        json!("Olive Owner"),
        "metadata only, labelled by display name: {v}"
    );
    let (_, hours, end_reason) = w.row().await.unwrap();
    assert!(
        (hours - 4.0).abs() < 0.01,
        "the default is 4 hours: {hours}"
    );
    assert_eq!(end_reason, None);

    // One grant per workspace and person: a second open is 409, never an extension.
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "a second open must not extend the first", "hours": 24}),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(code(&v), "emergency_access_active", "{v}");
    let (_, hours, _) = w.row().await.unwrap();
    assert!(
        (hours - 4.0).abs() < 0.01,
        "the first grant is unchanged (no extension): {hours}"
    );

    // The 1-hour bound is accepted (for the other owner).
    let (s, v) = w
        .open(
            &w.other_tok,
            json!({"reason": "another owner opens too", "hours": 1}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
async fn a_caller_with_own_access_gains_nothing() {
    let w = world().await;
    // The org owner who also holds own `can_own` in the workspace: 409 `already_has_access`.
    w.h.bind_ws(w.ws, w.opener, "owner").await;
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "they already own this workspace"}),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(code(&v), "already_has_access", "{v}");
    assert!(w.row().await.is_none(), "no grant row was written");
}

#[tokio::test]
async fn keys_never_use_a_grant() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "keys cannot ride along"}))
        .await;
    assert_eq!(s, 201, "{v}");
    // The opener's `auo_` org key and the person's workspace key read only with their own
    // access: the grant never widens a key (C-1165: it is a session's exception).
    seed_key(&w.h, w.org, None, w.opener, "auo_orgkey-token").await;
    seed_key(&w.h, w.org, Some(w.ws), w.person, "auk_wskey-token").await;
    // The org key is confined to no workspace and holds no own access: refused.
    let (s, _, v) =
        w.h.send(
            Method::GET,
            &format!("/v1/conversations/{}/events", w.conv),
            &[("authorization", &bearer("auo_orgkey-token"))],
            None,
        )
        .await;
    assert!(
        matches!(s.as_u16(), 403 | 404),
        "the org key cannot use the grant: {s} {v}"
    );
    // The workspace key keeps exactly its own key authority: it reads its own workspace (its
    // own access), which proves the route answered on authority, not on the grant.
    let (s, _, v) =
        w.h.send(
            Method::GET,
            &format!("/v1/conversations/{}/events", w.conv),
            &[("authorization", &bearer("auk_wskey-token"))],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "the key's own access still works: {v}");
}

// ---------------------------------------------------------------- announced

#[tokio::test]
async fn opening_notifies_the_people_and_the_other_owners_with_the_banner_set() {
    let mock = MockPush::start().await;
    let w = world_with(|c| {
        c.push.enabled = true;
        c.push.vapid.subject = Some(SUBJECT.into());
        c.push.allowed_endpoint_hosts = vec![HOST.into(), "*.push.services.mozilla.com".into()];
        c.push
            .dev_endpoint_overrides
            .insert(HOST.to_owned(), mock.url());
    })
    .await;
    let key = aurora_api::domains::push::vapid::active(&w.h.state)
        .await
        .unwrap()
        .1
        .public_b64u();
    // Subscriptions for the person and the other owner; the opener registers none.
    for (path, tok) in [
        ("/push/person", &w.person_tok),
        ("/push/owner", &w.other_tok),
    ] {
        let k = mock.subscribe(path);
        let key = key.clone();
        let (s, _, v) =
            w.h.send(
                Method::POST,
                "/v1/me/push-subscriptions",
                &[("cookie", cookie(tok).as_str()), ("origin", ORIGIN)],
                Some(json!({
                    "endpoint": format!("https://{HOST}{path}"),
                    "keys": {"p256dh": k.p256dh(), "auth": k.auth()},
                    "label": "Laptop",
                    "show_details": false,
                    "vapid_public_key": key,
                })),
            )
            .await;
        assert!((200..300).contains(&s.as_u16()), "{v}");
    }

    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "the reported incident needs the transcript"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    // E-mail: to the workspace's person and the other owner; never the opener. The body names
    // people by display name and carries the reason.
    let mail = w.h.mail.sent();
    let person_email: String = sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1")
        .bind(w.person)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let other_email: String = sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1")
        .bind(w.other)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let mut tos: Vec<String> = mail.iter().map(|m| m.to.clone()).collect();
    tos.sort();
    let mut expected = vec![person_email.clone(), other_email.clone()];
    expected.sort();
    assert_eq!(
        tos, expected,
        "the person and the other owner, not the opener"
    );
    for m in &mail {
        assert_eq!(m.subject, "Emergency read access to w", "{}", m.subject);
        assert!(
            m.body.contains("Olive Owner") && m.body.contains("the reported incident"),
            "the body names the opener and the reason: {}",
            m.body
        );
        assert!(
            !m.body.contains(&person_email) && !m.body.contains(&other_email),
            "no e-mail address as a label: {}",
            m.body
        );
    }

    // Web Push: one per registered device of the recipients, never to the opener's path.
    let got = mock.received();
    let mut paths: Vec<&str> = got.iter().map(|r| r.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, vec!["/push/owner", "/push/person"], "no opener push");

    // The banner fields on the workspace (C-1165): the person sees who opened it, why and
    // until; `emergency_reader` is true only for the opener.
    let (s, v) = w
        .call(
            &w.person_tok,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["audience"]["emergency"]["id"],
        json!(grant.to_string()),
        "{v}"
    );
    assert_eq!(
        v["audience"]["emergency"]["by"]["display_name"],
        json!("Olive Owner"),
        "{v}"
    );
    assert_eq!(
        v["audience"]["emergency"]["reason"],
        json!("the reported incident needs the transcript"),
        "{v}"
    );
    assert_eq!(v["audience"]["emergency"]["active"], json!(true), "{v}");
    assert_eq!(v["emergency_reader"], json!(false), "{v}");
    let (s, v) = w
        .call(
            &w.opener_tok,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["emergency_reader"], json!(true), "{v}");
}

#[tokio::test]
async fn a_personal_workspace_tells_its_person() {
    let w = world().await;
    // The person's own personal workspace: the org owner may open it, and its person is told.
    // The org's other active owner is told too (C-1165: the workspace's people *and* every
    // other org owner — the personal workspace changes the former, not the latter).
    let personal = w.h.workspace_owned_by(w.org, w.person).await;
    let (s, v) =
        w.h.call_tok(
            &w.opener_tok,
            Method::POST,
            &format!("/v1/workspaces/{personal}/emergency-access"),
            Some(json!({"reason": "the personal workspace incident"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let mail = w.h.mail.sent();
    let mut tos: Vec<&str> = mail.iter().map(|m| m.to.as_str()).collect();
    tos.sort_unstable();
    assert_eq!(
        tos,
        vec!["oo@x.io", "p@x.io"],
        "the person and the other owner, not the opener: {:?}",
        mail.iter().map(|m| &m.to).collect::<Vec<_>>()
    );
    let person_mail = mail.iter().find(|m| m.to == "p@x.io").unwrap();
    // The body names the opener by display name, never an e-mail address.
    assert!(
        person_mail.body.contains("Olive Owner"),
        "{}",
        person_mail.body
    );
    assert!(!person_mail.body.contains('@'), "{}", person_mail.body);
}

/// (Review HIGH-1) The people's banner shows ACTIVE access whenever any grant is active: an
/// owner's open grant may not be hidden behind another owner's newer, already-ended one, and two
/// simultaneous active grants are both announced (the newest active one by name, the other by
/// count).
#[tokio::test]
async fn the_peoples_banner_shows_active_access_even_when_a_newer_grant_ended() {
    let w = world().await;
    // Owner A opens for 24 hours.
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "owner A opens for a long day"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let a_grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // Owner B opens after A and ends theirs at once.
    let (s, v) = w
        .open(&w.other_tok, json!({"reason": "owner B opens and ends"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let b_grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = w
        .call(
            &w.other_tok,
            Method::POST,
            &format!("/v1/emergency-access/{b_grant}/end"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // The banner is A's ACTIVE grant, not B's newer ended one.
    let (s, v) = w
        .call(
            &w.person_tok,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["audience"]["emergency"]["id"], json!(a_grant), "{v}");
    assert_eq!(v["audience"]["emergency"]["active"], json!(true), "{v}");
    assert_eq!(v["audience"]["emergency_active_more"], json!(0), "{v}");

    // A third owner opens too: both active grants are announced — the newest active by name,
    // the other by count.
    let third = w.h.user("third@x.io", false).await;
    w.h.member(w.org, third, "owner", "active").await;
    let (_, third_tok) = w.h.good_session(third).await;
    let (s, v) = w
        .open(&third_tok, json!({"reason": "owner C opens after A"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let c_grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = w
        .call(
            &w.person_tok,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["audience"]["emergency"]["id"], json!(c_grant), "{v}");
    assert_eq!(v["audience"]["emergency"]["active"], json!(true), "{v}");
    assert_eq!(v["audience"]["emergency_active_more"], json!(1), "{v}");
}

// ---------------------------------------------------------------- recorded

#[tokio::test]
async fn every_content_read_is_recorded_before_the_body() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "reading the transcript"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    // The transcript read works and records the view before the body.
    let (s, v) = w.events(&w.opener_tok).await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v.to_string().contains(MARKER),
        "the emergency reader reads the direct conversation: {v}"
    );
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT (metadata->>'emergency_access')::uuid, target_id::uuid FROM audit_log \
         WHERE action = 'conversation.emergency_view'",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "one row");
    assert_eq!(rows[0].0, grant, "the row names the grant");
    assert_eq!(rows[0].1, w.conv, "the row names the conversation");

    // The same hour's other reads (events again, the export) deduplicate onto it.
    let (s, _) = w.events(&w.opener_tok).await;
    assert_eq!(s, 200);
    let (s, v) = w
        .call(
            &w.opener_tok,
            Method::GET,
            &format!("/v1/conversations/{}/export", w.conv),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "deduplicated once per actor, conversation and hour");
    let exp: (String,) = sqlx::query_as(
        "SELECT metadata::text FROM audit_log WHERE action = 'conversation.export' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(
        exp.0.contains("\"via\": \"emergency\""),
        "the export records via: emergency: {}",
        exp.0
    );

    // An attachment the person uploaded downloads for the emergency reader (and records).
    let part = "hello attachment";
    let (ct, body) = multipart("file", "a.txt", "text/plain", part.as_bytes());
    let (s, bytes) = raw(
        &w.h,
        Method::POST,
        &format!("/v1/conversations/{}/attachments", w.conv),
        &[
            ("cookie", cookie(&w.person_tok).as_str()),
            ("origin", ORIGIN),
            ("content-type", ct.as_str()),
        ],
        body,
    )
    .await;
    assert_eq!(s, 201, "the upload: {}", String::from_utf8_lossy(&bytes));
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let att: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, bytes) = raw(
        &w.h,
        Method::GET,
        &format!("/v1/attachments/{att}/content"),
        &[
            ("cookie", cookie(&w.opener_tok).as_str()),
            ("origin", ORIGIN),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(s, 200, "the emergency reader downloads the attachment");
    assert_eq!(bytes, part.as_bytes(), "the bytes are the attachment's");

    // The recording is not best-effort: a failed audit write refuses the read with 503 and
    // returns no content.
    sqlx::query(
        "CREATE FUNCTION audit_critical_fail() RETURNS trigger AS $$ BEGIN \
         RAISE EXCEPTION 'audit down'; END $$ LANGUAGE plpgsql",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER audit_critical_fail BEFORE INSERT ON audit_log FOR EACH ROW \
         WHEN (NEW.critical) EXECUTE FUNCTION audit_critical_fail()",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    // Move the hour window so the next read needs a fresh row (the dedupe would otherwise skip
    // the insert), then read: refused, no content.
    sqlx::query(
        "UPDATE audit_log SET created_at = now() - interval '2 hours' \
         WHERE action = 'conversation.emergency_view'",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    let (s, v) = w.events(&w.opener_tok).await;
    assert_eq!(s, 503, "{v}");
    assert!(!v.to_string().contains(MARKER), "no content: {v}");
    // The grant itself survived (the refusal is the read's, not the grant's).
    assert!(w.grant_id().await != Uuid::nil());

    // With the fault gone the same read succeeds and records.
    sqlx::query("DROP TRIGGER audit_critical_fail ON audit_log")
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION audit_critical_fail")
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w.events(&w.opener_tok).await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 2, "the new hour's row");
}

fn multipart(part: &str, filename: &str, ctype: &str, data: &[u8]) -> (String, Vec<u8>) {
    let b = "XBOUNDARYX";
    let mut body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"{part}\"; filename=\"{filename}\"\r\nContent-Type: {ctype}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={b}"), body)
}

#[tokio::test]
async fn the_metadata_responses_carry_no_content() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "metadata only responses"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // The open response never carries conversation content.
    assert!(!v.to_string().contains(MARKER), "{v}");
    // The list (own access, here the person's) and the end response: metadata only.
    let (s, v) = w
        .call(
            &w.person_tok,
            Method::GET,
            &format!("/v1/workspaces/{}/emergency-access", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(!v.to_string().contains(MARKER), "{v}");
    let (s, v) = w
        .call(
            &w.opener_tok,
            Method::POST,
            &format!("/v1/emergency-access/{grant}/end"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(!v.to_string().contains(MARKER), "{v}");
    assert_eq!(v["end_reason"], json!("ended"), "{v}");
}

// ---------------------------------------------------------------- one workspace

#[tokio::test]
async fn the_grant_covers_one_workspace_only() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "only this workspace"}))
        .await;
    assert_eq!(s, 201, "{v}");
    // Another workspace of the same org stays closed to the reader: no conversation read, no
    // agent memory list. (The workspace GET itself still answers — an org owner sees that
    // metadata anyway — but it must not name the caller an emergency reader there.)
    let agent2 = w.h.byo_agent(w.org, w.ws2, "bot2", "Bot2").await;
    let (s, v) = w
        .call(
            &w.person_tok,
            Method::POST,
            &format!("/v1/agents/{agent2}/conversations/direct"),
            None,
        )
        .await;
    assert!((200..300).contains(&s), "{v}");
    let conv2: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    for uri in [
        format!("/v1/conversations/{conv2}"),
        format!("/v1/agents/{agent2}/memories"),
    ] {
        let (s, v) = w.call(&w.opener_tok, Method::GET, &uri, None).await;
        assert!(
            matches!(s, 403 | 404),
            "outside the granted workspace: {s} {v}"
        );
    }
    let (s, v) = w
        .call(
            &w.opener_tok,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws2),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["emergency_reader"], json!(false), "{v}");
    assert_eq!(v["audience"]["emergency"], json!(null), "{v}");
    // And the granted workspace still answers.
    let (s, _) = w.events(&w.opener_tok).await;
    assert_eq!(s, 200);
}

// ---------------------------------------------------------------- expiry and ending

#[tokio::test]
async fn an_expired_grant_refuses_at_use_before_any_sweep() {
    let w = world().await;
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "expires between check and read"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // The clock moves past `expires_at` (the sweep has not run). `opened_at` moves too, so the
    // row keeps satisfying the window check (expires_at > opened_at).
    sqlx::query(
        "UPDATE workspace_emergency_access \
         SET opened_at = now() - interval '4 hours', expires_at = now() - interval '1 second'",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    let (s, v) = w.events(&w.opener_tok).await;
    assert!(
        matches!(s, 403 | 404),
        "expired at use, no sweep needed: {s} {v}"
    );
    let (s, v) = w
        .call(
            &w.opener_tok,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["emergency_reader"], json!(false), "{v}");
    assert_eq!(v["audience"]["emergency"]["active"], json!(false), "{v}");
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_emergency_access \
         WHERE ended_at IS NULL AND expires_at > now()",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(active, 0, "the grant is no longer active");
}

#[tokio::test]
async fn ending_is_owner_only_idempotent_and_critical() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "ending it early"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // A member and an org admin are refused (403 `org_owner_required`).
    for tok in [&w.member_tok, &w.admin_tok] {
        let (s, v) = w
            .call(
                tok,
                Method::POST,
                &format!("/v1/emergency-access/{grant}/end"),
                None,
            )
            .await;
        assert_eq!(s, 403, "{v}");
        assert_eq!(code(&v), "org_owner_required", "{v}");
    }
    // The other owner ends it: 200 `ended`, the critical audit row written.
    let (s, v) = w
        .call(
            &w.other_tok,
            Method::POST,
            &format!("/v1/emergency-access/{grant}/end"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["end_reason"], json!("ended"), "{v}");
    assert_eq!(v["active"], json!(false), "{v}");
    let critical: bool = sqlx::query_scalar(
        "SELECT critical FROM audit_log WHERE action = 'workspace.emergency_access.end'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(critical, "the end row is security-critical");
    // Idempotent: a second end answers 200 with the row and writes no new audit row.
    let (s, v) = w
        .call(
            &w.other_tok,
            Method::POST,
            &format!("/v1/emergency-access/{grant}/end"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'workspace.emergency_access.end'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "no second critical row");
    // Reads refuse now.
    let (s, _) = w.events(&w.opener_tok).await;
    assert!(matches!(s, 403 | 404), "after the end: {s}");
    // A new grant may be opened (never extended: a fresh one), and it notifies again.
    let before = w.h.mail.sent().len();
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "a fresh grant after the end"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert!(
        w.h.mail.sent().len() > before,
        "the new grant notifies again"
    );
}

#[tokio::test]
async fn losing_the_owner_role_ends_the_grant_with_access_lost() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "role loss ends it"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    // Lowered to member, in the org member PATCH: the grant ends in the same transaction.
    let (s, v) = w
        .call(
            &w.other_tok,
            Method::PATCH,
            &format!("/v1/orgs/{}/members/{}", w.org, w.opener),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let row: (Option<String>,) =
        sqlx::query_as("SELECT end_reason FROM workspace_emergency_access WHERE id = $1")
            .bind(grant)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        row.0.as_deref(),
        Some("access_lost"),
        "ended with access_lost"
    );
    let (s, _) = w.events(&w.opener_tok).await;
    assert!(matches!(s, 403 | 404), "the read refuses: {s}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'workspace.emergency_access.end' \
         AND metadata->>'end_reason' = 'access_lost'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(n >= 1, "the access-lost audit row");

    // Suspended: a fresh grant ends the same way.
    let (s, v) = w
        .call(
            &w.other_tok,
            Method::PATCH,
            &format!("/v1/orgs/{}/members/{}", w.org, w.opener),
            Some(json!({"role": "owner"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "suspension ends it too"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let grant2: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = w
        .call(
            &w.other_tok,
            Method::PATCH,
            &format!("/v1/orgs/{}/members/{}", w.org, w.opener),
            Some(json!({"status": "suspended"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let reason: Option<String> =
        sqlx::query_scalar("SELECT end_reason FROM workspace_emergency_access WHERE id = $1")
            .bind(grant2)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(reason.as_deref(), Some("access_lost"));

    // Removed: the same.
    let (s, v) = w
        .call(
            &w.other_tok,
            Method::PATCH,
            &format!("/v1/orgs/{}/members/{}", w.org, w.opener),
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "removal ends it too"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let grant3: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = w
        .call(
            &w.other_tok,
            Method::DELETE,
            &format!("/v1/orgs/{}/members/{}", w.org, w.opener),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let reason: Option<String> =
        sqlx::query_scalar("SELECT end_reason FROM workspace_emergency_access WHERE id = $1")
            .bind(grant3)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(reason.as_deref(), Some("access_lost"));
}

// ---------------------------------------------------------------- races and the index

#[tokio::test]
async fn two_owners_opening_at_once_each_get_their_grant() {
    let w = world().await;
    let (a, b) = tokio::join!(
        w.open(&w.opener_tok, json!({"reason": "two owners at once A"})),
        w.open(&w.other_tok, json!({"reason": "two owners at once B"})),
    );
    assert_eq!(a.0, 201, "{}", a.1);
    assert_eq!(b.0, 201, "{}", b.1);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_emergency_access WHERE ended_at IS NULL",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 2, "one per owner; the index is per (workspace, person)");
}

#[tokio::test]
async fn the_partial_unique_index_refuses_a_second_active_row() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the index backs the 409"}))
        .await;
    assert_eq!(s, 201, "{v}");
    // A second **active** row for the same workspace and person is impossible at the index,
    // not only at the route.
    let dup = sqlx::query(
        "INSERT INTO workspace_emergency_access \
         (id, org_id, workspace_id, user_id, reason, opened_at, expires_at) \
         VALUES ($1,$2,$3,$4,'index test', now(), now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(w.org)
    .bind(w.ws)
    .bind(w.opener)
    .execute(w.h.pool())
    .await;
    assert!(dup.is_err(), "the partial unique index refused the insert");
    // The list still answers (metadata only) for a caller with own access.
    let (s, v) = w
        .call(
            &w.person_tok,
            Method::GET,
            &format!("/v1/workspaces/{}/emergency-access", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

// ---------------------------------------------------------------- the sweep

#[tokio::test]
async fn the_sweep_expires_due_grants_and_prunes_past_retention() {
    let w = world().await;
    // One grant already past its expiry, one ended 401 days ago (past retention).
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "the sweep expires this one"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let due: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    sqlx::query(
        "UPDATE workspace_emergency_access \
         SET opened_at = now() - interval '4 hours', expires_at = now() - interval '1 minute' \
         WHERE id = $1",
    )
    .bind(due)
    .execute(w.h.pool())
    .await
    .unwrap();
    let old: Uuid = sqlx::query_scalar(
        "INSERT INTO workspace_emergency_access \
         (id, org_id, workspace_id, user_id, reason, opened_at, expires_at, ended_at, end_reason) \
         VALUES ($1,$2,$3,$4,'a row long past retention', now() - interval '401 days', \
                 now() - interval '400 days', now() - interval '400 days', 'ended') RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(w.org)
    .bind(w.ws)
    .bind(w.opener)
    .fetch_one(w.h.pool())
    .await
    .unwrap();

    let ended = aurora_api::domains::emergency_access::service::expire_due(&w.h.state)
        .await
        .unwrap();
    assert_eq!(ended, 1, "exactly the due grant");
    let reason: Option<String> =
        sqlx::query_scalar("SELECT end_reason FROM workspace_emergency_access WHERE id = $1")
            .bind(due)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(reason.as_deref(), Some("expired"));
    let critical: bool = sqlx::query_scalar(
        "SELECT critical FROM audit_log WHERE action = 'workspace.emergency_access.expire' \
         AND target_id = $1",
    )
    .bind(due.to_string())
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(critical, "the expiry row is security-critical");

    // The retention prune: rows past 400 days go, the rest stay.
    let pruned = aurora_db::repo::emergency_access::prune(w.h.pool(), 400)
        .await
        .unwrap();
    assert_eq!(pruned, 1, "the 401-day row");
    let gone: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM workspace_emergency_access WHERE id = $1")
            .bind(old)
            .fetch_optional(w.h.pool())
            .await
            .unwrap();
    assert!(gone.is_none(), "the old row is deleted");
}
