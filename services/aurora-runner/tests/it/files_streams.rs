//! (P5-FILES-1, `16 §W21` leg 1 and `§W20` leg 2) The runner's stream listener serves
//! `kind: "files"` grants (`Bearer auf_…`): the redemption carries `kind: "files"`; the API's
//! **first** message must be exactly the bare `sftp` `exec.open` within 5 s (anything else →
//! 4502 and nothing reaches the supervisor); the runner then allocates the `pty_id`, sends leg
//! 2's `exec.open`, and answers `{"kind":"exec.opened"}` or closes 4502 / 4409; afterwards
//! `[0x02]`/`[0x03]` binary messages and `exec.eof`/`exec.exit` relay both ways; the per-sandbox
//! cap is 4 (4409); an older supervisor that ignores `exec.open` → 4502; a channel loss → 4503;
//! and `auf_` legs never stamp the keep-awake while `auy_` legs never open a files exec.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use futures::SinkExt;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;
use wiremock::MockServer;

use crate::stream_support::{close_code_slow, data, dial, frame, next_leg, next_msg, silent};
use crate::terminal_streams::{
    Client, WINDOW, close_code, connect_supervisor, mount_runner_api, next_text, rig,
};

const OPEN: &str = r#"{"kind":"exec.open","mode":"sftp"}"#;

/// Dials, sends the bare open and answers the supervisor's `exec.open` with `exec.opened`;
/// returns the API's leg and the allocated id.
async fn open_leg(
    sup: &mut Client,
    st: std::net::SocketAddr,
) -> (WebSocketStream<MaybeTlsStream<TcpStream>>, u32) {
    let mut c = dial(st, "auf_testgrant").await.unwrap();
    c.send(Message::text(OPEN)).await.unwrap();
    let Message::Text(t) = next_msg(sup).await else {
        panic!("expected exec.open");
    };
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(v["kind"], "exec.open", "{v}");
    let id = v["body"]["pty_id"].as_u64().unwrap() as u32;
    sup.send(frame(
        "exec.opened",
        json!({"pty_id": id, "window": WINDOW}),
    ))
    .await
    .unwrap();
    let Message::Text(t) = next_leg(&mut c).await else {
        panic!("expected exec.opened");
    };
    assert_eq!(t.as_str(), r#"{"kind":"exec.opened"}"#);
    (c, id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_leg_round_trip_and_never_stamps_keep_awake() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;

    let mut c = dial(st, "auf_testgrant").await.unwrap();
    c.send(Message::text(OPEN)).await.unwrap();
    // Leg 2: exactly an `sftp` `exec.open` with the terminal window, no command, no tty, no env.
    let Message::Text(t) = next_msg(&mut sup).await else {
        panic!("expected a text frame");
    };
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(v["kind"], "exec.open", "a files leg never opens a PTY: {v}");
    assert_eq!(v["body"]["mode"], "sftp");
    assert_eq!(v["body"]["window"], WINDOW);
    assert!(v["body"].get("command").is_none() && v["body"].get("tty").is_none());
    assert_eq!(v["body"]["env"], json!({}));
    let id = v["body"]["pty_id"].as_u64().unwrap() as u32;
    sup.send(frame(
        "exec.opened",
        json!({"pty_id": id, "window": WINDOW}),
    ))
    .await
    .unwrap();
    assert_eq!(
        next_leg(&mut c).await,
        Message::text(r#"{"kind":"exec.opened"}"#)
    );

    // API → sandbox: `[0x02][payload]` arrives framed `[0x02][pty_id BE][payload]`.
    let mut m = vec![0x02];
    m.extend_from_slice(b"hello");
    c.send(Message::binary(m)).await.unwrap();
    assert_eq!(next_msg(&mut sup).await, data(0x02, id, b"hello"));

    // sandbox → API: stdout `[0x02]…` and stderr `[0x03]…`.
    sup.send(data(0x02, id, b"out")).await.unwrap();
    sup.send(data(0x03, id, b"err")).await.unwrap();
    let mut got = Vec::new();
    while got.len() < 2 {
        if let Message::Binary(b) = next_leg(&mut c).await {
            got.push(b.to_vec());
        }
    }
    assert_eq!(got[0], [&[0x02][..], b"out"].concat());
    assert_eq!(got[1], [&[0x03][..], b"err"].concat());

    // `exec.eof` API → runner → supervisor.
    c.send(Message::text(r#"{"kind":"exec.eof"}"#))
        .await
        .unwrap();
    let Message::Text(t) = next_msg(&mut sup).await else {
        panic!("expected exec.eof");
    };
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(v["kind"], "exec.eof");
    assert_eq!(v["body"]["pty_id"], u64::from(id));

    // `exec.exit` then `pty.close { eof }`: the API sees the exit text, then a normal 1000.
    sup.send(frame("exec.exit", json!({"pty_id": id, "status": 3})))
        .await
        .unwrap();
    sup.send(frame("pty.close", json!({"pty_id": id, "reason": "eof"})))
        .await
        .unwrap();
    let Message::Text(t) = next_leg(&mut c).await else {
        panic!("expected exec.exit");
    };
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(v, json!({"kind": "exec.exit", "status": 3}));
    assert_eq!(close_code(&mut c).await, 1000);

    // The redemption named the kind; no keep-awake stamp ever left the runner.
    let reqs = api.received_requests().await.unwrap();
    let open = reqs
        .iter()
        .find(|r| r.url.path().ends_with("/preview-streams/open"))
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&open.body).unwrap()["kind"],
        "files"
    );
    assert!(
        !reqs
            .iter()
            .any(|r| r.url.path().ends_with("/terminal/keep-awake")),
        "auf_ legs must not stamp keep-awake"
    );
}

/// Anything but the bare `sftp` open closes the leg 4502 and never reaches the supervisor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anything_but_the_bare_sftp_open_is_4502_and_stays_off_the_supervisor() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    for first in [
        Message::text(r#"{"kind":"exec.open","mode":"shell"}"#),
        Message::text(r#"{"kind":"exec.open","mode":"exec","command":"ls"}"#),
        Message::text(r#"{"kind":"exec.open","mode":"sftp","command":"ls"}"#),
        Message::text(r#"{"kind":"exec.open","mode":"sftp","tty":{"term":"x","cols":1,"rows":1}}"#),
        Message::text(r#"{"kind":"exec.open","mode":"sftp","env":{"LANG":"C"}}"#),
        Message::text(r#"{"kind":"exec.open","mode":"sftp","extra":1}"#),
        Message::text(r#"{"kind":"pty.resize","cols":1,"rows":1}"#),
        Message::text("not json"),
        Message::binary(vec![0x02, 1, 2]),
    ] {
        let mut c = dial(st, "auf_testgrant").await.unwrap();
        c.send(first.clone()).await.unwrap();
        assert_eq!(close_code(&mut c).await, 4502, "{first:?}");
    }
    assert!(
        silent(&mut sup, Duration::from_millis(300)).await,
        "a refused first frame reached the supervisor"
    );
}

/// After the open only `[0x02]` binary and `exec.eof` text are legal; anything else is 4502.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_outside_the_allow_list_close_4502() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    for bad in [
        Message::binary(vec![0x03, 1]),
        Message::binary(vec![0x01, 1]),
        Message::binary(Vec::<u8>::new()),
        Message::text(r#"{"kind":"pty.resize","cols":1,"rows":1}"#),
        Message::text(r#"{"kind":"exec.eof","x":1}"#),
        Message::text(format!(
            r#"{{"kind":"exec.eof","pad":"{}"}}"#,
            "x".repeat(300)
        )),
    ] {
        let (mut c, _id) = open_leg(&mut sup, st).await;
        c.send(bad.clone()).await.unwrap();
        assert_eq!(close_code(&mut c).await, 4502, "{bad:?}");
        // The runner tells the supervisor the exec ended (`pty.close`); nothing else.
        let Message::Text(t) = next_msg(&mut sup).await else {
            panic!("expected pty.close");
        };
        assert!(t.contains(r#""kind":"pty.close""#), "{t}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fifth_leg_of_a_sandbox_is_4409() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    let mut legs = Vec::new();
    for _ in 0..aurora_core::limits::FILE_SESSIONS_PER_SANDBOX {
        legs.push(open_leg(&mut sup, st).await);
    }
    let mut c = dial(st, "auf_testgrant").await.unwrap();
    c.send(Message::text(OPEN)).await.unwrap();
    assert_eq!(close_code(&mut c).await, 4409);
    assert!(
        silent(&mut sup, Duration::from_millis(300)).await,
        "the capped open reached the supervisor"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_refusals_map_to_4409_and_4502() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    for (reason, want) in [("limit", 4409), ("unsupported", 4502), ("error", 4502)] {
        let mut c = dial(st, "auf_testgrant").await.unwrap();
        c.send(Message::text(OPEN)).await.unwrap();
        let Message::Text(t) = next_msg(&mut sup).await else {
            panic!("expected exec.open");
        };
        let id = serde_json::from_str::<Value>(&t).unwrap()["body"]["pty_id"]
            .as_u64()
            .unwrap();
        sup.send(frame(
            "exec.refused",
            json!({"pty_id": id, "reason": reason}),
        ))
        .await
        .unwrap();
        assert_eq!(close_code(&mut c).await, want, "{reason}");
    }
}

/// An older supervisor ignores `exec.*`: the open times out (5 s) → 4502.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_supervisor_that_ignores_exec_open_is_4502() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    let mut c = dial(st, "auf_testgrant").await.unwrap();
    c.send(Message::text(OPEN)).await.unwrap();
    let Message::Text(t) = next_msg(&mut sup).await else {
        panic!("expected exec.open");
    };
    assert!(t.contains(r#""kind":"exec.open""#));
    // No answer.
    assert_eq!(close_code_slow(&mut c).await, 4502);
}

/// No first message within 5 s → 4504.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_first_message_within_5s_is_4504() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    let mut c = dial(st, "auf_testgrant").await.unwrap();
    assert_eq!(close_code_slow(&mut c).await, 4504);
    assert!(silent(&mut sup, Duration::from_millis(200)).await);
}

/// A W6 reconnect/loss ends the leg 4503.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_loss_ends_the_leg_4503() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    let (mut c, _id) = open_leg(&mut sup, st).await;
    drop(sup);
    assert_eq!(close_code(&mut c).await, 4503);
}

/// The prefix selects the kind: an `auy_` grant never opens a files exec (the runner asks for
/// a PTY and ignores `exec.open` text), and its redemption names `terminal`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_auy_grant_never_opens_a_files_exec() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    let dialed = tokio::spawn(async move { dial(st, "auy_testgrant").await });
    let v = next_text(&mut sup).await;
    assert_eq!(v["kind"], "pty.open", "{v}");
    let id = v["body"]["pty_id"].as_u64().unwrap();
    sup.send(frame("pty.opened", json!({"pty_id": id, "window": WINDOW})))
        .await
        .unwrap();
    let mut c = dialed.await.unwrap().unwrap();
    c.send(Message::text(OPEN)).await.unwrap();
    assert!(
        silent(&mut sup, Duration::from_millis(300)).await,
        "an exec.open from a terminal leg reached the supervisor"
    );
}
