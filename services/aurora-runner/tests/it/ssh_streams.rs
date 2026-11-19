//! (P4-SSH-2, `16 §W20` leg 1 and leg 2) The runner's stream listener serves `kind: "ssh"` grants
//! (`Bearer auz_…`): the redemption carries `kind: "ssh"`; the terminator's first message is the
//! full `exec.open` (`shell`/`exec`/`sftp`, `tty`, `LANG`/`LC_*` env) within 5 s — anything else is
//! 4502 and nothing reaches the supervisor; the runner then allocates the `pty_id`, sends leg 2's
//! `exec.open` with those fields and answers `{"kind":"exec.opened"}` or closes 4502 / 4409;
//! `[0x02]`/`[0x03]` binary, `exec.eof`, `pty.resize` (only with a `tty`) and `exec.exit` relay;
//! bytes stamp the SSH keep-awake at most once per grace; the per-sandbox cap is 8 (4409), apart
//! from the files cap; an older supervisor → 4502; a channel loss → 4503; a runner the API did
//! not enable for SSH never redeems the grant; `auy_` legs never open an exec.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use futures::SinkExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use uuid::Uuid;
use wiremock::MockServer;

use crate::stream_support::{close_code_slow, data, dial, frame, next_leg, next_msg, silent};
use crate::terminal_streams::{
    Client, WINDOW, close_code, connect_supervisor, mount_runner_api, next_text, rig,
};

const SHELL: &str = r#"{"kind":"exec.open","mode":"shell","env":{}}"#;

/// Dials, sends `first`, answers the supervisor's `exec.open` with `exec.opened`; returns the
/// terminator's leg, the allocated id and the `exec.open` body the supervisor saw.
async fn open_leg_with(
    sup: &mut Client,
    st: std::net::SocketAddr,
    first: &str,
) -> (Client, u32, Value) {
    let mut c = dial(st, "auz_testgrant").await.unwrap();
    c.send(Message::text(first)).await.unwrap();
    let Message::Text(t) = next_msg(sup).await else {
        panic!("expected exec.open");
    };
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(
        v["kind"], "exec.open",
        "an SSH leg only ever opens an exec: {v}"
    );
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
    (c, id, v["body"].clone())
}

async fn open_leg(sup: &mut Client, st: std::net::SocketAddr) -> (Client, u32) {
    let (c, id, _) = open_leg_with(sup, st, SHELL).await;
    (c, id)
}

fn keep_awakes(reqs: &[wiremock::Request]) -> Vec<Value> {
    reqs.iter()
        .filter(|r| r.url.path().ends_with("/terminal/keep-awake"))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_leg_round_trip_stamps_ssh_keep_awake_once() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;

    let first = r#"{"kind":"exec.open","mode":"exec","command":"echo hi","tty":{"term":"xterm-256color","cols":80,"rows":24},"env":{"LANG":"C.UTF-8","LC_ALL":"C"}}"#;
    let (mut c, id, body) = open_leg_with(&mut sup, st, first).await;
    // Leg 2: the very fields of the first message, with the terminal window.
    assert_eq!(body["mode"], "exec");
    assert_eq!(body["command"], "echo hi");
    assert_eq!(
        body["tty"],
        json!({"term": "xterm-256color", "cols": 80, "rows": 24})
    );
    assert_eq!(body["env"], json!({"LANG": "C.UTF-8", "LC_ALL": "C"}));
    assert_eq!(body["window"], WINDOW);

    // terminator → sandbox: `[0x02][payload]` arrives framed `[0x02][pty_id BE][payload]`.
    for chunk in [&b"one"[..], b"two", b"three"] {
        let mut m = vec![0x02];
        m.extend_from_slice(chunk);
        c.send(Message::binary(m)).await.unwrap();
        assert_eq!(next_msg(&mut sup).await, data(0x02, id, chunk));
    }
    // sandbox → terminator: stdout `[0x02]` and stderr `[0x03]`.
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

    // `pty.resize` (the channel has a tty) reaches the supervisor with the allocated id.
    c.send(Message::text(
        r#"{"kind":"pty.resize","cols":120,"rows":40}"#,
    ))
    .await
    .unwrap();
    let v = loop {
        let Message::Text(t) = next_msg(&mut sup).await else {
            continue;
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["kind"] == "pty.resize" {
            break v;
        }
    };
    assert_eq!(v["body"], json!({"pty_id": id, "cols": 120, "rows": 40}));

    // `exec.eof` relays; `exec.exit` then `pty.close { eof }` end the leg with 1000.
    c.send(Message::text(r#"{"kind":"exec.eof"}"#))
        .await
        .unwrap();
    let v = loop {
        let Message::Text(t) = next_msg(&mut sup).await else {
            continue;
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["kind"] == "exec.eof" {
            break v;
        }
    };
    assert_eq!(v["body"]["pty_id"], u64::from(id));
    sup.send(frame("exec.exit", json!({"pty_id": id, "signal": "KILL"})))
        .await
        .unwrap();
    sup.send(frame("pty.close", json!({"pty_id": id, "reason": "eof"})))
        .await
        .unwrap();
    let Message::Text(t) = next_leg(&mut c).await else {
        panic!("expected exec.exit");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&t).unwrap(),
        json!({"kind": "exec.exit", "signal": "KILL"})
    );
    assert_eq!(close_code(&mut c).await, 1000);

    // The redemption named `ssh`; bytes both ways stamped exactly one `ssh` keep-awake within
    // the grace (never the terminal column).
    let reqs = api.received_requests().await.unwrap();
    let open = reqs
        .iter()
        .find(|r| r.url.path().ends_with("/preview-streams/open"))
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&open.body).unwrap()["kind"],
        "ssh"
    );
    // The stamp is detached from the leg: wait for it, then count.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let stamps = loop {
        let s = keep_awakes(&api.received_requests().await.unwrap());
        if !s.is_empty() || tokio::time::Instant::now() > deadline {
            break s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(stamps.len(), 1, "at most one stamp per grace: {stamps:?}");
    assert_eq!(stamps[0]["kind"], "ssh");
    assert_eq!(stamps[0]["sandbox_id"], json!(sandbox));
}

/// Frame validation of the first message: every malformed shape closes the leg 4502 and never
/// reaches the supervisor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_first_message_is_4502_and_stays_off_the_supervisor() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    let long = "x".repeat(aurora_core::limits::SSH_EXEC_COMMAND_MAX + 1);
    let many: serde_json::Map<String, Value> = (0..=aurora_core::limits::SSH_ENV_VARS_MAX)
        .map(|i| (format!("LC_{i}"), json!("v")))
        .collect();
    let cases = vec![
        Message::text(r#"{"kind":"exec.open","mode":"exec"}"#),
        Message::text(r#"{"kind":"exec.open","mode":"exec","command":"a\u0000b"}"#),
        Message::text(json!({"kind":"exec.open","mode":"exec","command":long}).to_string()),
        Message::text(r#"{"kind":"exec.open","mode":"shell","command":"ls"}"#),
        Message::text(r#"{"kind":"exec.open","mode":"sftp","command":"ls"}"#),
        Message::text(r#"{"kind":"exec.open","mode":"sftp","tty":{"term":"x","cols":1,"rows":1}}"#),
        Message::text(r#"{"kind":"exec.open","mode":"bash"}"#),
        Message::text(r#"{"kind":"exec.open","mode":"shell","env":{"PATH":"/evil"}}"#),
        Message::text(r#"{"kind":"exec.open","mode":"shell","env":{"AURORA_X":"1"}}"#),
        Message::text(json!({"kind":"exec.open","mode":"shell","env":many}).to_string()),
        Message::text(
            json!({"kind":"exec.open","mode":"shell","env":{"LANG":"x".repeat(257)}}).to_string(),
        ),
        Message::text(
            r#"{"kind":"exec.open","mode":"shell","tty":{"term":"x","cols":0,"rows":24}}"#,
        ),
        Message::text(
            r#"{"kind":"exec.open","mode":"shell","tty":{"term":"x","cols":1001,"rows":24}}"#,
        ),
        Message::text(
            r#"{"kind":"exec.open","mode":"shell","tty":{"term":"","cols":80,"rows":24}}"#,
        ),
        Message::text(r#"{"kind":"exec.open","mode":"shell","extra":1}"#),
        Message::text(r#"{"kind":"pty.resize","cols":1,"rows":1}"#),
        Message::text("not json"),
        Message::binary(vec![0x02, 1, 2]),
    ];
    for first in cases {
        let mut c = dial(st, "auz_testgrant").await.unwrap();
        c.send(first.clone()).await.unwrap();
        assert_eq!(close_code(&mut c).await, 4502, "{first:?}");
    }
    assert!(
        silent(&mut sup, Duration::from_millis(300)).await,
        "a refused first frame reached the supervisor"
    );
}

/// After the open only `[0x02]` binary, `exec.eof` and (with a tty) a bounded `pty.resize`.
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
        // No tty on this channel: a resize is not legal.
        Message::text(r#"{"kind":"pty.resize","cols":80,"rows":24}"#),
        Message::text(r#"{"kind":"exec.eof","x":1}"#),
        Message::text(r#"{"kind":"exec.open","mode":"shell"}"#),
        Message::text(format!(
            r#"{{"kind":"exec.eof","pad":"{}"}}"#,
            "x".repeat(300)
        )),
    ] {
        let (mut c, _id) = open_leg(&mut sup, st).await;
        c.send(bad.clone()).await.unwrap();
        assert_eq!(close_code(&mut c).await, 4502, "{bad:?}");
        let Message::Text(t) = next_msg(&mut sup).await else {
            panic!("expected pty.close");
        };
        assert!(t.contains(r#""kind":"pty.close""#), "{t}");
    }
    // With a tty a resize is legal only inside 1…1000 and with exactly its three keys.
    let tty = r#"{"kind":"exec.open","mode":"shell","tty":{"term":"xterm","cols":80,"rows":24}}"#;
    for bad in [
        r#"{"kind":"pty.resize","cols":0,"rows":24}"#,
        r#"{"kind":"pty.resize","cols":80,"rows":1001}"#,
        r#"{"kind":"pty.resize","cols":80}"#,
        r#"{"kind":"pty.resize","cols":80,"rows":24,"pty_id":1}"#,
        r#"{"kind":"pty.resize","cols":"80","rows":24}"#,
    ] {
        let (mut c, _id, _) = open_leg_with(&mut sup, st, tty).await;
        c.send(Message::text(bad)).await.unwrap();
        assert_eq!(close_code(&mut c).await, 4502, "{bad}");
        let Message::Text(t) = next_msg(&mut sup).await else {
            panic!("expected pty.close");
        };
        assert!(t.contains(r#""kind":"pty.close""#), "{t}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ninth_leg_of_a_sandbox_is_4409_and_files_legs_have_their_own_cap() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    let mut legs = Vec::new();
    for _ in 0..aurora_core::limits::SSH_CHANNELS_PER_SANDBOX {
        legs.push(open_leg(&mut sup, st).await);
    }
    let mut c = dial(st, "auz_testgrant").await.unwrap();
    c.send(Message::text(SHELL)).await.unwrap();
    assert_eq!(close_code(&mut c).await, 4409);
    assert!(
        silent(&mut sup, Duration::from_millis(300)).await,
        "the capped open reached the supervisor"
    );
    // A files leg still opens beside the eight SSH legs.
    let mut f = dial(st, "auf_testgrant").await.unwrap();
    f.send(Message::text(r#"{"kind":"exec.open","mode":"sftp"}"#))
        .await
        .unwrap();
    let Message::Text(t) = next_msg(&mut sup).await else {
        panic!("expected exec.open");
    };
    assert!(t.contains(r#""kind":"exec.open""#), "{t}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_refusals_map_to_4409_and_4502() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig(&api).await;
    let mut sup = connect_supervisor(ch).await;
    for (reason, want) in [("limit", 4409), ("unsupported", 4502), ("error", 4502)] {
        let mut c = dial(st, "auz_testgrant").await.unwrap();
        c.send(Message::text(SHELL)).await.unwrap();
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
    let mut c = dial(st, "auz_testgrant").await.unwrap();
    c.send(Message::text(SHELL)).await.unwrap();
    let Message::Text(t) = next_msg(&mut sup).await else {
        panic!("expected exec.open");
    };
    assert!(t.contains(r#""kind":"exec.open""#));
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
    let mut c = dial(st, "auz_testgrant").await.unwrap();
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

/// A runner the API did not enable for SSH (`ssh_enabled` false) answers the dial 403 and never
/// redeems the grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runner_without_ssh_enabled_never_redeems_an_auz_grant() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (_ch, st, server, _s1, _s2) = rig(&api).await;
    server.set_ssh_enabled(false);
    match dial(st, "auz_testgrant").await {
        Err(WsError::Http(r)) => assert_eq!(r.status().as_u16(), 403),
        other => panic!("expected a 403, got {:?}", other.map(|_| ())),
    }
    let reqs = api.received_requests().await.unwrap();
    assert!(
        !reqs
            .iter()
            .any(|r| r.url.path().ends_with("/preview-streams/open")),
        "the grant was redeemed"
    );
}

/// The prefix selects the kind: an `auy_` grant never opens an SSH exec.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_auy_grant_never_opens_an_ssh_exec() {
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
    c.send(Message::text(SHELL)).await.unwrap();
    assert!(
        silent(&mut sup, Duration::from_millis(300)).await,
        "an exec.open from a terminal leg reached the supervisor"
    );
}
