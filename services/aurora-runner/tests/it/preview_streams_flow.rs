//! (FIX-060) Flow control and isolation of the runner's stream plane, on real WebSockets
//! on both legs (the browser/relay client and the fake supervisor):
//! - sandbox → browser data has its **own** window: 1 MiB of terminal or preview output,
//!   credited as the runner writes it onward, arrives whole and the leg stays open;
//! - a browser that stops reading closes only its own leg: another sandbox on the same runner
//!   keeps streaming both ways, and opens and releases keep working, meanwhile;
//! - `*.close`/`*.refused`/`*.credit` naming another sandbox's leg id are ignored.
//!
//! WebSocket consumers answer Pings (tungstenite does on read) and bound every wait
//! (FIX-051/FIX-055 playbooks).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use aurora_runner::api::{ApiClient, RetryPolicy};
use aurora_runner::channel::{ChannelServer, ChannelSettings};
use aurora_runner::hub::ChannelHub as _;
use aurora_runner::stream_listener::StreamListener;
use futures::stream::{SplitSink, SplitStream};
use futures::{FutureExt, SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{Message, handshake::client::Request as WsRequest};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, client_async, connect_async};
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

const KEY: [u8; 32] = [31; 32];
const WINDOW: usize = 131_072;
const CHUNK: usize = 16 * 1024;
const MIB: usize = 1 << 20;
const STEP: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Term,
    Port,
}

impl Kind {
    fn tag(self) -> u8 {
        match self {
            Kind::Term => 0x02,
            Kind::Port => 0x01,
        }
    }
    fn open(self) -> &'static str {
        match self {
            Kind::Term => "pty.open",
            Kind::Port => "stream.open",
        }
    }
    fn opened(self) -> &'static str {
        match self {
            Kind::Term => "pty.opened",
            Kind::Port => "stream.opened",
        }
    }
    fn credit(self) -> &'static str {
        match self {
            Kind::Term => "pty.credit",
            Kind::Port => "stream.credit",
        }
    }
    fn id_key(self) -> &'static str {
        match self {
            Kind::Term => "pty_id",
            Kind::Port => "stream_id",
        }
    }
    fn grant(self, who: &str) -> String {
        match self {
            Kind::Term => format!("auy_{who}"),
            Kind::Port => format!("aux_{who}"),
        }
    }
}

/// One sandbox the API mock knows: its identity and the bootstrap token that claims it.
struct Sandbox {
    id: Uuid,
    boot: Uuid,
    token: &'static str,
    who: &'static str,
}

struct Env {
    st: std::net::SocketAddr,
    ch: std::net::SocketAddr,
    _api: MockServer,
    _server: ChannelServer,
    _stops: (watch::Sender<bool>, watch::Sender<bool>),
}

fn sandbox(token: &'static str, who: &'static str) -> Sandbox {
    Sandbox {
        id: Uuid::new_v4(),
        boot: Uuid::new_v4(),
        token,
        who,
    }
}

/// The runner half plus an API mock that knows every given sandbox (claim by token, grant
/// redemption by grant name: `auy_<who>` terminal, `aux_<who>` port 3000).
async fn env(boxes: &[&Sandbox]) -> Env {
    let api = MockServer::start().await;
    for s in boxes {
        Mock::given(method("POST"))
            .and(path("/v1/runner/channel/claim"))
            .and(body_partial_json(json!({"bootstrap_token": s.token})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"sandbox_id": s.id, "boot_id": s.boot})),
            )
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/runner/preview-streams/open"))
            .and(body_partial_json(json!({"grant": Kind::Term.grant(s.who)})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sandbox_id": s.id, "boot_id": s.boot, "window": WINDOW,
                "idle_secs": 600, "max_lifetime_secs": 14400
            })))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/runner/preview-streams/open"))
            .and(body_partial_json(json!({"grant": Kind::Port.grant(s.who)})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sandbox_id": s.id, "boot_id": s.boot, "port": 3000,
                "share_id": Uuid::now_v7(), "window": WINDOW,
                "idle_secs": 600, "max_lifetime_secs": 14400
            })))
            .mount(&api)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/v1/runner/channel/hold"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/runner/terminal/keep-awake"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&api)
        .await;

    let server = ChannelServer::new(ChannelSettings::new(KEY));
    let retry = RetryPolicy {
        attempts: 1,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(10),
    };
    server.attach_api(Arc::new(
        ApiClient::new(&api.uri(), "aur_test", retry).unwrap(),
    ));
    let streams = StreamListener::new(server.clone());
    server.set_streams(streams.clone());
    let ch = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ch_addr = ch.local_addr().unwrap();
    let (stop1, stop_rx1) = watch::channel(false);
    let s = server.clone();
    tokio::spawn(async move { s.serve(ch, stop_rx1).await });
    let st = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let st_addr = st.local_addr().unwrap();
    let (stop2, stop_rx2) = watch::channel(false);
    let api_w = server.api_watch();
    tokio::spawn(async move {
        aurora_runner::stream_listener::serve(st, streams, api_w, None, stop_rx2).await
    });
    server.set_previews_enabled(true);
    Env {
        st: st_addr,
        ch: ch_addr,
        _api: api,
        _server: server,
        _stops: (stop1, stop2),
    }
}

/// The fake supervisor (W6 client), split so reads and writes can run side by side.
struct Sup {
    tx: SplitSink<Client, Message>,
    rx: SplitStream<Client>,
}

impl Sup {
    async fn connect(env: &Env, s: &Sandbox) -> Self {
        let (mut ws, _) = connect_async(format!("ws://{}/v1/supervisor", env.ch))
            .await
            .unwrap();
        ws.send(Message::text(
            json!({"hello": {"protocol": 1, "bootstrap_token": s.token}}).to_string(),
        ))
        .await
        .unwrap();
        let (tx, rx) = ws.split();
        let mut sup = Self { tx, rx };
        let _welcome = sup.next_text().await;
        sup
    }

    /// Next text frame (pings, acks and binary skipped); a close is returned as `{"close":…}`.
    async fn next_text(&mut self) -> Value {
        loop {
            match tokio::time::timeout(STEP, self.rx.next())
                .await
                .expect("supervisor wait timed out")
            {
                Some(Ok(Message::Text(t))) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["kind"] != "ack" {
                        return v;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return json!({"close": 1}),
                Some(Ok(_)) => {}
            }
        }
    }

    /// Next binary data message (control frames and text skipped).
    async fn next_binary(&mut self) -> Vec<u8> {
        loop {
            match tokio::time::timeout(STEP, self.rx.next())
                .await
                .expect("supervisor data wait timed out")
            {
                Some(Ok(Message::Binary(b))) => return b.to_vec(),
                Some(Ok(_)) => {}
                other => panic!("supervisor channel ended awaiting data: {other:?}"),
            }
        }
    }

    async fn send_frame(&mut self, kind: &str, body: Value) {
        let m = json!({"seq": 0, "ack": 0, "kind": kind, "body": body}).to_string();
        self.tx.send(Message::text(m)).await.unwrap();
    }

    async fn send_data(&mut self, kind: Kind, id: u32, payload: &[u8]) {
        let mut b = vec![kind.tag()];
        b.extend_from_slice(&id.to_be_bytes());
        b.extend_from_slice(payload);
        tokio::time::timeout(STEP, self.tx.send(Message::binary(b)))
            .await
            .expect("supervisor send stalled")
            .unwrap();
    }
}

/// One open leg: the runner's id for it and the browser/relay-side client.
struct Leg {
    id: u32,
    ws: Client,
}

/// Dials the stream listener, answers the supervisor's open, returns the upgraded leg.
/// `small_rcvbuf` shrinks the client's receive buffer so a client that stops reading stalls
/// the runner's writes after a few hundred KiB instead of megabytes.
async fn open_leg(env: &Env, sup: &mut Sup, kind: Kind, who: &str, small_rcvbuf: bool) -> Leg {
    let st = env.st;
    let grant = kind.grant(who);
    let dial = tokio::spawn(async move {
        let mut req: WsRequest = format!("ws://{st}/v1/preview-stream")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert(AUTHORIZATION, format!("Bearer {grant}").parse().unwrap());
        let fut = async {
            if small_rcvbuf {
                let sock = TcpSocket::new_v4().unwrap();
                sock.set_recv_buffer_size(2048).unwrap();
                let tcp = sock.connect(st).await.unwrap();
                client_async(req, MaybeTlsStream::Plain(tcp))
                    .await
                    .unwrap()
                    .0
            } else {
                connect_async(req).await.unwrap().0
            }
        };
        tokio::time::timeout(STEP, fut)
            .await
            .expect("dial timed out")
    });
    let open = sup.next_text().await;
    assert_eq!(open["kind"], kind.open(), "{open}");
    let id = open["body"][kind.id_key()].as_u64().unwrap() as u32;
    sup.send_frame(kind.opened(), json!({kind.id_key(): id, "window": WINDOW}))
        .await;
    Leg {
        id,
        ws: dial.await.unwrap(),
    }
}

/// Next binary message from the browser side (Pings/Pongs skipped, bounded).
async fn browser_binary(ws: &mut Client) -> Vec<u8> {
    loop {
        match tokio::time::timeout(STEP, ws.next())
            .await
            .expect("browser wait timed out")
        {
            Some(Ok(Message::Binary(b))) => return b.to_vec(),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            other => panic!("browser leg ended awaiting data: {other:?}"),
        }
    }
}

/// Both directions work on a leg: browser → supervisor and supervisor → browser.
async fn assert_round_trip(sup: &mut Sup, leg: &mut Leg, kind: Kind) {
    leg.ws
        .send(Message::binary(b"to-sandbox".to_vec()))
        .await
        .unwrap();
    let b = sup.next_binary().await;
    assert_eq!(b[0], kind.tag());
    assert_eq!(&b[1..5], &leg.id.to_be_bytes());
    assert_eq!(&b[5..], b"to-sandbox");
    sup.send_data(kind, leg.id, b"to-browser").await;
    assert_eq!(browser_binary(&mut leg.ws).await, b"to-browser");
}

/// The supervisor sends 1 MiB toward the browser in window-bounded pieces, honouring the
/// credit the runner returns; the browser receives all of it and the leg stays open.
/// Before FIX-060 the sandbox → browser data drew on the browser → sandbox window, so the leg
/// died `4502` after the first 128 KiB (and the runner never returned any credit).
async fn one_mib_toward_the_browser(kind: Kind) {
    let a = sandbox("aup_a", "a");
    let env = env(&[&a]).await;
    let mut sup = Sup::connect(&env, &a).await;
    let Leg { id, ws } = open_leg(&env, &mut sup, kind, "a", false).await;

    let reader = tokio::spawn(async move {
        let mut ws = ws;
        let mut got = 0usize;
        while got < MIB {
            got += browser_binary(&mut ws).await.len();
        }
        (ws, got)
    });

    let (mut sent, mut credit) = (0usize, WINDOW);
    let chunk = vec![b'q'; CHUNK];
    while sent < MIB {
        if credit < CHUNK {
            let f = sup.next_text().await;
            assert_eq!(f["kind"], kind.credit(), "only credit may arrive: {f}");
            credit += f["body"]["bytes"].as_u64().unwrap() as usize;
        } else {
            sup.send_data(kind, id, &chunk).await;
            credit -= CHUNK;
            sent += CHUNK;
        }
    }
    let (ws, got) = reader.await.unwrap();
    assert_eq!(got, MIB);

    // Still open: both directions work after more than one window.
    let mut leg = Leg { id, ws };
    assert_round_trip(&mut sup, &mut leg, kind).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_output_beyond_one_window_flows_and_the_leg_stays_open() {
    one_mib_toward_the_browser(Kind::Term).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preview_body_beyond_one_window_flows_and_the_leg_stays_open() {
    one_mib_toward_the_browser(Kind::Port).await;
}

/// Leg A's browser stops reading while its supervisor keeps sending (ignoring credit): that
/// leg is closed (`error` to its supervisor — over-window data), and sandbox B on the same
/// runner keeps working throughout — a fresh terminal opens, streams both ways and releases,
/// over and over, within a deadline. Before FIX-060 the pump awaited A's full queue holding
/// the runner-wide routes lock, so B's every open, release and data frame stalled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_browser_closes_only_its_own_leg() {
    let (a, b) = (sandbox("aup_a", "a"), sandbox("aup_b", "b"));
    let env = env(&[&a, &b]).await;
    let mut sup_a = Sup::connect(&env, &a).await;
    let mut sup_b = Sup::connect(&env, &b).await;
    // Held, never read: this browser is the stalled one.
    let stalled = open_leg(&env, &mut sup_a, Kind::Term, "a", true).await;
    let a_id = stalled.id;

    let flood = tokio::spawn(async move {
        let chunk = vec![b'z'; CHUNK];
        for _ in 0..8192 {
            sup_a.send_data(Kind::Term, a_id, &chunk).await;
            // Control frames the runner sent meanwhile (non-blocking).
            while let Some(Some(Ok(Message::Text(t)))) = sup_a.rx.next().now_or_never() {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["kind"] == "pty.close" {
                    return v;
                }
            }
        }
        // Everything sent: the runner must close the leg now.
        loop {
            let v = sup_a.next_text().await;
            if v["kind"] == "pty.close" {
                return v;
            }
        }
    });

    for _ in 0..8 {
        let mut leg = open_leg(&env, &mut sup_b, Kind::Term, "b", false).await;
        assert_round_trip(&mut sup_b, &mut leg, Kind::Term).await;
        leg.ws.close(None).await.unwrap();
        let closed = sup_b.next_text().await;
        assert_eq!(closed["kind"], "pty.close", "{closed}");
        assert_eq!(closed["body"]["pty_id"], u64::from(leg.id));
    }

    let closed = tokio::time::timeout(Duration::from_secs(60), flood)
        .await
        .expect("leg A was never closed")
        .unwrap();
    assert_eq!(closed["body"]["pty_id"], u64::from(a_id), "{closed}");
    assert_eq!(closed["body"]["reason"], "error", "{closed}");
    drop(stalled);
}

/// A supervisor can close, refuse or credit only its own legs: frames naming another
/// sandbox's leg id (ids are sequential and guessable) are ignored and the leg stays open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frames_naming_another_sandboxes_leg_are_ignored() {
    let (a, b) = (sandbox("aup_a", "a"), sandbox("aup_b", "b"));
    let env = env(&[&a, &b]).await;
    let mut sup_a = Sup::connect(&env, &a).await;
    let mut sup_b = Sup::connect(&env, &b).await;
    let mut term = open_leg(&env, &mut sup_a, Kind::Term, "a", false).await;
    let mut port = open_leg(&env, &mut sup_a, Kind::Port, "a", false).await;

    // Sandbox B's supervisor names A's legs.
    for kind in [Kind::Term, Kind::Port] {
        let id = if kind == Kind::Term { term.id } else { port.id };
        let key = kind.id_key();
        let prefix = if kind == Kind::Term { "pty" } else { "stream" };
        sup_b
            .send_frame(
                &format!("{prefix}.close"),
                json!({key: id, "reason": "eof"}),
            )
            .await;
        sup_b
            .send_frame(
                &format!("{prefix}.refused"),
                json!({key: id, "reason": "limit"}),
            )
            .await;
        sup_b
            .send_frame(&format!("{prefix}.credit"), json!({key: id, "bytes": 1}))
            .await;
        sup_b
            .send_frame(kind.opened(), json!({key: id, "window": 1}))
            .await;
        sup_b.send_data(kind, id, b"forged").await;
    }
    // B's pump handles frames in order: once a B leg round-trips, all of the above ran.
    let mut own = open_leg(&env, &mut sup_b, Kind::Term, "b", false).await;
    assert_round_trip(&mut sup_b, &mut own, Kind::Term).await;

    // A's legs are untouched: no close, no forged bytes, both directions still work.
    assert_round_trip(&mut sup_a, &mut term, Kind::Term).await;
    assert_round_trip(&mut sup_a, &mut port, Kind::Port).await;
}
