//! (FIX-091, SEC-82, `16 §W18`) The supervisor's preview stream engine against the real pump,
//! driven by a fake runner over hand-built channel ends (like `pty_env_probe`'s rig): the share
//! list is the only allow-list (unlisted/reserved ports and an empty list are refused, nothing
//! non-loopback is dialled, `::1` is the fallback), share removal closes live streams, the
//! credit window and the per-sandbox cap hold, a stalled app never stalls the pump, violations
//! are counted in a 60 s window per connection, and closed ids race legally.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aurora_protocol::runner::{StreamClose, StreamCloseReason, StreamOpen};
use aurora_protocol::supervisor::RunnerToSupervisor;
use aurora_supervisor::channel::{self, StreamIn, StreamOut};
use aurora_supervisor::streams::{Streams, StreamsHandle};
use aurora_supervisor::supervisor::PreviewShareEntry;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use uuid::Uuid;

const T: Duration = Duration::from_secs(10);
const WINDOW: u32 = 131_072;

/// What the fake runner sees of the engine.
enum Seen {
    Frame(Value),
    Data(u32, Vec<u8>),
    Abort,
}

struct Rig {
    handle: Arc<StreamsHandle>,
    tx: mpsc::UnboundedSender<StreamIn>,
    out_rx: mpsc::Receiver<StreamOut>,
    /// Kept alive: a dropped sender would resolve the engine's `epoch.changed()`.
    _epoch_tx: tokio::sync::watch::Sender<u64>,
}

fn rig() -> Rig {
    let (out_tx, out_rx) = mpsc::channel(4096);
    let (tx, rx) = mpsc::unbounded_channel();
    let (epoch_tx, epoch_rx) = tokio::sync::watch::channel(0u64);
    let handle = Streams::spawn(channel::StreamEnds {
        sender: channel::StreamSender::from_mpsc(out_tx),
        inbound: rx,
        epoch: epoch_rx,
    });
    Rig {
        handle,
        tx,
        out_rx,
        _epoch_tx: epoch_tx,
    }
}

fn shares(ports: &[u16]) -> HashMap<Uuid, PreviewShareEntry> {
    ports
        .iter()
        .map(|p| {
            (
                Uuid::from_u128(u128::from(*p)),
                PreviewShareEntry {
                    port: *p,
                    start_command: None,
                    cwd: None,
                },
            )
        })
        .collect()
}

impl Rig {
    /// Installs the share list and waits until the pump applied it (a later frame is ordered
    /// behind it on the pump's commands only via `stream_closed`, so poll the effect instead).
    async fn share(&self, ports: &[u16]) {
        self.handle.set_shares(&shares(ports));
        // `stream_closed` on an id that never existed round-trips through the pump's command
        // queue: when it answers, the `Shares` command before it was applied.
        let _ = self.handle.stream_closed(u32::MAX).await;
    }

    fn send(&self, msg: RunnerToSupervisor) {
        let _ = self.tx.send(StreamIn::Frame(msg));
    }

    fn open(&self, id: u32, port: u16) {
        self.send(RunnerToSupervisor::StreamOpen(StreamOpen {
            stream_id: id,
            port,
            window: WINDOW,
        }));
    }

    fn data(&self, id: u32, payload: &[u8]) {
        let _ = self.tx.send(StreamIn::Data {
            stream_id: id,
            payload: payload.to_vec(),
        });
    }

    /// The next thing the engine sent (flushes skipped); `None` after `d` of silence.
    async fn next_within(&mut self, d: Duration) -> Option<Seen> {
        let deadline = tokio::time::Instant::now() + d;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let out = tokio::time::timeout(left, self.out_rx.recv())
                .await
                .ok()??;
            match out {
                StreamOut::Text(t) => return Some(Seen::Frame(serde_json::from_str(&t).unwrap())),
                StreamOut::Binary(b) => {
                    assert_eq!(b[0], 0x01, "preview data is tag 0x01");
                    let id = u32::from_be_bytes([b[1], b[2], b[3], b[4]]);
                    return Some(Seen::Data(id, b[5..].to_vec()));
                }
                StreamOut::Flush => {}
                StreamOut::Abort => return Some(Seen::Abort),
            }
        }
    }

    /// The next `stream.*` frame of kind `kind` for `id`, skipping credit and data.
    async fn frame(&mut self, kind: &str, id: u32) -> Value {
        let deadline = tokio::time::Instant::now() + T;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next_within(left).await {
                Some(Seen::Frame(f)) => {
                    if f["body"]["stream_id"] == id && f["kind"] == kind {
                        return f;
                    }
                }
                Some(Seen::Data(..)) => {}
                Some(Seen::Abort) => panic!("engine aborted the channel (waiting for {kind})"),
                None => panic!("no {kind} for stream {id} within {T:?}"),
            }
        }
    }

    /// `stream.opened` or `stream.refused` for `id`: the engine's answer to an open.
    async fn answer(&mut self, id: u32) -> Value {
        let deadline = tokio::time::Instant::now() + T;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next_within(left).await {
                Some(Seen::Frame(f)) => {
                    if f["body"]["stream_id"] == id
                        && (f["kind"] == "stream.opened" || f["kind"] == "stream.refused")
                    {
                        return f;
                    }
                }
                Some(Seen::Data(..)) => {}
                Some(Seen::Abort) => panic!("engine aborted the channel"),
                None => panic!("no answer to the open of stream {id} within {T:?}"),
            }
        }
    }

    /// Asserts the engine says nothing (and does not abort) for `d`.
    async fn silent_for(&mut self, d: Duration) {
        match self.next_within(d).await {
            None => {}
            Some(Seen::Abort) => panic!("engine aborted the channel"),
            Some(Seen::Frame(f)) => panic!("unexpected frame {f}"),
            Some(Seen::Data(id, _)) => panic!("unexpected data on {id}"),
        }
    }
}

/// A loopback listener that accepts and holds connections without reading them (a stalled
/// app), returning its port; the accepted sockets stay open for the test's lifetime.
async fn stalled_app() -> (u16, tokio::task::JoinHandle<()>) {
    // A tiny receive buffer so the kernel absorbs little before the writer stalls.
    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.set_recv_buffer_size(4096).unwrap();
    sock.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let l = sock.listen(256).unwrap();
    let port = l.local_addr().unwrap().port();
    let h = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = l.accept().await {
            held.push(s);
        }
    });
    (port, h)
}

/// A loopback echo app.
async fn echo_app() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

/// SEC-82: a port not in the share list is refused `not_shared`, and the app is never dialled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unlisted_port_is_refused_not_shared_and_never_dialled() {
    let app = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = app.local_addr().unwrap().port();
    let other = port
        .checked_add(1)
        .filter(|p| *p >= 1024)
        .unwrap_or(port - 1);
    let mut r = rig();
    r.share(&[other]).await;
    r.open(1, port);
    let a = r.answer(1).await;
    assert_eq!(a["kind"], "stream.refused", "{a}");
    assert_eq!(a["body"]["reason"], "not_shared", "{a}");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), app.accept())
            .await
            .is_err(),
        "the engine dialled an unlisted port"
    );
}

/// An empty share list refuses everything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_share_list_refuses_everything() {
    let port = echo_app().await;
    let mut r = rig();
    r.share(&[]).await;
    r.open(1, port);
    let a = r.answer(1).await;
    assert_eq!(a["kind"], "stream.refused", "{a}");
    assert_eq!(a["body"]["reason"], "not_shared", "{a}");
}

/// The supervisor's own listeners (and privileged ports) can never be shared: even a share
/// row naming one is dropped from the list, and the port is never dialled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserved_ports_are_never_dialled() {
    let mut r = rig();
    r.share(&[3128, 3129, 3130, 80]).await;
    // If the sandbox lets this test bind a reserved port, prove nothing connects to it.
    let probe = TcpListener::bind("127.0.0.1:3129").await.ok();
    for (i, port) in [3128u16, 3129, 3130, 80].into_iter().enumerate() {
        let id = i as u32 + 1;
        r.open(id, port);
        let a = r.answer(id).await;
        assert_eq!(a["kind"], "stream.refused", "port {port}: {a}");
        assert_eq!(a["body"]["reason"], "not_shared", "port {port}: {a}");
    }
    if let Some(l) = probe {
        assert!(
            tokio::time::timeout(Duration::from_millis(200), l.accept())
                .await
                .is_err(),
            "a reserved port was dialled"
        );
    }
}

/// Removing a share from the list closes its live streams (`stream.close reset`) and the
/// port is refused afterwards (a removed share keeps streaming — SEC-82 — if the pump stalls).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn share_removal_closes_live_streams() {
    let port = echo_app().await;
    let mut r = rig();
    r.share(&[port]).await;
    r.open(1, port);
    assert_eq!(r.answer(1).await["kind"], "stream.opened");
    r.share(&[]).await;
    let c = r.frame("stream.close", 1).await;
    assert_eq!(
        c["body"]["reason"],
        StreamCloseReason::Reset.as_str(),
        "{c}"
    );
    r.open(2, port);
    assert_eq!(r.answer(2).await["kind"], "stream.refused");
}

/// Data beyond the credit window closes the stream with `error` (a protocol violation).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_beyond_the_window_closes_the_stream_with_error() {
    let (port, _app) = stalled_app().await;
    let mut r = rig();
    r.share(&[port]).await;
    r.open(1, port);
    assert_eq!(r.answer(1).await["kind"], "stream.opened");
    r.data(1, &vec![7u8; WINDOW as usize + 1]);
    let c = r.frame("stream.close", 1).await;
    assert_eq!(c["body"]["reason"], "error", "{c}");
}

/// The 65th concurrent stream of a sandbox is refused `limit`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_65th_open_is_refused_limit() {
    let (port, _app) = stalled_app().await;
    let mut r = rig();
    r.share(&[port]).await;
    for id in 1..=64 {
        r.open(id, port);
        assert_eq!(r.answer(id).await["kind"], "stream.opened", "stream {id}");
    }
    r.open(65, port);
    let a = r.answer(65).await;
    assert_eq!(a["kind"], "stream.refused", "{a}");
    assert_eq!(a["body"]["reason"], "limit", "{a}");
}

/// `127.0.0.1` first, then `[::1]`: an app listening on `::1` only is reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ipv6_loopback_is_the_fallback() {
    let Ok(l) = TcpListener::bind("[::1]:0").await else {
        eprintln!("no IPv6 loopback on this host; skipping");
        return;
    };
    let port = l.local_addr().unwrap().port();
    let accepted = tokio::spawn(async move { l.accept().await.is_ok() });
    let mut r = rig();
    r.share(&[port]).await;
    r.open(1, port);
    assert_eq!(r.answer(1).await["kind"], "stream.opened");
    assert!(accepted.await.unwrap(), "the ::1 listener was not reached");
}

/// An app listening on a non-loopback address of this host is never reached: the engine dials
/// loopback only, so the open is refused `refused`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_listener_on_a_non_loopback_address_is_not_reached() {
    // This host's outward address (no packet is sent: connecting a UDP socket only routes).
    let local = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|()| s))
        .and_then(|s| s.local_addr());
    let Ok(local) = local else {
        eprintln!("no non-loopback address on this host; skipping");
        return;
    };
    if local.ip().is_loopback() {
        return;
    }
    let l = TcpListener::bind((local.ip(), 0)).await.unwrap();
    let port = l.local_addr().unwrap().port();
    let mut r = rig();
    r.share(&[port]).await;
    r.open(1, port);
    let a = r.answer(1).await;
    assert_eq!(a["kind"], "stream.refused", "{a}");
    assert_eq!(a["body"]["reason"], "refused", "{a}");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), l.accept())
            .await
            .is_err(),
        "a non-loopback listener was reached"
    );
}

/// FIX-091 item 1: an app that stopped reading stalls only its own stream. The fake runner
/// pushes small frames at it until the kernel stops absorbing them (credit stops coming back),
/// then proves the pump is alive: another stream opens and echoes, and removing the stalled
/// stream's share closes it at once (before the fix the pump awaited the app's queue, so open,
/// echo and the share removal all hung).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_app_stalls_no_other_stream() {
    let (stalled_port, _app) = stalled_app().await;
    let echo_port = echo_app().await;
    let mut r = rig();
    r.share(&[stalled_port, echo_port]).await;
    r.open(1, stalled_port);
    assert_eq!(r.answer(1).await["kind"], "stream.opened");

    // Small frames until the app's socket is full and credit stops.
    const FRAME: usize = 100;
    let mut unacked = 0usize;
    let mut sent = 0usize;
    loop {
        while unacked + FRAME <= WINDOW as usize {
            r.data(1, &[1u8; FRAME]);
            unacked += FRAME;
            sent += 1;
        }
        // Drain the credit the engine returned; a quiet second means the app stopped reading.
        let mut got = false;
        while let Some(s) = r.next_within(Duration::from_secs(1)).await {
            if let Seen::Frame(f) = s {
                assert_ne!(
                    f["kind"], "stream.close",
                    "the stalled stream was closed: {f}"
                );
                if f["kind"] == "stream.credit" {
                    unacked -= f["body"]["bytes"].as_u64().unwrap() as usize;
                    got = true;
                    break;
                }
            }
        }
        if !got {
            break;
        }
        assert!(sent < 1_000_000, "the app never stalled");
    }
    assert!(
        sent > 17,
        "fewer frames than the old queue depth were sent ({sent})"
    );

    // The pump still serves everything else.
    r.open(2, echo_port);
    assert_eq!(r.answer(2).await["kind"], "stream.opened");
    r.data(2, b"ping");
    let deadline = tokio::time::Instant::now() + T;
    let mut echoed = Vec::new();
    while echoed.len() < 4 {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match r.next_within(left).await {
            Some(Seen::Data(2, d)) => echoed.extend(d),
            Some(_) => {}
            None => panic!("no echo on the healthy stream while another app is stalled"),
        }
    }
    assert_eq!(echoed, b"ping");
    r.share(&[echo_port]).await;
    let c = r.frame("stream.close", 1).await;
    assert_eq!(c["body"]["reason"], "reset", "{c}");
}

/// Data for an id that was closed on this connection is a legal close race: dropped silently,
/// never a violation (`16 §W18`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_after_close_is_dropped_not_a_violation() {
    let port = echo_app().await;
    let mut r = rig();
    r.share(&[port]).await;
    r.open(1, port);
    assert_eq!(r.answer(1).await["kind"], "stream.opened");
    r.send(RunnerToSupervisor::StreamClose(StreamClose {
        stream_id: 1,
        reason: StreamCloseReason::Eof,
        bytes_in: None,
        bytes_out: None,
    }));
    for _ in 0..10 {
        r.data(1, b"late");
    }
    // Still healthy: no abort, and a fresh stream opens.
    r.open(2, port);
    assert_eq!(r.answer(2).await["kind"], "stream.opened");
    r.silent_for(Duration::from_millis(100)).await;
}

/// Four violations within 60 s close the channel (4400); the count decays (a paused clock
/// moves past the window) and resets with the connection.
#[tokio::test(start_paused = true)]
async fn four_violations_within_a_minute_abort_the_channel_and_the_count_decays() {
    let mut r = rig();
    // Unknown ids (never opened): violations.
    for id in 10..13 {
        r.data(id, b"x");
    }
    r.silent_for(Duration::from_secs(1)).await;
    // The window passes: the three old violations no longer count.
    tokio::time::advance(Duration::from_secs(61)).await;
    for id in 20..23 {
        r.data(id, b"x");
    }
    r.silent_for(Duration::from_secs(1)).await;
    // A connection change resets the count.
    let _ = r.tx.send(StreamIn::Reset);
    for id in 30..33 {
        r.data(id, b"x");
    }
    r.silent_for(Duration::from_secs(1)).await;
    // The fourth within the window aborts.
    r.data(33, b"x");
    match r.next_within(Duration::from_secs(5)).await {
        Some(Seen::Abort) => {}
        Some(Seen::Frame(f)) => panic!("unexpected frame {f}"),
        Some(Seen::Data(..)) => panic!("unexpected data"),
        None => panic!("four violations in a minute did not abort the channel"),
    }
}
