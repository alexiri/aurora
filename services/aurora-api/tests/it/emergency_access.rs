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
    // people by display name and carries the reason. The notices come from the spawned task
    // (review MEDIUM-6): poll instead of assuming they landed before the response returned.
    until_notices(&w.h, 2).await;
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
    let got = mock.wait_for(2, std::time::Duration::from_secs(5)).await;
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

/// (Review HIGH-3) A caller who holds own access AND an active grant is an emergency reader too
/// (their conversation reads ARE recorded as emergency views): the workspace answer says so in
/// BOTH branches — with own access and with a grant only — and carries the caller's OWN active
/// grant id, the one their reader banner's "End now" ends (never another owner's newest grant).
#[tokio::test]
async fn a_reader_with_own_access_gets_the_reader_banner_and_their_own_grant_id() {
    let w = world().await;
    // An org owner who is also a workspace member: own access, not can_own, so a grant is allowed.
    let both = w.h.user("both@x.io", false).await;
    w.h.member(w.org, both, "owner", "active").await;
    w.h.bind_ws(w.ws, both, "member").await;
    let (_, both_tok) = w.h.good_session(both).await;

    // Without a grant: not an emergency reader, no id.
    let (s, v) = w
        .call(
            &both_tok,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["emergency_reader"], json!(false), "{v}");
    assert!(v["emergency_reader_id"].is_null(), "{v}");

    // They open a grant: the own-access branch now answers emergency_reader with THEIR grant id.
    let (s, v) = w
        .open(&both_tok, json!({"reason": "the member-owner grant"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = w
        .call(
            &both_tok,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["emergency_reader"], json!(true), "{v}");
    assert_eq!(v["emergency_reader_id"], json!(grant), "{v}");

    // The grant-only branch (no own access): the same shape, with that reader's own grant id.
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the grant-only reader"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let only: Uuid = v["id"].as_str().unwrap().parse().unwrap();
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
    assert_eq!(v["emergency_reader_id"], json!(only), "{v}");
    // Each reader sees their own grant's id, never one id for everyone.
    assert_ne!(grant, only);

    // And the id is the one "End now" ends: the member-owner ends their own grant by it.
    let (s, v) = w
        .call(
            &both_tok,
            Method::POST,
            &format!("/v1/emergency-access/{grant}/end"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
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
    let mail = {
        until_notices(&w.h, 2).await;
        w.h.mail.sent()
    };
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

/// The notices go out from a spawned task after the commit (review MEDIUM-6): poll briefly
/// instead of assuming they landed before the response returned.
async fn until_notices(h: &Harness, want: usize) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while h.mail.sent().len() < want {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the notices never arrived"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// (Review MEDIUM-6) The notices go out from a spawned task after the commit: the open answers
/// 201 at once, and a failing recipient (the push service answering 500) does not stop the
/// others — both pushes are attempted and every e-mail still arrives.
#[tokio::test]
async fn a_failing_recipient_does_not_stop_the_others_and_the_open_still_returns_201() {
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
    // The push service is down: every push send fails.
    mock.respond(aurora_testkit::push::Answer::status(500));

    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the push service is down"}))
        .await;
    assert_eq!(s, 201, "{v}: the grant stands, the notices are logged");

    // Both pushes were ATTEMPTED — one failing recipient did not stop the other (the failing
    // one may be retried, so wait for both paths, not a fixed count).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let got = loop {
        let g = mock.received();
        let attempted = |p: &str| g.iter().any(|r| r.path == p);
        if attempted("/push/owner") && attempted("/push/person") {
            break g;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "both recipients attempted: {g:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(got.len() >= 2);
    // And the e-mails (a different channel) all arrived.
    until_notices(&w.h, 2).await;
    let mail = w.h.mail.sent();
    let tos: Vec<&str> = mail.iter().map(|m| m.to.as_str()).collect();
    assert!(
        tos.contains(&"p@x.io") && tos.contains(&"oo@x.io"),
        "{tos:?}"
    );
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
    // The export's audit row is best-effort (`audit::record` spawns it): poll briefly instead
    // of assuming it landed before the response returned (as `until_notices` does).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let exp: (String,) = loop {
        match sqlx::query_as::<_, (String,)>(
            "SELECT metadata::text FROM audit_log WHERE action = 'conversation.export' \
             ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(w.h.pool())
        .await
        {
            Ok(row) => break row,
            Err(sqlx::Error::RowNotFound) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the export audit row never arrived"
                );
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(e) => panic!("reading the export audit row failed: {e}"),
        }
    };
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

    // (Review LOW-8) The same refusal on the attachment download: 503 `audit_unavailable`,
    // never disguised as a missing attachment (404) — and the bytes never leave.
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
    assert_eq!(
        s,
        503,
        "the audit refusal is not a 404: {}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(
        !String::from_utf8_lossy(&bytes).contains("hello attachment"),
        "no bytes: {}",
        String::from_utf8_lossy(&bytes)
    );

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

/// (Review LOW-10) Reopening within the 60 s between `expires_at` passing and the minute sweep:
/// the caller's own expired-but-unswept row would trip the partial unique index with a generic
/// error — the open ends it (the same shape the sweep gives it) and answers 201.
#[tokio::test]
async fn reopening_after_expiry_within_the_sweep_window_is_201_not_a_conflict() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the first, soon expired"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let first: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // Wind `expires_at` back: use already refuses it, but the sweep has not ended the row yet
    // (`opened_at` moves too: the window CHECK needs expires_at > opened_at).
    sqlx::query("UPDATE workspace_emergency_access \
                 SET opened_at = now() - interval '1 hour', expires_at = now() - interval '1 second' \
                 WHERE id = $1")
        .bind(first)
        .execute(w.h.pool())
        .await
        .unwrap();

    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the reopen in the window"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let second: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert_ne!(first, second);

    // The stale row got the sweep's shape (ended at its expiry, reason `expired`), the new one
    // is the only active grant.
    let (ended, reason): (Option<chrono::DateTime<chrono::Utc>>, Option<String>) =
        sqlx::query_as("SELECT ended_at, end_reason FROM workspace_emergency_access WHERE id = $1")
            .bind(first)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(reason.as_deref(), Some("expired"), "{v}");
    assert!(ended.is_some());
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_emergency_access \
         WHERE workspace_id = $1 AND user_id = $2 AND ended_at IS NULL",
    )
    .bind(w.ws)
    .bind(w.opener)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "exactly one active grant");
}

/// (Review LOW-12) The conversation LIST carries conversation content (each row's active turn's
/// text, and for direct rows who else takes part): the emergency reader's views for the listed
/// conversations are recorded before the body, and a failed audit write refuses the read with
/// 503 — the route was already a C-1165 read; only the recording was missing. (The workspace
/// activity list is metadata only: no message text or preview, nothing to record.)
#[tokio::test]
async fn the_conversation_list_records_the_emergency_views_it_carries() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the list recording"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let agent: Uuid = sqlx::query_scalar("SELECT agent_id FROM conversations WHERE id = $1")
        .bind(w.conv)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    // The world's direct conversation carries an active turn (its text rides on the list row);
    // the list must record the view for the conversation it lists.

    let (s, v) =
        w.h.call_tok(
            &w.opener_tok,
            Method::GET,
            &format!("/v1/agents/{agent}/conversations"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v.to_string().contains(MARKER),
        "the list carries the active turn's text: {v}"
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view' \
         AND target_id = $1",
    )
    .bind(w.conv.to_string())
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "the listed conversation's view is recorded");

    // The recording is not best-effort: drop the row so the dedup does not skip the write, fail
    // the write, and the list refuses with 503 carrying no content.
    sqlx::query("DELETE FROM audit_log WHERE action = 'conversation.emergency_view'")
        .execute(w.h.pool())
        .await
        .unwrap();
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
    let (s, v) =
        w.h.call_tok(
            &w.opener_tok,
            Method::GET,
            &format!("/v1/agents/{agent}/conversations"),
            None,
        )
        .await;
    assert_eq!(s, 503, "{v}");
    assert!(!v.to_string().contains(MARKER), "no content: {v}");
    sqlx::query("DROP TRIGGER audit_critical_fail ON audit_log")
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION audit_critical_fail")
        .execute(w.h.pool())
        .await
        .unwrap();
}

/// (Review MEDIUM-4) The dedup is per grant: after end-and-reopen within the hour, reads under
/// the NEW grant record again, attributed to the new grant — never silently skipped or
/// attributed to the old one.
#[tokio::test]
async fn reads_under_a_new_grant_record_again_even_within_the_hour() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "grant A of the hour"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let a: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = w.events(&w.opener_tok).await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view' \
         AND metadata->>'emergency_access' = $1",
    )
    .bind(a.to_string())
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "one row under grant A");

    // End A and reopen as B within the same hour: the next read records under B.
    let (s, v) = w
        .call(
            &w.opener_tok,
            Method::POST,
            &format!("/v1/emergency-access/{a}/end"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "grant B, same hour"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let b: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = w.events(&w.opener_tok).await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view' \
         AND metadata->>'emergency_access' = $1",
    )
    .bind(b.to_string())
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "a fresh row under grant B, not the deduped old one");
}

/// (Review MEDIUM-5) Losing ownership in ONE organization ends only that organization's grants:
/// `end_lost_for_user` is scoped by org, and the audit rows stay in the right org.
#[tokio::test]
async fn losing_ownership_in_one_org_ends_only_that_orgs_grants() {
    let w = world().await;
    // The opener is an owner of a second organization too, and opened a grant there as well.
    let org2 = w.h.org().await;
    let ws_in2 = w.h.workspace(org2).await;
    w.h.member(org2, w.opener, "owner", "active").await;
    w.h.member(org2, w.other, "owner", "active").await;
    let (s, v) =
        w.h.call_tok(
            &w.opener_tok,
            Method::POST,
            &format!("/v1/workspaces/{ws_in2}/emergency-access"),
            Some(json!({"reason": "the second org's incident"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // And a grant in the first org.
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the first org's incident"}))
        .await;
    assert_eq!(s, 201, "{v}");

    // The second org lowers the opener to member: only that org's grant ends.
    let (s, v) =
        w.h.call_tok(
            &w.other_tok,
            Method::PATCH,
            &format!("/v1/orgs/{org2}/members/{}", w.opener),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    let (org2_lost, org2_reason): (Option<bool>, Option<String>) = sqlx::query_as(
        "SELECT ended_at IS NOT NULL, end_reason FROM workspace_emergency_access \
         WHERE workspace_id = $1",
    )
    .bind(ws_in2)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(org2_lost, Some(true), "the second org's grant ended");
    assert_eq!(org2_reason.as_deref(), Some("access_lost"));
    let (org1_lost, org1_reason): (Option<bool>, Option<String>) = sqlx::query_as(
        "SELECT ended_at IS NOT NULL, end_reason FROM workspace_emergency_access \
         WHERE workspace_id = $1",
    )
    .bind(w.ws)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(org1_lost, Some(false), "the first org's grant is untouched");
    assert_eq!(org1_reason, None);
}

/// (Review MEDIUM-7) The notices go to the **effective** owners — the same
/// `effective_org_members` view the open check uses. A group-derived admin (the strongest a
/// group may map, SEC-71) is not an owner and is not told; the direct owners are.
#[tokio::test]
async fn notices_go_to_the_effective_owners_not_group_admins() {
    let w = world().await;
    // A member who is an admin through a group (no API route maps it; the view computes it).
    // Deliberately no own workspace access: only the owner branch could ever tell them.
    let grouped = w.h.user("grouped@x.io", false).await;
    w.h.member(w.org, grouped, "member", "active").await;
    let g = Uuid::now_v7();
    sqlx::query("INSERT INTO groups (id, org_id, name, org_role) VALUES ($1,$2,'G','admin')")
        .bind(g)
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_members (group_id, org_id, user_id) VALUES ($1,$2,$3)")
        .bind(g)
        .bind(w.org)
        .bind(grouped)
        .execute(w.h.pool())
        .await
        .unwrap();
    let role: String = sqlx::query_scalar(
        "SELECT role FROM effective_org_members WHERE org_id = $1 AND user_id = $2",
    )
    .bind(w.org)
    .bind(grouped)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(role, "admin", "the view derives the group's admin role");

    // The open tells the person and the other owner — not the group admin.
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "the effective owners notice"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    until_notices(&w.h, 2).await;
    let mail = w.h.mail.sent();
    let tos: Vec<&str> = mail.iter().map(|m| m.to.as_str()).collect();
    assert!(
        tos.contains(&"oo@x.io") && tos.contains(&"p@x.io") && !tos.contains(&"grouped@x.io"),
        "owner-only: {tos:?}"
    );
}

/// (Review MEDIUM-4) A live conversation stream keeps recording: when the hour window rolls
/// over, the resync tick writes a fresh `emergency_view` row — 24 hours of live reading leave
/// more than the one row from the open.
#[tokio::test]
async fn a_live_conversation_stream_records_again_when_the_dedup_window_rolls_over() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the live stream"}))
        .await;
    assert_eq!(s, 201, "{v}");
    // A short resync tick for the test (production: 10–15 s).
    let app = test_router(
        w.h.state
            .clone()
            .with_stream_recheck(std::time::Duration::from_millis(250)),
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/conversations/{}/stream", w.conv))
                .header("cookie", cookie(&w.opener_tok))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let mut body = Body::new(res.into_body());

    // The open recorded the view.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view'",
        )
        .fetch_one(w.h.pool())
        .await
        .unwrap();
        if n >= 1 || tokio::time::Instant::now() >= deadline {
            assert!(n >= 1, "the open recorded the view");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Wind the window back: the next tick records a fresh row while the stream stays open.
    sqlx::query(
        "UPDATE audit_log SET created_at = now() - interval '2 hours' \
         WHERE action = 'conversation.emergency_view'",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut frames = 0;
    loop {
        match tokio::time::timeout_at(deadline, body.frame()).await {
            Ok(Some(Ok(_))) => frames += 1,
            _ => break,
        }
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view' \
             AND created_at > now() - interval '1 hour'",
        )
        .fetch_one(w.h.pool())
        .await
        .unwrap();
        if n >= 1 {
            break;
        }
    }
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view' \
         AND created_at > now() - interval '1 hour'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "the tick recorded a fresh row");
    assert!(frames > 0, "the stream stayed open and flowing");
}

/// (Verify LOW-3) When the dedup hour rolls over on an open stream, content does not flow
/// before the fresh `emergency_view` row is written: the stream checks the window before each
/// content frame and records first. The resync tick is set past the test's length, so only the
/// per-frame gate can record — a delta pushed right after the rollover arrives only once the
/// row exists.
#[tokio::test]
async fn the_stream_records_before_content_when_the_dedup_window_rolls_over() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the gated stream"}))
        .await;
    assert_eq!(s, 201, "{v}");
    // A resync tick slower than the test: only the per-frame gate can record after the rollover.
    let app = test_router(
        w.h.state
            .clone()
            .with_stream_recheck(std::time::Duration::from_secs(60)),
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/conversations/{}/stream", w.conv))
                .header("cookie", cookie(&w.opener_tok))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let mut body = Body::new(res.into_body());

    // The stream's initial read recorded the view.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view'",
        )
        .fetch_one(w.h.pool())
        .await
        .unwrap();
        if n >= 1 || tokio::time::Instant::now() >= deadline {
            assert!(n >= 1, "the stream's initial read recorded the view");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Roll the window over, then push an ephemeral content delta at once. The sleep lets the
    // stream's IMMEDIATE first tick (tokio intervals fire at once) pass while the window is
    // still fresh; after the wind the next tick is 15 s away, so only the per-frame gate can
    // record during the test.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    sqlx::query(
        "UPDATE audit_log SET created_at = now() - interval '2 hours' \
         WHERE action = 'conversation.emergency_view'",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    let turn = Uuid::now_v7();
    w.h.state
        .conversation_bus
        .publish_delta(
            w.h.pool(),
            w.conv,
            turn,
            "text.delta",
            (Some("m1"), None),
            "the gated delta",
        )
        .await;

    // The first content frame must carry the fresh row with it: recorded BEFORE the frame.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut bytes = Vec::new();
    loop {
        match tokio::time::timeout_at(deadline, body.frame()).await {
            Ok(Some(Ok(f))) => {
                if let Ok(d) = f.into_data() {
                    bytes.extend_from_slice(&d);
                }
            }
            _ => break,
        }
        if std::str::from_utf8(&bytes)
            .unwrap_or("")
            .contains("text.delta")
        {
            break;
        }
    }
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap_or("")
            .contains("text.delta"),
        "the delta arrived: {}",
        String::from_utf8_lossy(&bytes)
    );
    let rows: Vec<(chrono::DateTime<chrono::Utc>, String)> = sqlx::query_as(
        "SELECT created_at, coalesce(metadata->>'emergency_access','?') FROM audit_log \
         WHERE action = 'conversation.emergency_view' ORDER BY id",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    eprintln!("DEBUG view rows: {rows:?}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'conversation.emergency_view' \
         AND created_at > now() - interval '1 hour'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "the fresh row was written before the content frame");
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

/// (Verify LOW-2a) With two grants active the reader banner must show the caller's OWN expiry,
/// not the newest grant's (`audience.emergency` may belong to another owner). The workspace
/// answer carries the own grant's `emergency_reader_expires_at` — the same grant "End now"
/// ends (`emergency_reader_id`).
#[tokio::test]
async fn the_workspace_answer_names_the_callers_own_grant_expiry() {
    let w = world().await;
    let (s, v) = w
        .open(
            &w.opener_tok,
            json!({"reason": "the opener's own grant", "hours": 1}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let first: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let first_expires = v["expires_at"].as_str().unwrap().to_owned();
    // The second owner opens the NEWEST grant: `audience.emergency` shows this one.
    let (s, v) = w
        .open(
            &w.other_tok,
            json!({"reason": "the other owner's grant", "hours": 4}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let second_expires = v["expires_at"].as_str().unwrap().to_owned();
    assert_ne!(first_expires, second_expires);

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
    assert_eq!(v["emergency_reader_id"], json!(first.to_string()), "{v}");
    assert_eq!(
        v["emergency_reader_expires_at"],
        json!(first_expires),
        "the OWN grant's expiry, never the newest one's: {v}"
    );
    assert_eq!(
        v["audience"]["emergency"]["expires_at"],
        json!(second_expires)
    );
}

/// (Verify LOW-2b) `emergency_reader*` is the SESSION caller's answer: an API key of the same
/// user can never use the grant (keys read with their own access only), so the key's workspace
/// answer keeps the flag false and carries no grant id or expiry.
#[tokio::test]
async fn an_api_key_of_the_grant_holder_is_not_announced_an_emergency_reader() {
    let w = world().await;
    // The person: a workspace member who also becomes an org owner and opens a grant — a caller
    // with own access AND a grant (the HIGH-3 case).
    sqlx::query("UPDATE org_members SET role = 'owner' WHERE org_id = $1 AND user_id = $2")
        .bind(w.org)
        .bind(w.person)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .open(&w.person_tok, json!({"reason": "the member-owner's grant"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let grant: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    seed_key(&w.h, w.org, Some(w.ws), w.person, "auk_wskey-token").await;

    // The session: announced, with their own grant.
    let (s, v) = w
        .call(
            &w.person_tok,
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["emergency_reader"], json!(true), "{v}");
    assert_eq!(v["emergency_reader_id"], json!(grant.to_string()), "{v}");
    assert_eq!(
        v["emergency_reader_expires_at"],
        json!(v["audience"]["emergency"]["expires_at"]),
        "their own grant is the newest one here"
    );

    // The key of the same user: the flag stays false, no grant rides on it.
    let (s, _, v) =
        w.h.send(
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            &[("authorization", &bearer("auk_wskey-token"))],
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["emergency_reader"], json!(false), "{v}");
    assert!(v["emergency_reader_id"].is_null(), "{v}");
    assert!(v["emergency_reader_expires_at"].is_null(), "{v}");
}

/// (Verify MEDIUM-1) The reopen-in-the-sweep-window path must leave the same audit trail the
/// sweep would have: the reopen ends the caller's own expired-but-unswept grant in the open
/// transaction **and writes the critical `workspace.emergency_access.expire` row there**, so
/// the minute sweep later finds nothing open and the log has open → expire → open.
#[tokio::test]
async fn the_reopen_writes_the_expire_audit_row_the_sweep_would_have() {
    let w = world().await;
    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the first, soon expired"}))
        .await;
    assert_eq!(s, 201, "{v}");
    let first: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // Wind the grant past its expiry before the sweep runs (the window CHECK moves `opened_at`
    // too).
    sqlx::query(
        "UPDATE workspace_emergency_access \
         SET opened_at = now() - interval '1 hour', expires_at = now() - interval '1 second' \
         WHERE id = $1",
    )
    .bind(first)
    .execute(w.h.pool())
    .await
    .unwrap();

    let (s, v) = w
        .open(&w.opener_tok, json!({"reason": "the reopen in the window"}))
        .await;
    assert_eq!(s, 201, "{v}");

    // The trail for the opener's grants: open, expire, open — the expire row critical, naming
    // the ended grant, exactly the sweep's shape.
    let rows: Vec<(String, Uuid, bool)> = sqlx::query_as(
        "SELECT action, target_id::uuid, critical FROM audit_log \
         WHERE workspace_id = $1 \
           AND action IN ('workspace.emergency_access.open', 'workspace.emergency_access.expire') \
         ORDER BY id",
    )
    .bind(w.ws)
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    let actions: Vec<&str> = rows
        .iter()
        .map(|(a, _, _)| a.rsplit('.').next().unwrap())
        .collect();
    assert_eq!(actions, vec!["open", "expire", "open"], "{rows:?}");
    assert_eq!(rows[1].1, first, "the expire row targets the ended grant");
    assert!(rows[1].2, "the expire row is critical");
}

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
    // The notices leave from a spawned task after the commit (review MEDIUM-6): poll.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while w.h.mail.sent().len() <= before {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the new grant notifies again"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
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
