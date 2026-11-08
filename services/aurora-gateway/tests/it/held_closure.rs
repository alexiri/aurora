#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P2-FU-2 (docs/06 §BYO admission item 1, C-207): a request held for approval when its
//! credential is frozen or BYO-disabled is answered with the admission refusal (403 `frozen` /
//! `byo_disabled`), its approval is `abandoned` with `resolved_at`, and only then does the
//! tunnel close. Nothing is ever released upstream. Real database, real approvals.

use crate::hold::{
    CustomResolver, Env, World, approval_env, no_llm, pending_for, start_with, wait_status,
};
use async_trait::async_trait;
use aurora_gateway::auth::{
    ByoAdmission, CredentialKind, CredentialResolver, RequestIdentity, ResolveError,
};
use aurora_gateway::invalidation::{Invalidatable, Invalidate, Scope};
use aurora_llm::governance::{ByoInputs, ByoInstance, ByoPosture, effective_byo};
use aurora_testkit::TestDb;
use base64::Engine;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::TlsConnector;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Flip {
    Frozen,
    ByoDisabled,
}

#[derive(Default)]
struct State {
    frozen: Mutex<bool>,
    byo_off: Mutex<bool>,
}

struct Cred {
    w: Arc<World>,
    token: &'static str,
    kind: CredentialKind,
    credential: Uuid,
    /// Subject to BYO control (BYO `aug_` / `auc_`); hosted credentials carry none.
    byo: bool,
    st: Arc<State>,
}

impl Cred {
    fn identity(&self) -> Result<Option<RequestIdentity>, ResolveError> {
        if *self.st.frozen.lock().unwrap() {
            return Err(ResolveError::Frozen { level: "workspace" });
        }
        let inst = ByoInstance {
            minimum_posture: ByoPosture::Enabled,
            ..ByoInstance::default()
        };
        let off = *self.st.byo_off.lock().unwrap();
        let byo = self.byo.then(|| ByoAdmission {
            effective: effective_byo(
                &inst,
                &ByoInputs {
                    workspace_posture: off.then_some("disabled"),
                    ..ByoInputs::default()
                },
            ),
            org_slug: "acme".into(),
            ws_slug: "main".into(),
        });
        Ok(Some(RequestIdentity {
            credential_id: self.credential,
            kind: self.kind,
            agent_id: self.w.agent,
            workspace_id: self.w.ws,
            org_id: self.w.org,
            conversation_id: None,
            turn_id: None,
            context_label: None,
            speaker_user_id: None,
            capabilities: vec![],
            byo,
        }))
    }
}

#[async_trait]
impl CredentialResolver for Cred {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        if token == self.token {
            self.identity()
        } else {
            Ok(None)
        }
    }
    async fn resolve_credential(&self, id: Uuid) -> Result<Option<RequestIdentity>, ResolveError> {
        if id == self.credential {
            self.identity()
        } else {
            Ok(None)
        }
    }
}

struct Case {
    token: &'static str,
    kind: CredentialKind,
    byo: bool,
    flip: Flip,
}

fn cases() -> Vec<Case> {
    let c = |token, kind, byo, flip| Case {
        token,
        kind,
        byo,
        flip,
    };
    vec![
        // frozen: aug_ (BYO), aub_, auc_ (hosted and BYO)
        c("aug_t", CredentialKind::Agent, true, Flip::Frozen),
        c("aub_t", CredentialKind::Boot, false, Flip::Frozen),
        c("auc_t", CredentialKind::Context, false, Flip::Frozen),
        c("auc_t", CredentialKind::Context, true, Flip::Frozen),
        // byo_disabled: aug_ and BYO auc_
        c("aug_t", CredentialKind::Agent, true, Flip::ByoDisabled),
        c("auc_t", CredentialKind::Context, true, Flip::ByoDisabled),
    ]
}

struct Setup {
    env: Env,
    st: Arc<State>,
}

async fn setup(db: &TestDb, case: &Case) -> Setup {
    let st = Arc::new(State::default());
    let (st2, token, kind, byo) = (st.clone(), case.token, case.kind, case.byo);
    let custom: CustomResolver = Box::new(move |w: Arc<World>| {
        Arc::new(Cred {
            w,
            token,
            kind,
            credential: Uuid::now_v7(),
            byo,
            st: st2,
        }) as Arc<dyn CredentialResolver>
    });
    let env = start_with(
        approval_env,
        no_llm(),
        db.pool().clone(),
        Duration::from_secs(60),
        Some(custom),
    )
    .await;
    Setup { env, st }
}

impl Setup {
    /// The freeze or posture flip, delivered as the gateway's invalidation.
    fn flip(&self, flip: Flip) {
        match flip {
            Flip::Frozen => *self.st.frozen.lock().unwrap() = true,
            Flip::ByoDisabled => *self.st.byo_off.lock().unwrap() = true,
        }
        self.env.tunnels.as_ref().unwrap().invalidate(&Invalidate {
            scope: Scope::Agent(self.env.w.agent),
        });
    }

    async fn check(&self, case: &Case, status: u16, body: &str, id: Uuid, label: &str) {
        assert_eq!(status, 403, "{label}: {body}");
        let j: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let (code, level) = match case.flip {
            Flip::Frozen => ("frozen", "workspace"),
            Flip::ByoDisabled => ("byo_disabled", "workspace"),
        };
        assert_eq!(j["error"]["code"], code, "{label}: {body}");
        assert_eq!(j["error"]["details"]["level"], level, "{label}: {body}");
        wait_status(&self.env.pool, id, "abandoned").await;
        let resolved: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar("SELECT resolved_at FROM approvals WHERE id=$1")
                .bind(id)
                .fetch_one(&self.env.pool)
                .await
                .unwrap();
        assert!(resolved.is_some(), "{label}: abandoned sets resolved_at");
        assert_eq!(
            self.env.up_hits.load(Ordering::SeqCst),
            0,
            "{label}: nothing reaches the upstream"
        );
        assert_eq!(
            self.env.tunnels.as_ref().unwrap().len(),
            0,
            "{label}: the registry entry is gone with the connection"
        );
    }
}

fn basic(token: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("agent:{token}"))
}

/// Absolute-form lane: the held request is refused, then the connection ends.
#[tokio::test]
async fn held_absolute_request_is_refused_then_closed() {
    let db = TestDb::new().await;
    for case in cases() {
        let label = format!("{} byo={} {:?} absolute", case.token, case.byo, case.flip);
        let s = setup(&db, &case).await;
        let mut c = crate::common::connect(s.env.proxy).await;
        let req = format!(
            "GET http://{up}/x HTTP/1.1\r\nHost: {up}\r\nProxy-Authorization: Basic {}\r\n\r\n",
            basic(case.token),
            up = s.env.up
        );
        c.write_all(req.as_bytes()).await.unwrap();
        let reader = tokio::spawn(async move {
            let mut out = Vec::new();
            let _ = c.read_to_end(&mut out).await;
            String::from_utf8_lossy(&out).into_owned()
        });
        let (id, _, status) = pending_for(&s.env.pool, s.env.w.ws).await;
        assert_eq!(status, "pending", "{label}");
        s.flip(case.flip);
        let text = tokio::time::timeout(Duration::from_secs(8), reader)
            .await
            .unwrap_or_else(|_| panic!("{label}: connection not closed after the answer"))
            .unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        s.check(&case, status, &chunked(body), id, &label).await;
    }
}

fn chunked(b: &str) -> String {
    // Keep only the JSON payload of a chunked or plain body.
    match (b.find('{'), b.rfind('}')) {
        (Some(a), Some(z)) if z > a => b[a..=z].to_owned(),
        _ => b.to_owned(),
    }
}

/// MITM lane (HTTP inside a CONNECT tunnel): the held request is refused with a 403 before the
/// tunnel closes.
#[tokio::test]
async fn held_request_in_a_tunnel_is_refused_before_the_tunnel_closes() {
    let db = TestDb::new().await;
    for case in cases() {
        let label = format!("{} byo={} {:?} mitm", case.token, case.byo, case.flip);
        let s = setup(&db, &case).await;
        let target = s.env.up;
        let mut raw = crate::common::connect(s.env.proxy).await;
        raw.write_all(
            format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {}\r\n\r\n",
                basic(case.token)
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut b = [0u8; 1];
            raw.read_exact(&mut b).await.unwrap();
            head.push(b[0]);
        }
        assert!(
            String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"),
            "{label}: {}",
            String::from_utf8_lossy(&head)
        );
        let tls = TlsConnector::from(crate::common::client_config(
            &s.env.gw_ca_pem,
            &[b"http/1.1"],
        ))
        .connect(
            rustls::pki_types::ServerName::IpAddress(target.ip().into()),
            raw,
        )
        .await
        .unwrap();
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .unwrap();
        let conn = tokio::spawn(conn);
        let req = Request::builder()
            .uri(format!("https://{target}/x"))
            .header("host", target.to_string())
            .body(Full::new(Bytes::new()))
            .unwrap();
        let resp = tokio::spawn(async move {
            let r = sender.send_request(req).await.unwrap();
            let status = r.status().as_u16();
            let body = r.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8_lossy(&body).into_owned())
        });
        let (id, _, status) = pending_for(&s.env.pool, s.env.w.ws).await;
        assert_eq!(status, "pending", "{label}");
        s.flip(case.flip);
        let (status, body) = tokio::time::timeout(Duration::from_secs(8), resp)
            .await
            .unwrap_or_else(|_| panic!("{label}: the held request got no answer"))
            .unwrap();
        s.check(&case, status, &body, id, &label).await;
        // The tunnel is closed afterwards.
        tokio::time::timeout(Duration::from_secs(8), conn)
            .await
            .unwrap_or_else(|_| panic!("{label}: tunnel still open after the answer"))
            .unwrap()
            .ok();
    }
}
