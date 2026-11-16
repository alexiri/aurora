#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The Files backend (phase 5, P5-FILES-1, D-316, SEC-125, SEC-126; C-898..C-903, C-906;
//! docs/18 §Files, API). The API is an SFTP client over the runner's `auf_` leg; here the runner
//! is a fake that redeems the grant at the served API exactly as the real listener does, then
//! speaks SFTP v3 with an in-process server over a real temp directory standing in for
//! `/workspace` (symlinks, modes and renames are the host filesystem's own). The fake counts
//! SFTP requests, so "no SFTP request on a 400" is proven, not assumed.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use aurora_core::token;
use futures::{SinkExt, StreamExt};
use http::Method;
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Packet, Status, StatusCode, Version,
};
use russh_sftp::server::{Handler, StatusReply};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

// ------------------------------------------------------------------- the fake sftp-server

struct Fs {
    root: PathBuf,
    reqs: Arc<AtomicUsize>,
    handles: HashMap<String, H>,
    next: u32,
}

enum H {
    File(std::fs::File),
    Dir(Vec<File>),
}

fn e(code: StatusCode) -> StatusReply {
    StatusReply::new(code)
}

fn io(err: &std::io::Error) -> StatusReply {
    e(match err.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    })
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

fn attrs_of(m: &std::fs::Metadata) -> FileAttributes {
    FileAttributes {
        size: Some(m.len()),
        uid: Some(m.uid()),
        gid: Some(m.gid()),
        permissions: Some(m.mode()),
        atime: Some(m.atime() as u32),
        mtime: Some(m.mtime() as u32),
        ..FileAttributes::empty()
    }
}

impl Fs {
    fn host(&self, p: &str) -> PathBuf {
        match p.strip_prefix("/workspace") {
            Some("") => self.root.clone(),
            Some(r) if r.starts_with('/') => self.root.join(&r[1..]),
            _ => PathBuf::from(p),
        }
    }
    fn virt(&self, h: &Path) -> String {
        match h.strip_prefix(&self.root) {
            Ok(r) if r.as_os_str().is_empty() => "/workspace".into(),
            Ok(r) => format!("/workspace/{}", r.display()),
            Err(_) => h.display().to_string(),
        }
    }
    fn handle(&mut self, h: H) -> Handle {
        self.next += 1;
        let k = format!("h{}", self.next);
        self.handles.insert(k.clone(), h);
        Handle { id: 0, handle: k }
    }
    fn count(&self) {
        self.reqs.fetch_add(1, Ordering::SeqCst);
    }
}

impl Handler for Fs {
    type Error = StatusReply;

    fn unimplemented(&self) -> StatusReply {
        e(StatusCode::OpUnsupported)
    }

    async fn init(&mut self, _v: u32, _e: HashMap<String, String>) -> Result<Version, StatusReply> {
        let mut v = Version::new();
        v.extensions
            .insert("posix-rename@openssh.com".into(), "1".into());
        Ok(v)
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, StatusReply> {
        self.count();
        let mut o = std::fs::OpenOptions::new();
        o.read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE))
            .create_new(pflags.contains(OpenFlags::EXCLUDE) && pflags.contains(OpenFlags::CREATE))
            .create(pflags.contains(OpenFlags::CREATE))
            .truncate(pflags.contains(OpenFlags::TRUNCATE))
            .mode(attrs.permissions.unwrap_or(0o644));
        let f = o.open(self.host(&filename)).map_err(|x| io(&x))?;
        let mut h = self.handle(H::File(f));
        h.id = id;
        Ok(h)
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusReply> {
        self.count();
        self.handles.remove(&handle);
        Ok(ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, StatusReply> {
        self.count();
        let Some(H::File(f)) = self.handles.get(&handle) else {
            return Err(e(StatusCode::Failure));
        };
        let mut buf = vec![0u8; len as usize];
        let n = f.read_at(&mut buf, offset).map_err(|x| io(&x))?;
        if n == 0 {
            return Err(e(StatusCode::Eof));
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, StatusReply> {
        self.count();
        let Some(H::File(f)) = self.handles.get(&handle) else {
            return Err(e(StatusCode::Failure));
        };
        f.write_all_at(&data, offset).map_err(|x| io(&x))?;
        Ok(ok(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusReply> {
        self.count();
        let m = std::fs::symlink_metadata(self.host(&path)).map_err(|x| io(&x))?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&m),
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusReply> {
        self.count();
        let m = std::fs::metadata(self.host(&path)).map_err(|x| io(&x))?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&m),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, StatusReply> {
        self.count();
        let Some(H::File(f)) = self.handles.get(&handle) else {
            return Err(e(StatusCode::Failure));
        };
        let m = f.metadata().map_err(|x| io(&x))?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&m),
        })
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, StatusReply> {
        self.count();
        let Some(H::File(f)) = self.handles.get(&handle) else {
            return Err(e(StatusCode::Failure));
        };
        if let Some(m) = attrs.permissions {
            f.set_permissions(std::fs::Permissions::from_mode(m & 0o7777))
                .map_err(|x| io(&x))?;
        }
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, StatusReply> {
        self.count();
        let dir = self.host(&path);
        let mut v = vec![
            File::new(".", FileAttributes::empty()),
            File::new("..", FileAttributes::empty()),
        ];
        for ent in std::fs::read_dir(&dir).map_err(|x| io(&x))? {
            let ent = ent.map_err(|x| io(&x))?;
            let m = std::fs::symlink_metadata(ent.path()).map_err(|x| io(&x))?;
            v.push(File::new(
                ent.file_name().to_string_lossy().to_string(),
                attrs_of(&m),
            ));
        }
        let mut h = self.handle(H::Dir(v));
        h.id = id;
        Ok(h)
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, StatusReply> {
        self.count();
        let Some(H::Dir(v)) = self.handles.get_mut(&handle) else {
            return Err(e(StatusCode::Failure));
        };
        if v.is_empty() {
            return Err(e(StatusCode::Eof));
        }
        let take: Vec<File> = v.drain(..v.len().min(64)).collect();
        Ok(Name { id, files: take })
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, StatusReply> {
        self.count();
        std::fs::remove_file(self.host(&filename)).map_err(|x| io(&x))?;
        Ok(ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, StatusReply> {
        self.count();
        let p = self.host(&path);
        std::fs::create_dir(&p).map_err(|x| io(&x))?;
        if let Some(m) = attrs.permissions {
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(m & 0o7777));
        }
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, StatusReply> {
        self.count();
        std::fs::remove_dir(self.host(&path)).map_err(|x| io(&x))?;
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusReply> {
        self.count();
        let h = self.host(&path);
        // OpenSSH: a missing *final* component resolves to itself under its resolved parent.
        let real = match std::fs::canonicalize(&h) {
            Ok(r) => r,
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => {
                let parent = h.parent().ok_or_else(|| e(StatusCode::NoSuchFile))?;
                let name = h.file_name().ok_or_else(|| e(StatusCode::NoSuchFile))?;
                std::fs::canonicalize(parent)
                    .map_err(|x| io(&x))?
                    .join(name)
            }
            Err(x) => return Err(io(&x)),
        };
        Ok(Name {
            id,
            files: vec![File::dummy(self.virt(&real))],
        })
    }

    async fn readlink(&mut self, id: u32, path: String) -> Result<Name, StatusReply> {
        self.count();
        let t = std::fs::read_link(self.host(&path)).map_err(|x| io(&x))?;
        Ok(Name {
            id,
            files: vec![File::dummy(t.display().to_string())],
        })
    }

    async fn extended(
        &mut self,
        id: u32,
        request: String,
        data: Vec<u8>,
    ) -> Result<Packet, StatusReply> {
        self.count();
        if request != "posix-rename@openssh.com" {
            return Err(e(StatusCode::OpUnsupported));
        }
        let mut rest = &data[..];
        let mut take = || {
            let n = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            let s = String::from_utf8(rest[4..4 + n].to_vec()).unwrap();
            rest = &rest[4 + n..];
            s
        };
        let (a, b) = (take(), take());
        std::fs::rename(self.host(&a), self.host(&b)).map_err(|x| io(&x))?;
        Ok(Packet::Status(ok(id)))
    }
}

// --------------------------------------------------------------------------- the fake runner

/// The runner's API → supervisor credit window as the worst case the contract allows: the
/// supervisor returns credit for a request no later than its reply, so the bytes of requests
/// the server has not yet answered count against `TERMINAL_STREAM_WINDOW` (a real supervisor
/// credits at the stdin write, earlier; the runner closes the leg 4502 when the window is
/// exceeded). Both directions are SFTP streams: `u32` length + payload per packet, one reply
/// per request, in order.
#[derive(Default)]
struct Window {
    up: Vec<u8>,
    down: Vec<u8>,
    pending: std::collections::VecDeque<u64>,
    inflight: u64,
}

impl Window {
    fn packets(buf: &mut Vec<u8>) -> Vec<u64> {
        let mut out = Vec::new();
        while buf.len() >= 4 {
            let n = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
            if buf.len() < 4 + n {
                break;
            }
            out.push(4 + n as u64);
            buf.drain(..4 + n);
        }
        out
    }
    /// Bytes API → server; `true` while within the window.
    fn request(&mut self, b: &[u8]) -> bool {
        self.up.extend_from_slice(b);
        for p in Self::packets(&mut self.up) {
            self.pending.push_back(p);
            self.inflight += p;
        }
        self.inflight <= aurora_core::limits::TERMINAL_STREAM_WINDOW as u64
    }
    /// Bytes server → API: each reply credits the oldest request.
    fn reply(&mut self, b: &[u8]) {
        self.down.extend_from_slice(b);
        for _ in Self::packets(&mut self.down) {
            self.inflight -= self.pending.pop_front().unwrap_or(0);
        }
    }
}

struct Fake {
    /// Legs closed 4502 because the API sent more than the credit window allows.
    window_violations: Arc<AtomicUsize>,
    /// Milliseconds the fake holds each SFTP reply (a runner ↔ supervisor round trip); `0`
    /// answers as fast as the in-process server does.
    reply_delay_ms: Arc<AtomicU64>,
    root: PathBuf,
    outside: PathBuf,
    /// SFTP requests the server handled (INIT excluded).
    reqs: Arc<AtomicUsize>,
    /// Legs accepted and redeemed.
    legs: Arc<AtomicUsize>,
    /// Bumping this closes every live leg with 4503 (a W6 reconnect or a park).
    kill: tokio::sync::watch::Sender<u64>,
    _dir: Scratch,
}

/// A scratch directory removed on drop (no `tempfile` dependency in this workspace).
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("aurora-files-it-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fake runner at the rig's replica: accepts `auf_` legs, redeems the grant at the served
/// API as a runner would (`kind: "files"`), checks the first frame, answers `exec.opened` and
/// bridges the `[0x02]` frames to the in-process SFTP server.
async fn fake_runner(r: &Rig, api: &str) -> Fake {
    let dir = Scratch::new();
    let base = std::fs::canonicalize(&dir.0).unwrap();
    let root = base.join("workspace");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let reqs = Arc::new(AtomicUsize::new(0));
    let legs = Arc::new(AtomicUsize::new(0));
    let violations = Arc::new(AtomicUsize::new(0));
    let (kill, _) = tokio::sync::watch::channel(0u64);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    sqlx::query("UPDATE runner_replicas SET advertised_host = $1")
        .bind(addr)
        .execute(r.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE runners SET capabilities = capabilities || '{\"files\": true}'::jsonb")
        .execute(r.h.pool())
        .await
        .unwrap();
    let rep: Uuid = sqlx::query_scalar("SELECT id FROM runner_replicas LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let (api, tok) = (api.to_owned(), r.tok.clone());
    let (root2, reqs2, legs2, kill2) = (root.clone(), reqs.clone(), legs.clone(), kill.clone());
    let viol2 = violations.clone();
    let delay = Arc::new(AtomicU64::new(0));
    let delay2 = delay.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (api, tok, root, reqs, legs, viol, delay) = (
                api.clone(),
                tok.clone(),
                root2.clone(),
                reqs2.clone(),
                legs2.clone(),
                viol2.clone(),
                delay2.clone(),
            );
            let mut kill_rx = kill2.subscribe();
            tokio::spawn(async move {
                let auth = Arc::new(std::sync::Mutex::new(String::new()));
                let a2 = auth.clone();
                #[allow(clippy::result_large_err)]
                let cb = move |req: &tokio_tungstenite::tungstenite::http::Request<()>,
                               resp: tokio_tungstenite::tungstenite::http::Response<()>| {
                    *a2.lock().unwrap() = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await else {
                    return;
                };
                let grant = auth
                    .lock()
                    .unwrap()
                    .strip_prefix("Bearer ")
                    .unwrap_or_default()
                    .to_owned();
                let redeemed = reqwest::Client::new()
                    .post(format!("{api}/v1/runner/preview-streams/open"))
                    .header("authorization", format!("Bearer {tok}"))
                    .header("x-aurora-replica", rep.to_string())
                    .json(&json!({"grant": grant, "kind": "files"}))
                    .send()
                    .await
                    .unwrap();
                if !redeemed.status().is_success() {
                    let _ = ws.close(None).await;
                    return;
                }
                // The first frame must be exactly the bare sftp open (W21).
                let Some(Ok(Message::Text(t))) = ws.next().await else {
                    return;
                };
                assert_eq!(t.as_str(), r#"{"kind":"exec.open","mode":"sftp"}"#);
                ws.send(Message::Text(r#"{"kind":"exec.opened"}"#.into()))
                    .await
                    .unwrap();
                legs.fetch_add(1, Ordering::SeqCst);
                let (mut sftp_io, srv_io) = tokio::io::duplex(1 << 16);
                russh_sftp::server::run(
                    srv_io,
                    Fs {
                        root,
                        reqs,
                        handles: HashMap::new(),
                        next: 0,
                    },
                )
                .await;
                let mut buf = vec![0u8; 16 * 1024 + 1];
                buf[0] = 0x02;
                let mut window = Window::default();
                let mut held: std::collections::VecDeque<(tokio::time::Instant, Vec<u8>)> =
                    std::collections::VecDeque::new();
                loop {
                    tokio::select! {
                        _ = kill_rx.changed() => {
                            let _ = ws.send(Message::Close(Some(
                                tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                    code: 4503.into(),
                                    reason: "".into(),
                                })))
                                .await;
                            return;
                        }
                        m = ws.next() => match m {
                            Some(Ok(Message::Binary(b))) if b.first() == Some(&0x02) => {
                                if !window.request(&b[1..]) {
                                    viol.fetch_add(1, Ordering::SeqCst);
                                    let _ = ws.send(Message::Close(Some(
                                        tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                            code: 4502.into(),
                                            reason: "".into(),
                                        })))
                                        .await;
                                    return;
                                }
                                if sftp_io.write_all(&b[1..]).await.is_err() { return; }
                            }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                            _ => {}
                        },
                        n = sftp_io.read(&mut buf[1..]) => match n {
                            Ok(n) if n > 0 => {
                                let due = tokio::time::Instant::now()
                                    + std::time::Duration::from_millis(delay.load(Ordering::Relaxed));
                                held.push_back((due, buf[..=n].to_vec()));
                            }
                            _ => return,
                        },
                        // A held reply is due: send it. Credit comes back with the reply, so
                        // only now may the API's next request count against a smaller window.
                        () = async {
                            match held.front() {
                                Some((due, _)) => tokio::time::sleep_until(*due).await,
                                None => std::future::pending().await,
                            }
                        } => {
                            let Some((_, reply)) = held.pop_front() else { continue };
                            if ws.send(Message::Binary(reply.clone().into())).await.is_err() { return; }
                            window.reply(&reply[1..]);
                        }
                    }
                }
            });
        }
    });
    Fake {
        window_violations: violations,
        reply_delay_ms: delay,
        root,
        outside,
        reqs,
        legs,
        kill,
        _dir: dir,
    }
}

// ----------------------------------------------------------------------------- the rig

struct Env {
    r: Rig,
    api: String,
    fake: Fake,
    agent: Uuid,
    sb: Uuid,
    http: reqwest::Client,
}

async fn serve(r: &Rig) -> String {
    let app = r.h.app.clone();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l, app.into_make_service()).await.unwrap();
    });
    format!("http://{addr}")
}

async fn sandbox(r: &Rig, handle: &str, state: &str) -> (Uuid, Uuid) {
    let agent = r.agent(handle, false).await;
    let sb = Uuid::now_v7();
    let runner: Uuid = sqlx::query_scalar("SELECT id FROM runners LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let replica: Uuid = sqlx::query_scalar("SELECT id FROM runner_replicas LIMIT 1")
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
    .bind(replica)
    .bind(state)
    .bind(Uuid::now_v7())
    .execute(r.h.pool())
    .await
    .unwrap();
    (agent, sb)
}

async fn env() -> Env {
    let r = rig_with(|c| c.terminal.enabled = true).await;
    let api = serve(&r).await;
    let fake = fake_runner(&r, &api).await;
    let (agent, sb) = sandbox(&r, "files-agent", "running").await;
    Env {
        r,
        api,
        fake,
        agent,
        sb,
        http: reqwest::Client::new(),
    }
}

impl Env {
    fn url(&self, tail: &str) -> String {
        format!("{}/v1/agents/{}/files{tail}", self.api, self.agent)
    }
    fn rb(&self, m: reqwest::Method, tail: &str) -> reqwest::RequestBuilder {
        self.http
            .request(m, self.url(tail))
            .header("cookie", &self.r.cookie)
            .header("origin", ORIGIN)
    }
    async fn json(&self, rb: reqwest::RequestBuilder) -> (u16, Value) {
        let res = rb.send().await.unwrap();
        let s = res.status().as_u16();
        (s, res.json().await.unwrap_or(Value::Null))
    }
    async fn get(&self, tail: &str) -> (u16, Value) {
        self.json(self.rb(reqwest::Method::GET, tail)).await
    }
    async fn op(&self, body: Value) -> (u16, Value) {
        self.json(self.rb(reqwest::Method::POST, "/ops").json(&body))
            .await
    }
    async fn put(&self, path: &str, body: Vec<u8>, hdr: &[(&str, &str)]) -> (u16, Value) {
        let mut rb = self
            .rb(reqwest::Method::PUT, &format!("/content?path={path}"))
            .body(body);
        for (k, v) in hdr {
            rb = rb.header(*k, *v);
        }
        self.json(rb).await
    }
    fn ws(&self, rel: &str) -> PathBuf {
        self.fake.root.join(rel)
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

// ---------------------------------------------------------------------------- the tests

/// SEC-125 auth matrix: feature off, anonymous, API key, foreign workspace, BYO, frozen.
#[tokio::test]
async fn auth_matrix_refuses_everyone_but_a_session_with_can_use() {
    let off = rig_with(|_| {}).await;
    let (a, _) = sandbox(&off, "off", "running").await;
    let (s, _, v) = off
        .h
        .send(
            Method::GET,
            &format!("/v1/agents/{a}/files"),
            &[("cookie", &off.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (404, "terminal_disabled"), "{v}");

    let t = env().await;
    // anonymous
    let res = t.http.get(t.url("")).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 401);
    // API key (workspace key of the same workspace)
    let user = t.r.h.user("k@x.io", false).await;
    let (_, tok) = t.r.h.api_key(user, t.r.org, Some(t.r.ws), false).await;
    let res = t
        .http
        .get(t.url(""))
        .header("authorization", bearer(&tok))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 401);
    // BYO
    let byo = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,'byo-a','byo-a','byo','fake')",
    )
    .bind(byo)
    .bind(t.r.org)
    .bind(t.r.ws)
    .execute(t.r.h.pool())
    .await
    .unwrap();
    let res = t.rb(reqwest::Method::GET, "").send().await.unwrap();
    assert_eq!(res.status().as_u16(), 200, "the hosted agent works");
    let res = t
        .http
        .get(format!("{}/v1/agents/{byo}/files", t.api))
        .header("cookie", &t.r.cookie)
        .send()
        .await
        .unwrap();
    let st = res.status().as_u16();
    let v: Value = res.json().await.unwrap();
    assert_eq!((st, code(&v)), (422, "files_not_supported"), "{v}");
    assert_eq!(v["error"]["details"]["reason"], "byo");
    // another workspace's agent is invisible (404)
    let other_org = t.r.h.org().await;
    let other_ws = t.r.h.workspace(other_org).await;
    let foreign = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(foreign)
    .bind(other_org)
    .bind(other_ws)
    .execute(t.r.h.pool())
    .await
    .unwrap();
    let res = t
        .http
        .get(format!("{}/v1/agents/{foreign}/files", t.api))
        .header("cookie", &t.r.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 404);
    // frozen agent: 423, no listing
    let (fa, _) = sandbox(&t.r, "frozen", "running").await;
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 'incident', \
         frozen_authority = 'workspace_owner' WHERE id = $1",
    )
    .bind(fa)
    .execute(t.r.h.pool())
    .await
    .unwrap();
    let res = t
        .http
        .get(format!("{}/v1/agents/{fa}/files", t.api))
        .header("cookie", &t.r.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 423);
}

/// A parked sandbox is a wake demand (503 `sandbox_waking`, `retry-after`), an unprovisioned or
/// failed one 409 `sandbox_unavailable` with its state.
#[tokio::test]
async fn parked_sandbox_wakes_and_dead_ones_are_409() {
    let t = env().await;
    let (a, sb) = sandbox(&t.r, "parked", "stopped").await;
    let res = t
        .http
        .get(format!("{}/v1/agents/{a}/files", t.api))
        .header("cookie", &t.r.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 503);
    assert_eq!(res.headers()["retry-after"], "2");
    let v: Value = res.json().await.unwrap();
    assert_eq!(code(&v), "sandbox_waking");
    let wake: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT terminal_wake_at FROM sandboxes WHERE id = $1")
            .bind(sb)
            .fetch_one(t.r.h.pool())
            .await
            .unwrap();
    assert!(wake.is_some(), "the wake demand is the terminal's");
    let (f, _) = sandbox(&t.r, "failed-sb", "failed").await;
    let res = t
        .http
        .get(format!("{}/v1/agents/{f}/files", t.api))
        .header("cookie", &t.r.cookie)
        .send()
        .await
        .unwrap();
    let st = res.status().as_u16();
    let v: Value = res.json().await.unwrap();
    assert_eq!((st, code(&v)), (409, "sandbox_unavailable"), "{v}");
    assert_eq!(v["error"]["details"]["state"], "failed");
}

/// C-899 path table: every malformed path is 400 `invalid_path` and no leg or SFTP request
/// ever happens (counting fake).
#[tokio::test]
async fn invalid_paths_are_400_before_any_sftp_request() {
    let t = env().await;
    let long_comp = format!("/workspace/{}", "a".repeat(256));
    let long_path = format!("/workspace/{}", "a/".repeat(2100));
    for bad in [
        "workspace/a",
        "/etc/passwd",
        "/workspace2",
        "/workspace/../etc",
        "/workspace/./a",
        "/workspace//a",
        "/workspace/a%00b",
        long_comp.as_str(),
        long_path.as_str(),
    ] {
        let enc = bad.replace('/', "%2F");
        for tail in [
            format!("?path={enc}"),
            format!("/text?path={enc}"),
            format!("/content?path={enc}"),
        ] {
            let (s, v) = t.get(&tail).await;
            assert_eq!((s, code(&v)), (400, "invalid_path"), "{bad:?} {tail}: {v}");
        }
        let (s, v) = t
            .op(json!({"op": "mkdir", "path": bad.replace("%00", "\0")}))
            .await;
        assert_eq!((s, code(&v)), (400, "invalid_path"), "{bad:?}: {v}");
        let (s, v) = t.put(&bad.replace('/', "%2F"), b"x".to_vec(), &[]).await;
        assert_eq!((s, code(&v)), (400, "invalid_path"), "{bad:?}: {v}");
    }
    assert_eq!(
        t.fake.legs.load(Ordering::SeqCst),
        0,
        "no session was opened"
    );
    assert_eq!(
        t.fake.reqs.load(Ordering::SeqCst),
        0,
        "no SFTP request was sent"
    );
}

/// Listing: directories first then byte order, `.`/`..` never listed, symlink targets resolved
/// (inside / outside / dangling), modes, read-only badge on `.aurora`; one leg serves many requests.
#[tokio::test]
async fn listing_orders_entries_and_resolves_symlinks() {
    let t = env().await;
    std::fs::create_dir(t.ws("zdir")).unwrap();
    std::fs::create_dir(t.ws(".aurora")).unwrap();
    std::fs::write(t.ws("b.txt"), "b").unwrap();
    std::fs::write(t.ws("a.txt"), "aa").unwrap();
    std::fs::set_permissions(t.ws("a.txt"), std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::write(t.fake.outside.join("secret"), "s").unwrap();
    std::os::unix::fs::symlink(t.fake.outside.join("secret"), t.ws("out")).unwrap();
    std::os::unix::fs::symlink(t.ws("b.txt"), t.ws("in")).unwrap();
    std::os::unix::fs::symlink(t.ws("nowhere"), t.ws("dangling")).unwrap();
    let (s, v) = t.get("").await;
    assert_eq!(s, 200, "{v}");
    let names: Vec<&str> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [".aurora", "zdir", "a.txt", "b.txt", "dangling", "in", "out"]
    );
    assert_eq!(v["truncated"], false);
    let by = |n: &str| {
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(by("a.txt")["mode"], "0444");
    assert_eq!(by("a.txt")["writable"], false);
    assert_eq!(by("b.txt")["writable"], true);
    assert_eq!(by(".aurora")["writable"], false);
    assert_eq!(by("in")["target"]["kind"], "file");
    assert_eq!(by("in")["target"]["outside_workspace"], false);
    assert_eq!(by("out")["target"]["outside_workspace"], true);
    assert_eq!(by("dangling")["target"]["kind"], "missing");
    // a file is not a directory
    let (s, v) = t.get("?path=%2Fworkspace%2Fb.txt").await;
    assert_eq!((s, code(&v)), (422, "not_a_directory"), "{v}");
    let (s, v) = t.get("?path=%2Fworkspace%2Fnope").await;
    assert_eq!((s, code(&v)), (404, "path_not_found"), "{v}");
    assert_eq!(t.fake.legs.load(Ordering::SeqCst), 1, "one pooled session");
}

/// Symlinks resolving outside `/workspace` are refused for list, read, write and as a rename
/// target (SEC-125); the file outside is untouched.
#[tokio::test]
async fn symlinks_outside_the_workspace_are_refused() {
    let t = env().await;
    std::fs::write(t.fake.outside.join("secret"), "s").unwrap();
    std::os::unix::fs::symlink(&t.fake.outside, t.ws("odir")).unwrap();
    std::os::unix::fs::symlink(t.fake.outside.join("secret"), t.ws("ofile")).unwrap();
    std::fs::write(t.ws("mine"), "m").unwrap();
    for tail in [
        "?path=%2Fworkspace%2Fodir",
        "/content?path=%2Fworkspace%2Fofile",
        "/text?path=%2Fworkspace%2Fofile",
    ] {
        let (s, v) = t.get(tail).await;
        assert_eq!(
            (s, code(&v)),
            (403, "path_outside_workspace"),
            "{tail}: {v}"
        );
    }
    let (s, v) = t.put("%2Fworkspace%2Fofile", b"pwn".to_vec(), &[]).await;
    assert_eq!((s, code(&v)), (403, "path_outside_workspace"), "{v}");
    let (s, v) = t
        .op(json!({"op": "rename", "path": "/workspace/mine", "to": "/workspace/odir/mine"}))
        .await;
    assert_eq!((s, code(&v)), (403, "path_outside_workspace"), "{v}");
    assert_eq!(
        std::fs::read_to_string(t.fake.outside.join("secret")).unwrap(),
        "s"
    );
    assert!(std::fs::read_dir(&t.fake.outside).unwrap().count() == 1);
}

/// `/workspace/.aurora` is read-only for every write; `/workspace` cannot be renamed or deleted;
/// replacing a `0444` file is refused (the rename would bypass the bit).
#[tokio::test]
async fn read_only_paths_and_the_owner_write_bit() {
    let t = env().await;
    std::fs::create_dir(t.ws(".aurora")).unwrap();
    std::fs::write(t.ws(".aurora/cfg"), "c").unwrap();
    std::fs::write(t.ws("ro"), "r").unwrap();
    std::fs::set_permissions(t.ws("ro"), std::fs::Permissions::from_mode(0o444)).unwrap();
    let (s, _) = t.get("/content?path=%2Fworkspace%2F.aurora%2Fcfg").await;
    assert_eq!(s, 200, "reading .aurora is allowed");
    let (s, v) = t
        .put("%2Fworkspace%2F.aurora%2Fcfg", b"x".to_vec(), &[])
        .await;
    assert_eq!((s, code(&v)), (403, "path_read_only"), "{v}");
    for body in [
        json!({"op": "mkdir", "path": "/workspace/.aurora/new"}),
        json!({"op": "rename", "path": "/workspace/.aurora/cfg", "to": "/workspace/x"}),
        json!({"op": "rename", "path": "/workspace/ro", "to": "/workspace/.aurora/x"}),
        json!({"op": "delete", "path": "/workspace/.aurora", "recursive": true}),
        json!({"op": "delete", "path": "/workspace"}),
        json!({"op": "rename", "path": "/workspace", "to": "/workspace/x"}),
    ] {
        let (s, v) = t.op(body.clone()).await;
        assert_eq!((s, code(&v)), (403, "path_read_only"), "{body}: {v}");
    }
    let (s, v) = t.put("%2Fworkspace%2Fro", b"new".to_vec(), &[]).await;
    assert_eq!((s, code(&v)), (403, "permission_denied"), "{v}");
    assert_eq!(std::fs::read_to_string(t.ws("ro")).unwrap(), "r");
}

/// PUT: create (201), `If-None-Match: *` on an existing path (409), `If-Match` right/wrong/
/// missing, atomic replace keeping the mode, no temp file left, parent must exist.
#[tokio::test]
async fn upload_preconditions_and_atomic_replace() {
    let t = env().await;
    let (s, v) = t
        .put(
            "%2Fworkspace%2Fa.txt",
            b"hello".to_vec(),
            &[("if-none-match", "*")],
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let etag = v["etag"].as_str().unwrap().to_owned();
    assert_eq!(
        etag,
        format!(
            "sha256:{}",
            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(b"hello"))
        )
    );
    assert_eq!(v["entry"]["mode"], "0644");
    let (s, v) = t
        .put(
            "%2Fworkspace%2Fa.txt",
            b"x".to_vec(),
            &[("if-none-match", "*")],
        )
        .await;
    assert_eq!((s, code(&v)), (409, "file_exists"), "{v}");
    std::fs::set_permissions(t.ws("a.txt"), std::fs::Permissions::from_mode(0o640)).unwrap();
    let (s, v) = t
        .put(
            "%2Fworkspace%2Fa.txt",
            b"stale".to_vec(),
            &[("if-match", "\"sha256:00\"")],
        )
        .await;
    assert_eq!((s, code(&v)), (409, "file_changed"), "{v}");
    assert_eq!(v["error"]["details"]["current_etag"], json!(etag));
    let (s, v) = t
        .put(
            "%2Fworkspace%2Fnew.txt",
            b"x".to_vec(),
            &[("if-match", "\"sha256:00\"")],
        )
        .await;
    assert_eq!((s, code(&v)), (409, "file_changed"), "{v}");
    assert!(!t.ws("new.txt").exists());
    let (s, v) = t
        .put(
            "%2Fworkspace%2Fa.txt",
            b"fresh".to_vec(),
            &[("if-match", &format!("\"{etag}\""))],
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(std::fs::read_to_string(t.ws("a.txt")).unwrap(), "fresh");
    assert_eq!(
        std::fs::metadata(t.ws("a.txt")).unwrap().mode() & 0o7777,
        0o640,
        "mode kept"
    );
    let (s, v) = t
        .put("%2Fworkspace%2Fmissing%2Fa.txt", b"x".to_vec(), &[])
        .await;
    assert_eq!((s, code(&v)), (404, "path_not_found"), "{v}");
    assert_eq!(v["error"]["details"]["missing"], "parent");
    let leftovers: Vec<_> = std::fs::read_dir(&t.fake.root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".aurora-upload-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// 413 before any write, and an aborted body leaves no temp file (cleanup on every failure).
#[tokio::test]
async fn oversize_is_413_and_an_aborted_body_cleans_up() {
    let t = env().await;
    let big = aurora_core::limits::FILES_UPLOAD_MAX + 1;
    let res = t
        .rb(reqwest::Method::PUT, "/content?path=%2Fworkspace%2Fbig")
        .header("content-length", big.to_string())
        .body(reqwest::Body::wrap_stream(futures::stream::once(async {
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"x"))
        })))
        .send()
        .await;
    if let Ok(res) = res {
        assert_eq!(res.status().as_u16(), 413);
    }
    assert!(!t.ws("big").exists());
    // aborted body: declared 100 KiB, 40 KiB sent, then the stream errors
    let chunks = futures::stream::iter(vec![
        Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![7u8; 40 * 1024])),
        Err(std::io::Error::other("client went away")),
    ]);
    let res = t
        .rb(reqwest::Method::PUT, "/content?path=%2Fworkspace%2Fabort")
        .header("content-length", (100 * 1024).to_string())
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await;
    assert!(res.is_err() || res.unwrap().status().as_u16() >= 400);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let any = std::fs::read_dir(&t.fake.root)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.contains("abort")
            });
        if !any {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "temp file left behind"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!t.ws("abort").exists());
}

/// Recursive delete never follows a symlink (a link to an outside directory is removed as a
/// link), and refuses up front over `FILES_DELETE_MAX` with nothing deleted.
#[tokio::test]
async fn recursive_delete_is_lstat_based_and_capped() {
    let t = env().await;
    std::fs::create_dir_all(t.ws("tree/sub")).unwrap();
    std::fs::write(t.ws("tree/sub/f"), "f").unwrap();
    std::fs::write(t.fake.outside.join("keep"), "k").unwrap();
    std::os::unix::fs::symlink(&t.fake.outside, t.ws("tree/link")).unwrap();
    let (s, v) = t
        .op(json!({"op": "delete", "path": "/workspace/tree"}))
        .await;
    assert_eq!((s, code(&v)), (409, "directory_not_empty"), "{v}");
    let (s, v) = t
        .op(json!({"op": "delete", "path": "/workspace/tree", "recursive": true}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["deleted"], 4, "tree, sub, f, link");
    assert!(!t.ws("tree").exists());
    assert_eq!(
        std::fs::read_to_string(t.fake.outside.join("keep")).unwrap(),
        "k"
    );
    std::fs::create_dir(t.ws("many")).unwrap();
    for i in 0..=aurora_core::limits::FILES_DELETE_MAX {
        std::fs::write(t.ws(&format!("many/{i}")), "").unwrap();
    }
    let (s, v) = t
        .op(json!({"op": "delete", "path": "/workspace/many", "recursive": true}))
        .await;
    assert_eq!((s, code(&v)), (422, "too_many_entries"), "{v}");
    assert_eq!(
        std::fs::read_dir(t.ws("many")).unwrap().count(),
        aurora_core::limits::FILES_DELETE_MAX + 1,
        "nothing deleted"
    );
}

/// mkdir / rename outcomes of C-898.
#[tokio::test]
async fn mkdir_and_rename() {
    let t = env().await;
    let (s, v) = t
        .op(json!({"op": "mkdir", "path": "/workspace/a/b", "parents": true}))
        .await;
    assert_eq!(s, 201, "{v}");
    assert!(t.ws("a/b").is_dir());
    let (s, v) = t
        .op(json!({"op": "mkdir", "path": "/workspace/a/b", "parents": true}))
        .await;
    assert_eq!(s, 200, "mkdir -p on an existing directory: {v}");
    let (s, v) = t.op(json!({"op": "mkdir", "path": "/workspace/a"})).await;
    assert_eq!((s, code(&v)), (409, "file_exists"), "{v}");
    std::fs::write(t.ws("x"), "x").unwrap();
    std::fs::write(t.ws("y"), "y").unwrap();
    let (s, v) = t
        .op(json!({"op": "rename", "path": "/workspace/x", "to": "/workspace/y"}))
        .await;
    assert_eq!((s, code(&v)), (409, "file_exists"), "{v}");
    let (s, v) = t
        .op(json!({"op": "rename", "path": "/workspace/a", "to": "/workspace/a/b/c"}))
        .await;
    assert_eq!((s, code(&v)), (422, "invalid_path"), "{v}");
    let (s, v) = t
        .op(json!({"op": "rename", "path": "/workspace/x", "to": "/workspace/z"}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["entry"]["path"], "/workspace/z");
    assert!(t.ws("z").exists() && !t.ws("x").exists());
}

/// The editor read: etag, line ending, and the three `file_not_editable` reasons.
#[tokio::test]
async fn text_reads_and_refusals() {
    let t = env().await;
    std::fs::write(t.ws("crlf.txt"), "a\r\nb").unwrap();
    std::fs::write(t.ws("bin"), b"a\0b").unwrap();
    std::fs::write(t.ws("latin"), b"caf\xe9").unwrap();
    std::fs::write(
        t.ws("big"),
        vec![b'a'; (aurora_core::limits::FILES_EDIT_MAX + 1) as usize],
    )
    .unwrap();
    let (s, v) = t.get("/text?path=%2Fworkspace%2Fcrlf.txt").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["line_ending"].as_str(), v["content"].as_str()),
        (Some("crlf"), Some("a\r\nb"))
    );
    for (name, reason) in [
        ("bin", "binary"),
        ("latin", "not_utf8"),
        ("big", "too_large"),
    ] {
        let (s, v) = t.get(&format!("/text?path=%2Fworkspace%2F{name}")).await;
        assert_eq!((s, code(&v)), (422, "file_not_editable"), "{v}");
        assert_eq!(v["error"]["details"]["reason"], reason);
    }
}

/// C-903: whatever the name or content, the download is an octet-stream attachment with the
/// hardening headers; hostile names stay safe.
#[tokio::test]
async fn download_headers_never_serve_inline() {
    let t = env().await;
    std::fs::write(t.ws("p.html"), "<script>alert(1)</script>").unwrap();
    std::fs::write(t.ws("p.svg"), "<svg onload=alert(1)/>").unwrap();
    std::fs::write(t.ws("fake.png"), "<html>").unwrap();
    std::fs::write(t.ws("q\"x.txt"), "q").unwrap();
    for name in ["p.html", "p.svg", "fake.png", "q\"x.txt"] {
        let enc = format!("%2Fworkspace%2F{}", name.replace('"', "%22"));
        let res = t
            .rb(reqwest::Method::GET, &format!("/content?path={enc}"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), 200, "{name}");
        let h = res.headers().clone();
        assert_eq!(h["content-type"], "application/octet-stream");
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert_eq!(h["cache-control"], "no-store");
        assert_eq!(h["content-security-policy"], "sandbox; default-src 'none'");
        let cd = h["content-disposition"].to_str().unwrap().to_owned();
        assert!(cd.starts_with("attachment; filename=\""), "{cd}");
        assert!(
            !cd[..cd.find("filename*").unwrap()].contains("q\"x"),
            "{cd}"
        );
        let body = res.bytes().await.unwrap();
        assert_eq!(
            h["content-length"].to_str().unwrap(),
            body.len().to_string()
        );
    }
}

/// A multi-block file round-trips byte for byte through the pipelined upload and download.
#[tokio::test]
async fn large_file_round_trips() {
    let t = env().await;
    let data: Vec<u8> = (0..(300 * 1024 + 17)).map(|i| (i % 251) as u8).collect();
    let (s, v) = t.put("%2Fworkspace%2Fbig.bin", data.clone(), &[]).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(std::fs::read(t.ws("big.bin")).unwrap(), data);
    let res = t
        .rb(reqwest::Method::GET, "/content?path=%2Fworkspace%2Fbig.bin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.bytes().await.unwrap().to_vec(), data);
}

/// The pipelined upload stays inside the leg's credit window (`TERMINAL_STREAM_WINDOW`) even
/// when credit lags until the reply: 4 × 32 KiB of data plus the SFTP request headers must not
/// exceed 128 KiB, or the runner closes the leg 4502 and the upload dies mid-transfer (found
/// on kind: 1 MiB and 20 MiB uploads answered 422 intermittently).
#[tokio::test]
async fn upload_never_exceeds_the_credit_window() {
    let t = env().await;
    // Replies take a round trip: the whole pipeline is on the wire before the first one lands.
    t.fake.reply_delay_ms.store(25, Ordering::Relaxed);
    for (i, size) in [1usize << 20, (1 << 20) + 5, 160 * 1024, 5 * 32 * 1024]
        .into_iter()
        .enumerate()
    {
        let data: Vec<u8> = (0..size).map(|j| (j % 253) as u8).collect();
        let name = format!("win{i}.bin");
        let (s, v) = t
            .put(&format!("%2Fworkspace%2F{name}"), data.clone(), &[])
            .await;
        assert_eq!(s, 201, "{size} bytes: {v}");
        assert_eq!(std::fs::read(t.ws(&name)).unwrap(), data, "{size} bytes");
    }
    assert_eq!(t.fake.window_violations.load(Ordering::SeqCst), 0);
}

/// Keep-awake: a request stamps `files_active_until` (throttled to once per 30 s) and the
/// reason shows in `keep_awake_reasons`.
#[tokio::test]
async fn requests_stamp_the_files_keep_awake_reason() {
    let t = env().await;
    let stamp = |sb: Uuid| {
        let pool = t.r.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
                "SELECT files_active_until FROM sandboxes WHERE id = $1",
            )
            .bind(sb)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert!(stamp(t.sb).await.is_none());
    assert_eq!(t.get("").await.0, 200);
    let first = stamp(t.sb).await.expect("stamped");
    assert_eq!(t.get("").await.0, 200);
    assert_eq!(
        stamp(t.sb).await,
        Some(first),
        "second request within 30 s: throttled"
    );
    let (s, _, v) =
        t.r.h
            .send(
                Method::GET,
                &format!("/v1/agents/{}/sandbox", t.agent),
                &[("cookie", &t.r.cookie), ("origin", ORIGIN)],
                None,
            )
            .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    assert!(
        v["keep_awake_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "files"),
        "{v}"
    );
}

/// A closed leg (W6 reconnect, park) drops the session; the next request opens a new one with a
/// fresh single-use grant (the pool reopens transparently).
#[tokio::test]
async fn pool_reopens_after_a_leg_close() {
    let t = env().await;
    assert_eq!(t.get("").await.0, 200);
    assert_eq!(t.fake.legs.load(Ordering::SeqCst), 1);
    t.fake.kill.send_modify(|g| *g += 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (s, v) = t.get("").await;
        if s == 200 {
            break;
        }
        assert!(
            matches!((s, code(&v)), (503, "files_unavailable")),
            "{s} {v}"
        );
        assert!(std::time::Instant::now() < deadline, "never reopened");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(t.fake.legs.load(Ordering::SeqCst), 2);
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM file_stream_grants")
        .fetch_one(t.r.h.pool())
        .await
        .unwrap();
    assert_eq!(grants, 2, "one grant per leg");
}

/// Grants: single use, boot bound, `auf_` only as kind `files`, other prefixes never.
#[tokio::test]
async fn grants_are_single_use_boot_bound_and_kind_bound() {
    let t = env().await;
    let (agent, sb) = sandbox(&t.r, "grant-agent", "running").await;
    let boot: Uuid = sqlx::query_scalar("SELECT boot_id FROM sandboxes WHERE id = $1")
        .bind(sb)
        .fetch_one(t.r.h.pool())
        .await
        .unwrap();
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(t.r.h.pool())
        .await
        .unwrap();
    let mint = |boot: Uuid, prefix: token::TokenPrefix| {
        let pool = t.r.h.pool().clone();
        async move {
            let g = token::generate(prefix);
            let hash = hex::encode(token::hash(g.expose()));
            sqlx::query(
                "INSERT INTO file_stream_grants (id, agent_id, sandbox_id, boot_id, granted_for, grant_hash, expires_at) \
                 VALUES ($1,$2,$3,$4,$5,$6, now() + interval '30 seconds')",
            )
            .bind(Uuid::now_v7())
            .bind(agent)
            .bind(sb)
            .bind(boot)
            .bind(user)
            .bind(hash)
            .execute(&pool)
            .await
            .unwrap();
            g.expose().to_owned()
        }
    };
    let redeem = |grant: String, kind: &'static str| {
        let t = &t;
        async move {
            let b = bearer(&t.r.tok);
            let rid = t.r.rep.to_string();
            let (s, _, v) =
                t.r.h
                    .send(
                        Method::POST,
                        "/v1/runner/preview-streams/open",
                        &[("authorization", &b), ("x-aurora-replica", &rid)],
                        Some(json!({"grant": grant, "kind": kind})),
                    )
                    .await;
            (s.as_u16(), v)
        }
    };
    let g = mint(boot, token::TokenPrefix::FilesStream).await;
    let (s, v) = redeem(g.clone(), "files").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["boot_id"], json!(boot.to_string()));
    assert!(
        v.get("port").is_none() && v.get("share_id").is_none() && v.get("shell").is_none(),
        "{v}"
    );
    let (s, v) = redeem(g, "files").await;
    assert_eq!((s, code(&v)), (403, "grant_invalid"), "single use: {v}");
    // stale boot
    let stale = mint(Uuid::now_v7(), token::TokenPrefix::FilesStream).await;
    assert_eq!(redeem(stale, "files").await.0, 409);
    // an auf_ grant redeemed as another kind, and other prefixes redeemed as files
    let g = mint(boot, token::TokenPrefix::FilesStream).await;
    assert_eq!(redeem(g.clone(), "terminal").await.0, 403);
    assert_eq!(redeem(g, "port").await.0, 403);
    for p in [
        token::TokenPrefix::TerminalStream,
        token::TokenPrefix::PreviewStream,
    ] {
        let other = mint(boot, p).await;
        assert_eq!(redeem(other, "files").await.0, 403, "{p:?}");
    }
}

/// Audit rows carry paths and sizes, never contents; a canary written and read back appears in
/// no audit row and no request-log row.
#[tokio::test]
async fn audit_rows_have_paths_and_sizes_but_no_content() {
    let t = env().await;
    let canary = "CANARY-7f3a91c2-do-not-log";
    let (s, _) = t
        .put("%2Fworkspace%2Fc.txt", canary.as_bytes().to_vec(), &[])
        .await;
    assert_eq!(s, 201);
    let res = t
        .rb(reqwest::Method::GET, "/content?path=%2Fworkspace%2Fc.txt")
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), canary);
    assert_eq!(t.get("/text?path=%2Fworkspace%2Fc.txt").await.0, 200);
    assert_eq!(
        t.op(json!({"op": "mkdir", "path": "/workspace/d"})).await.0,
        201
    );
    assert_eq!(
        t.op(json!({"op": "rename", "path": "/workspace/c.txt", "to": "/workspace/d/c.txt"}))
            .await
            .0,
        200
    );
    assert_eq!(
        t.op(json!({"op": "delete", "path": "/workspace/d", "recursive": true}))
            .await
            .0,
        200
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let rows = loop {
        let rows: Vec<(String, Value)> = sqlx::query_as(
            "SELECT action, metadata FROM audit_log WHERE action LIKE 'agent.files.%' ORDER BY created_at",
        )
        .fetch_all(t.r.h.pool())
        .await
        .unwrap();
        if rows.len() >= 6 || std::time::Instant::now() > deadline {
            break rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let actions: Vec<&str> = rows.iter().map(|(a, _)| a.as_str()).collect();
    for want in [
        "agent.files.write",
        "agent.files.download",
        "agent.files.mkdir",
        "agent.files.rename",
        "agent.files.delete",
    ] {
        assert!(actions.contains(&want), "{want} in {actions:?}");
    }
    let write = rows.iter().find(|(a, _)| a == "agent.files.write").unwrap();
    assert_eq!(write.1["path"], "/workspace/c.txt");
    assert_eq!(write.1["bytes"], canary.len());
    assert_eq!(write.1["created"], true);
    let all = serde_json::to_string(&rows).unwrap();
    assert!(!all.contains("CANARY"), "audit rows leaked content");
    let log_hits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM request_log WHERE path LIKE '%CANARY%' OR query LIKE '%CANARY%'",
    )
    .fetch_one(t.r.h.pool())
    .await
    .unwrap_or(0);
    assert_eq!(log_hits, 0);
}

/// `FILE_TRANSFERS_PER_USER` concurrent downloads per person; the next one is 429.
#[tokio::test]
async fn too_many_concurrent_transfers_are_429() {
    let t = env().await;
    std::fs::write(t.ws("big"), vec![1u8; 4 * 1024 * 1024]).unwrap();
    let mut held = vec![];
    for _ in 0..aurora_core::limits::FILE_TRANSFERS_PER_USER {
        let res = t
            .rb(reqwest::Method::GET, "/content?path=%2Fworkspace%2Fbig")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), 200);
        held.push(res); // body unread: the transfer slot stays taken
    }
    let (s, v) = t.get("/content?path=%2Fworkspace%2Fbig").await;
    assert_eq!((s, code(&v)), (429, "too_many_transfers"), "{v}");
    drop(held);
}

/// A runner without the `files` capability: 422 `files_not_supported` reason `runner`.
#[tokio::test]
async fn an_older_runner_is_files_not_supported() {
    let t = env().await;
    sqlx::query("UPDATE runners SET capabilities = capabilities - 'files'")
        .execute(t.r.h.pool())
        .await
        .unwrap();
    let (s, v) = t.get("").await;
    assert_eq!((s, code(&v)), (422, "files_not_supported"), "{v}");
    assert_eq!(v["error"]["details"]["reason"], "runner");
}
