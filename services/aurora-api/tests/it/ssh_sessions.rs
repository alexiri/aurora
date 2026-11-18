#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SSH sessions and the terminator plane (phase 5, P4-SSH-2 Part A, D-239, SEC-94, SEC-95;
//! C-793, C-794, C-796): plane authentication (`aut_` only, one 401 body), the session open with
//! its refusals and its single `agent.ssh.open` row, single-use channel grants and their
//! redemption (`kind: "ssh"` only), heartbeat revocation per cause closing the row once, the
//! close-exactly-once gate under concurrency, the wake demand and the shared wake budget, the
//! C-793 authority matrix, the keep-awake reason `ssh` and the terminator facts of `GET /v1/ssh`.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use base64::Engine;
use http::Method;
use serde_json::{Value, json};
use ssh_key::certificate::{Builder, CertType};
use ssh_key::private::Ed25519Keypair;
use ssh_key::{Certificate, PrivateKey};
use uuid::Uuid;

const TOKEN: &str = "aut_test-terminator-anchor-token";
const ALICE: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN5ElGab515YLw7PcnKusC/LWNtScxqkSkx78EjhVfhl alice@laptop";

fn seed_value(seed: u8) -> String {
    format!(
        "ed25519:{}",
        base64::engine::general_purpose::STANDARD.encode([seed; 32])
    )
}

fn ca_key(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

fn key_line(n: u8) -> String {
    ca_key(n).public_key().to_openssh().unwrap()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// The SSH rig: SSH on, CA seed 0x5a, the terminator anchor token, one retired CA when asked.
async fn sess_rig(previous: Option<u8>) -> Rig {
    rig_with(move |c| {
        c.ssh.enabled = true;
        c.ssh.public_host = "ssh.example.org".into();
        c.ssh_ca_key = Some(aurora_config::Redacted::new(seed_value(0x5a)));
        c.bootstrap_terminator_token = Some(aurora_config::Redacted::new(TOKEN.to_owned()));
        if let Some(p) = previous {
            c.ssh.previous_public_keys = vec![key_line(p)];
        }
    })
    .await
}

async fn plane_as(
    r: &Rig,
    token: &str,
    uri: &str,
    body: Value,
    replica: Option<Uuid>,
) -> (u16, Value) {
    let b = bearer(token);
    let rid = replica.map(|x| x.to_string());
    let mut hd = vec![("authorization", b.as_str())];
    if let Some(x) = &rid {
        hd.push(("x-aurora-replica", x.as_str()));
    }
    let (s, _, v) = r.h.send(Method::POST, uri, &hd, Some(body)).await;
    (s.as_u16(), v)
}

async fn plane(r: &Rig, uri: &str, body: Value, replica: Uuid) -> (u16, Value) {
    plane_as(r, TOKEN, uri, body, Some(replica)).await
}

async fn register(r: &Rig, host: u8) -> Uuid {
    let (s, v) = plane_as(
        r,
        TOKEN,
        "/v1/terminator/register",
        json!({"version": "0.1.0", "host_public_key": key_line(host)}),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["protocol"], 1);
    v["replica_id"].as_str().unwrap().parse().unwrap()
}

async fn add_key(r: &Rig, line: &str) -> Uuid {
    let (s, v) = r
        .term_call(
            Method::POST,
            "/v1/me/ssh-keys",
            &[],
            Some(json!({"public_key": line})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    v["id"].as_str().unwrap().parse().unwrap()
}

async fn mint(r: &Rig, agent: Uuid, line: &str) -> String {
    let (s, v) = r
        .term_call(
            Method::POST,
            &format!("/v1/agents/{agent}/ssh-certificate"),
            &[],
            Some(json!({"public_key": line})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    v["certificate"].as_str().unwrap().to_owned()
}

async fn member_id(r: &Rig) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email = 'm@x.io'")
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

/// A hosted agent with a sandbox row on the rig's runner (`state`, a current boot).
async fn sandbox_for(r: &Rig, agent: Uuid, state: &str) -> (Uuid, Uuid) {
    let sb = Uuid::now_v7();
    let boot = Uuid::now_v7();
    let runner: Uuid = sqlx::query_scalar("SELECT id FROM runners LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sandboxes (id, agent_id, runner_id, channel_replica_id, state, boot_id) \
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(sb)
    .bind(agent)
    .bind(runner)
    .bind(r.rep)
    .bind(state)
    .bind(boot)
    .execute(r.h.pool())
    .await
    .unwrap();
    (sb, boot)
}

struct Setup {
    r: Rig,
    agent: Uuid,
    rep: Uuid,
    key: Uuid,
    cert: String,
    sb: Uuid,
    boot: Uuid,
}

async fn setup(state: &str) -> Setup {
    let r = sess_rig(None).await;
    let agent = r.agent("ssh-a", false).await;
    let (sb, boot) = sandbox_for(&r, agent, state).await;
    let key = add_key(&r, ALICE).await;
    let cert = mint(&r, agent, ALICE).await;
    let rep = register(&r, 9).await;
    Setup {
        r,
        agent,
        rep,
        key,
        cert,
        sb,
        boot,
    }
}

async fn open_with(r: &Rig, rep: Uuid, cert: &str, username: &str) -> (u16, Value) {
    plane(
        r,
        "/v1/terminator/sessions",
        json!({"certificate": cert, "username": username, "client_address": "203.0.113.77"}),
        rep,
    )
    .await
}

async fn open_ok(s: &Setup) -> Uuid {
    let (st, v) = open_with(&s.r, s.rep, &s.cert, &s.agent.to_string()).await;
    assert_eq!(st, 201, "{v}");
    v["session_id"].as_str().unwrap().parse().unwrap()
}

async fn audit_count(r: &Rig, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
        .bind(action)
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

/// Best-effort audit rows land asynchronously, each on its own: poll (bounded) until at least
/// `want` exist, and return the count then (fewer if the deadline passed).
async fn wait_audit_rows(r: &Rig, action: &str, want: i64) -> i64 {
    let mut n = 0;
    for _ in 0..200 {
        n = audit_count(r, action).await;
        if n >= want {
            return n;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    n
}

async fn session_state(r: &Rig, id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, close_reason FROM ssh_sessions WHERE id = $1")
        .bind(id)
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

async fn heartbeat(r: &Rig, rep: Uuid, ids: &[Uuid]) -> Value {
    let sessions: Vec<Value> = ids
        .iter()
        .map(|i| json!({"id": i, "channels_open": 1, "channels_total": 1, "bytes_in": 10, "bytes_out": 20}))
        .collect();
    let (s, v) = plane(
        r,
        "/v1/terminator/heartbeat",
        json!({"sessions": sessions}),
        rep,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v
}

// ---- plane authentication ---------------------------------------------------------------

/// Only the `aut_` anchor opens the plane; every other credential and every missing piece is the
/// same 401 body; an unregistered replica is 403 `replica_unknown`; SSH off is 404.
#[tokio::test]
async fn ssh_sessions_plane_auth_is_uniform() {
    let r = sess_rig(None).await;
    let body = json!({"version": "1", "host_public_key": key_line(9)});
    let (s0, v0) = plane_as(
        &r,
        "aut_wrong-token",
        "/v1/terminator/register",
        body.clone(),
        None,
    )
    .await;
    let (s1, v1) = plane_as(&r, &r.tok, "/v1/terminator/register", body.clone(), None).await;
    let (s2, _, v2) =
        r.h.send(
            Method::POST,
            "/v1/terminator/register",
            &[],
            Some(body.clone()),
        )
        .await;
    assert_eq!((s0, s1, s2.as_u16()), (401, 401, 401));
    assert_eq!(v0, v1);
    assert_eq!(v1, v2);
    // A valid replica header is required after register.
    let (s, v) = plane_as(
        &r,
        TOKEN,
        "/v1/terminator/heartbeat",
        json!({"sessions": []}),
        None,
    )
    .await;
    assert_eq!((s, code(&v)), (403, "replica_unknown"), "{v}");
    let (s, v) = plane_as(
        &r,
        TOKEN,
        "/v1/terminator/heartbeat",
        json!({"sessions": []}),
        Some(Uuid::now_v7()),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "replica_unknown"), "{v}");
    let rep = register(&r, 9).await;
    let v = heartbeat(&r, rep, &[]).await;
    assert_eq!(v["revoked"], json!([]));
    assert_eq!(v["config"]["heartbeat_secs"], 30);
    assert_eq!(v["config"]["lease_secs"], 90);
    assert_eq!(v["config"]["channels_per_session"], 4);
    let cas = v["config"]["ca_public_keys"].as_array().unwrap();
    assert_eq!(cas.len(), 1);
    assert!(cas[0].as_str().unwrap().starts_with("ssh-ed25519 "));
    // Off: 404 before any credential check.
    let off = rig_with(|c| {
        c.bootstrap_terminator_token = Some(aurora_config::Redacted::new(TOKEN.to_owned()));
    })
    .await;
    let (s, v) = plane_as(&off, TOKEN, "/v1/terminator/register", body, None).await;
    assert_eq!((s, code(&v)), (404, "ssh_disabled"), "{v}");
}

/// A registered replica shows up in `GET /v1/ssh` (live count, host key, `known_hosts`) and in
/// the admin list; `register` refuses a host key that is not one ssh-ed25519 line.
#[tokio::test]
async fn ssh_sessions_terminator_facts_and_admin_list() {
    let r = sess_rig(None).await;
    let (s, v) = plane_as(
        &r,
        TOKEN,
        "/v1/terminator/register",
        json!({"version": "1", "host_public_key": "ssh-rsa AAAA"}),
        None,
    )
    .await;
    assert_eq!((s, code(&v)), (422, "invalid_host_key"), "{v}");
    let (_, f) = r.term_call(Method::GET, "/v1/ssh", &[], None).await;
    assert_eq!(f["terminators_live"], 0);
    let rep = register(&r, 9).await;
    let _ = register(&r, 9).await; // a second replica, the same host key
    let (_, f) = r.term_call(Method::GET, "/v1/ssh", &[], None).await;
    assert_eq!(f["terminators_live"], 2, "{f}");
    assert_eq!(
        f["host_keys"].as_array().unwrap().len(),
        1,
        "distinct keys: {f}"
    );
    assert_eq!(f["known_hosts"].as_array().unwrap().len(), 1);
    // Admin only; a member gets 403.
    let (s, _) = r
        .term_call(Method::GET, "/v1/admin/terminators", &[], None)
        .await;
    assert_eq!(s, 403);
    let admin = r.h.user("ia@x.io", true).await;
    let (_, sess) = r.h.good_session(admin).await;
    let (s, _, v) =
        r.h.send(
            Method::GET,
            "/v1/admin/terminators",
            &[("cookie", &cookie(&sess)), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    let t = v["terminators"].as_array().unwrap();
    assert_eq!(t.len(), 2);
    assert!(t.iter().all(|x| x["live"] == true));
    assert!(t.iter().any(|x| x["id"] == rep.to_string().as_str()));
}

// ---- session open ----------------------------------------------------------------------

/// A valid certificate opens a session (201), writes one `agent.ssh.open` audit row without the
/// certificate, stamps the key's `last_used_at` and coarsens the client address.
#[tokio::test]
async fn ssh_sessions_open_succeeds_with_one_audit_row() {
    let s = setup("running").await;
    let (st, v) = open_with(&s.r, s.rep, &s.cert, &s.agent.to_string()).await;
    assert_eq!(st, 201, "{v}");
    assert_eq!(v["sandbox"], "running");
    assert_eq!(v["agent_id"], s.agent.to_string());
    assert_eq!(v["lease_secs"], 90);
    let id: Uuid = v["session_id"].as_str().unwrap().parse().unwrap();
    let (addr, lease_ok): (Option<String>, bool) = sqlx::query_as(
        "SELECT client_address, lease_until > now() FROM ssh_sessions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(s.r.h.pool())
    .await
    .unwrap();
    assert!(lease_ok);
    assert_eq!(
        addr.as_deref(),
        Some("203.0.113.0/24"),
        "coarsened at insert"
    );
    let last: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_used_at FROM ssh_keys WHERE id = $1")
            .bind(s.key)
            .fetch_one(s.r.h.pool())
            .await
            .unwrap();
    assert!(last.is_some());
    assert_eq!(wait_audit_rows(&s.r, "agent.ssh.open", 1).await, 1);
    let meta: Value =
        sqlx::query_scalar("SELECT metadata FROM audit_log WHERE action = 'agent.ssh.open'")
            .fetch_one(s.r.h.pool())
            .await
            .unwrap();
    assert!(
        !meta.to_string().contains("ssh-ed25519-cert"),
        "no certificate in audit"
    );
    assert_eq!(meta["client_address"], "203.0.113.0/24");
}

async fn refused(s: &Setup, cert: &str, username: &str) -> (u16, Value) {
    open_with(&s.r, s.rep, cert, username).await
}

/// Every refusal of SEC-94 is the same hint-free 403 `{reason: "denied"}` (and `limit` over a cap).
#[tokio::test]
async fn ssh_sessions_open_refusals() {
    let s = setup("running").await;
    let agent = s.agent.to_string();
    let denied = json!({"reason": "denied"});

    // Garbage, a username that is not the principal, a certificate of the wrong shape.
    assert_eq!(
        refused(&s, "not a certificate", &agent).await,
        (403, denied.clone())
    );
    assert_eq!(
        refused(&s, &s.cert, &Uuid::now_v7().to_string()).await,
        (403, denied.clone())
    );

    // A foreign CA: same fields, signed by another key.
    let user = member_id(&s.r).await;
    let key_id = format!("aurora:u={user}:k={}:a={}:w={}", s.key, s.agent, s.r.ws);
    let build = |ca: u8, key_id: &str, principal: &str| -> String {
        let subject = Certificate::from_openssh(&s.cert)
            .unwrap()
            .public_key()
            .clone();
        let now = chrono::Utc::now().timestamp() as u64;
        let mut b = Builder::new([7u8; 32], subject, now - 60, now + 600).unwrap();
        b.serial(5).unwrap();
        b.cert_type(CertType::User).unwrap();
        b.key_id(key_id.to_owned()).unwrap();
        b.valid_principal(principal.to_owned()).unwrap();
        b.extension("permit-pty", "").unwrap();
        b.sign(&ca_key(ca)).unwrap().to_openssh().unwrap()
    };
    assert_eq!(
        refused(&s, &build(0x77, &key_id, &agent), &agent).await,
        (403, denied.clone())
    );
    // The right CA but a key id naming another key / another agent: the binding fails.
    let wrong_key = format!(
        "aurora:u={user}:k={}:a={}:w={}",
        Uuid::now_v7(),
        s.agent,
        s.r.ws
    );
    assert_eq!(
        refused(&s, &build(0x5a, &wrong_key, &agent), &agent).await,
        (403, denied.clone())
    );
    let wrong_ws = format!(
        "aurora:u={user}:k={}:a={}:w={}",
        s.key,
        s.agent,
        Uuid::now_v7()
    );
    assert_eq!(
        refused(&s, &build(0x5a, &wrong_ws, &agent), &agent).await,
        (403, denied.clone())
    );
    // And the right CA with the right key id is accepted (the builder is not what failed).
    let (st, v) = refused(&s, &build(0x5a, &key_id, &agent), &agent).await;
    assert_eq!(st, 201, "{v}");

    // The registered fingerprint changed under the key id (a different key): denied.
    sqlx::query("UPDATE ssh_keys SET fingerprint = 'SHA256:other' WHERE id = $1")
        .bind(s.key)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    assert_eq!(refused(&s, &s.cert, &agent).await, (403, denied.clone()));
    // A deleted key: denied.
    sqlx::query("DELETE FROM ssh_keys WHERE id = $1")
        .bind(s.key)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    assert_eq!(refused(&s, &s.cert, &agent).await, (403, denied.clone()));
}

/// Lost access, a frozen agent and a BYO agent are refused identically at open.
#[tokio::test]
async fn ssh_sessions_open_refuses_lost_access_frozen_and_byo() {
    let s = setup("running").await;
    let agent = s.agent.to_string();
    let denied = (403, json!({"reason": "denied"}));
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'workspace_owner' WHERE id = $1",
    )
    .bind(s.agent)
    .execute(s.r.h.pool())
    .await
    .unwrap();
    assert_eq!(refused(&s, &s.cert, &agent).await, denied);
    sqlx::query("UPDATE agents SET frozen_at = NULL, frozen_reason = NULL, frozen_authority = NULL WHERE id = $1")
        .bind(s.agent)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    let user = member_id(&s.r).await;
    sqlx::query("DELETE FROM workspace_access WHERE workspace_id = $1 AND user_id = $2")
        .bind(s.r.ws)
        .bind(user)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    assert_eq!(refused(&s, &s.cert, &agent).await, denied);
}

/// The caps count open, lease-current rows globally: the per-user cap (4) and the per-agent cap
/// (8, shown by lowering `sessions_per_agent`) answer `limit`; a closed row frees its slot.
#[tokio::test]
async fn ssh_sessions_caps_are_global_and_free_on_close() {
    let s = setup("running").await;
    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(open_ok(&s).await);
    }
    let (st, v) = open_with(&s.r, s.rep, &s.cert, &s.agent.to_string()).await;
    assert_eq!((st, v), (403, json!({"reason": "limit"})));
    // A lease-expired row does not count even before the sweep closes it.
    sqlx::query("UPDATE ssh_sessions SET lease_until = now() - interval '1 second' WHERE id = $1")
        .bind(ids[0])
        .execute(s.r.h.pool())
        .await
        .unwrap();
    ids.push(open_ok(&s).await);
    let (st, _) = plane(
        &s.r,
        &format!("/v1/terminator/sessions/{}/close", ids[1]),
        json!({"reason": "client_disconnect", "channels_total": 0, "bytes_in": 0, "bytes_out": 0}),
        s.rep,
    )
    .await;
    assert_eq!(st, 200);
    open_ok(&s).await;
}

// ---- wake demand ------------------------------------------------------------------------

/// A parked sandbox gets the wake demand at open (`terminal_wake_at`), through the budget the
/// terminal shares; once the hourly budget is spent the open is refused as `limit`.
#[tokio::test]
async fn ssh_sessions_wake_demand_uses_the_shared_budget() {
    let s = setup("stopped").await;
    let (st, v) = open_with(&s.r, s.rep, &s.cert, &s.agent.to_string()).await;
    assert_eq!(st, 201, "{v}");
    assert_eq!(v["sandbox"], "waking");
    let at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT terminal_wake_at FROM sandboxes WHERE id = $1")
            .bind(s.sb)
            .fetch_one(s.r.h.pool())
            .await
            .unwrap();
    assert!(at.is_some());
    // 11 more demands exhaust the 12/hour budget of this agent (each session closed again).
    for _ in 0..11 {
        let (st, v) = open_with(&s.r, s.rep, &s.cert, &s.agent.to_string()).await;
        assert_eq!(st, 201, "{v}");
        let id = v["session_id"].as_str().unwrap();
        plane(
            &s.r,
            &format!("/v1/terminator/sessions/{id}/close"),
            json!({"reason": "client_disconnect", "channels_total": 0, "bytes_in": 0, "bytes_out": 0}),
            s.rep,
        )
        .await;
    }
    let (st, v) = open_with(&s.r, s.rep, &s.cert, &s.agent.to_string()).await;
    assert_eq!((st, v), (403, json!({"reason": "limit"})));
}

// ---- grants and redemption --------------------------------------------------------------

async fn grant(s: &Setup, session: Uuid, mode: &str) -> (u16, Value) {
    plane(
        &s.r,
        &format!("/v1/terminator/sessions/{session}/channels"),
        json!({"mode": mode}),
        s.rep,
    )
    .await
}

async fn redeem(s: &Setup, grant: &str, kind: &str) -> (u16, Value) {
    s.r.runner_call(
        "/v1/runner/preview-streams/open",
        json!({"grant": grant, "kind": kind}),
        s.r.rep,
    )
    .await
}

/// A grant is `auz_…`, single use, redeems only as `kind: "ssh"`, answers the binding, and is
/// refused when the boot changed, the session closed or the TTL passed; other kinds never see it.
#[tokio::test]
async fn ssh_sessions_grants_are_single_use_and_bound() {
    let s = setup("running").await;
    let id = open_ok(&s).await;
    let (st, g) = grant(&s, id, "shell").await;
    assert_eq!(st, 200, "{g}");
    let tok = g["grant"].as_str().unwrap().to_owned();
    assert!(tok.starts_with("auz_"));
    assert_eq!(g["expires_in"], 30);
    assert!(
        g["runner_endpoint"]
            .as_str()
            .unwrap()
            .contains("/v1/preview-stream")
    );
    // Not the terminal's or files' door.
    assert_eq!(redeem(&s, &tok, "terminal").await.0, 403);
    assert_eq!(redeem(&s, &tok, "files").await.0, 403);
    assert_eq!(redeem(&s, &tok, "port").await.0, 403);
    let (st, b) = redeem(&s, &tok, "ssh").await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["sandbox_id"], s.sb.to_string());
    assert_eq!(b["boot_id"], s.boot.to_string());
    assert!(b["port"].is_null());
    // Single use.
    assert_eq!(redeem(&s, &tok, "ssh").await.0, 403);
    // Stale boot.
    let (_, g2) = grant(&s, id, "exec").await;
    sqlx::query("UPDATE sandboxes SET boot_id = $2 WHERE id = $1")
        .bind(s.sb)
        .bind(Uuid::now_v7())
        .execute(s.r.h.pool())
        .await
        .unwrap();
    assert_eq!(
        redeem(&s, g2["grant"].as_str().unwrap(), "ssh").await.0,
        409
    );
    sqlx::query("UPDATE sandboxes SET boot_id = $2 WHERE id = $1")
        .bind(s.sb)
        .bind(s.boot)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    // Expired.
    let (_, g3) = grant(&s, id, "sftp").await;
    sqlx::query("UPDATE ssh_stream_grants SET expires_at = now() - interval '1 second'")
        .execute(s.r.h.pool())
        .await
        .unwrap();
    assert_eq!(
        redeem(&s, g3["grant"].as_str().unwrap(), "ssh").await.0,
        403
    );
    // A closed session: no grant, and an outstanding one is refused at redemption.
    let (_, g4) = grant(&s, id, "shell").await;
    plane(
        &s.r,
        &format!("/v1/terminator/sessions/{id}/close"),
        json!({"reason": "client_disconnect", "channels_total": 1, "bytes_in": 1, "bytes_out": 1}),
        s.rep,
    )
    .await;
    assert_eq!(
        redeem(&s, g4["grant"].as_str().unwrap(), "ssh").await.0,
        403
    );
    let (st, v) = grant(&s, id, "shell").await;
    assert_eq!((st, code(&v)), (403, "session_closed"), "{v}");
    // The hash, never the value, is what is stored.
    let leaked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ssh_stream_grants WHERE grant_hash = $1")
            .bind(&tok)
            .fetch_one(s.r.h.pool())
            .await
            .unwrap();
    assert_eq!(leaked, 0);
}

/// A stopped sandbox is a 409 `not_running` after the wait (and the wake demand is set); a
/// foreign replica's grant call is `session_closed`.
#[tokio::test]
async fn ssh_sessions_grant_waits_for_a_parked_sandbox() {
    let s = setup("stopped").await;
    let id = open_ok(&s).await;
    let (st, v) = grant(&s, id, "shell").await;
    assert_eq!((st, code(&v)), (409, "not_running"), "{v}");
    assert_eq!(v["error"]["details"]["retry_after_secs"], 2);
    let other = register(&s.r, 8).await;
    let (st, v) = plane(
        &s.r,
        &format!("/v1/terminator/sessions/{id}/channels"),
        json!({"mode": "shell"}),
        other,
    )
    .await;
    assert_eq!((st, code(&v)), (403, "session_closed"), "{v}");
}

// ---- heartbeat revocation and close-exactly-once ----------------------------------------

/// Each cause closes the row at detection with its reason, answers it revoked, and writes exactly
/// one `agent.ssh.close` row; an unknown id comes back revoked `unknown`.
#[tokio::test]
async fn ssh_sessions_heartbeat_revokes_per_cause_and_closes_once() {
    let s = setup("running").await;
    let kept = open_ok(&s).await;
    let key_gone = open_ok(&s).await;
    let v = heartbeat(&s.r, s.rep, &[kept, key_gone, Uuid::now_v7()]).await;
    assert_eq!(v["revoked"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["revoked"][0]["reason"], "unknown");
    let lease1: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT lease_until FROM ssh_sessions WHERE id = $1")
            .bind(kept)
            .fetch_one(s.r.h.pool())
            .await
            .unwrap();
    // Key deletion (SET NULL) → key_deleted for every session of the key.
    sqlx::query("DELETE FROM ssh_keys WHERE id = $1")
        .bind(s.key)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    let v = heartbeat(&s.r, s.rep, &[kept, key_gone]).await;
    let rev = v["revoked"].as_array().unwrap();
    assert_eq!(rev.len(), 2, "{v}");
    assert!(rev.iter().all(|x| x["reason"] == "key_deleted"));
    assert_eq!(
        session_state(&s.r, kept).await,
        ("closed".into(), Some("key_deleted".into()))
    );
    assert!(lease1 > chrono::Utc::now());
    // A heartbeat for an already-closed session is revoked again with the recorded reason and
    // writes nothing more.
    let v = heartbeat(&s.r, s.rep, &[kept]).await;
    assert_eq!(v["revoked"][0]["reason"], "key_deleted");
    assert_eq!(wait_audit_rows(&s.r, "agent.ssh.close", 2).await, 2);
    assert_eq!(audit_count(&s.r, "agent.ssh.close").await, 2);
}

/// Frozen and lost access revoke with their own reasons (a fresh key per case).
#[tokio::test]
async fn ssh_sessions_heartbeat_revokes_frozen_and_lost_access() {
    let s = setup("running").await;
    let a = open_ok(&s).await;
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'workspace_owner' WHERE id = $1",
    )
    .bind(s.agent)
    .execute(s.r.h.pool())
    .await
    .unwrap();
    let v = heartbeat(&s.r, s.rep, &[a]).await;
    assert_eq!(v["revoked"][0]["reason"], "frozen", "{v}");
    sqlx::query("UPDATE agents SET frozen_at = NULL, frozen_reason = NULL, frozen_authority = NULL WHERE id = $1")
        .bind(s.agent)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    let b = open_ok(&s).await;
    let user = member_id(&s.r).await;
    sqlx::query("DELETE FROM workspace_access WHERE workspace_id = $1 AND user_id = $2")
        .bind(s.r.ws)
        .bind(user)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    let v = heartbeat(&s.r, s.rep, &[b]).await;
    assert_eq!(v["revoked"][0]["reason"], "access_revoked", "{v}");
    assert_eq!(
        session_state(&s.r, b).await.1.as_deref(),
        Some("access_revoked")
    );
}

/// The guarded transition lets exactly one of many concurrent closers win (terminator close,
/// heartbeat detection and the sweep share it), and the winner's audit row is the only one.
#[tokio::test]
async fn ssh_sessions_close_is_exactly_once_under_concurrency() {
    let s = setup("running").await;
    let id = open_ok(&s).await;
    sqlx::query("UPDATE ssh_sessions SET lease_until = now() - interval '1 second' WHERE id = $1")
        .bind(id)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    let pool = s.r.h.pool().clone();
    let mut tasks = Vec::new();
    for i in 0..8 {
        let p = pool.clone();
        tasks.push(tokio::spawn(async move {
            if i % 2 == 0 {
                aurora_db::repo::ssh_sessions::close_session(&p, id, "client_disconnect", 1, 1, 1)
                    .await
                    .unwrap()
                    .map(|_| 1)
                    .unwrap_or(0)
            } else {
                aurora_db::repo::ssh_sessions::sweep_expired(&p, 10)
                    .await
                    .unwrap()
                    .len()
            }
        }));
    }
    let mut won = 0;
    for t in tasks {
        won += t.await.unwrap();
    }
    assert_eq!(won, 1, "exactly one transition");
    // Over HTTP: closing again, or after the sweep, is `{ok: true}` and writes no second row.
    for reason in ["client_disconnect", "idle_timeout"] {
        let (st, v) = plane(
            &s.r,
            &format!("/v1/terminator/sessions/{id}/close"),
            json!({"reason": reason, "channels_total": 9, "bytes_in": 9, "bytes_out": 9}),
            s.rep,
        )
        .await;
        assert_eq!((st, &v), (200, &json!({"ok": true})));
    }
    let (_, reason): (String, Option<String>) = session_state(&s.r, id).await;
    assert!(matches!(
        reason.as_deref(),
        Some("client_disconnect" | "lease_expired")
    ));
    // Only the HTTP path and the sweep write audit rows; the repo-level winner here did not, so
    // no row may exceed one in total.
    assert!(audit_count(&s.r, "agent.ssh.close").await <= 1);
}

/// The sweep closes lease-expired rows (`lease_expired`) with one audit row each.
#[tokio::test]
async fn ssh_sessions_sweep_closes_lease_expired_rows_once() {
    let s = setup("running").await;
    let id = open_ok(&s).await;
    sqlx::query("UPDATE ssh_sessions SET lease_until = now() - interval '1 second' WHERE id = $1")
        .bind(id)
        .execute(s.r.h.pool())
        .await
        .unwrap();
    let n = aurora_api::domains::ssh::sessions::sweep(&s.r.h.state)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        session_state(&s.r, id).await,
        ("closed".into(), Some("lease_expired".into()))
    );
    assert_eq!(
        aurora_api::domains::ssh::sessions::sweep(&s.r.h.state)
            .await
            .unwrap(),
        0
    );
    assert_eq!(wait_audit_rows(&s.r, "agent.ssh.close", 1).await, 1);
}

// ---- C-793 --------------------------------------------------------------------------------

/// The lists and the disconnect: own sessions, an owner's view with the person, a member's own
/// only, everyone else 404; disconnect marks the row and the next heartbeat revokes it.
#[tokio::test]
async fn ssh_sessions_lists_and_disconnect_authority() {
    let s = setup("running").await;
    let id = open_ok(&s).await;
    let (st, v) =
        s.r.term_call(Method::GET, "/v1/me/ssh-sessions", &[], None)
            .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(v["sessions"][0]["id"], id.to_string());
    assert_eq!(v["sessions"][0]["key"]["name"], "alice@laptop");
    assert_eq!(v["sessions"][0]["agent"]["id"], s.agent.to_string());
    assert!(
        v["sessions"][0].get("user").is_none(),
        "no person on a member's own list"
    );
    let (st, v) =
        s.r.term_call(
            Method::GET,
            &format!("/v1/agents/{}/ssh-sessions", s.agent),
            &[],
            None,
        )
        .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["sessions"].as_array().unwrap().len(), 1);

    // An owner sees everyone's, with `user`.
    let owner = s.r.h.user("o@x.io", false).await;
    s.r.h.member(s.r.org, owner, "member", "active").await;
    s.r.h.bind_ws(s.r.ws, owner, "owner").await;
    let (_, osess) = s.r.h.good_session(owner).await;
    let oc = cookie(&osess);
    let ohd = [("cookie", oc.as_str()), ("origin", ORIGIN)];
    let (st, _, v) =
        s.r.h
            .send(
                Method::GET,
                &format!("/v1/agents/{}/ssh-sessions", s.agent),
                &ohd,
                None,
            )
            .await;
    assert_eq!(st.as_u16(), 200, "{v}");
    assert_eq!(
        v["sessions"][0]["user"]["id"],
        member_id(&s.r).await.to_string()
    );
    // A stranger (not in the workspace) gets 404 on the list and the disconnect.
    let stranger = s.r.h.user("x@x.io", false).await;
    let (_, ssess) = s.r.h.good_session(stranger).await;
    let sc = cookie(&ssess);
    let shd = [("cookie", sc.as_str()), ("origin", ORIGIN)];
    let (st, _, _) =
        s.r.h
            .send(
                Method::GET,
                &format!("/v1/agents/{}/ssh-sessions", s.agent),
                &shd,
                None,
            )
            .await;
    assert_eq!(st.as_u16(), 404);
    let (st, _, v) =
        s.r.h
            .send(
                Method::DELETE,
                &format!("/v1/ssh-sessions/{id}"),
                &shd,
                None,
            )
            .await;
    assert_eq!(
        (st.as_u16(), code(&v)),
        (404, "ssh_session_not_found"),
        "{v}"
    );

    // The owner disconnects: 202, the heartbeat answers it revoked with the owner reason.
    let (st, _, v) =
        s.r.h
            .send(
                Method::DELETE,
                &format!("/v1/ssh-sessions/{id}"),
                &ohd,
                None,
            )
            .await;
    assert_eq!(
        (st.as_u16(), &v["disconnecting"]),
        (202, &json!(true)),
        "{v}"
    );
    let v = heartbeat(&s.r, s.rep, &[id]).await;
    assert_eq!(v["revoked"][0]["reason"], "owner_disconnect", "{v}");
    assert_eq!(wait_audit_rows(&s.r, "ssh_session.disconnect", 1).await, 1);
    // The person disconnects their own: user reason.
    let id2 = open_ok(&s).await;
    let (st, v) =
        s.r.term_call(
            Method::DELETE,
            &format!("/v1/ssh-sessions/{id2}"),
            &[],
            None,
        )
        .await;
    assert_eq!((st, &v["disconnecting"]), (202, &json!(true)), "{v}");
    let v = heartbeat(&s.r, s.rep, &[id2]).await;
    assert_eq!(v["revoked"][0]["reason"], "user_disconnect", "{v}");
    // Closed sessions leave the lists.
    let (_, v) =
        s.r.term_call(Method::GET, "/v1/me/ssh-sessions", &[], None)
            .await;
    assert_eq!(v["sessions"].as_array().unwrap().len(), 0);
}

// ---- keep-awake ---------------------------------------------------------------------------

/// The runner's keep-awake with `kind: "ssh"` stamps `ssh_active_until` (reason `ssh` in the
/// sandbox view), never the terminal's; absent kind stays the terminal's; an unknown kind is 422.
#[tokio::test]
async fn ssh_sessions_keep_awake_kind_ssh() {
    let s = setup("running").await;
    let (st, v) =
        s.r.runner_call(
            "/v1/runner/terminal/keep-awake",
            json!({"sandbox_id": s.sb, "kind": "bogus"}),
            s.r.rep,
        )
        .await;
    assert_eq!((st, code(&v)), (422, "invalid_kind"), "{v}");
    let (st, v) =
        s.r.runner_call(
            "/v1/runner/terminal/keep-awake",
            json!({"sandbox_id": s.sb, "kind": "ssh"}),
            s.r.rep,
        )
        .await;
    assert_eq!(st, 200, "{v}");
    let (ssh, term, since): (bool, bool, bool) = sqlx::query_as(
        "SELECT ssh_active_until > now(), terminal_active_until IS NOT NULL, ssh_awake_since IS NOT NULL FROM sandboxes WHERE id = $1",
    )
    .bind(s.sb)
    .fetch_one(s.r.h.pool())
    .await
    .unwrap();
    assert!(ssh && !term && since);
    let (st, view) =
        s.r.term_call(
            Method::GET,
            &format!("/v1/agents/{}/sandbox", s.agent),
            &[],
            None,
        )
        .await;
    assert_eq!(st, 200, "{view}");
    assert!(
        view["keep_awake_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "ssh"),
        "{view}"
    );
    // The terminal's stamp (absent kind) does not touch the SSH clock.
    sqlx::query(
        "UPDATE sandboxes SET ssh_active_until = NULL, ssh_awake_since = NULL WHERE id = $1",
    )
    .bind(s.sb)
    .execute(s.r.h.pool())
    .await
    .unwrap();
    let (st, _) =
        s.r.runner_call(
            "/v1/runner/terminal/keep-awake",
            json!({"sandbox_id": s.sb}),
            s.r.rep,
        )
        .await;
    assert_eq!(st, 200);
    let ssh: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT ssh_active_until FROM sandboxes WHERE id = $1")
            .bind(s.sb)
            .fetch_one(s.r.h.pool())
            .await
            .unwrap();
    assert!(ssh.is_none());
}

/// (Part B, C-796) The runner registration answer carries `ssh_enabled` only when the instance
/// has `ssh.enabled` **and** the runner advertised `ssh`; otherwise the field is absent (false).
#[tokio::test(flavor = "multi_thread")]
async fn ssh_runner_registration_answers_ssh_enabled_only_for_a_runner_with_it() {
    let r = sess_rig(None).await;
    for (adv, want) in [(Some(true), true), (Some(false), false), (None, false)] {
        let mut caps = crate::hosted::caps();
        if let Some(b) = adv {
            caps["ssh"] = json!(b);
        }
        let (s, v) = r
            .runner(
                None,
                "/v1/runner/register",
                json!({"version": "0.1", "backend": "fake", "capabilities": caps,
                       "advertised_host": "runner-a"}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(
            v["config"].get("ssh_enabled").and_then(Value::as_bool),
            want.then_some(true),
            "advertised {adv:?}: {v}"
        );
    }
    // SSH off on the instance: a runner that advertises `ssh` is still not enabled.
    let off = rig_with(|_| {}).await;
    let mut caps = crate::hosted::caps();
    caps["ssh"] = json!(true);
    let (s, v) = off
        .runner(
            None,
            "/v1/runner/register",
            json!({"version": "0.1", "backend": "fake", "capabilities": caps,
                   "advertised_host": "runner-a"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["config"].get("ssh_enabled").is_none(), "{v}");
}
