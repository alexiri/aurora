//! The rig: stub API, stub runner, server, certificates and a client.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aurora_terminator::limits::PreAuth;
use aurora_terminator::plane::Plane;
use aurora_terminator::state::{Settings, Shared};
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use russh::client;
use russh::keys::ssh_key::certificate::{Builder, CertType};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::{Certificate, PrivateKey};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

pub const TOKEN: &str = "aut_test";

pub fn key(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Open {
    Ok,
    Denied,
    Limit,
    Error,
}

/// What the stub API saw and how it answers.
pub struct Stub {
    pub calls: Mutex<Vec<String>>,
    pub sessions: Mutex<Vec<Value>>,
    pub closes: Mutex<Vec<(Uuid, Value)>>,
    pub open: Mutex<Open>,
    pub heartbeat_fail: Mutex<bool>,
    pub revoke: Mutex<Vec<Uuid>>,
    pub ca_lines: Mutex<Vec<String>>,
    pub runner_endpoint: Mutex<String>,
    pub next: Mutex<Option<Uuid>>,
}

impl Stub {
    pub fn count(&self, path: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains(path))
            .count()
    }
}

async fn register(State(s): State<Arc<Stub>>) -> Json<Value> {
    s.calls.lock().unwrap().push("register".into());
    Json(json!({
        "replica_id": Uuid::new_v4(), "protocol": 1,
        "config": cfg(&s)
    }))
}

fn cfg(s: &Stub) -> Value {
    json!({
        "ca_public_keys": *s.ca_lines.lock().unwrap(),
        "heartbeat_secs": 30, "lease_secs": 90, "wake_wait_secs": 5,
        "idle_timeout_secs": 600, "max_lifetime_secs": 14400, "channels_per_session": 4
    })
}

async fn sessions(
    State(s): State<Arc<Stub>>,
    Json(b): Json<Value>,
) -> (http::StatusCode, Json<Value>) {
    s.calls.lock().unwrap().push("sessions".into());
    s.sessions.lock().unwrap().push(b);
    match *s.open.lock().unwrap() {
        Open::Ok => {
            let id = s.next.lock().unwrap().take().unwrap_or_else(Uuid::new_v4);
            (
                http::StatusCode::CREATED,
                Json(
                    json!({"session_id": id, "agent_id": Uuid::new_v4(), "sandbox": "running",
                            "lease_secs": 90, "expires_at": "2099-01-01T00:00:00Z"}),
                ),
            )
        }
        Open::Denied => (
            http::StatusCode::FORBIDDEN,
            Json(json!({"reason": "denied"})),
        ),
        Open::Limit => (
            http::StatusCode::FORBIDDEN,
            Json(json!({"reason": "limit"})),
        ),
        Open::Error => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"code": "internal", "message": "x"}})),
        ),
    }
}

async fn channels(
    State(s): State<Arc<Stub>>,
    Path(_id): Path<Uuid>,
    Json(b): Json<Value>,
) -> Json<Value> {
    s.calls
        .lock()
        .unwrap()
        .push(format!("channels:{}", b["mode"]));
    Json(json!({"grant": "auz_stubgrant", "expires_in": 30,
                "runner_endpoint": *s.runner_endpoint.lock().unwrap()}))
}

async fn heartbeat(
    State(s): State<Arc<Stub>>,
    Json(b): Json<Value>,
) -> (http::StatusCode, Json<Value>) {
    s.calls.lock().unwrap().push("heartbeat".into());
    if *s.heartbeat_fail.lock().unwrap() {
        return (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"code": "internal", "message": "x"}})),
        );
    }
    let live: Vec<Uuid> = b["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap().parse().unwrap())
        .collect();
    let revoked: Vec<Value> = s
        .revoke
        .lock()
        .unwrap()
        .iter()
        .filter(|i| live.contains(i))
        .map(|i| json!({"id": i, "reason": "user_disconnect"}))
        .collect();
    (
        http::StatusCode::OK,
        Json(json!({"revoked": revoked, "config": cfg(&s)})),
    )
}

async fn close(
    State(s): State<Arc<Stub>>,
    Path(id): Path<Uuid>,
    Json(b): Json<Value>,
) -> Json<Value> {
    s.calls.lock().unwrap().push("close".into());
    s.closes.lock().unwrap().push((id, b));
    Json(json!({"ok": true}))
}

/// The runner stub: records `exec.open` frames and the grant, echoes stdin to stdout, answers
/// `exec.eof` with stderr `bye`, `exec.exit { status: 7 }` and a normal close.
pub struct Runner {
    pub addr: SocketAddr,
    pub opens: Arc<Mutex<Vec<Value>>>,
    pub grants: Arc<Mutex<Vec<String>>>,
}

pub async fn runner() -> Runner {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let opens = Arc::new(Mutex::new(Vec::new()));
    let grants = Arc::new(Mutex::new(Vec::new()));
    let (o, g) = (opens.clone(), grants.clone());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else {
                return;
            };
            let (o, g) = (o.clone(), g.clone());
            tokio::spawn(async move {
                let cb = |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                          resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    if let Some(a) = req.headers().get("authorization") {
                        g.lock().unwrap().push(a.to_str().unwrap().to_owned());
                    }
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await else {
                    return;
                };
                let Some(Ok(Message::Text(t))) = ws.next().await else {
                    return;
                };
                o.lock().unwrap().push(serde_json::from_str(&t).unwrap());
                ws.send(Message::text(r#"{"kind":"exec.opened"}"#))
                    .await
                    .unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    match m {
                        Message::Binary(b) if b.first() == Some(&0x02) => {
                            ws.send(Message::binary(b.to_vec())).await.unwrap();
                        }
                        Message::Text(t) if t.contains("exec.eof") => {
                            ws.send(Message::binary(b"\x03bye".to_vec())).await.unwrap();
                            ws.send(Message::text(r#"{"kind":"exec.exit","status":7}"#))
                                .await
                                .unwrap();
                            let _ = ws.close(None).await;
                            return;
                        }
                        Message::Close(_) => return,
                        _ => {}
                    }
                }
            });
        }
    });
    Runner {
        addr,
        opens,
        grants,
    }
}

pub struct Rig {
    pub stub: Arc<Stub>,
    pub runner: Runner,
    pub shared: Arc<Shared>,
    pub ssh: SocketAddr,
    pub ca: PrivateKey,
    pub user_key: Arc<PrivateKey>,
    pub agent: Uuid,
    pub stop: watch::Sender<bool>,
}

pub struct RigOpts {
    pub settings: Settings,
    pub pre: Arc<PreAuth>,
}

impl Default for RigOpts {
    fn default() -> Self {
        Self {
            settings: Settings {
                auth_failure_delay: Duration::from_millis(150),
                auth_timeout: Duration::from_secs(20),
                heartbeat_period: Some(Duration::from_secs(3600)),
                notice_wait: Duration::from_millis(500),
                idle: None,
                lifetime: None,
            },
            pre: PreAuth::new(256, 100, 6000, 100),
        }
    }
}

pub async fn rig(o: RigOpts) -> Rig {
    let ca = key(0x5a);
    let runner = runner().await;
    let stub = Arc::new(Stub {
        calls: Mutex::default(),
        sessions: Mutex::default(),
        closes: Mutex::default(),
        open: Mutex::new(Open::Ok),
        heartbeat_fail: Mutex::new(false),
        revoke: Mutex::default(),
        ca_lines: Mutex::new(vec![ca.public_key().to_openssh().unwrap()]),
        runner_endpoint: Mutex::new(format!("ws://{}/v1/preview-stream", runner.addr)),
        next: Mutex::new(None),
    });
    let api = Router::new()
        .route("/v1/terminator/register", post(register))
        .route("/v1/terminator/sessions", post(sessions))
        .route("/v1/terminator/sessions/{id}/channels", post(channels))
        .route("/v1/terminator/heartbeat", post(heartbeat))
        .route("/v1/terminator/sessions/{id}/close", post(close))
        .with_state(stub.clone());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, api).await.unwrap() });

    let host = key(0x33);
    let (stop, stop_rx) = watch::channel(false);
    let shared = Shared::new(
        Plane::new(&format!("http://{api_addr}"), TOKEN).unwrap(),
        o.settings,
        o.pre,
        stop_rx.clone(),
        host.public_key().to_openssh().unwrap(),
    );
    tokio::spawn(shared.clone().run_plane());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ssh = l.local_addr().unwrap();
    tokio::spawn(aurora_terminator::server::serve(
        l,
        shared.clone(),
        host,
        stop_rx,
    ));
    // Wait for the registration (the plane marks the replica healthy).
    for _ in 0..200 {
        if shared.is_healthy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(shared.is_healthy(), "the terminator did not register");
    Rig {
        stub,
        runner,
        shared,
        ssh,
        ca,
        user_key: Arc::new(key(0x11)),
        agent: Uuid::new_v4(),
        stop,
    }
}

/// How a certificate is built (defaults: a good one for `rig.agent`).
#[derive(Clone)]
pub struct CertSpec {
    pub ca: PrivateKey,
    pub key_id: Option<String>,
    /// The principals (default: `[rig.agent]`, a certificate minted before D-445).
    pub principals: Option<Vec<String>>,
    pub cert_type: CertType,
    pub valid_after: i64,
    pub valid_before: i64,
    pub critical: bool,
    pub permit_pty: bool,
}

impl Rig {
    pub fn spec(&self) -> CertSpec {
        let now = chrono::Utc::now().timestamp();
        CertSpec {
            ca: self.ca.clone(),
            key_id: None,
            principals: None,
            cert_type: CertType::User,
            valid_after: now - 60,
            valid_before: now + 600,
            critical: false,
            permit_pty: true,
        }
    }

    pub fn cert(&self, s: &CertSpec) -> Certificate {
        let mut b = Builder::new(
            [7u8; 32],
            self.user_key.public_key().key_data().clone(),
            s.valid_after as u64,
            s.valid_before as u64,
        )
        .unwrap();
        b.serial(1).unwrap();
        b.cert_type(s.cert_type).unwrap();
        b.key_id(s.key_id.clone().unwrap_or_else(|| {
            format!(
                "aurora:u={}:k={}:a={}:w={}",
                Uuid::new_v4(),
                Uuid::new_v4(),
                self.agent,
                Uuid::new_v4()
            )
        }))
        .unwrap();
        for p in s
            .principals
            .clone()
            .unwrap_or_else(|| vec![self.agent.to_string()])
        {
            b.valid_principal(p).unwrap();
        }
        if s.permit_pty {
            b.extension("permit-pty", "").unwrap();
        }
        if s.critical {
            b.critical_option("force-command", "/bin/false").unwrap();
        }
        b.sign(&s.ca).unwrap()
    }

    pub fn good(&self) -> Certificate {
        self.cert(&self.spec())
    }
}

pub struct Client;

impl client::Handler for Client {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        _key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub type Conn = client::Handle<Client>;

pub async fn connect(addr: SocketAddr) -> Conn {
    let cfg = Arc::new(client::Config::default());
    client::connect(cfg, addr, Client).await.unwrap()
}

/// What a failed authentication looks like to the client.
#[derive(Debug, PartialEq, Eq)]
pub struct Failure {
    pub methods: Vec<String>,
    pub partial: bool,
}

pub fn failure(r: &client::AuthResult) -> Option<Failure> {
    match r {
        client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => Some(Failure {
            methods: remaining_methods
                .iter()
                .map(|m| <&str>::from(m).to_owned())
                .collect(),
            partial: *partial_success,
        }),
        client::AuthResult::Success => None,
    }
}

pub async fn raw_tcp(addr: SocketAddr) -> TcpStream {
    TcpStream::connect(addr).await.unwrap()
}
