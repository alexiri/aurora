#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-SSH-1 secret-grep (SEC-93, C-791): with a known CA seed, a run through key registration,
//! mints, refusals and a garbled-key startup refusal leaves the seed (and its OpenSSH private
//! form) in no log line and no response body. Own test binary with a single test: it installs a
//! scoped tracing subscriber, which must not race other tests' callsite registration (like
//! `sec10_logs.rs`).

#[path = "it/common/mod.rs"]
mod common;

use crate::common::*;
use base64::Engine;
use http::Method;
use serde_json::{Value, json};
use ssh_key::PrivateKey;
use ssh_key::private::Ed25519Keypair;
use uuid::Uuid;

#[derive(Clone, Default)]
struct LogBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> LogBuf {
        self.clone()
    }
}

#[tokio::test]
async fn ssh_ca_seed_is_in_no_log_line_and_no_response_body() {
    let buf = LogBuf::default();
    let _g = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(buf.clone())
            .finish(),
    );
    tracing::callsite::rebuild_interest_cache();

    let seed_bytes = [0x5bu8; 32];
    let seed_b64 = base64::engine::general_purpose::STANDARD.encode(seed_bytes);
    let ca_value = format!("ed25519:{seed_b64}");
    let ca_private = PrivateKey::from(Ed25519Keypair::from_seed(&seed_bytes));
    // Everything that would give the key away if it leaked, in any common encoding.
    let pem = ca_private
        .to_openssh(ssh_key::LineEnding::LF)
        .unwrap()
        .to_string();
    let pem_body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let secrets = [
        seed_b64.clone(),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(seed_bytes),
        hex::encode(seed_bytes),
        pem_body,
    ];

    let value = ca_value.clone();
    let h = Harness::with_config(move |c| {
        c.ssh.enabled = true;
        c.ssh_ca_key = Some(aurora_config::Redacted::new(value));
    })
    .await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let user = h.user("m@x.io", false).await;
    h.member(org, user, "member", "active").await;
    h.bind_ws(ws, user, "member").await;
    let (_, sess) = h.good_session(user).await;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,'a','a','hosted','fake')",
    )
    .bind(agent)
    .bind(org)
    .bind(ws)
    .execute(h.pool())
    .await
    .unwrap();

    let c = cookie(&sess);
    let hd = [("cookie", c.as_str()), ("origin", ORIGIN)];
    let user_key = PrivateKey::from(Ed25519Keypair::from_seed(&[1; 32]))
        .public_key()
        .to_openssh()
        .unwrap();
    let mut bodies: Vec<Value> = Vec::new();
    for (m, uri, body) in [
        (
            Method::POST,
            "/v1/me/ssh-keys".to_owned(),
            Some(json!({"public_key": user_key})),
        ),
        (Method::GET, "/v1/me/ssh-keys".to_owned(), None),
        (Method::GET, "/v1/ssh".to_owned(), None),
        (Method::GET, "/v1/instance".to_owned(), None),
        (
            Method::POST,
            format!("/v1/agents/{agent}/ssh-certificate"),
            Some(json!({"public_key": user_key})),
        ),
        // Refusals carry error bodies too.
        (
            Method::POST,
            format!("/v1/agents/{agent}/ssh-certificate"),
            Some(json!({"public_key": ca_value})),
        ),
        (
            Method::POST,
            "/v1/me/ssh-keys".to_owned(),
            Some(json!({"public_key": pem})),
        ),
    ] {
        let (_, _, v) = h.send(m, &uri, &hd, body).await;
        bodies.push(v);
    }
    // A startup refusal over a garbled key, and one over the real seed in a wrong form.
    for bad in [
        "not-a-key".to_owned(),
        format!("ed25519:{}", &seed_b64[..20]),
    ] {
        let mut cfg = (*h.state.config).clone();
        cfg.ssh_ca_key = Some(aurora_config::Redacted::new(bad));
        let e = aurora_api::app::AppState::try_new(h.pool().clone(), cfg)
            .err()
            .expect("a garbled key must refuse");
        bodies.push(json!(e));
    }

    let logs = String::from_utf8_lossy(&buf.0.lock().unwrap()).into_owned();
    assert!(
        !logs.is_empty(),
        "the capture must be live, or the check is vacuous"
    );
    let bodies_text = serde_json::to_string(&bodies).unwrap();
    assert!(
        bodies_text.contains("certificate"),
        "the run must have minted a certificate: {bodies_text}"
    );
    for s in &secrets {
        assert!(!logs.contains(s.as_str()), "CA key material in a log line");
        assert!(
            !bodies_text.contains(s.as_str()),
            "CA key material in a body"
        );
    }
    // The configuration's Debug output (what a startup log would print) is redacted too.
    assert!(!format!("{:?}", h.state.config).contains(&seed_b64));
}
