#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! HOOK-004 (part 1): gateway <-> hook host, black box. Run with `--test-threads=1`.
//!
//! The signed `fixture-hooks` package is installed through the admin API (trusted key,
//! upload, approve) so the real hook host loads it from the API's registry snapshot.
//! The fixture guest only reaches `auth.hooks.example.test`, which cannot be resolved
//! from a test machine, so a *successful* mint through the real guest is impossible here;
//! the happy mint path (and the loop fallback) use a stub W9 server instead.
mod common;

use aurora_secrets::{Purpose, SealContext, SecretVault, StoreRegistry};
use common::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

const HOST: &str = "api.hooks.example.test";

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Trust the fixture key, upload and approve `fixture-hooks`; returns the package row id.
async fn install_fixture(st: &Stack, api: &mut Api) -> String {
    // Signup never yields an unverified instance admin; promote directly.
    sqlx::query("UPDATE users SET is_instance_admin = true")
        .execute(st.pool())
        .await
        .unwrap();
    api.relogin("owner@e2e.test").await;
    let dist = root().join("plugins/fixtures");
    let pubkey = std::fs::read_to_string(dist.join("test-key/test-signing-key.pub")).unwrap();
    let (s, v) = api
        .post(
            "/v1/admin/trusted-keys",
            json!({"publisher": "fixtures", "name": "fixture test key", "public_key": pubkey.trim()}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let pkg = std::fs::read(dist.join("dist/fixture-hooks.aurora-plugin"))
        .expect("run `bash scripts/fixtures/build.sh` first");
    let (s, v) = api.upload_plugin(&pkg).await;
    assert_eq!(s, 201, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let (s, v) = api
        .post(&format!("/v1/admin/plugins/{id}/approve"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "active", "{v}");
    id
}

/// Active connection for `hooks-demo` with sealed credentials, expired when `expired`.
async fn connection(st: &Stack, api: &Api, expired: bool) -> Uuid {
    let id = Uuid::now_v7();
    let (org, ws): (Uuid, Uuid) = (api.org.parse().unwrap(), api.ws.parse().unwrap());
    let keys = {
        use base64::Engine;
        format!(
            "k1:{}",
            base64::engine::general_purpose::STANDARD.encode([b'a'; 32])
        )
    };
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&keys, "k1").unwrap());
    let vault = SecretVault::new(
        StoreRegistry::from_config(&aurora_config::SecretsSection::default(), keyring).unwrap(),
    );
    let r = vault
        .seal(
            &SealContext {
                purpose: Purpose::ConnectionCredentials,
                owner_id: id,
                org_id: org,
                workspace_id: Some(ws),
            },
            aurora_core::Secret::from(
                serde_json::to_vec(&json!({"access_token": "stale", "installation_id": "7"}))
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
    let secs: f64 = if expired { -3600.0 } else { 3600.0 };
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, credentials_expires_at) VALUES ($1,$2,$3,'hooks-demo','fixture-hooks','1.0.0','fx','active',$4, now() + make_interval(secs => $5))")
        .bind(id).bind(org).bind(ws).bind(r.to_column()).bind(secs)
        .execute(st.pool()).await.unwrap();
    id
}

async fn grant_conn(st: &Stack, api: &Api, agent: &str, conn: Uuid) {
    // The connection grant route is not available yet; insert the grant row directly.
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(Uuid::now_v7())
        .bind(api.ws.parse::<Uuid>().unwrap())
        .bind(agent.parse::<Uuid>().unwrap())
        .bind(conn)
        .execute(st.pool())
        .await
        .unwrap();
    // What the grant compiler (BE-033) will derive: allow the agent on the connection.
    let rule = json!({"name": "Grant: fixture-hooks", "enabled": true,
        "identities": [{"kind": "agent", "id": agent}],
        "targets": [{"type": "connection", "id": conn}],
        "action": "allow", "require_approval": false, "rate_limit": null, "conditions": []});
    sqlx::query("INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) VALUES ($1,$2,$3,'grant',$4,0,$5,$6)")
        .bind(Uuid::now_v7())
        .bind(api.org.parse::<Uuid>().unwrap())
        .bind(api.ws.parse::<Uuid>().unwrap())
        .bind(conn)
        .bind(Uuid::now_v7())
        .bind(rule)
        .execute(st.pool())
        .await
        .unwrap();
}

#[derive(Clone, Copy)]
enum StubMode {
    MintOk,
    Hang,
}

/// Minimal W9 stand-in: records `(path, bearer, json body)`; `Hang` never answers.
async fn stub_hookhost(mode: StubMode) -> (String, Arc<Mutex<Vec<(String, String, Value)>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<(String, String, Value)>>> = Arc::default();
    let out = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = l.accept().await else {
                return;
            };
            let out = out.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head, body) = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_string();
                        let cl = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        while buf.len() < i + 4 + cl {
                            let n = sock.read(&mut tmp).await.unwrap_or(0);
                            if n == 0 {
                                return;
                            }
                            buf.extend_from_slice(&tmp[..n]);
                        }
                        break (head, buf[i + 4..i + 4 + cl].to_vec());
                    }
                };
                let path = head
                    .lines()
                    .next()
                    .unwrap()
                    .split(' ')
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let bearer = head
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("authorization: ")
                            .or(l.strip_prefix("Authorization: "))
                    })
                    .unwrap_or("")
                    .to_owned();
                out.lock().unwrap().push((
                    path,
                    bearer,
                    serde_json::from_slice(&body).unwrap_or(Value::Null),
                ));
                match mode {
                    StubMode::Hang => tokio::time::sleep(Duration::from_secs(60)).await,
                    StubMode::MintOk => {
                        let exp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs()
                            + 3600;
                        let b = json!({"ok": {"credentials": [
                            {"key": "access_token", "value": "minted-123"},
                            {"key": "installation_id", "value": "7"}], "expires_at": exp}})
                        .to_string();
                        let _ = sock
                            .write_all(
                                format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}", b.len())
                                    .as_bytes(),
                            )
                            .await;
                    }
                }
            });
        }
    });
    (url, seen)
}

async fn hold_policy(api: &Api) {
    api.set_policy(
        "block",
        json!([net_rule(
            "ask",
            "allow",
            HOST,
            json!({"require_approval": true})
        )]),
    )
    .await;
}

/// POST /v1/items through gateway A in a task; returns the join handle.
fn post_item(st: &Stack, token: &str) -> tokio::task::JoinHandle<(u16, String)> {
    let c = via_proxy(&st.gw_a, token, &st.ca_pem);
    tokio::spawn(async move {
        let r = c
            .post(format!("https://{HOST}/v1/items"))
            .header("authorization", "Bearer agent-forged")
            .json(&json!({"name": "widget"}))
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    })
}

async fn deny(api: &Api, a: &Value) {
    let (s, v) = api
        .post(
            &format!("/v1/approvals/{}/decision", a["id"].as_str().unwrap()),
            json!({"decision": "deny"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn h1_held_request_gets_hook_summary_without_egress() {
    let st = Stack::start(Opts {
        hookhost: true,
        ..Opts::default()
    })
    .await;
    let mut api = st.signup("owner@e2e.test").await;
    let (_, token) = api.byo_agent("hook-agent").await;
    install_fixture(&st, &mut api).await;
    st.wait_hookhost_ready().await;
    hold_policy(&api).await;
    // The first call pays the cold component compile (> 1 s budget in debug builds) and
    // falls back; the compile finishes in the background, so the next hold gets the hook.
    let h = post_item(&st, &token);
    let a = api.wait_pending().await;
    deny(&api, &a).await;
    h.await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let h = post_item(&st, &token);
    let a = api.wait_pending().await;
    let sum = &a["summary"];
    assert_eq!(sum["source"], "hook", "{sum}\n{}", st.log_tails());
    assert_eq!(sum["title"], "Create item");
    let fields = sum["fields"].as_array().unwrap();
    let f = |l: &str| {
        fields
            .iter()
            .find(|f| f["label"] == l)
            .map(|f| f["value"].clone())
    };
    assert_eq!(f("Egress"), Some(json!("denied")), "{sum}");
    assert_eq!(f("Path"), Some(json!("/v1/items")), "{sum}");
    deny(&api, &a).await;
    let (s, b) = h.await.unwrap();
    assert_eq!(
        (s, err_code(&b).as_str()),
        (403, "manual_approval_denied"),
        "{b}"
    );
    st.finish().await;
}

#[tokio::test]
async fn h2_mint_through_real_hookhost_failure_is_credential_unavailable() {
    let st = Stack::start(Opts {
        hookhost: true,
        ..Opts::default()
    })
    .await;
    let mut api = st.signup("owner@e2e.test").await;
    let (agent, token) = api.byo_agent("hook-agent").await;
    install_fixture(&st, &mut api).await;
    st.wait_hookhost_ready().await;
    let conn = connection(&st, &api, true).await;
    grant_conn(&st, &api, &agent, conn).await;
    api.set_policy("allow", json!([])).await;
    let c = via_proxy(&st.gw_a, &token, &st.ca_pem);
    let r = c
        .get(format!("https://{HOST}/v1/items"))
        .send()
        .await
        .unwrap();
    let (s, b) = (r.status().as_u16(), r.text().await.unwrap());
    // The guest's egress to the (unresolvable) auth host fails inside the sandbox: the hook
    // error must surface as credential_unavailable and nothing is forwarded upstream.
    assert_eq!(
        err_code(&b),
        "credential_unavailable",
        "{s} {b}\n{}",
        st.log_tails()
    );
    let m = reqwest::get(format!("http://{HH_IP}:4104/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        m.lines()
            .any(|l| l.contains("aurora_hook_calls_total") && l.contains("token.mint")),
        "hook host never saw token.mint:\n{m}"
    );
    st.finish().await;
}

#[tokio::test]
async fn h3_mint_ok_via_stub_rotates_stored_credentials() {
    let (url, seen) = stub_hookhost(StubMode::MintOk).await;
    let st = Stack::start(Opts {
        hookhost_url: Some(url),
        ..Opts::default()
    })
    .await;
    let mut api = st.signup("owner@e2e.test").await;
    let (agent, token) = api.byo_agent("hook-agent").await;
    install_fixture(&st, &mut api).await;
    let conn = connection(&st, &api, true).await;
    grant_conn(&st, &api, &agent, conn).await;
    api.set_policy("allow", json!([])).await;
    let c = via_proxy(&st.gw_a, &token, &st.ca_pem);
    // The upstream host is unresolvable, so the request itself fails after injection;
    // what matters is that the mint happened and was persisted.
    let _ = c.get(format!("https://{HOST}/v1/items")).send().await;
    let calls = seen.lock().unwrap().clone();
    let (path, bearer, body) = calls.first().expect("stub saw no call");
    assert_eq!(path, "/internal/v1/hooks/token.mint");
    assert!(bearer.starts_with("Bearer "), "{bearer}");
    assert_eq!(body["app_id"], "hooks-demo");
    assert_eq!(body["package_id"], "fixture-hooks");
    assert!(
        body["package_digest"]
            .as_str()
            .is_some_and(|d| !d.is_empty())
    );
    let (ver, fresh): (i64, bool) = sqlx::query_as(
        "SELECT credentials_version, credentials_expires_at > now() + interval '30 minutes' FROM app_connections WHERE id=$1",
    )
    .bind(conn)
    .fetch_one(st.pool())
    .await
    .unwrap();
    assert_eq!((ver, fresh), (2, true), "{}", st.log_tails());
    st.finish().await;
}

#[tokio::test]
async fn h4_looping_summarize_falls_back_within_budget() {
    let (url, seen) = stub_hookhost(StubMode::Hang).await;
    let st = Stack::start(Opts {
        hookhost_url: Some(url),
        ..Opts::default()
    })
    .await;
    let mut api = st.signup("owner@e2e.test").await;
    let (_, token) = api.byo_agent("hook-agent").await;
    install_fixture(&st, &mut api).await;
    hold_policy(&api).await;
    let t0 = Instant::now();
    let h = post_item(&st, &token);
    let a = api.wait_pending().await;
    let took = t0.elapsed();
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|c| c.0.ends_with("approval.summarize")),
        "summarize hook was never called"
    );
    // Template fallback (the fixture declares an `approvals.create_item` template).
    assert_eq!(a["summary"]["source"], "template", "{}", a["summary"]);
    assert_eq!(a["summary"]["title"], "Create item");
    assert!(took < Duration::from_millis(3500), "hold delayed {took:?}");
    deny(&api, &a).await;
    h.await.unwrap();
    st.finish().await;
}
