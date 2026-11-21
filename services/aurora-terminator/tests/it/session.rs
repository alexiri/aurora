//! Authenticated sessions: the leg round trip, the channel/request allow-list, notices before a
//! disconnect, revocation, idle and the heartbeat fail-closed rule.

use std::time::Duration;

use russh::ChannelMsg;
use russh::client::Msg;
use uuid::Uuid;

use crate::common::*;

async fn login(r: &Rig) -> Conn {
    let mut c = connect(r.ssh).await;
    let res = c
        .authenticate_openssh_cert(r.agent.to_string(), r.user_key.clone(), r.good())
        .await
        .unwrap();
    assert!(
        failure(&res).is_none(),
        "a valid certificate must authenticate"
    );
    c
}

/// The next success/failure reply of a request, skipping anything else.
async fn reply(ch: &mut russh::Channel<Msg>) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ch.wait())
            .await
            .expect("no reply")
        {
            Some(ChannelMsg::Success) => return true,
            Some(ChannelMsg::Failure) => return false,
            Some(_) => {}
            None => panic!("the channel ended before a reply"),
        }
    }
}

/// Everything until the channel closes: (stdout, stderr, exit status, saw eof).
async fn drain(ch: &mut russh::Channel<Msg>) -> (Vec<u8>, Vec<u8>, Option<u32>) {
    let (mut o, mut e, mut st) = (Vec::new(), Vec::new(), None);
    while let Ok(Some(m)) = tokio::time::timeout(Duration::from_secs(10), ch.wait()).await {
        match m {
            ChannelMsg::Data { data } => o.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => e.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => st = Some(exit_status),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    (o, e, st)
}

async fn wait_for<F: Fn() -> bool>(f: F, what: &str) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exec_channel_round_trips_with_status_stderr_and_one_close_report() {
    let r = rig(RigOpts::default()).await;
    let c = login(&r).await;
    let mut ch = c.channel_open_session().await.unwrap();
    ch.set_env(true, "LANG", "C.UTF-8").await.unwrap();
    assert!(reply(&mut ch).await, "LANG is accepted");
    ch.exec(true, "echo hi").await.unwrap();
    assert!(reply(&mut ch).await, "exec accepted");
    ch.data_bytes(&b"ping"[..]).await.unwrap();
    ch.eof().await.unwrap();
    let (out, err, status) = drain(&mut ch).await;
    assert_eq!(out, b"ping", "stdin is echoed by the stub runner");
    assert_eq!(err, b"bye");
    assert_eq!(status, Some(7), "the exit status is propagated");

    let opens = r.runner.opens.lock().unwrap().clone();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0]["kind"], "exec.open");
    assert_eq!(opens[0]["mode"], "exec");
    assert_eq!(opens[0]["command"], "echo hi");
    assert_eq!(opens[0]["env"]["LANG"], "C.UTF-8");
    assert!(opens[0].get("tty").is_none());
    assert_eq!(
        r.runner.grants.lock().unwrap().as_slice(),
        ["Bearer auz_stubgrant"]
    );
    assert_eq!(r.stub.count("channels:\"exec\""), 1);
    drop(ch);
    drop(c);
    wait_for(
        || !r.stub.closes.lock().unwrap().is_empty(),
        "the close report",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let closes = r.stub.closes.lock().unwrap().clone();
    assert_eq!(closes.len(), 1, "exactly one close report");
    assert_eq!(closes[0].1["reason"], "client_disconnect");
    assert_eq!(closes[0].1["channels_total"], 1);
    assert_eq!(closes[0].1["channels_by_mode"]["exec"], 1);
    assert_eq!(closes[0].1["bytes_in"], 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_channels_and_requests_open_nothing_downstream() {
    let r = rig(RigOpts::default()).await;
    let c = login(&r).await;
    assert!(
        c.channel_open_direct_tcpip("10.0.0.1", 22, "127.0.0.1", 1)
            .await
            .is_err()
    );
    assert!(
        c.channel_open_direct_streamlocal("/var/run/docker.sock")
            .await
            .is_err()
    );
    assert!(c.channel_open_x11("127.0.0.1", 1).await.is_err());
    assert!(c.tcpip_forward("0.0.0.0", 8080).await.is_err());
    assert!(c.streamlocal_forward("/tmp/x").await.is_err());

    let mut ch = c.channel_open_session().await.unwrap();
    ch.request_x11(true, false, "MIT-MAGIC-COOKIE-1", "00", 0)
        .await
        .unwrap();
    assert!(!reply(&mut ch).await, "x11-req");
    ch.agent_forward(true).await.unwrap();
    assert!(!reply(&mut ch).await, "auth-agent-req");
    ch.set_env(true, "AURORA_X", "1").await.unwrap();
    assert!(!reply(&mut ch).await, "env AURORA_X");
    ch.set_env(true, "PATH", "/evil").await.unwrap();
    assert!(!reply(&mut ch).await, "env PATH");
    ch.request_subsystem(true, "netconf").await.unwrap();
    assert!(!reply(&mut ch).await, "another subsystem");
    ch.exec(true, "a\0b").await.unwrap();
    assert!(!reply(&mut ch).await, "a command with NUL");
    ch.request_pty(true, &"x".repeat(65), 80, 24, 0, 0, &[])
        .await
        .unwrap();
    assert!(!reply(&mut ch).await, "a 65-byte TERM");
    ch.request_pty(true, "xt\u{7}erm", 80, 24, 0, 0, &[])
        .await
        .unwrap();
    assert!(!reply(&mut ch).await, "a TERM with a control character");
    assert_eq!(
        r.stub.count("channels"),
        0,
        "no grant was requested for a refused request"
    );
    assert!(r.runner.opens.lock().unwrap().is_empty());

    // The shell still works on a fresh channel, with the pty the certificate permits.
    let mut sh = c.channel_open_session().await.unwrap();
    sh.request_pty(true, "xterm", 100, 30, 0, 0, &[])
        .await
        .unwrap();
    assert!(reply(&mut sh).await, "pty-req");
    sh.request_shell(true).await.unwrap();
    assert!(reply(&mut sh).await, "shell");
    let opens = r.runner.opens.lock().unwrap().clone();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0]["mode"], "shell");
    assert_eq!(
        opens[0]["tty"],
        serde_json::json!({"term": "xterm", "cols": 100, "rows": 30})
    );
    // A second `shell` on the same channel is refused.
    sh.request_shell(true).await.unwrap();
    assert!(!reply(&mut sh).await);

    // A client with no terminal of its own (`ssh -tt host` fed by a pipe) sends 0×0: "unspecified", so the pty gets
    // 80×24 (found by the kind SSH e2e with a real OpenSSH client). Above 1000 stays refused.
    let mut z = c.channel_open_session().await.unwrap();
    z.request_pty(true, "xterm", 0, 0, 0, 0, &[]).await.unwrap();
    assert!(reply(&mut z).await, "pty-req 0x0");
    z.request_shell(true).await.unwrap();
    assert!(reply(&mut z).await, "shell");
    let opens = r.runner.opens.lock().unwrap().clone();
    assert_eq!(
        opens.last().unwrap()["tty"],
        serde_json::json!({"term": "xterm", "cols": 80, "rows": 24})
    );
    let mut big = c.channel_open_session().await.unwrap();
    big.request_pty(true, "xterm", 1001, 24, 0, 0, &[])
        .await
        .unwrap();
    assert!(!reply(&mut big).await, "pty-req above 1000 columns");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_without_permit_pty_gets_no_pty() {
    let r = rig(RigOpts::default()).await;
    let mut c = connect(r.ssh).await;
    let mut s = r.spec();
    s.permit_pty = false;
    let res = c
        .authenticate_openssh_cert(r.agent.to_string(), r.user_key.clone(), r.cert(&s))
        .await
        .unwrap();
    assert!(failure(&res).is_none());
    let mut ch = c.channel_open_session().await.unwrap();
    ch.request_pty(true, "xterm", 80, 24, 0, 0, &[])
        .await
        .unwrap();
    assert!(!reply(&mut ch).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn at_most_the_configured_channels_are_open_at_once() {
    let r = rig(RigOpts::default()).await;
    let c = login(&r).await;
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(c.channel_open_session().await.unwrap());
    }
    assert!(
        c.channel_open_session().await.is_err(),
        "the fifth channel is refused"
    );
}

/// Heartbeat failures ×3 close every session and refuse new authentications until one succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn three_failed_heartbeats_close_every_session_and_refuse_new_ones() {
    let mut o = RigOpts::default();
    o.settings.heartbeat_period = Some(Duration::from_millis(80));
    o.settings.notice_wait = Duration::from_millis(200);
    let r = rig(o).await;
    let c = login(&r).await;
    let mut ch = c.channel_open_session().await.unwrap();
    ch.request_shell(true).await.unwrap();
    assert!(reply(&mut ch).await);
    *r.stub.heartbeat_fail.lock().unwrap() = true;
    let (_, err, status) = drain(&mut ch).await;
    let notice = String::from_utf8_lossy(&err).into_owned();
    assert!(notice.contains("lost contact"), "{notice}");
    assert_eq!(status, Some(255));
    wait_for(
        || !r.stub.closes.lock().unwrap().is_empty(),
        "the close report",
    )
    .await;
    assert_eq!(
        r.stub.closes.lock().unwrap()[0].1["reason"],
        "lease_expired"
    );
    // New authentications are refused (and never reach the API) while unhealthy.
    let before = r.stub.count("sessions");
    let mut c2 = connect(r.ssh).await;
    let res = c2
        .authenticate_openssh_cert(r.agent.to_string(), r.user_key.clone(), r.good())
        .await
        .unwrap();
    assert!(failure(&res).is_some());
    assert_eq!(r.stub.count("sessions"), before);
    // A successful heartbeat reopens the door.
    *r.stub.heartbeat_fail.lock().unwrap() = false;
    wait_for(|| r.shared.is_healthy(), "a healthy heartbeat").await;
    let mut c3 = connect(r.ssh).await;
    let res = c3
        .authenticate_openssh_cert(r.agent.to_string(), r.user_key.clone(), r.good())
        .await
        .unwrap();
    assert!(failure(&res).is_none());
}

/// A revoked session (heartbeat answer) prints its notice on stderr before the disconnect and is
/// reported closed once with the cause.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_session_gets_a_notice_then_a_disconnect() {
    let mut o = RigOpts::default();
    o.settings.heartbeat_period = Some(Duration::from_millis(80));
    o.settings.notice_wait = Duration::from_millis(300);
    let r = rig(o).await;
    let id = Uuid::new_v4();
    *r.stub.next.lock().unwrap() = Some(id);
    let c = login(&r).await;
    let mut ch = c.channel_open_session().await.unwrap();
    ch.request_shell(true).await.unwrap();
    assert!(reply(&mut ch).await);
    r.stub.revoke.lock().unwrap().push(id);
    let (_, err, status) = drain(&mut ch).await;
    let notice = String::from_utf8_lossy(&err).into_owned();
    assert!(
        notice.contains("revoked or the session was ended"),
        "{notice}"
    );
    assert_eq!(status, Some(255));
    wait_for(
        || !r.stub.closes.lock().unwrap().is_empty(),
        "the close report",
    )
    .await;
    let closes = r.stub.closes.lock().unwrap().clone();
    assert_eq!(closes.len(), 1);
    assert_eq!(closes[0].0, id);
    assert_eq!(closes[0].1["reason"], "revoked:user_disconnect");
    wait_for(|| c.is_closed(), "the disconnect").await;
}

/// The idle timeout (shortened) ends a session that carries no bytes, notice first.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_session_is_closed_with_a_notice() {
    let mut o = RigOpts::default();
    o.settings.idle = Some(Duration::from_millis(400));
    o.settings.notice_wait = Duration::from_millis(300);
    let r = rig(o).await;
    let c = login(&r).await;
    let mut ch = c.channel_open_session().await.unwrap();
    ch.request_shell(true).await.unwrap();
    assert!(reply(&mut ch).await);
    let (_, err, status) = drain(&mut ch).await;
    assert!(String::from_utf8_lossy(&err).contains("no activity"));
    assert_eq!(status, Some(255));
    wait_for(
        || !r.stub.closes.lock().unwrap().is_empty(),
        "the close report",
    )
    .await;
    assert_eq!(r.stub.closes.lock().unwrap()[0].1["reason"], "idle");
}

/// (FIX-199, D-434) A client that only sends `keepalive@openssh.com` is a live client: it keeps
/// a session open for several idle timeouts, and the idle timeout runs again once it stops.
#[tokio::test(flavor = "multi_thread")]
async fn keepalives_alone_keep_a_session_open_and_silence_ends_it() {
    let mut o = RigOpts::default();
    o.settings.idle = Some(Duration::from_millis(1500));
    o.settings.notice_wait = Duration::from_millis(300);
    let r = rig(o).await;
    let c = login(&r).await;
    let mut ch = c.channel_open_session().await.unwrap();
    ch.request_shell(true).await.unwrap();
    assert!(reply(&mut ch).await);
    // Three idle timeouts of keepalives and nothing else.
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(4500) {
        c.send_keepalive(true).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            r.stub.closes.lock().unwrap().is_empty(),
            "closed after {:?} although the client sent keepalives",
            start.elapsed()
        );
    }
    // Silence: closed as idle, notice first.
    let (_, err, status) = drain(&mut ch).await;
    assert!(String::from_utf8_lossy(&err).contains("no activity"));
    assert_eq!(status, Some(255));
    wait_for(
        || !r.stub.closes.lock().unwrap().is_empty(),
        "the close report",
    )
    .await;
    assert_eq!(r.stub.closes.lock().unwrap()[0].1["reason"], "idle");
}

/// (FIX-199, D-434) Keepalives restart the idle clock only: the lifetime still ends them.
#[tokio::test(flavor = "multi_thread")]
async fn the_lifetime_still_ends_a_keepalive_only_session() {
    let mut o = RigOpts::default();
    o.settings.idle = Some(Duration::from_millis(1500));
    o.settings.lifetime = Some(Duration::from_millis(2500));
    o.settings.notice_wait = Duration::from_millis(300);
    let r = rig(o).await;
    let c = login(&r).await;
    let mut ch = c.channel_open_session().await.unwrap();
    ch.request_shell(true).await.unwrap();
    assert!(reply(&mut ch).await);
    let pump = async {
        loop {
            if c.send_keepalive(true).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    };
    let (_, err, status) = tokio::select! {
        out = drain(&mut ch) => out,
        () = pump => unreachable!("the keepalive pump ends only when the connection does"),
    };
    assert!(String::from_utf8_lossy(&err).contains("maximum session time"));
    assert_eq!(status, Some(255));
    wait_for(
        || !r.stub.closes.lock().unwrap().is_empty(),
        "the close report",
    )
    .await;
    assert_eq!(r.stub.closes.lock().unwrap()[0].1["reason"], "lifetime");
}

/// A graceful shutdown says goodbye to every session (`terminator_shutdown`).
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_says_goodbye_to_every_session() {
    let mut o = RigOpts::default();
    o.settings.notice_wait = Duration::from_millis(300);
    let r = rig(o).await;
    let c = login(&r).await;
    let mut ch = c.channel_open_session().await.unwrap();
    ch.request_shell(true).await.unwrap();
    assert!(reply(&mut ch).await);
    r.stop.send(true).unwrap();
    let (_, err, _) = drain(&mut ch).await;
    assert!(String::from_utf8_lossy(&err).contains("restarting"));
    wait_for(
        || !r.stub.closes.lock().unwrap().is_empty(),
        "the close report",
    )
    .await;
    assert_eq!(
        r.stub.closes.lock().unwrap()[0].1["reason"],
        "terminator_shutdown"
    );
}
