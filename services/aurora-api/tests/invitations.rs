#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

async fn call(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let c = cookie(tok);
    let (s, _, v) = h
        .send(m, uri, &[("cookie", &c), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}

async fn verify(h: &Harness, u: Uuid) {
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE id = $1")
        .bind(u)
        .execute(h.pool())
        .await
        .unwrap();
}

struct Ctx {
    org: Uuid,
    admin_tok: String,
}

async fn setup(h: &Harness) -> Ctx {
    let org = h.org().await;
    let a = h.user("owner@x.io", false).await;
    h.member(org, a, "owner", "active").await;
    let (_, admin_tok) = h.good_session(a).await;
    Ctx { org, admin_tok }
}

/// Wait for the detached mailer and return the token of the last mail to `to`.
async fn mailed_token(h: &Harness, to: &str) -> String {
    for _ in 0..100 {
        if let Some(m) = h.mail.sent().into_iter().rev().find(|m| m.to == to) {
            let t = m.link.split("token=").nth(1).unwrap().to_owned();
            assert!(t.starts_with("aui_"));
            assert!(m.link.contains("/join?token="));
            return t;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no mail to {to}");
}

fn no_token(v: &Value, tok: &str) {
    assert!(!v.to_string().contains(tok), "token leaked: {v}");
    assert!(!v.to_string().contains("aui_"), "token-like value leaked");
}

async fn invite(h: &Harness, c: &Ctx, email: &str, role: &str, ws: Vec<Uuid>) -> (u16, Value) {
    call(
        h,
        &c.admin_tok,
        Method::POST,
        &format!("/v1/orgs/{}/invitations", c.org),
        Some(json!({"email": email, "role": role, "workspace_ids": ws})),
    )
    .await
}

#[tokio::test]
async fn create_list_cancel_permissions_and_flow() {
    let h = Harness::new().await;
    let c = setup(&h).await;
    let ws = h.workspace(c.org).await;
    let (s, v) = invite(&h, &c, "New@X.io", "member", vec![ws]).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["email"], "new@x.io");
    assert_eq!(v["status"], "pending");
    let tok = mailed_token(&h, "new@x.io").await;
    no_token(&v, &tok);

    let list = format!("/v1/orgs/{}/invitations", c.org);
    let (s, l) = call(&h, &c.admin_tok, Method::GET, &list, None).await;
    assert_eq!(s, 200);
    assert_eq!(l["items"].as_array().unwrap().len(), 1);
    no_token(&l, &tok);
    let (_, l) = call(
        &h,
        &c.admin_tok,
        Method::GET,
        &format!("{list}?status=accepted"),
        None,
    )
    .await;
    assert!(l["items"].as_array().unwrap().is_empty());

    // owner role is not invitable; foreign workspace rejected
    let (s, _) = invite(&h, &c, "o@x.io", "owner", vec![]).await;
    assert_eq!(s, 400);
    let other = h.org().await;
    let ows = h.workspace(other).await;
    let (s, v) = invite(&h, &c, "o@x.io", "member", vec![ows]).await;
    assert_eq!(s, 400, "{v}");

    // plain member: 403 on list/create/cancel
    let m = h.user("m@x.io", false).await;
    h.member(c.org, m, "member", "active").await;
    let (_, mt) = h.good_session(m).await;
    let (s, _) = call(&h, &mt, Method::GET, &list, None).await;
    assert_eq!(s, 403);
    let (s, _) = call(
        &h,
        &mt,
        Method::POST,
        &list,
        Some(json!({"email":"z@x.io","role":"member"})),
    )
    .await;
    assert_eq!(s, 403);
    let id = v_id(&h, c.org).await;
    let (s, _) = call(
        &h,
        &mt,
        Method::DELETE,
        &format!("/v1/invitations/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 403);

    // preview
    let (s, p) = h
        .get(&format!("/v1/invitations/preview?token={tok}"), &[])
        .await;
    assert_eq!(s.as_u16(), 200, "{p}");
    assert_eq!(p["email"], "new@x.io");
    assert_eq!(p["status"], "pending");
    no_token(&p, &tok);

    // cancel
    let (s, _) = call(
        &h,
        &c.admin_tok,
        Method::DELETE,
        &format!("/v1/invitations/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 204);
    let (s, _) = h
        .get(&format!("/v1/invitations/preview?token={tok}"), &[])
        .await;
    assert_eq!(s.as_u16(), 404);
    let (s, _) = call(
        &h,
        &c.admin_tok,
        Method::DELETE,
        &format!("/v1/invitations/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 404);
    let (_, l) = call(
        &h,
        &c.admin_tok,
        Method::GET,
        &format!("{list}?status=cancelled"),
        None,
    )
    .await;
    assert_eq!(l["items"].as_array().unwrap().len(), 1);

    for a in ["invitation.create", "invitation.cancel"] {
        wait_audit(&h, a).await;
    }
}

async fn wait_audit(h: &Harness, action: &str) {
    for _ in 0..100 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(h.pool())
            .await
            .unwrap();
        if n > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no audit {action}");
}

async fn v_id(h: &Harness, org: Uuid) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM invitations WHERE org_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(org)
    .fetch_one(h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn reinvite_rotates_token() {
    let h = Harness::new().await;
    let c = setup(&h).await;
    let (s, v1) = invite(&h, &c, "a@x.io", "member", vec![]).await;
    assert_eq!(s, 201);
    let t1 = mailed_token(&h, "a@x.io").await;
    h.mail.clear();
    let (s, v2) = invite(&h, &c, "a@x.io", "admin", vec![]).await;
    assert_eq!(s, 201);
    assert_eq!(v1["id"], v2["id"]);
    assert_eq!(v2["role"], "admin");
    let t2 = mailed_token(&h, "a@x.io").await;
    assert_ne!(t1, t2);
    let (s, _) = h
        .get(&format!("/v1/invitations/preview?token={t1}"), &[])
        .await;
    assert_eq!(s.as_u16(), 404);
    let (s, _) = h
        .get(&format!("/v1/invitations/preview?token={t2}"), &[])
        .await;
    assert_eq!(s.as_u16(), 200);
}

#[tokio::test]
async fn already_member_and_cap() {
    let h = Harness::new().await;
    let c = setup(&h).await;
    let m = h.user("m@x.io", false).await;
    h.member(c.org, m, "member", "suspended").await;
    let (s, v) = invite(&h, &c, "m@x.io", "member", vec![]).await;
    assert_eq!(s, 409);
    assert_eq!(code(&v), "already_member");

    // cap: owner(1) + suspended not counted; cap 2 => one more invite fits
    sqlx::query(
        "UPDATE organizations SET settings = jsonb_build_object('max_members', 2) WHERE id = $1",
    )
    .bind(c.org)
    .execute(h.pool())
    .await
    .unwrap();
    assert_eq!(invite(&h, &c, "p1@x.io", "member", vec![]).await.0, 201);
    // re-invite does not consume another seat
    assert_eq!(invite(&h, &c, "p1@x.io", "member", vec![]).await.0, 201);
    let (s, v) = invite(&h, &c, "p2@x.io", "member", vec![]).await;
    assert_eq!(s, 422);
    assert_eq!(code(&v), "member_limit_reached");
}

#[tokio::test]
async fn accept_flow_and_errors() {
    let h = Harness::new().await;
    let c = setup(&h).await;
    let ws = h.workspace(c.org).await;
    invite(&h, &c, "new@x.io", "member", vec![ws]).await;
    let tok = mailed_token(&h, "new@x.io").await;
    let body = || Some(json!({"token": tok}));

    // API key => session_required
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM org_members WHERE org_id = $1")
        .bind(c.org)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (_, key) = h.api_key(owner, c.org, None, false).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/invitations/accept",
            &[("authorization", &bearer(&key)), ("origin", ORIGIN)],
            body(),
        )
        .await;
    assert_eq!(s.as_u16(), 403);
    assert_eq!(code(&v), "session_required");

    // unverified
    let u = h.user("new@x.io", false).await;
    let (_, ut) = h.good_session(u).await;
    let (s, v) = call(&h, &ut, Method::POST, "/v1/invitations/accept", body()).await;
    assert_eq!((s, code(&v)), (403, "email_unverified"));

    // mismatch
    let w = h.user("other@x.io", false).await;
    verify(&h, w).await;
    let (_, wt) = h.good_session(w).await;
    let (s, v) = call(&h, &wt, Method::POST, "/v1/invitations/accept", body()).await;
    assert_eq!((s, code(&v)), (403, "email_mismatch"));

    // success
    verify(&h, u).await;
    let (s, v) = call(&h, &ut, Method::POST, "/v1/invitations/accept", body()).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["org"]["id"], json!(c.org));
    assert_eq!(v["workspace_ids"], json!([ws]));
    no_token(&v, &tok);
    let role: String =
        sqlx::query_scalar("SELECT role FROM org_members WHERE org_id=$1 AND user_id=$2")
            .bind(c.org)
            .bind(u)
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert_eq!(role, "member");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_access WHERE workspace_id=$1 AND user_id=$2",
    )
    .bind(ws)
    .bind(u)
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
    let st: String = sqlx::query_scalar("SELECT status FROM invitations WHERE org_id=$1")
        .bind(c.org)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(st, "accepted");
    wait_audit(&h, "invitation.accept").await;

    // reuse => 404
    let (s, _) = call(&h, &ut, Method::POST, "/v1/invitations/accept", body()).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn accept_expired_and_suspended() {
    let h = Harness::new().await;
    let c = setup(&h).await;
    invite(&h, &c, "e@x.io", "member", vec![]).await;
    let tok = mailed_token(&h, "e@x.io").await;
    let u = h.user("e@x.io", false).await;
    verify(&h, u).await;
    let (_, ut) = h.good_session(u).await;
    sqlx::query("UPDATE invitations SET expires_at = now() - interval '1 hour'")
        .execute(h.pool())
        .await
        .unwrap();
    let (s, v) = call(
        &h,
        &ut,
        Method::POST,
        "/v1/invitations/accept",
        Some(json!({"token": tok})),
    )
    .await;
    assert_eq!((s, code(&v)), (410, "invitation_expired"));
    let (s, _) = h
        .get(&format!("/v1/invitations/preview?token={tok}"), &[])
        .await;
    assert_eq!(s.as_u16(), 404);
    let (_, l) = call(
        &h,
        &c.admin_tok,
        Method::GET,
        &format!("/v1/orgs/{}/invitations?status=expired", c.org),
        None,
    )
    .await;
    assert_eq!(l["items"][0]["status"], "expired");

    // suspended member: 409, not reactivated
    sqlx::query("UPDATE invitations SET expires_at = now() + interval '1 day'")
        .execute(h.pool())
        .await
        .unwrap();
    h.member(c.org, u, "member", "suspended").await;
    let (s, v) = call(
        &h,
        &ut,
        Method::POST,
        "/v1/invitations/accept",
        Some(json!({"token": tok})),
    )
    .await;
    assert_eq!((s, code(&v)), (409, "already_member"));
}
