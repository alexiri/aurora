//! (P5-REFINE-4, C-1058, D-416; UX review 5 computer-1) An open terminal or SSH session keeps the
//! computer awake whether or not bytes flow: the listener posts the keep-awake stamp every half
//! grace while the leg is open, and stops when it closes. Run against the real listener with a
//! shortened grace (production: `TERMINAL_AWAKE_GRACE` / `SSH_AWAKE_GRACE`, 120 s).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use aurora_runner::stream_listener::{Bounds, StreamListener};
use futures::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;
use wiremock::MockServer;

use crate::stream_support::{dial, frame, next_leg, next_msg};
use crate::terminal_streams::{WINDOW, connect_supervisor, mount_runner_api, next_text, rig_with};

const GRACE: Duration = Duration::from_millis(600);

fn short_grace(server: aurora_runner::channel::ChannelServer) -> std::sync::Arc<StreamListener> {
    StreamListener::with_bounds(
        server,
        Duration::from_secs(600),
        Duration::from_secs(4 * 3600),
        Bounds {
            terminal_awake_grace: GRACE,
            ssh_awake_grace: GRACE,
            ..Bounds::default()
        },
    )
}

/// The keep-awake posts the API received, by kind (`terminal` when absent).
async fn stamps(api: &MockServer, kind: &str) -> usize {
    api.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().ends_with("/runner/terminal/keep-awake"))
        .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap())
        .filter(|b| b["kind"].as_str().unwrap_or("terminal") == kind)
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quiet_terminal_keeps_stamping_until_it_closes() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig_with(&api, short_grace).await;
    let mut sup = connect_supervisor(ch).await;

    let leg = tokio::spawn(async move { dial(st, "auy_testgrant").await });
    let opened = next_text(&mut sup).await;
    assert_eq!(opened["kind"], "pty.open", "{opened}");
    let pty_id = opened["body"]["pty_id"].as_u64().unwrap();
    sup.send(frame(
        "pty.opened",
        json!({"pty_id": pty_id, "window": WINDOW}),
    ))
    .await
    .unwrap();
    let mut term = leg.await.unwrap().unwrap();

    // No byte either way for more than two graces: the heartbeat alone stamps.
    tokio::time::sleep(GRACE * 2 + GRACE / 3).await;
    let open = stamps(&api, "terminal").await;
    assert!(open >= 2, "a quiet open terminal stamped {open} times");
    assert_eq!(stamps(&api, "ssh").await, 0);

    // The browser leaves: the leg ends and so do the stamps.
    term.close(None).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), next_msg(&mut sup)).await;
    tokio::time::sleep(GRACE / 2).await;
    let closed = stamps(&api, "terminal").await;
    tokio::time::sleep(GRACE * 2).await;
    assert_eq!(
        stamps(&api, "terminal").await,
        closed,
        "no stamp after close"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quiet_ssh_session_keeps_stamping_kind_ssh_until_it_closes() {
    let api = MockServer::start().await;
    let (sandbox, boot) = (Uuid::new_v4(), Uuid::new_v4());
    mount_runner_api(&api, sandbox, boot).await;
    let (ch, st, _server, _s1, _s2) = rig_with(&api, short_grace).await;
    let mut sup = connect_supervisor(ch).await;

    let mut c = dial(st, "auz_testgrant").await.unwrap();
    c.send(Message::text(
        r#"{"kind":"exec.open","mode":"shell","env":{}}"#,
    ))
    .await
    .unwrap();
    let Message::Text(t) = next_msg(&mut sup).await else {
        panic!("expected exec.open");
    };
    let v: Value = serde_json::from_str(&t).unwrap();
    let id = v["body"]["pty_id"].as_u64().unwrap();
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

    tokio::time::sleep(GRACE * 2 + GRACE / 3).await;
    let open = stamps(&api, "ssh").await;
    assert!(open >= 2, "a quiet SSH session stamped {open} times");
    assert_eq!(stamps(&api, "terminal").await, 0);

    c.close(None).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), next_msg(&mut sup)).await;
    tokio::time::sleep(GRACE / 2).await;
    let closed = stamps(&api, "ssh").await;
    tokio::time::sleep(GRACE * 2).await;
    assert_eq!(stamps(&api, "ssh").await, closed, "no stamp after close");
}
