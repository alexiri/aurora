#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_core::token;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct World {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner_tok: String,
    member_tok: String,
}

async fn world() -> World {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = d;
        c.advertised_proxy_url = "https://aurora.example.org:4102".into();
        c.sandbox_proxy_url = "https://aurora-gateway:4102".into();
        // These tests exercise the BYO lifecycle as a plain member; the governed default is
        // covered in byo.rs (RFE-025).
        c.governance.byo.minimum_posture = aurora_config::ByoPosture::Enabled;
    })
    .await;
    let org = h.org().await;
    let owner = h.user("o@x.io", false).await;
    let member = h.user("m@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    h.member(org, member, "member", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, member, "member").await;
    let (_, owner_tok) = h.good_session(owner).await;
    let (_, member_tok) = h.good_session(member).await;
    World {
        h,
        org,
        ws,
        owner_tok,
        member_tok,
    }
}

async fn sess(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    h.call_tok(tok, m, uri, body).await
}
async fn bear(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let b = bearer(tok);
    let (s, _, v) = h.send(m, uri, &[("authorization", &b)], body).await;
    (s.as_u16(), v)
}
fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

async fn create(w: &World, body: Value) -> (u16, Value) {
    sess(
        &w.h,
        &w.member_tok,
        Method::POST,
        &format!("/v1/workspaces/{}/agents", w.ws),
        Some(body),
    )
    .await
}

#[tokio::test]
async fn byo_create_token_once_hash_only_and_gateway_resolves() {
    let w = world().await;
    let (s, v) = create(&w, json!({"name":"My Bot","kind":"byo"})).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["agent"]["handle"], "my-bot");
    let tok = v["token"].as_str().unwrap().to_owned();
    assert!(tok.starts_with("aug_"));
    assert!(
        v["container_config"]["proxy_url"]
            .as_str()
            .unwrap()
            .contains(&tok)
    );
    // FIX-010 (C-300): BYO agents get the advertised (external) address, never the sandbox one.
    let pu = v["container_config"]["proxy_url"].as_str().unwrap();
    assert!(pu.ends_with("@aurora.example.org:4102"), "{pu}");
    assert!(!pu.contains("aurora-gateway"), "{pu}");
    // only the hash is stored
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM gateway_credentials")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(stored, token::hash(&tok).to_vec());
    // gateway repo resolves it
    let id = aurora_db::repo::gateway::find_credential_identity(w.h.pool(), &token::hash(&tok))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(id.kind, "agent");
    assert_eq!(id.org_id, w.org);
    // handle derivation avoids collisions; explicit duplicate conflicts
    let (s, v2) = create(&w, json!({"name":"My Bot","kind":"byo"})).await;
    assert_eq!(
        (s, v2["agent"]["handle"].as_str().unwrap()),
        (201, "my-bot-2")
    );
    let (s, v3) = create(&w, json!({"name":"x","handle":"my-bot","kind":"byo"})).await;
    assert_eq!((s, code(&v3)), (409, "handle_taken"));
    let (s, _) = create(&w, json!({"name":"x","handle":"Bad_Handle","kind":"byo"})).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn regenerate_revokes_old_token_and_contexts() {
    let w = world().await;
    let (_, v) = create(&w, json!({"name":"Bot","kind":"byo"})).await;
    let old = v["token"].as_str().unwrap().to_owned();
    let aid = v["agent"]["id"].as_str().unwrap().to_owned();
    let (s, c) = bear(
        &w.h,
        &old,
        Method::POST,
        "/v1/agent/context-credentials",
        Some(json!({"label":"  job\u{7} 1 ","ttl_secs":120})),
    )
    .await;
    assert_eq!(s, 201, "{c}");
    let ctx = c["credential"].as_str().unwrap().to_owned();
    assert!(ctx.starts_with("auc_"));
    let label: String =
        sqlx::query_scalar("SELECT context_label FROM gateway_credentials WHERE kind='context'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(label, "job 1");
    // bounds
    for ttl in [59, 86401] {
        let (s, _) = bear(
            &w.h,
            &old,
            Method::POST,
            "/v1/agent/context-credentials",
            Some(json!({"label":"a","ttl_secs":ttl})),
        )
        .await;
        assert_eq!(s, 400);
    }
    let (s, _) = bear(
        &w.h,
        &old,
        Method::POST,
        "/v1/agent/context-credentials",
        Some(json!({"label":" \u{7} "})),
    )
    .await;
    assert_eq!(s, 400);
    let (s, _) = bear(
        &w.h,
        &old,
        Method::POST,
        "/v1/agent/context-credentials",
        Some(json!({"label":"a","conversation_id":Uuid::now_v7()})),
    )
    .await;
    assert_eq!(s, 404);

    // member (not owner) can regenerate: can_use
    let (s, r) = sess(
        &w.h,
        &w.member_tok,
        Method::POST,
        &format!("/v1/agents/{aid}/token"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{r}");
    let new = r["token"].as_str().unwrap().to_owned();
    assert_ne!(new, old);
    let (s, _) = bear(&w.h, &old, Method::GET, "/v1/agent/container-config", None).await;
    assert_eq!(s, 401);
    let h = token::hash(&ctx);
    assert!(
        aurora_db::repo::gateway::find_credential_identity(w.h.pool(), &h)
            .await
            .unwrap()
            .is_none()
    );
    let (s, cfg) = bear(&w.h, &new, Method::GET, "/v1/agent/container-config", None).await;
    assert_eq!(s, 200);
    assert!(cfg["proxy_url"].as_str().unwrap().contains(&new));
    assert_eq!(cfg["env"]["SSL_CERT_FILE"], "/aurora/ca.pem");
    // agent token on any other route is 401
    for uri in ["/v1/me", &format!("/v1/agents/{aid}"), "/v1/gateway/ca.pem"] {
        let (s, _) = bear(&w.h, &new, Method::GET, uri, None).await;
        assert_eq!(s, 401, "{uri}");
    }
}

#[tokio::test]
async fn container_config_placeholder_and_ca() {
    let w = world().await;
    let (_, v) = create(&w, json!({"name":"Bot","kind":"byo"})).await;
    let aid = v["agent"]["id"].as_str().unwrap().to_owned();
    let (s, cfg) = sess(
        &w.h,
        &w.member_tok,
        Method::GET,
        &format!("/v1/agents/{aid}/container-config"),
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert!(cfg["proxy_url"].as_str().unwrap().contains("<AGENT_TOKEN>"));
    assert!(cfg["ca_pem"].as_str().unwrap().starts_with("-----BEGIN"));
    // (phase 1, RFE-002, SEC-40) the container config never names a plaintext gateway URL
    let url = cfg["proxy_url"].as_str().unwrap();
    assert!(url.starts_with("https://"), "{url}");
    for (k, v) in cfg["env"].as_object().unwrap() {
        assert!(
            !v.as_str().unwrap().contains("http://"),
            "env {k} must not hold a plaintext URL: {v}"
        );
    }
    for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        assert!(
            cfg["env"][k].as_str().unwrap().starts_with("https://"),
            "{k}"
        );
    }
    assert_eq!(cfg["proxy_ca_path"], cfg["ca_path"]);
    assert_eq!(cfg["env"]["GIT_CONFIG_KEY_0"], "http.proxySSLCAInfo");
    assert_eq!(cfg["env"]["GIT_CONFIG_VALUE_0"], cfg["proxy_ca_path"]);
    assert!(cfg["proxy_tls"].is_null(), "CA-issued proxy certificate");
    let c = cookie(&w.member_tok);
    let (s, hd, _) =
        w.h.send(Method::GET, "/v1/gateway/ca.pem", &[("cookie", &c)], None)
            .await;
    assert_eq!(s, 200);
    assert_eq!(hd["content-type"], "application/x-pem-file");
}

#[tokio::test]
async fn hosted_rules_and_limits() {
    let w = world().await;
    let (s, v) = create(&w, json!({"name":"H","kind":"hosted"})).await;
    assert_eq!(s, 201, "{v}");
    assert!(v["token"].is_null() && v["container_config"].is_null());
    let aid = v["agent"]["id"].as_str().unwrap().to_owned();
    let (s, e) = sess(
        &w.h,
        &w.member_tok,
        Method::GET,
        &format!("/v1/agents/{aid}/container-config"),
        None,
    )
    .await;
    assert_eq!((s, code(&e)), (403, "hosted_agent_token_hidden"));
    let (s, e) = sess(
        &w.h,
        &w.member_tok,
        Method::POST,
        &format!("/v1/agents/{aid}/token"),
        None,
    )
    .await;
    assert_eq!((s, code(&e)), (403, "hosted_agent_token_hidden"));
    // limit
    sqlx::query("UPDATE organizations SET settings = '{\"max_agents\": 1}' WHERE id = $1")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, e) = create(&w, json!({"name":"B","kind":"byo"})).await;
    assert_eq!((s, code(&e)), (422, "agent_limit_reached"));
    // hosted disabled
    let h = Harness::with_config(|c| c.hosted.enabled = false).await;
    let org = h.org().await;
    let u = h.user("a@x.io", false).await;
    h.member(org, u, "owner", "active").await;
    let ws = h.workspace(org).await;
    let (_, t) = h.good_session(u).await;
    let (s, e) = sess(
        &h,
        &t,
        Method::POST,
        &format!("/v1/workspaces/{ws}/agents"),
        Some(json!({"name":"H","kind":"hosted"})),
    )
    .await;
    assert_eq!((s, code(&e)), (422, "hosted_disabled"));
}

#[tokio::test]
async fn patch_and_delete_revoke() {
    let w = world().await;
    let (_, v) = create(&w, json!({"name":"Bot","kind":"byo"})).await;
    let aid = v["agent"]["id"].as_str().unwrap().to_owned();
    let tok = v["token"].as_str().unwrap().to_owned();
    let (s, p) = sess(
        &w.h,
        &w.member_tok,
        Method::PATCH,
        &format!("/v1/agents/{aid}"),
        Some(json!({"name":"Renamed","effort":"high"})),
    )
    .await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(
        (p["name"].as_str(), p["handle"].as_str()),
        (Some("Renamed"), Some("bot"))
    );
    let (s, _) = sess(
        &w.h,
        &w.member_tok,
        Method::PATCH,
        &format!("/v1/agents/{aid}"),
        Some(json!({"handle":"nope"})),
    )
    .await;
    assert_eq!(s, 400);
    let (s, _) = sess(
        &w.h,
        &w.member_tok,
        Method::PATCH,
        &format!("/v1/agents/{aid}"),
        Some(json!({"resource_overrides":{}})),
    )
    .await;
    assert_eq!(s, 403);
    // outsider: 404
    let out = w.h.user("out@x.io", false).await;
    let (_, ot) = w.h.good_session(out).await;
    let (s, _) = sess(
        &w.h,
        &ot,
        Method::DELETE,
        &format!("/v1/agents/{aid}"),
        None,
    )
    .await;
    assert_eq!(s, 404);
    let (s, _) = sess(
        &w.h,
        &w.owner_tok,
        Method::DELETE,
        &format!("/v1/agents/{aid}"),
        None,
    )
    .await;
    assert_eq!(s, 204);
    assert!(
        aurora_db::repo::gateway::find_credential_identity(w.h.pool(), &token::hash(&tok))
            .await
            .unwrap()
            .is_none()
    );
    let (s, _) = bear(&w.h, &tok, Method::GET, "/v1/agent/container-config", None).await;
    assert_eq!(s, 401);
    let (s, _) = sess(
        &w.h,
        &w.owner_tok,
        Method::GET,
        &format!("/v1/agents/{aid}"),
        None,
    )
    .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn container_config_flags_an_operator_proxy_certificate() {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = d;
        c.gateway_proxy_operator_cert = true;
        c.governance.byo.minimum_posture = aurora_config::ByoPosture::Enabled;
    })
    .await;
    let org = h.org().await;
    let member = h.user("m@x.io", false).await;
    h.member(org, member, "member", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, member, "member").await;
    let (_, tok) = h.good_session(member).await;
    let (s, v) = sess(
        &h,
        &tok,
        Method::POST,
        &format!("/v1/workspaces/{ws}/agents"),
        Some(json!({"name":"Bot","kind":"byo"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let cfg = &v["container_config"];
    assert_eq!(cfg["proxy_tls"], "operator_certificate");
    // The proxy leg is verified with system roots: no separate proxy trust anchor env for git.
    assert!(cfg["env"].get("GIT_CONFIG_KEY_0").is_none());
}

/// (P5-REFINE-6, C-1090) The organization's agents across its workspaces, for its owners and
/// admins only; paged; `paused` when frozen.
#[tokio::test]
async fn an_organizations_agents_for_its_owners_and_admins() {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws1 = h.workspace(org).await;
    let ws2 = h.workspace(org).await;
    let a1 = h.hosted_agent(org, ws1, "alpha").await;
    let a2 = h.byo_agent(org, ws2, "bravo", "Bravo").await;
    // (D-425) One with a running turn (working, last used now), one idle (ready).
    let a3 = h.byo_agent(org, ws2, "charlie", "Charlie").await;
    let a4 = h.byo_agent(org, ws2, "delta", "Delta").await;
    let conv = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
         VALUES ($1,$2,$3,'api',$1::text, now())",
    )
    .bind(conv)
    .bind(a3)
    .bind(ws2)
    .execute(h.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text)
         VALUES ($1,$2,$3,'turn','running','{}','system','s','hi')",
    )
    .bind(Uuid::now_v7())
    .bind(conv)
    .bind(a3)
    .execute(h.pool())
    .await
    .unwrap();
    let other = h.org().await;
    let ows = h.workspace(other).await;
    h.hosted_agent(other, ows, "elsewhere").await;
    sqlx::query("UPDATE agents SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'org_admin' WHERE id = $1")
        .bind(a2)
        .execute(h.pool())
        .await
        .unwrap();
    let admin = h.user("adm@x.io", false).await;
    h.member(org, admin, "admin", "active").await;
    let member = h.user("mem@x.io", false).await;
    h.member(org, member, "member", "active").await;
    h.bind_ws(ws1, member, "owner").await;
    let outsider = h.user("out@x.io", false).await;
    let (_, at) = h.good_session(admin).await;
    let (_, mt) = h.good_session(member).await;
    let (_, ot) = h.good_session(outsider).await;
    let uri = format!("/v1/orgs/{org}/agents");
    let (s, v) = h.call_tok(&at, Method::GET, &uri, None).await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 4, "{v}");
    let find = |id: Uuid| items.iter().find(|a| a["id"] == json!(id)).unwrap();
    assert_eq!(find(a1)["kind"], "hosted");
    // (D-425) A hosted agent with no AI key cannot answer: the header's "Needs a key".
    assert_eq!(find(a1)["state"], "needs_key", "{v}");
    assert!(find(a1)["last_active_at"].is_null());
    assert!(find(a1)["workspace"]["slug"].is_string());
    assert_eq!(find(a1)["workspace"]["id"], json!(ws1));
    assert!(find(a1)["workspace"]["personal_of"].is_null());
    assert_eq!(find(a2)["kind"], "byo");
    assert_eq!(find(a2)["state"], "paused");
    assert_eq!(find(a3)["state"], "working");
    assert!(find(a3)["last_active_at"].is_string(), "{v}");
    assert_eq!(find(a4)["state"], "ready");
    // Paged.
    let (_, p1) = h
        .call_tok(&at, Method::GET, &format!("{uri}?limit=1"), None)
        .await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 1);
    assert!(p1["next_cursor"].is_string());
    // A member (even a workspace owner) is refused; an outsider does not see the organization.
    assert_eq!(h.call_tok(&mt, Method::GET, &uri, None).await.0, 403);
    assert_eq!(h.call_tok(&ot, Method::GET, &uri, None).await.0, 404);
}
