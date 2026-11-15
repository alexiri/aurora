#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use std::time::Duration;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

#[tokio::test]
async fn session_cookie_auth() {
    let h = Harness::new().await;
    let u = h.user("a@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    let (s, v) = h.get("/t/whoami", &[("cookie", &cookie(&tok))]).await;
    assert_eq!(s, 200);
    assert_eq!(v["source"], "dashboard");
    assert_eq!(h.get("/t/whoami", &[]).await.0, 401);

    let (_, t) = h.session(u, 0, 3600, true).await;
    let (s, v) = h.get("/t/whoami", &[("cookie", &cookie(&t))]).await;
    assert_eq!((s.as_u16(), code(&v)), (401, "unauthenticated"));
    // idle-expired (config idle 14d) and absolutely expired
    let (_, t) = h.session(u, -1_300_000, 3600, false).await;
    assert_eq!(h.get("/t/whoami", &[("cookie", &cookie(&t))]).await.0, 401);
    let (_, t) = h.session(u, 0, -5, false).await;
    assert_eq!(h.get("/t/whoami", &[("cookie", &cookie(&t))]).await.0, 401);
    // disabled user
    let d = h.user("d@x.io", false).await;
    let (_, t) = h.good_session(d).await;
    h.disable_user(d).await;
    assert_eq!(h.get("/t/whoami", &[("cookie", &cookie(&t))]).await.0, 401);
    // wrong prefix cookie
    assert_eq!(
        h.get("/t/whoami", &[("cookie", "aurora_session=auk_zzz")])
            .await
            .0,
        401
    );
}

#[tokio::test]
async fn session_touch_at_most_once_per_minute() {
    let h = Harness::new().await;
    let u = h.user("a@x.io", false).await;
    let (id, tok) = h.session(u, -120, 3600, false).await;
    h.get("/t/whoami", &[("cookie", &cookie(&tok))]).await;
    let age: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - last_seen_at)::float8 FROM sessions WHERE id=$1",
    )
    .bind(id)
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert!(age < 30.0, "touched: {age}");
}

#[tokio::test]
async fn api_key_auth() {
    let h = Harness::new().await;
    let u = h.user("a@x.io", false).await;
    let org = h.org().await;
    h.member(org, u, "member", "active").await;
    let ws = h.workspace(org).await;
    let (_, key) = h.api_key(u, org, Some(ws), false).await;
    let (s, v) = h
        .get("/t/whoami", &[("authorization", &bearer(&key))])
        .await;
    assert_eq!((s.as_u16(), v["source"].as_str()), (200, Some("api")));
    let last: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_used_at FROM api_keys WHERE user_id=$1")
            .bind(u)
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert!(last.is_some());

    let (_, rev) = h.api_key(u, org, Some(ws), true).await;
    assert_eq!(
        h.get("/t/whoami", &[("authorization", &bearer(&rev))])
            .await
            .0,
        401
    );

    // other prefixes are 401 too
    let (_, st) = h.good_session(u).await;
    assert_eq!(
        h.get("/t/whoami", &[("authorization", &bearer(&st))])
            .await
            .0,
        401
    );

    // bad key never falls back to a valid cookie
    let (s, _) = h
        .get(
            "/t/whoami",
            &[
                ("authorization", "Bearer auk_bogus"),
                ("cookie", &cookie(&st)),
            ],
        )
        .await;
    assert_eq!(s, 401);

    // owner suspended => key dead
    let u2 = h.user("b@x.io", false).await;
    h.member(org, u2, "admin", "active").await;
    let (_, k2) = h.api_key(u2, org, None, false).await;
    assert_eq!(
        h.get("/t/whoami", &[("authorization", &bearer(&k2))])
            .await
            .0,
        200
    );
    h.state.session_cache.clear();
    h.set_member_status(org, u2, "suspended").await;
    assert_eq!(
        h.get("/t/whoami", &[("authorization", &bearer(&k2))])
            .await
            .0,
        401
    );
}

#[tokio::test]
async fn session_only_and_instance_admin() {
    let h = Harness::new().await;
    let u = h.user("a@x.io", false).await;
    let admin = h.user("root@x.io", true).await;
    let org = h.org().await;
    h.member(org, u, "owner", "active").await;
    let (_, key) = h.api_key(u, org, None, false).await;
    let (_, st) = h.good_session(u).await;
    let (_, at) = h.good_session(admin).await;

    let (s, _, v) = h
        .send(
            Method::POST,
            "/t/so",
            &[("authorization", &bearer(&key))],
            None,
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "session_required"));
    let (s, _, _) = h
        .send(
            Method::POST,
            "/t/so",
            &[("cookie", &cookie(&st)), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s, 200);

    let (s, v) = h.get("/t/ia", &[("cookie", &cookie(&st))]).await;
    assert_eq!((s.as_u16(), code(&v)), (403, "instance_admin_required"));
    assert_eq!(h.get("/t/ia", &[("cookie", &cookie(&at))]).await.0, 200);
}

#[tokio::test]
async fn csrf() {
    let h =
        Harness::with_config(|c| c.auth.trusted_origins = vec!["https://app.example.com".into()])
            .await;
    let u = h.user("a@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    let c = cookie(&tok);
    let post = |hs: Vec<(&'static str, String)>| {
        let h = &h;
        async move {
            let hs: Vec<(&str, &str)> = hs.iter().map(|(a, b)| (*a, b.as_str())).collect();
            h.send(Method::POST, "/t/post", &hs, None).await
        }
    };
    let (s, _, v) = post(vec![("cookie", c.clone())]).await;
    assert_eq!((s.as_u16(), code(&v)), (403, "csrf_rejected"));
    let (s, _, v) = post(vec![
        ("cookie", c.clone()),
        ("origin", "http://evil.test".into()),
    ])
    .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "csrf_rejected"));
    assert_eq!(
        post(vec![("cookie", c.clone()), ("origin", ORIGIN.into())])
            .await
            .0,
        200
    );
    assert_eq!(
        post(vec![
            ("cookie", c.clone()),
            ("origin", "https://app.example.com".into())
        ])
        .await
        .0,
        200
    );
    // GET is safe
    assert_eq!(h.get("/t/whoami", &[("cookie", &c)]).await.0, 200);
    // bearer is exempt
    let org = h.org().await;
    h.member(org, u, "member", "active").await;
    let (_, key) = h.api_key(u, org, None, false).await;
    assert_eq!(post(vec![("authorization", bearer(&key))]).await.0, 200);
    // A non-Bearer Authorization header never falls back to the cookie, so it
    // cannot be used to skip the CSRF check (orchestrator regression test).
    let (s, _, v) = post(vec![
        ("cookie", c.clone()),
        ("authorization", "Basic Zm9vOmJhcg==".into()),
    ])
    .await;
    assert_eq!((s.as_u16(), code(&v)), (401, "unauthenticated"));
}

#[tokio::test]
async fn access_law_matrix() {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let mk = |e: &'static str| {
        let h = &h;
        async move { h.user(e, false).await }
    };
    let owner = mk("o@x.io").await;
    let admin = mk("ad@x.io").await;
    let bound_m = mk("bm@x.io").await;
    let bound_o = mk("bo@x.io").await;
    let unbound = mk("un@x.io").await;
    let susp_admin = mk("sa@x.io").await;
    let outsider = mk("out@x.io").await;
    h.member(org, owner, "owner", "active").await;
    h.member(org, admin, "admin", "active").await;
    h.member(org, bound_m, "member", "active").await;
    h.member(org, bound_o, "member", "active").await;
    h.member(org, unbound, "member", "active").await;
    h.member(org, susp_admin, "admin", "suspended").await;
    h.bind_ws(ws, bound_m, "member").await;
    h.bind_ws(ws, bound_o, "owner").await;
    h.bind_ws(ws, susp_admin, "owner").await;
    let p = h.pool();
    use aurora_db::authz;
    let q = |u| authz::workspace_access(p, u, ws);
    let a = q(owner).await.unwrap().unwrap();
    assert!(a.can_own() && a.org_id == org);
    assert!(q(admin).await.unwrap().unwrap().can_own());
    let m = q(bound_m).await.unwrap().unwrap();
    assert!(!m.can_own() && m.can_use());
    assert!(q(bound_o).await.unwrap().unwrap().can_own());
    assert!(q(unbound).await.unwrap().is_none());
    assert!(q(susp_admin).await.unwrap().is_none());
    assert!(q(outsider).await.unwrap().is_none());
    assert!(
        !authz::can_use(p, owner, uuid::Uuid::now_v7())
            .await
            .unwrap()
    );
    assert!(authz::can_own(p, admin, ws).await.unwrap());
    let mem = authz::org_membership(p, susp_admin, org)
        .await
        .unwrap()
        .unwrap();
    assert!(!mem.is_active());

    // HTTP layer: 404 invisible, 403 forbidden when visible but insufficient
    let (_, tm) = h.good_session(bound_m).await;
    let (_, tu) = h.good_session(unbound).await;
    let c = |t: &str| cookie(t);
    assert_eq!(
        h.get(&format!("/t/ws/{ws}"), &[("cookie", &c(&tm))])
            .await
            .0,
        200
    );
    let (s, v) = h
        .get(&format!("/t/ws/{ws}/own"), &[("cookie", &c(&tm))])
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "forbidden"));
    assert_eq!(
        h.get(&format!("/t/ws/{ws}"), &[("cookie", &c(&tu))])
            .await
            .0,
        404
    );
    let (s, v) = h
        .get(&format!("/t/org/{org}"), &[("cookie", &c(&tm))])
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "forbidden"));
    let (_, to) = h.good_session(outsider).await;
    assert_eq!(
        h.get(&format!("/t/org/{org}"), &[("cookie", &c(&to))])
            .await
            .0,
        404
    );
}

#[tokio::test]
async fn key_confinement() {
    let h = Harness::new().await;
    let org = h.org().await;
    let org2 = h.org().await;
    let ws1 = h.workspace(org).await;
    let ws2 = h.workspace(org).await;
    let ws_other = h.workspace(org2).await;
    let u = h.user("a@x.io", false).await;
    h.member(org, u, "owner", "active").await;
    h.member(org2, u, "owner", "active").await;
    let (_, wk) = h.api_key(u, org, Some(ws1), false).await;
    let (_, ok) = h.api_key(u, org, None, false).await;
    let b = bearer(&wk);
    let (s, v) = h
        .get(&format!("/t/ws/{ws2}"), &[("authorization", &b)])
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "workspace_mismatch"));
    assert_eq!(
        h.get(&format!("/t/ws/{ws1}"), &[("authorization", &b)])
            .await
            .0,
        200
    );
    let ob = bearer(&ok);
    assert_eq!(
        h.get(&format!("/t/ws/{ws2}"), &[("authorization", &ob)])
            .await
            .0,
        200
    );
    assert_eq!(
        h.get(&format!("/t/ws/{ws_other}"), &[("authorization", &ob)])
            .await
            .0,
        404
    );
    assert_eq!(
        h.get(&format!("/t/org/{org}"), &[("authorization", &ob)])
            .await
            .0,
        200
    );
    assert_eq!(
        h.get(&format!("/t/org/{org2}"), &[("authorization", &ob)])
            .await
            .0,
        404
    );
}

#[tokio::test]
async fn json_and_query_rejections() {
    let h = Harness::new().await;
    let (s, _, v) = h
        .send(Method::POST, "/t/json", &[], Some(json!({"nope": 1})))
        .await;
    assert_eq!((s.as_u16(), code(&v)), (400, "invalid_request"));
    assert!(v["error"]["details"]["fields"].is_array());
    let (s, _, v) = h
        .send(Method::POST, "/t/json", &[], Some(json!({"name": 5})))
        .await;
    assert_eq!(s, 400);
    assert_eq!(v["error"]["details"]["fields"][0]["path"], "/name");
    let (s, _, _) = h
        .send(Method::POST, "/t/json", &[], Some(json!({"name": "x"})))
        .await;
    assert_eq!(s, 200);
    // wrong content type
    let req = http::Request::builder()
        .method("POST")
        .uri("/t/json")
        .header("content-type", "text/plain")
        .body(axum::body::Body::from("{\"name\":\"x\"}"))
        .unwrap();
    let res = tower::ServiceExt::oneshot(h.app.clone(), req)
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    // query
    let (s, v) = h.get("/t/page?limit=500", &[]).await;
    assert_eq!((s.as_u16(), code(&v)), (400, "invalid_request"));
    let (s, v) = h.get("/t/page?limit=abc", &[]).await;
    assert_eq!((s.as_u16(), code(&v)), (400, "invalid_request"));
    assert_eq!(h.get("/t/page?limit=10", &[]).await.1["limit"], 10);
}

#[tokio::test]
async fn rate_limit_429() {
    let h = Harness::new().await;
    assert_eq!(h.get("/t/limited", &[]).await.0, 200);
    assert_eq!(h.get("/t/limited", &[]).await.0, 200);
    let (s, hd, v) = h.send(Method::GET, "/t/limited", &[], None).await;
    assert_eq!((s.as_u16(), code(&v)), (429, "rate_limited"));
    assert!(hd.get("retry-after").is_some());
}

#[tokio::test]
async fn notify_flushes_cache() {
    let h = Harness::new().await;
    let _l = aurora_api::http::auth::spawn_revocation_listener(
        h.pool().clone(),
        h.state.session_cache.clone(),
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    let u = h.user("a@x.io", false).await;
    let (id, tok) = h.good_session(u).await;
    let c = cookie(&tok);
    assert_eq!(h.get("/t/whoami", &[("cookie", &c)]).await.0, 200);
    // raw revoke leaves the cached entry alive...
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id=$1")
        .bind(id)
        .execute(h.pool())
        .await
        .unwrap();
    assert_eq!(h.get("/t/whoami", &[("cookie", &c)]).await.0, 200);
    // ...until the NOTIFY arrives.
    sqlx::query("SELECT pg_notify('aurora_auth_revoked', $1)")
        .bind(json!({"kind": "session", "id": id}).to_string())
        .execute(h.pool())
        .await
        .unwrap();
    let mut status = 200;
    for _ in 0..40 {
        status = h.get("/t/whoami", &[("cookie", &c)]).await.0.as_u16();
        if status == 401 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(status, 401);
}

#[tokio::test]
async fn repo_revoke_notifies_and_audit() {
    let h = Harness::new().await;
    let u = h.user("a@x.io", false).await;
    let (id, tok) = h.good_session(u).await;
    let (_, tok2) = h.good_session(u).await;
    assert!(
        aurora_db::repo::sessions::revoke(h.pool(), id)
            .await
            .unwrap()
    );
    assert_eq!(
        h.get("/t/whoami", &[("cookie", &cookie(&tok))]).await.0,
        401
    );
    let n = aurora_db::repo::sessions::revoke_all_for_user_except(h.pool(), u, None)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        h.get("/t/whoami", &[("cookie", &cookie(&tok2))]).await.0,
        401
    );

    use aurora_api::audit::{self, AuditActor, AuditEvent, Outcome};
    audit::record(
        &h.state,
        AuditEvent::new(AuditActor::system("test"), "thing.create", Outcome::Success)
            .target("thing", "1"),
    );
    let mut n = 0i64;
    for _ in 0..40 {
        n = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='thing.create'")
            .fetch_one(h.pool())
            .await
            .unwrap();
        if n == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(n, 1);
}

#[tokio::test]
async fn instance_and_openapi() {
    let h = Harness::new().await;
    let (s, v) = h.get("/v1/instance", &[]).await;
    assert_eq!(s, 200);
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["email_delivery"], "log");
    assert_eq!(v["setup_errors"], json!([]));
    for k in [
        "signup_mode",
        "org_creation",
        "oidc_providers",
        "features",
        "default_agent_name",
        "external_url",
    ] {
        assert!(!v[k].is_null(), "{k}");
    }
    let (s, v) = h.get("/v1/openapi.json", &[]).await;
    assert_eq!(s, 200);
    assert!(v["paths"]["/v1/instance"].is_object());
    let (s, v) = h.get("/v1/nope", &[]).await;
    assert_eq!((s.as_u16(), code(&v)), (404, "not_found"));
}

/// (FIX-119) Every operation that declares a security requirement documents the two answers any authenticated
/// route can give regardless of its handler: 401 (no or an invalid credential) and 429 (the limiter).
#[test]
fn every_authenticated_operation_documents_401_and_429() {
    let doc: Value = serde_json::from_str(&aurora_api::openapi::document_json()).unwrap();
    let mut checked = 0;
    let mut missing = Vec::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            if !matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete") {
                continue;
            }
            if op["security"].as_array().is_none_or(|s| s.is_empty()) {
                continue;
            }
            checked += 1;
            for code in ["401", "429"] {
                if op["responses"][code].is_null() {
                    missing.push(format!("{} {path}: no {code}", method.to_uppercase()));
                }
            }
        }
    }
    assert!(
        checked > 250,
        "expected hundreds of authenticated operations, saw {checked}"
    );
    assert!(
        missing.is_empty(),
        "{} gaps:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn openapi_matches_checked_in() {
    let generated = aurora_api::openapi::document_json();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/openapi.json");
    if std::env::var_os("UPDATE_OPENAPI").is_some() {
        std::fs::write(path, &generated).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(path).unwrap();
    let a: Value = serde_json::from_str(&on_disk).unwrap();
    let b: Value = serde_json::from_str(&generated).unwrap();
    assert_eq!(
        a, b,
        "web/openapi.json drifted; run UPDATE_OPENAPI=1 cargo test -p aurora-api --test it openapi_matches_checked_in"
    );
}
