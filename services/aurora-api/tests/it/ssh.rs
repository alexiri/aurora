#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SSH keys and the SSH certificate mint (phase 5, P4-SSH-1, D-239, SEC-93, C-790..C-792;
//! docs/18 §SSH access): key registration (canonical form, the `ssh-keygen -lf` fingerprint, the
//! refusals, the cap, session-only writes, critical audit), the mint matrix (session, own keys,
//! service key, anonymous, `can_use`, BYO, freeze, unregistered key), the certificate's exact
//! fields (verified against the CA public key with `ssh-key` and, when present, `ssh-keygen`),
//! the exact mint ledger across two API pools, the 24 h retention sweep, `ssh_disabled` and the
//! startup refusal. The secret-grep (CA seed in no log line and no body) is `tests/ssh_logs.rs`
//! (its own binary: a scoped tracing subscriber).

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use base64::Engine;
use http::Method;
use serde_json::{Value, json};
use ssh_key::certificate::CertType;
use ssh_key::private::Ed25519Keypair;
use ssh_key::{Certificate, HashAlg, PrivateKey};
use uuid::Uuid;

/// A registered-key fixture generated with `ssh-keygen -t ed25519 -C alice@laptop`;
/// `ALICE_FP` is what `ssh-keygen -lf` printed for it (checked in, not recomputed).
const ALICE: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN5ElGab515YLw7PcnKusC/LWNtScxqkSkx78EjhVfhl alice@laptop";
const ALICE_CANONICAL: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN5ElGab515YLw7PcnKusC/LWNtScxqkSkx78EjhVfhl";
const ALICE_FP: &str = "SHA256:gOAeoDRLhPIlNGAongiqcd5fskHrxCKhw3I7so57P3c";
const RSA: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDcjw6trCCvia0evQzTJn80SEOmTgPV/LXxdK5loGnv7hpo7GI0EP1n6kOXEIY/zHZ1DY2xvSuemVvMQkRW6W3B7LuJnq/Y79/X9sQji7spTicqSlSoKmAS9kZ3tN+6sk9ebfL91xcDWXMvvdh67Igc8n/bu9WzaIjge0KsV1Mp4pl+/03IuNCs8fF3qaBKlovvESvnuAazJ0aw6aqcOuakaoJsfN6kcZ44Vf7jH62HFatUedGSspzCvdG85pffbzCjmwdAbOgmJt7oW+3iQTaCWY2+q0Ykg8QRHD2DnycisPkW3A4fg6aOvhr34MotJgwlySa0nKPULH7gtO3LiHDP rsa@x";
const ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBPXrNQ4THPdlH2gs08aflviQMgq7dG83tYnMPnvZ9/9ZO6LIERc9YOOX7YrzW6HN7FAVMlJADgCzRWvlLbSXLhs= ecdsa@x";

/// The CA seed the rigs use (a known value, so the secret-grep can look for it).
pub(crate) fn ca_key_value() -> String {
    format!(
        "ed25519:{}",
        base64::engine::general_purpose::STANDARD.encode([0x5au8; 32])
    )
}

fn ca_public() -> ssh_key::PublicKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[0x5a; 32]))
        .public_key()
        .clone()
}

/// A deterministic ed25519 public key line (`n` picks the key).
fn key_line(n: u8) -> String {
    PrivateKey::from(Ed25519Keypair::from_seed(&[n; 32]))
        .public_key()
        .to_openssh()
        .unwrap()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

pub(crate) async fn ssh_rig() -> Rig {
    rig_with(|c| {
        c.ssh.enabled = true;
        c.ssh.public_host = "ssh.example.org".into();
        c.ssh_ca_key = Some(aurora_config::Redacted::new(ca_key_value()));
    })
    .await
}

async fn member_id(r: &Rig) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email = 'm@x.io'")
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

async fn add_key(r: &Rig, line: &str) -> (u16, Value) {
    r.term_call(
        Method::POST,
        "/v1/me/ssh-keys",
        &[],
        Some(json!({"public_key": line})),
    )
    .await
}

async fn mint(r: &Rig, agent: Uuid, line: &str) -> (u16, Value) {
    r.term_call(
        Method::POST,
        &format!("/v1/agents/{agent}/ssh-certificate"),
        &[],
        Some(json!({"public_key": line})),
    )
    .await
}

async fn mint_as(
    r: &Rig,
    headers: &[(&str, &str)],
    agent: Uuid,
    line: &str,
) -> (u16, http::HeaderMap, Value) {
    let (s, h, v) =
        r.h.send(
            Method::POST,
            &format!("/v1/agents/{agent}/ssh-certificate"),
            headers,
            Some(json!({"public_key": line})),
        )
        .await;
    (s.as_u16(), h, v)
}

async fn wait_audit(r: &Rig, action: &str) -> Value {
    for _ in 0..200 {
        let row: Option<Value> = sqlx::query_scalar(
            "SELECT metadata FROM audit_log WHERE action = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(action)
        .fetch_optional(r.h.pool())
        .await
        .unwrap();
        if let Some(m) = row {
            return m;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no audit row {action}");
}

// ---- C-790 keys -----------------------------------------------------------------------------

/// A valid key is stored canonical (comment dropped) with the `ssh-keygen -lf` fingerprint; the
/// name defaults to the comment; list/rename/delete work; the audit rows are critical and carry
/// the name and fingerprint only.
#[tokio::test]
async fn ssh_keys_add_list_rename_delete_with_critical_audit() {
    let r = ssh_rig().await;
    let (s, v) = add_key(&r, &format!("  {ALICE}\n")).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["public_key"], ALICE_CANONICAL);
    assert_eq!(v["fingerprint"], ALICE_FP);
    assert_eq!(v["name"], "alice@laptop");
    assert_eq!(v["algorithm"], "ssh-ed25519");
    assert!(v["last_used_at"].is_null());
    let id = v["id"].as_str().unwrap().to_owned();

    // An explicit name wins; no comment and no name gives the default.
    let (s, v2) = r
        .term_call(
            Method::POST,
            "/v1/me/ssh-keys",
            &[],
            Some(json!({"public_key": key_line(2), "name": "  Phone "})),
        )
        .await;
    assert_eq!((s, v2["name"].as_str()), (201, Some("Phone")), "{v2}");
    let (s, v3) = add_key(&r, &key_line(3)).await;
    assert_eq!((s, v3["name"].as_str()), (201, Some("SSH key")), "{v3}");
    let (s, v4) = r
        .term_call(
            Method::POST,
            "/v1/me/ssh-keys",
            &[],
            Some(json!({"public_key": key_line(4), "name": "x".repeat(81)})),
        )
        .await;
    assert_eq!((s, code(&v4)), (400, "invalid_request"), "{v4}");

    let (s, l) = r.term_call(Method::GET, "/v1/me/ssh-keys", &[], None).await;
    assert_eq!(s, 200, "{l}");
    assert_eq!(l["keys"].as_array().unwrap().len(), 3);
    // Newest first.
    assert_eq!(l["keys"][2]["id"], id.as_str());
    assert_eq!(l["keys"][0]["name"], "SSH key");

    let (s, p) = r
        .term_call(
            Method::PATCH,
            &format!("/v1/me/ssh-keys/{id}"),
            &[],
            Some(json!({"name": "  Work laptop "})),
        )
        .await;
    assert_eq!((s, p["name"].as_str()), (200, Some("Work laptop")), "{p}");
    let (s, p) = r
        .term_call(
            Method::PATCH,
            &format!("/v1/me/ssh-keys/{id}"),
            &[],
            Some(json!({"name": "   "})),
        )
        .await;
    assert_eq!((s, code(&p)), (400, "invalid_request"), "{p}");

    let (s, _) = r
        .term_call(Method::DELETE, &format!("/v1/me/ssh-keys/{id}"), &[], None)
        .await;
    assert_eq!(s, 204);
    let (s, d) = r
        .term_call(Method::DELETE, &format!("/v1/me/ssh-keys/{id}"), &[], None)
        .await;
    assert_eq!((s, code(&d)), (404, "not_found"), "{d}");

    // Critical audit rows, name + fingerprint only (never the key line).
    for (action, name) in [
        ("ssh_key.create", "alice@laptop"),
        ("ssh_key.delete", "Work laptop"),
    ] {
        let rows: Vec<(Value, String)> = sqlx::query_as(
            "SELECT metadata, outcome FROM audit_log \
              WHERE action = $1 AND metadata->>'fingerprint' = $2",
        )
        .bind(action)
        .bind(ALICE_FP)
        .fetch_all(r.h.pool())
        .await
        .unwrap();
        assert_eq!(rows.len(), 1, "{action}");
        assert_eq!(rows[0].1, "success");
        assert_eq!(rows[0].0, json!({"name": name, "fingerprint": ALICE_FP}));
        assert!(aurora_core::audit::is_critical(action));
    }
}

/// RSA/ECDSA are unsupported types; a certificate line, an options-prefixed line, a private key
/// paste and garbage are invalid; a duplicate is 409.
#[tokio::test]
async fn ssh_keys_refusals_use_the_c790_codes() {
    let r = ssh_rig().await;
    let cert_line =
        "ssh-ed25519-cert-v01@openssh.com AAAAIHNzaC1lZDI1NTE5LWNlcnQtdjAxQG9wZW5zc2guY29t x";
    let options = format!("command=\"echo hi\",no-pty {ALICE}");
    let private = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmU=\n-----END OPENSSH PRIVATE KEY-----";
    for (line, want) in [
        (RSA, "ssh_key_unsupported_type"),
        (ECDSA, "ssh_key_unsupported_type"),
        (cert_line, "ssh_key_invalid"),
        (options.as_str(), "ssh_key_invalid"),
        (private, "ssh_key_invalid"),
        ("hello world", "ssh_key_invalid"),
        ("", "ssh_key_invalid"),
        ("ssh-ed25519 AAAA", "ssh_key_invalid"),
    ] {
        let (s, v) = add_key(&r, line).await;
        assert_eq!((s, code(&v)), (422, want), "{line}: {v}");
    }
    // Nothing was stored.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM ssh_keys")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    let (s, _) = add_key(&r, ALICE).await;
    assert_eq!(s, 201);
    // The same key again, with a different comment: still a duplicate.
    let (s, v) = add_key(&r, &format!("{ALICE_CANONICAL} other-comment")).await;
    assert_eq!((s, code(&v)), (409, "ssh_key_exists"), "{v}");
}

/// The 11th key is 409 `ssh_key_limit`.
#[tokio::test]
async fn ssh_keys_limit_is_ten_per_person() {
    let r = ssh_rig().await;
    for n in 1..=10u8 {
        let (s, v) = add_key(&r, &key_line(n)).await;
        assert_eq!(s, 201, "key {n}: {v}");
    }
    let (s, v) = add_key(&r, &key_line(11)).await;
    assert_eq!((s, code(&v)), (409, "ssh_key_limit"), "{v}");
    // Deleting one frees a slot.
    let (_, l) = r.term_call(Method::GET, "/v1/me/ssh-keys", &[], None).await;
    let id = l["keys"][0]["id"].as_str().unwrap();
    let (s, _) = r
        .term_call(Method::DELETE, &format!("/v1/me/ssh-keys/{id}"), &[], None)
        .await;
    assert_eq!(s, 204);
    let (s, v) = add_key(&r, &key_line(11)).await;
    assert_eq!(s, 201, "{v}");
}

/// Keys are the person's own: another person's id is 404 on PATCH and DELETE, and their list
/// never shows it.
#[tokio::test]
async fn ssh_keys_belong_to_their_person() {
    let r = ssh_rig().await;
    let (_, v) = add_key(&r, ALICE).await;
    let id = v["id"].as_str().unwrap().to_owned();
    let other = r.h.user("o@x.io", false).await;
    r.h.member(r.org, other, "member", "active").await;
    let (_, sess) = r.h.good_session(other).await;
    let c = cookie(&sess);
    let hd = [("cookie", c.as_str()), ("origin", ORIGIN)];
    let uri = format!("/v1/me/ssh-keys/{id}");
    let (s, _, v) =
        r.h.send(Method::PATCH, &uri, &hd, Some(json!({"name": "mine now"})))
            .await;
    assert_eq!((s.as_u16(), code(&v)), (404, "not_found"), "{v}");
    let (s, _, v) = r.h.send(Method::DELETE, &uri, &hd, None).await;
    assert_eq!((s.as_u16(), code(&v)), (404, "not_found"), "{v}");
    let (s, _, l) = r.h.send(Method::GET, "/v1/me/ssh-keys", &hd, None).await;
    assert_eq!(s, 200);
    assert!(l["keys"].as_array().unwrap().is_empty(), "{l}");
    // Two people may register the same public key.
    let (s, _, v) =
        r.h.send(
            Method::POST,
            "/v1/me/ssh-keys",
            &hd,
            Some(json!({"public_key": ALICE})),
        )
        .await;
    assert_eq!(s.as_u16(), 201, "{v}");
    // The first person's key is untouched.
    let (_, l) = r.term_call(Method::GET, "/v1/me/ssh-keys", &[], None).await;
    assert_eq!(l["keys"][0]["name"], "alice@laptop");
}

/// API keys read but never write: POST/PATCH/DELETE answer 403 `session_required`.
#[tokio::test]
async fn ssh_keys_api_key_cannot_write() {
    let r = ssh_rig().await;
    let (_, v) = add_key(&r, ALICE).await;
    let id = v["id"].as_str().unwrap().to_owned();
    let user = member_id(&r).await;
    let (_, tok) = r.h.api_key(user, r.org, Some(r.ws), false).await;
    let b = bearer(&tok);
    let hd = [("authorization", b.as_str())];
    for (m, uri, body) in [
        (
            Method::POST,
            "/v1/me/ssh-keys".to_owned(),
            Some(json!({"public_key": key_line(3)})),
        ),
        (
            Method::PATCH,
            format!("/v1/me/ssh-keys/{id}"),
            Some(json!({"name": "x"})),
        ),
        (Method::DELETE, format!("/v1/me/ssh-keys/{id}"), None),
    ] {
        let (s, _, v) = r.h.send(m.clone(), &uri, &hd, body).await;
        assert_eq!(
            (s.as_u16(), code(&v)),
            (403, "session_required"),
            "{m} {uri}: {v}"
        );
    }
    // Reading is allowed.
    let (s, _, l) = r.h.send(Method::GET, "/v1/me/ssh-keys", &hd, None).await;
    assert_eq!(s.as_u16(), 200, "{l}");
    assert_eq!(l["keys"].as_array().unwrap().len(), 1);
}

// ---- off ------------------------------------------------------------------------------------

/// Every SSH route answers 404 `ssh_disabled` while `ssh.enabled` is false — even for an
/// anonymous caller — and `features.ssh` says so.
#[tokio::test]
async fn ssh_routes_are_404_ssh_disabled_while_off() {
    let r = rig_with(|_| {}).await;
    let agent = r.agent("off-agent", false).await;
    let id = Uuid::now_v7();
    for (m, uri, body) in [
        (Method::GET, "/v1/me/ssh-keys".to_owned(), None),
        (
            Method::POST,
            "/v1/me/ssh-keys".to_owned(),
            Some(json!({"public_key": ALICE})),
        ),
        (
            Method::PATCH,
            format!("/v1/me/ssh-keys/{id}"),
            Some(json!({"name": "x"})),
        ),
        (Method::DELETE, format!("/v1/me/ssh-keys/{id}"), None),
        (
            Method::POST,
            format!("/v1/agents/{agent}/ssh-certificate"),
            Some(json!({"public_key": ALICE})),
        ),
        (Method::GET, "/v1/ssh".to_owned(), None),
    ] {
        let (s, v) = r.term_call(m.clone(), &uri, &[], body.clone()).await;
        assert_eq!((s, code(&v)), (404, "ssh_disabled"), "{m} {uri}: {v}");
        // Anonymous: the same answer (the switch is checked before authentication).
        let (s, _, v) = r.h.send(m.clone(), &uri, &[("origin", ORIGIN)], body).await;
        assert_eq!(
            (s.as_u16(), code(&v)),
            (404, "ssh_disabled"),
            "anon {m} {uri}: {v}"
        );
    }
    let (_, v) = r.h.get("/v1/instance", &[]).await;
    assert_eq!(v["features"]["ssh"], false, "{v}");
    let on = ssh_rig().await;
    let (_, v) = on.h.get("/v1/instance", &[]).await;
    assert_eq!(v["features"]["ssh"], true, "{v}");
}

/// Startup with `ssh.enabled` and no/garbled CA key fails naming the variable (never the key).
#[tokio::test]
async fn ssh_enabled_without_a_parsable_ca_key_refuses_to_start() {
    let r = rig_with(|_| {}).await;
    for key in [None, Some("super-secret-garbage".to_owned())] {
        let mut cfg = (*r.h.state.config).clone();
        cfg.ssh.enabled = true;
        cfg.ssh_ca_key = key.clone().map(aurora_config::Redacted::new);
        let e = aurora_api::app::AppState::try_new(r.h.pool().clone(), cfg)
            .err()
            .expect("must refuse");
        assert!(e.contains("AURORA_SSH_CA_KEY"), "{e}");
        assert!(!e.contains("super-secret-garbage"), "{e}");
    }
}

// ---- C-792 facts ----------------------------------------------------------------------------

#[tokio::test]
async fn ssh_facts_shape_before_any_terminator_exists() {
    let r = ssh_rig().await;
    let (s, v) = r.term_call(Method::GET, "/v1/ssh", &[], None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v,
        json!({"host": "ssh.example.org", "port": 22, "cert_ttl_secs": 900,
               "terminators_live": 0, "host_keys": [], "known_hosts": []})
    );
}

// ---- C-791 mint -----------------------------------------------------------------------------

/// The SEC-93 mint auth matrix.
#[tokio::test]
async fn ssh_cert_auth_matrix() {
    let r = ssh_rig().await;
    let agent = r.agent("ssh-a", false).await;
    let user = member_id(&r).await;
    let (s, _) = add_key(&r, ALICE).await;
    assert_eq!(s, 201);

    // Session: 201.
    let (s, v) = mint(&r, agent, ALICE).await;
    assert_eq!(s, 201, "{v}");

    // Own workspace key (inside its workspace) and own org key: 201.
    let (_, auk) = r.h.api_key(user, r.org, Some(r.ws), false).await;
    let (_, auo) = r.h.api_key(user, r.org, None, false).await;
    for tok in [&auk, &auo] {
        let b = bearer(tok);
        let (s, _, v) = mint_as(&r, &[("authorization", &b)], agent, ALICE).await;
        assert_eq!(s, 201, "{v}");
    }
    // A workspace key of another workspace: 403 `workspace_mismatch`.
    let ws2 = r.h.workspace(r.org).await;
    let (_, auk2) = r.h.api_key(user, r.org, Some(ws2), false).await;
    let b = bearer(&auk2);
    let (s, _, v) = mint_as(&r, &[("authorization", &b)], agent, ALICE).await;
    assert_eq!((s, code(&v)), (403, "workspace_mismatch"), "{v}");

    // A service key: 403.
    let (_, svc) = r.h.api_key(user, r.org, Some(r.ws), false).await;
    sqlx::query("UPDATE api_keys SET kind = 'service' WHERE token_hash = $1")
        .bind(&aurora_core::token::hash(&svc)[..])
        .execute(r.h.pool())
        .await
        .unwrap();
    let b = bearer(&svc);
    let (s, _, v) = mint_as(&r, &[("authorization", &b)], agent, ALICE).await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");

    // An agent credential and an anonymous caller: 401.
    let agent_tok = aurora_core::token::generate(aurora_core::token::TokenPrefix::Gateway)
        .expose()
        .clone();
    let b = bearer(&agent_tok);
    let (s, _, v) = mint_as(&r, &[("authorization", &b)], agent, ALICE).await;
    assert_eq!((s, code(&v)), (401, "unauthenticated"), "{v}");
    let (s, _, v) = mint_as(&r, &[("origin", ORIGIN)], agent, ALICE).await;
    assert_eq!((s, code(&v)), (401, "unauthenticated"), "{v}");

    // An org member without `can_use` of the workspace: 404, the same as an unknown agent.
    let outsider = r.h.user("out@x.io", false).await;
    r.h.member(r.org, outsider, "member", "active").await;
    let (_, sess) = r.h.good_session(outsider).await;
    let c = cookie(&sess);
    let (s, _, v) = mint_as(&r, &[("cookie", &c), ("origin", ORIGIN)], agent, ALICE).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    let (s, v) = mint(&r, Uuid::now_v7(), ALICE).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");

    // A key that is not registered: 422.
    let (s, v) = mint(&r, agent, &key_line(9)).await;
    assert_eq!((s, code(&v)), (422, "ssh_key_not_registered"), "{v}");
    // An RSA key: 422 before anything else is touched.
    let (s, v) = mint(&r, agent, RSA).await;
    assert_eq!((s, code(&v)), (422, "ssh_key_unsupported_type"), "{v}");
}

/// A BYO agent has no sandbox (422); a frozen agent, workspace or org is 423.
#[tokio::test]
async fn ssh_cert_refuses_byo_and_frozen() {
    let r = ssh_rig().await;
    add_key(&r, ALICE).await;
    let byo = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) \
         VALUES ($1,$2,$3,'byo-a','byo-a','byo','medium','')",
    )
    .bind(byo)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = mint(&r, byo, ALICE).await;
    assert_eq!((s, code(&v)), (422, "ssh_not_supported"), "{v}");

    let agent = r.agent("frozen-a", false).await;
    let (s, _) = mint(&r, agent, ALICE).await;
    assert_eq!(s, 201);
    for (sql, id) in [
        (
            "UPDATE agents SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'workspace_owner' WHERE id = $1",
            agent,
        ),
        (
            "UPDATE workspaces SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'org_admin' WHERE id = $1",
            r.ws,
        ),
        (
            "UPDATE organizations SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'instance_admin' WHERE id = $1",
            r.org,
        ),
    ] {
        sqlx::query(sql).bind(id).execute(r.h.pool()).await.unwrap();
        let (s, v) = mint(&r, agent, ALICE).await;
        assert_eq!((s, code(&v)), (423, "frozen"), "{sql}: {v}");
    }
}

/// The minted certificate: parses, verifies against the CA public key, and has exactly the
/// C-791 fields (principals = agent id then handle, C-1121; `username` the handle; key id, non-zero serial, `permit-pty` only, no critical
/// options, validity `now − 60 s … now + ttl`). The key's `last_used_at` is stamped and the
/// best-effort audit row names agent, workspace, fingerprint, serial and `valid_before`.
#[tokio::test]
async fn ssh_cert_has_exactly_the_specified_fields() {
    let r = ssh_rig().await;
    let agent = r.agent("cert-a", false).await;
    let (_, k) = add_key(&r, ALICE).await;
    let key_id = k["id"].as_str().unwrap().to_owned();
    let user = member_id(&r).await;
    let t0 = chrono::Utc::now().timestamp() as u64;
    let (s, v) = mint(&r, agent, ALICE).await;
    assert_eq!(s, 201, "{v}");
    let t1 = chrono::Utc::now().timestamp() as u64;

    let cert = Certificate::from_openssh(v["certificate"].as_str().unwrap()).unwrap();
    // Verifies against the CA public key (signature, window) and against no other key.
    let ca = ca_public().fingerprint(HashAlg::Sha256);
    cert.validate_at(t1, [&ca]).unwrap();
    let other = PrivateKey::from(Ed25519Keypair::from_seed(&[1; 32]))
        .public_key()
        .fingerprint(HashAlg::Sha256);
    assert!(cert.validate_at(t1, [&other]).is_err());

    assert_eq!(cert.cert_type(), CertType::User);
    // C-1121, SEC-147: the agent id first, then the handle (the user name to connect as).
    assert_eq!(
        cert.valid_principals(),
        [agent.to_string(), "cert-a".to_owned()]
    );
    assert_eq!(
        cert.key_id(),
        format!("aurora:u={user}:k={key_id}:a={agent}:w={}", r.ws)
    );
    assert_ne!(cert.serial(), 0);
    assert_eq!(v["serial"], cert.serial().to_string());
    assert!(cert.critical_options().is_empty());
    let ext: Vec<(&String, &String)> = cert.extensions().iter().collect();
    assert_eq!(ext.len(), 1);
    assert_eq!(ext[0].0, "permit-pty");
    assert!(cert.valid_after() >= t0 - 60 && cert.valid_after() <= t1 - 60);
    assert!(cert.valid_before() >= t0 + 900 && cert.valid_before() <= t1 + 900);
    assert_eq!(cert.comment(), format!("aurora:{agent}"));
    // The certified key is the registered one.
    let subject = ssh_key::PublicKey::from_openssh(ALICE).unwrap();
    assert_eq!(cert.public_key(), subject.key_data());

    assert_eq!(v["principal"], agent.to_string());
    assert_eq!(v["username"], "cert-a");
    assert_eq!(v["key_id"], cert.key_id());
    assert_eq!(v["host"], "ssh.example.org");
    assert_eq!(v["port"], 22);
    assert_eq!(v["known_hosts"], json!([]));
    assert!(v["valid_before"].as_str().is_some() && v["valid_after"].as_str().is_some());

    // An independent reader agrees: OpenSSH's own `ssh-keygen -L`.
    let dir = std::env::temp_dir().join(format!("aurora-ssh-cert-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("id-cert.pub");
    std::fs::write(&f, v["certificate"].as_str().unwrap()).unwrap();
    match std::process::Command::new("ssh-keygen")
        .arg("-L")
        .arg("-f")
        .arg(&f)
        .output()
    {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{text}");
            assert!(
                text.contains("Type: ssh-ed25519-cert-v01@openssh.com user certificate"),
                "{text}"
            );
            assert!(text.contains(&agent.to_string()), "{text}");
            // OpenSSH lists both principals, one per line, the handle after the id.
            let after = text.split("Principals:").nth(1).unwrap_or("");
            let names: Vec<&str> = after
                .lines()
                .skip(1)
                .map(str::trim)
                .take_while(|l| !l.contains(':'))
                .collect();
            assert_eq!(names, [agent.to_string().as_str(), "cert-a"], "{text}");
            assert!(text.contains("permit-pty"), "{text}");
            assert!(!text.contains("permit-port-forwarding"), "{text}");
            assert!(
                text.contains(&format!(
                    "Signing CA: ED25519 {}",
                    ca_public().fingerprint(HashAlg::Sha256)
                )),
                "{text}"
            );
        }
        Err(e) => {
            eprintln!("ssh-keygen not available ({e}); the ssh-key reader above is the check")
        }
    }

    // Side effects: last_used_at, the mint ledger and the best-effort audit row.
    let (_, l) = r.term_call(Method::GET, "/v1/me/ssh-keys", &[], None).await;
    assert!(l["keys"][0]["last_used_at"].is_string(), "{l}");
    let count: i32 = sqlx::query_scalar("SELECT count FROM ssh_mint_counters WHERE agent_id = $1")
        .bind(agent)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let m = wait_audit(&r, "agent.ssh_certificate.mint").await;
    assert_eq!(m["agent"], agent.to_string());
    assert_eq!(m["workspace"], r.ws.to_string());
    assert_eq!(m["fingerprint"], ALICE_FP);
    assert_eq!(m["serial"], cert.serial().to_string());
    assert!(m["valid_before"].is_string());
    assert!(
        !m.to_string().contains("ssh-ed25519"),
        "no key material in the audit row"
    );
}

/// Two mints differ in serial and nonce (random, non-zero serial).
#[tokio::test]
async fn ssh_cert_serials_are_random() {
    let r = ssh_rig().await;
    let agent = r.agent("serial-a", false).await;
    add_key(&r, ALICE).await;
    let (_, a) = mint(&r, agent, ALICE).await;
    let (_, b) = mint(&r, agent, ALICE).await;
    assert_ne!(a["serial"], b["serial"]);
    assert_ne!(a["certificate"], b["certificate"]);
}

/// `ssh.cert_ttl_secs` is honoured.
#[tokio::test]
async fn ssh_cert_ttl_follows_configuration() {
    let r = rig_with(|c| {
        c.ssh.enabled = true;
        c.ssh.cert_ttl_secs = 120;
        c.ssh_ca_key = Some(aurora_config::Redacted::new(ca_key_value()));
    })
    .await;
    let agent = r.agent("ttl-a", false).await;
    add_key(&r, ALICE).await;
    let (s, v) = mint(&r, agent, ALICE).await;
    assert_eq!(s, 201, "{v}");
    let cert = Certificate::from_openssh(v["certificate"].as_str().unwrap()).unwrap();
    assert_eq!(cert.valid_before() - cert.valid_after(), 120 + 60);
}

// ---- the mint ledger ------------------------------------------------------------------------

/// 30 mints succeed and the 31st is 429 with `Retry-After`; run concurrently on two API states
/// with their own pools over one database the total is exactly 30. A refused mint is not counted
/// and other agents/people have their own counters.
#[tokio::test]
async fn ssh_cert_mint_limit_is_exact_across_two_api_pools() {
    use tower::ServiceExt;
    let r = ssh_rig().await;
    let agent = r.agent("limit-a", false).await;
    let other_agent = r.agent("limit-b", false).await;
    add_key(&r, ALICE).await;

    // The second API replica: its own pool and state on the same database.
    let pool2 = sqlx::PgPool::connect(r.h.db.url()).await.unwrap();
    let state2 = aurora_api::app::AppState::new(pool2, (*r.h.state.config).clone());
    let app2 = test_router(state2);

    let body = json!({"public_key": ALICE}).to_string();
    let mut tasks = Vec::new();
    for i in 0..40 {
        let app = if i % 2 == 0 {
            r.h.app.clone()
        } else {
            app2.clone()
        };
        let req = http::Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/agents/{agent}/ssh-certificate"))
            .header("cookie", &r.cookie)
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.clone()))
            .unwrap();
        tasks.push(tokio::spawn(async move {
            let res = app.oneshot(req).await.unwrap();
            let status = res.status().as_u16();
            let retry = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            (status, retry)
        }));
    }
    let mut ok = 0;
    let mut limited = 0;
    for t in tasks {
        let (s, retry) = t.await.unwrap();
        match s {
            201 => ok += 1,
            429 => {
                limited += 1;
                let secs: u64 = retry.expect("Retry-After on a 429").parse().unwrap();
                assert!((1..=3600).contains(&secs), "{secs}");
            }
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!((ok, limited), (30, 10));
    let count: i32 = sqlx::query_scalar("SELECT count FROM ssh_mint_counters WHERE agent_id = $1")
        .bind(agent)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(count, 30, "refused mints are not counted");
    let (s, v) = mint(&r, agent, ALICE).await;
    assert_eq!((s, code(&v)), (429, "ssh_mint_rate_limited"), "{v}");
    // Another agent has its own counter.
    let (s, v) = mint(&r, other_agent, ALICE).await;
    assert_eq!(s, 201, "{v}");
}

/// The retention sweep prunes ledger rows older than 24 h and keeps the current hour.
#[tokio::test]
async fn ssh_mint_counters_older_than_a_day_are_pruned_by_the_sweep() {
    let r = ssh_rig().await;
    let agent = r.agent("prune-a", false).await;
    add_key(&r, ALICE).await;
    let (s, _) = mint(&r, agent, ALICE).await;
    assert_eq!(s, 201);
    let user = member_id(&r).await;
    for hours in [25, 23] {
        sqlx::query(
            "INSERT INTO ssh_mint_counters (user_id, agent_id, window_start, count) \
             VALUES ($1, $2, date_trunc('hour', now()) - make_interval(hours => $3), 5)",
        )
        .bind(user)
        .bind(agent)
        .bind(hours)
        .execute(r.h.pool())
        .await
        .unwrap();
    }
    aurora_db::repo::preview_relay::prune_preview_expiries(r.h.pool())
        .await
        .unwrap();
    let ages: Vec<i64> = sqlx::query_scalar(
        "SELECT round(extract(epoch FROM now() - window_start) / 3600)::bigint \
           FROM ssh_mint_counters ORDER BY window_start",
    )
    .fetch_all(r.h.pool())
    .await
    .unwrap();
    assert_eq!(ages.len(), 2, "the 25 h row is gone: {ages:?}");
    assert!(ages[0] >= 22 && ages[0] <= 24, "{ages:?}");
}

/// `can_ssh` is one predicate for every caller (also used by P4-SSH-2): its answers.
#[tokio::test]
async fn can_ssh_answers_each_refusal() {
    use aurora_db::authz::{SshAccess, can_ssh};
    let r = ssh_rig().await;
    let agent = r.agent("pred-a", false).await;
    let user = member_id(&r).await;
    let mut conn = r.h.pool().acquire().await.unwrap();
    assert!(matches!(
        can_ssh(&mut conn, true, user, agent).await.unwrap(),
        SshAccess::Allowed { workspace_id, org_id } if workspace_id == r.ws && org_id == r.org
    ));
    assert_eq!(
        can_ssh(&mut conn, false, user, agent).await.unwrap(),
        SshAccess::Disabled
    );
    assert_eq!(
        can_ssh(&mut conn, true, user, Uuid::now_v7())
            .await
            .unwrap(),
        SshAccess::Denied
    );
    assert_eq!(
        can_ssh(&mut conn, true, Uuid::now_v7(), agent)
            .await
            .unwrap(),
        SshAccess::Denied
    );
    drop(conn);
    r.h.disable_user(user).await;
    let mut conn = r.h.pool().acquire().await.unwrap();
    assert_eq!(
        can_ssh(&mut conn, true, user, agent).await.unwrap(),
        SshAccess::Denied
    );
}
