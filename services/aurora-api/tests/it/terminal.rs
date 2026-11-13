#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The web terminal's API surface (phase 4, P4-TERM, D-161, SEC-87, C-616..C-621;
//! docs/18 §Web terminal): the upgrade auth matrix (session-only, same-origin, `can_use`,
//! hosted, not frozen, feature on), the single-use `auy_` grant (hashed, ≤ 30 s, the
//! `agent.terminal.open` audit row with metadata only), the runner-plane redemption with
//! `kind: "terminal"` (single-use, boot-bound, no port/share in the answer), the
//! keep-awake stamp and the leg-1 proxy through a real served router (bytes flow, the
//! browser's close is final, and a full run leaves no secret in the API's own surfaces:
//! no request-log row, no transcript event, no grant value in the audit metadata).

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use aurora_core::token::{self, TokenPrefix};
use futures::{SinkExt, StreamExt};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// Extract the bare token from an `Authorization: Bearer …` header value.
fn ws_bearer(auth: &str) -> Option<&str> {
    auth.strip_prefix("Bearer ")
}

/// The rig with the terminal enabled on the instance.
async fn term_rig() -> Rig {
    rig_with(|c| {
        c.terminal.enabled = true;
    })
    .await
}

/// A hosted agent with a running sandbox row (the rig's runner + replica), owned by the
/// rig's workspace; `boot` names the current boot. `state` is any live schema state
/// (`stopped` reads as parked in the admission logic — anything but starting/running).
async fn sandbox_on(r: &Rig, handle: &str, state: &str) -> (Uuid, Uuid, Uuid) {
    let agent = r.agent(handle, false).await;
    let sb = Uuid::now_v7();
    let boot = Uuid::now_v7();
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
    .bind(boot)
    .execute(r.h.pool())
    .await
    .unwrap();
    (agent, sb, boot)
}

impl Rig {
    /// The member's call (session cookie, same-origin `Origin` unless the extras carry one —
    /// `headers.get(ORIGIN)` reads the *first* header, so a hostile Origin must replace).
    pub(crate) async fn term_call(
        &self,
        m: Method,
        uri: &str,
        extra: &[(&str, &str)],
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut hd: Vec<(&str, &str)> = vec![("cookie", &self.cookie)];
        if !extra.iter().any(|(k, _)| *k == "origin") {
            hd.push(("origin", ORIGIN));
        }
        hd.extend_from_slice(extra);
        let (s, _, v) = self.h.send(m, uri, &hd, body).await;
        (s.as_u16(), v)
    }

    /// The API's runner-plane call (bearer `aur_…` + the replica header).
    pub(crate) async fn runner_call(&self, uri: &str, body: Value, replica: Uuid) -> (u16, Value) {
        let b = bearer(&self.tok);
        let rid = replica.to_string();
        let (s, _, v) = self
            .h
            .send(
                Method::POST,
                uri,
                &[("authorization", &b), ("x-aurora-replica", &rid)],
                Some(body),
            )
            .await;
        (s.as_u16(), v)
    }

    /// The redemption the runner's stream listener makes (`kind: "terminal"`, C-617).
    pub(crate) async fn redeem(&self, grant: &str, replica: Uuid) -> (u16, Value) {
        self.runner_call(
            "/v1/runner/preview-streams/open",
            json!({"grant": grant, "kind": "terminal"}),
            replica,
        )
        .await
    }

    /// The live grants of this agent (consumed, unexpired).
    pub(crate) async fn grant_rows(&self, agent: Uuid) -> Vec<(bool, bool)> {
        sqlx::query_as::<
            _,
            (
                Option<chrono::DateTime<chrono::Utc>>,
                chrono::DateTime<chrono::Utc>,
            ),
        >("SELECT consumed_at, expires_at FROM terminal_stream_grants WHERE agent_id = $1")
        .bind(agent)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
        .into_iter()
        .map(|(c, e)| (c.is_some(), e > chrono::Utc::now()))
        .collect()
    }
}

// ---------------------------------------------------------------- the auth matrix (C-616)

/// Off → every surface 404 `terminal_disabled` before any auth (the route answers first).
/// The route check runs before the WS extractor, so a plain call reaches it.
#[tokio::test]
async fn disabled_instance_answers_404_terminal_disabled() {
    let r = rig_with(|_| {}).await; // terminal.enabled defaults off
    let (agent, _, _) = sandbox_on(&r, "off-sb", "running").await;
    let (s, v) = r
        .term_call(
            Method::GET,
            &format!("/v1/agents/{agent}/terminal"),
            &[],
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (404, "terminal_disabled"), "{v}");
}

/// Anonymous → 401 (no session cookie; the `Caller` extractor answers before the upgrade).
#[tokio::test]
async fn anonymous_caller_is_refused_401() {
    let r = term_rig().await;
    let (agent, _, _) = sandbox_on(&r, "anon-sb", "running").await;
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/agents/{agent}/terminal"),
            &[("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (401, "unauthenticated"), "{v}");
}

/// API keys are machine callers: 401 (the route refuses non-session callers, SEC-87).
#[tokio::test]
async fn api_key_caller_is_refused_401() {
    let r = term_rig().await;
    let (agent, _, _) = sandbox_on(&r, "key-sb", "running").await;
    let user = r.h.user("k@x.io", false).await;
    let (_, tok) = r.h.api_key(user, r.org, Some(r.ws), false).await;
    let b = bearer(&tok);
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/agents/{agent}/terminal"),
            &[("authorization", &b), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (401, "unauthenticated"), "{v}");
}

/// Cross-origin `Origin` → 403 `csrf_rejected` (same-origin only, C-616).
#[tokio::test]
async fn cross_origin_upgrade_is_refused() {
    let r = term_rig().await;
    let (agent, _, _) = sandbox_on(&r, "evil-sb", "running").await;
    let (s, v) = r
        .term_call(
            Method::GET,
            &format!("/v1/agents/{agent}/terminal"),
            &[
                ("origin", "http://evil.example.org"),
                ("connection", "upgrade"),
                ("upgrade", "websocket"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
                ("sec-websocket-version", "13"),
            ],
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (403, "csrf_rejected"), "{v}");
}

/// A BYO agent has no sandbox to shell into: 422 (served dial; the WS extractor needs a real
/// connection — `hyper::upgrade::OnUpgrade` never exists on `oneshot` — so every check
/// at/after admission runs through the served router below).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn byo_agent_answers_422_terminal_not_supported() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'byo','claude_code')",
    )
    .bind(agent)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let s = served::status(
        &url,
        &r.cookie,
        &format!("/v1/agents/{agent}/terminal"),
        ORIGIN,
    )
    .await;
    assert_eq!(s, 422);
}

/// An agent of another workspace is invisible: 404 (the `can_use` law).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_workspace_agent_is_invisible_404() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    // The other org/workspace/agent (outside the rig's authority).
    let other_org = r.h.org().await;
    let other_ws = r.h.workspace(other_org).await;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(agent)
    .bind(other_org)
    .bind(other_ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let s = served::status(
        &url,
        &r.cookie,
        &format!("/v1/agents/{agent}/terminal"),
        ORIGIN,
    )
    .await;
    assert_eq!(s, 404);
}

/// Frozen agent → 423 (the freeze banner is the dashboard's only view).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frozen_agent_answers_423() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    let (agent, _, _) = sandbox_on(&r, "frozen-sb", "running").await;
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 'incident', \
         frozen_authority = 'workspace_owner' WHERE id = $1",
    )
    .bind(agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let s = served::status(
        &url,
        &r.cookie,
        &format!("/v1/agents/{agent}/terminal"),
        ORIGIN,
    )
    .await;
    assert_eq!(s, 423);
}

/// An agent without a sandbox row: 404 (waking is meaningless without a start request).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unprovisioned_agent_answers_404() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    let agent = r.agent("bare-terminal", false).await;
    let s = served::status(
        &url,
        &r.cookie,
        &format!("/v1/agents/{agent}/terminal"),
        ORIGIN,
    )
    .await;
    assert_eq!(s, 404);
}

/// A stopped (parked) sandbox → 202 and the wake demand recorded (C-621); frozen never gets
/// here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parked_sandbox_answers_202_waking_and_sets_the_wake_demand() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    let (agent, sb, _boot) = sandbox_on(&r, "parked-sb", "stopped").await;
    let s = served::status(
        &url,
        &r.cookie,
        &format!("/v1/agents/{agent}/terminal"),
        ORIGIN,
    )
    .await;
    assert_eq!(s, 202);
    let due: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT terminal_wake_at FROM sandboxes WHERE id = $1")
            .bind(sb)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert!(due.is_some(), "the wake demand must be set");
    // No grant is minted for a parked sandbox.
    assert!(r.grant_rows(agent).await.is_empty());
    // No audit row: the open did not happen (the wake is not an open).
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'agent.terminal.open'")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 0);
}

// ---------------------------------------------------------------- the grant (C-618)

/// The mint path through a served dial whose runner endpoint is unreachable: the grant and
/// the audit row exist (both written before the dial), the dial failure is the documented
/// 502 `terminal_upstream`, and nothing about the open is request-logged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_mints_the_grant_and_audits_metadata_only() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    let (agent, _sb, _boot) = sandbox_on(&r, "mint-sb", "running").await;
    // `runner-a` has no DNS in the test host: the post-mint dial fails fast.
    let s = served::status(
        &url,
        &r.cookie,
        &format!("/v1/agents/{agent}/terminal"),
        ORIGIN,
    )
    .await;
    assert_eq!(s, 502, "post-mint proxy failure");
    // Exactly one live, unconsumed grant with a ≤ 30 s TTL.
    let rows = r.grant_rows(agent).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0], (false, true), "single row, unconsumed, unexpired");
    let ttl: i64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (expires_at - created_at))::bigint \
         FROM terminal_stream_grants WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(ttl, 30, "TERMINAL_GRANT_TTL");
    // The audit row (best-effort, spawned): `agent.terminal.open`, metadata = {agent,
    // workspace} only — poll briefly for the async write.
    let mut meta = None;
    for _ in 0..60 {
        if let Ok(m) = sqlx::query_as::<_, (Value, String)>(
            "SELECT metadata, source FROM audit_log \
             WHERE action = 'agent.terminal.open' AND target_id = $1",
        )
        .bind(agent.to_string())
        .fetch_one(r.h.pool())
        .await
        {
            meta = Some(m);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let (meta, src) = meta.expect("the agent.terminal.open audit row");
    assert_eq!(src, "dashboard");
    assert_eq!(meta["agent"], json!(agent.to_string()));
    assert_eq!(meta["workspace"], json!(r.ws.to_string()));
    assert_eq!(
        serde_json::to_string(&meta).unwrap(),
        serde_json::to_string(&json!({"agent": agent.to_string(), "workspace": r.ws.to_string()}))
            .unwrap(),
        "metadata carries no grant material: {meta}"
    );
    // No request-log row for the terminal open (terminal traffic is never request-logged).
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM request_log WHERE path LIKE '%terminal%'")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 0);
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// What a freshly upgraded terminal socket does: `Open` = bytes echo through the live leg,
/// `Closed(code)` = the API closed it with that code. Bounded wait, no sleeps.
#[derive(Debug, PartialEq)]
enum Leg {
    Open,
    Closed(u16),
}

async fn probe(ws: &mut Ws) -> Leg {
    use tokio_tungstenite::tungstenite::Message;
    // A send may fail when the API already closed the socket; the close frame is still read.
    let _ = ws.send(Message::binary(b"probe".to_vec())).await;
    loop {
        let m = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("probe timed out");
        match m {
            Some(Ok(Message::Binary(_))) => return Leg::Open,
            Some(Ok(Message::Close(f))) => return Leg::Closed(f.map_or(1005, |f| f.code.into())),
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return Leg::Closed(0),
        }
    }
}

/// Dial the terminal of `agent` as `cookie` and probe it.
async fn open_term(url: &str, cookie: &str, agent: Uuid) -> (Ws, Leg) {
    let (mut ws, resp) = served::dial(url, cookie, &format!("/v1/agents/{agent}/terminal"))
        .await
        .expect("the upgrade completes (caps close the socket, they do not refuse the dial)");
    assert_eq!(resp.status(), 101);
    let leg = probe(&mut ws).await;
    (ws, leg)
}

/// Close a leg and wait until the API has released its slot (the server closes the socket
/// last; reading to the end is the barrier, then the slot drop is racing only the task end —
/// callers retry the next open with a deadline).
async fn hang_up(mut ws: Ws) {
    let _ = ws.close(None).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while ws.next().await.is_some() {}
    })
    .await;
}

/// Open until it works (the previous leg's slot is released asynchronously), bounded.
async fn open_term_eventually(url: &str, cookie: &str, agent: Uuid) -> Ws {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (ws, leg) = open_term(url, cookie, agent).await;
        if leg == Leg::Open {
            return ws;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "slot never released: {leg:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// FIX-061: the per-user cap counts LIVE sessions. Two open sockets held → the third
/// upgrades and closes `4409`; after one closes, a new one opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_user_cap_counts_live_sessions_and_closes_4409() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    served::fake_runner_multi(&r).await;
    let (agent, _, _) = sandbox_on(&r, "cap-user", "running").await;

    let (w1, l1) = open_term(&url, &r.cookie, agent).await;
    let (w2, l2) = open_term(&url, &r.cookie, agent).await;
    assert_eq!((l1, l2), (Leg::Open, Leg::Open));
    let (_w3, l3) = open_term(&url, &r.cookie, agent).await;
    assert_eq!(l3, Leg::Closed(4409), "beyond TERMINALS_PER_USER (2 live)");

    hang_up(w1).await;
    let w4 = open_term_eventually(&url, &r.cookie, agent).await;
    drop((w2, w4));
}

/// FIX-061: the per-agent cap (4) spans users; a leg whose dial the runner refuses leaks no
/// slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_agent_cap_spans_users_and_failed_dials_leak_nothing() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    let (agent, _, _) = sandbox_on(&r, "cap-agent", "running").await;

    // The runner refuses (unreachable): three failed opens exceed what a leak would allow
    // (per user 2) — each answers 502 and releases its slot.
    for i in 0..3 {
        let s = served::status(
            &url,
            &r.cookie,
            &format!("/v1/agents/{agent}/terminal"),
            ORIGIN,
        )
        .await;
        assert_eq!(s, 502, "refused dial {i}");
    }
    served::fake_runner_multi(&r).await;

    // Two more members of the workspace.
    let mut cookies = vec![r.cookie.clone()];
    for n in 0..2 {
        let u = r.h.user(&format!("t{n}@x.io"), false).await;
        r.h.member(r.org, u, "member", "active").await;
        r.h.bind_ws(r.ws, u, "member").await;
        let (_, tok) = r.h.good_session(u).await;
        cookies.push(cookie(&tok));
    }
    // 2 + 2 live sessions on the agent from two users = TERMINALS_PER_AGENT.
    let mut held = Vec::new();
    for c in &cookies[..2] {
        for _ in 0..2 {
            let (w, l) = open_term(&url, c, agent).await;
            assert_eq!(l, Leg::Open);
            held.push(w);
        }
    }
    // A third user (0 of their own) is refused: the agent is full.
    let (_w, l) = open_term(&url, &cookies[2], agent).await;
    assert_eq!(l, Leg::Closed(4409), "beyond TERMINALS_PER_AGENT");
    // One of the first user's sessions ends → the third user fits.
    hang_up(held.remove(0)).await;
    let w = open_term_eventually(&url, &cookies[2], agent).await;
    drop((w, held));
}

/// FIX-061: an open terminal re-checks access (C-617 `4403` "access lost"): a member removed
/// from the workspace mid-session has their shell closed `4403`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_terminal_closes_4403_when_access_is_lost() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    served::fake_runner_multi(&r).await;
    let (agent, _, _) = sandbox_on(&r, "cap-recheck", "running").await;
    let (mut ws, leg) = open_term(&url, &r.cookie, agent).await;
    assert_eq!(leg, Leg::Open);
    // The only member of the rig's org (the caller) is suspended: no active membership.
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE org_id = $1")
        .bind(r.org)
        .execute(r.h.pool())
        .await
        .unwrap();
    // The periodic re-check (10 s) closes the leg; bounded wait.
    let end = tokio::time::timeout(std::time::Duration::from_secs(40), async {
        loop {
            match ws.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(f))) => {
                    return f.map(|f| u16::from(f.code));
                }
                Some(Ok(_)) => {}
                _ => return None,
            }
        }
    })
    .await
    .expect("the access re-check never closed the leg");
    assert_eq!(end, Some(4403));
}

/// FIX-061 item 4: the runner's terminal keep-awake report needs the runner credential.
#[tokio::test]
async fn terminal_keep_awake_requires_the_runner_credential() {
    let r = term_rig().await;
    let (_, sb, _) = sandbox_on(&r, "ka-auth", "running").await;
    let (s, _, v) =
        r.h.send(
            Method::POST,
            "/v1/runner/terminal/keep-awake",
            &[("x-aurora-replica", &r.rep.to_string())],
            Some(json!({"sandbox_id": sb})),
        )
        .await;
    assert_eq!(s.as_u16(), 401, "{v}");
}

// ---------------------------------------------------------------- redemption (C-617)

/// A grant minted by hand (as the upgrade would) redeems once with `kind: "terminal"`:
/// the answer is the binding only (no port, no share), and the second redemption is 403.
#[tokio::test]
async fn redemption_is_single_use_and_boot_bound() {
    let r = term_rig().await;
    let (agent, sb, boot) = sandbox_on(&r, "redeem-sb", "running").await;
    let grant = token::generate(TokenPrefix::TerminalStream);
    sqlx::query(
        "INSERT INTO terminal_stream_grants (id, agent_id, sandbox_id, boot_id, granted_by, grant_hash, expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6, now() + interval '30 seconds')",
    )
    .bind(Uuid::now_v7())
    .bind(agent)
    .bind(sb)
    .bind(boot)
    .bind(Uuid::now_v7())
    .bind(hex(&token::hash(grant.expose())))
    .execute(r.h.pool())
    .await
    .unwrap();
    let replica: Uuid = sqlx::query_scalar("SELECT id FROM runner_replicas LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r.redeem(grant.expose(), replica).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["sandbox_id"], json!(sb.to_string()), "{v}");
    assert_eq!(v["boot_id"], json!(boot.to_string()));
    assert_eq!(v["port"], Value::Null, "a terminal grant names no port");
    assert_eq!(
        v["share_id"],
        Value::Null,
        "a terminal grant names no share"
    );
    assert_eq!(v["window"], json!(131_072u64), "TERMINAL_STREAM_WINDOW");
    // Single use: the second redemption is 403 `grant_invalid`.
    let (s, v) = r.redeem(grant.expose(), replica).await;
    assert_eq!((s, code(&v)), (403, "grant_invalid"), "{v}");
}

/// A stale boot (the sandbox moved on) is 409 `not_current`.
#[tokio::test]
async fn stale_boot_redemption_is_409() {
    let r = term_rig().await;
    let (agent, sb, _boot) = sandbox_on(&r, "stale-sb", "running").await;
    let grant = token::generate(TokenPrefix::TerminalStream);
    sqlx::query(
        "INSERT INTO terminal_stream_grants (id, agent_id, sandbox_id, boot_id, granted_by, grant_hash, expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6, now() + interval '30 seconds')",
    )
    .bind(Uuid::now_v7())
    .bind(agent)
    .bind(sb)
    .bind(Uuid::now_v7()) // not the sandbox's boot
    .bind(Uuid::now_v7())
    .bind(hex(&token::hash(grant.expose())))
    .execute(r.h.pool())
    .await
    .unwrap();
    let replica: Uuid = sqlx::query_scalar("SELECT id FROM runner_replicas LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r.redeem(grant.expose(), replica).await;
    assert_eq!((s, code(&v)), (409, "not_current"), "{v}");
}

/// A frozen agent's grant does not redeem (defense in depth: admission checked twice).
#[tokio::test]
async fn frozen_agent_grant_does_not_redeem() {
    let r = term_rig().await;
    let (agent, sb, boot) = sandbox_on(&r, "frozen-redeem", "running").await;
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 'x', frozen_authority = 'workspace_owner' WHERE id = $1",
    )
    .bind(agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let grant = token::generate(TokenPrefix::TerminalStream);
    sqlx::query(
        "INSERT INTO terminal_stream_grants (id, agent_id, sandbox_id, boot_id, granted_by, grant_hash, expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6, now() + interval '30 seconds')",
    )
    .bind(Uuid::now_v7())
    .bind(agent)
    .bind(sb)
    .bind(boot)
    .bind(Uuid::now_v7())
    .bind(hex(&token::hash(grant.expose())))
    .execute(r.h.pool())
    .await
    .unwrap();
    let replica: Uuid = sqlx::query_scalar("SELECT id FROM runner_replicas LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r.redeem(grant.expose(), replica).await;
    // The redemption re-runs `ensure_agent_active`: 423 `frozen` (defense in depth, the
    // same shape as the open).
    assert_eq!((s, code(&v)), (423, "frozen"), "{v}");
}

/// A port grant (`aux_`) never opens a terminal and vice versa (the kind rides the prefix).
#[tokio::test]
async fn terminal_kind_refuses_a_port_grant_prefix() {
    let r = term_rig().await;
    let replica: Uuid = sqlx::query_scalar("SELECT id FROM runner_replicas LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    // An `aux_`-shaped value with kind terminal: the prefix wins (the value is not in the
    // terminal table) → 403.
    let aux = token::generate(TokenPrefix::PreviewStream);
    let (s, v) = r.redeem(aux.expose(), replica).await;
    assert_eq!((s, code(&v)), (403, "grant_invalid"), "{v}");
}

/// The keep-awake stamp (C-620): an owned running sandbox gets `terminal_active_until`;
/// a foreign sandbox id is a no-op.
#[tokio::test]
async fn keep_awake_stamps_only_owned_running_sandboxes() {
    let r = term_rig().await;
    let (_agent, sb, _boot) = sandbox_on(&r, "awake-sb", "running").await;
    let replica: Uuid = sqlx::query_scalar("SELECT id FROM runner_replicas LIMIT 1")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let (s, v) = r
        .runner_call(
            "/v1/runner/terminal/keep-awake",
            json!({"sandbox_id": sb}),
            replica,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let awake: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT terminal_active_until FROM sandboxes WHERE id = $1")
            .bind(sb)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert!(awake.is_some(), "the stamp refreshed terminal_active_until");
    // A foreign sandbox id updates nothing (no row, no error).
    let (s, _) = r
        .runner_call(
            "/v1/runner/terminal/keep-awake",
            json!({"sandbox_id": Uuid::now_v7()}),
            replica,
        )
        .await;
    assert_eq!(s, 200);
}

// ---------------------------------------------------------------- the served leg (C-617)

/// Served-router plumbing: the WS extractor needs a real connection (`hyper::upgrade::
/// OnUpgrade` never exists on `oneshot`), so every check at/after admission dials a real
/// loopback server here. The runner side is the rig's registered `runner-a` (unreachable:
/// its :4108 does not resolve) unless a test points the replica at a local fake first.
mod served {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

    /// Serve the rig's router on 127.0.0.1:0; returns `(ws://…, http://…)` — the dial base
    /// and the API base the fake runner redeems against (kept per-test: a global would race
    /// under parallel test threads).
    pub(crate) async fn serve(r: &Rig) -> (String, String) {
        let app = r.h.app.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        });
        (format!("ws://{addr}"), format!("http://{addr}"))
    }

    /// The HTTP status of a dial (JSON refusal for anything but 101; a 101 is closed
    /// immediately — the caller only needs the status).
    pub(crate) async fn status(url: &str, cookie: &str, path: &str, origin: &str) -> u16 {
        let mut req: tokio_tungstenite::tungstenite::handshake::client::Request =
            format!("{url}{path}").into_client_request().unwrap();
        req.headers_mut().insert("cookie", cookie.parse().unwrap());
        req.headers_mut().insert("origin", origin.parse().unwrap());
        match connect_async(req).await {
            Ok((mut ws, _)) => {
                let _ = ws.close(None).await; // not the case under test; just hang up
                101
            }
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => resp.status().as_u16(),
            Err(e) => panic!("dial failed with a non-HTTP error: {e:?}"),
        }
    }

    /// The full dial for the byte-bridge test (cookie + Origin headers as the dashboard
    /// sends them).
    pub(crate) async fn dial(
        url: &str,
        cookie: &str,
        path: &str,
    ) -> Result<
        (
            WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
            tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
        ),
        tokio_tungstenite::tungstenite::Error,
    > {
        let mut req: tokio_tungstenite::tungstenite::handshake::client::Request =
            format!("{url}{path}").into_client_request().unwrap();
        req.headers_mut().insert("cookie", cookie.parse().unwrap());
        req.headers_mut().insert("origin", ORIGIN.parse().unwrap());
        connect_async(req).await
    }

    /// A fake runner that accepts any number of legs and echoes binary frames (no grant
    /// redemption: the caps tests only need live sockets).
    pub(crate) async fn fake_runner_multi(r: &Rig) -> tokio::task::JoinHandle<()> {
        let fkr = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fk = fkr.local_addr().unwrap().to_string();
        sqlx::query("UPDATE runner_replicas SET advertised_host = $1")
            .bind(fk)
            .execute(r.h.pool())
            .await
            .unwrap();
        tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::Message;
            while let Ok((tcp, _)) = fkr.accept().await {
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await else {
                        return;
                    };
                    while let Some(Ok(m)) = ws.next().await {
                        match m {
                            Message::Binary(b) => {
                                let _ = ws.send(Message::Binary(b)).await;
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                });
            }
        })
    }

    /// Point the rig's replica at a local fake runner listener and return its task. The fake
    /// is the API-facing end of the real runner: it takes the dial's `Bearer auy_…` grant,
    /// redeems it at `api` exactly as a runner's :4108 listener would (C-617), then
    /// optionally echoes binary frames back. Without the redemption the API's mint would
    /// stay unconsumed — the bridge test pins the single-use flow end to end.
    pub(crate) async fn fake_runner(r: &Rig, api: &str, echo: bool) -> tokio::task::JoinHandle<()> {
        use futures::{SinkExt, StreamExt};
        // The API base for the redemption: the same router, reached over the real socket.
        // `dial_api` is set by [`serve`]; without it (dumb-echo use) skip redemption.
        let fkr = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fk = fkr.local_addr().unwrap().to_string();
        sqlx::query("UPDATE runner_replicas SET advertised_host = $2 WHERE id = $1")
            .bind(
                sqlx::query_scalar::<_, Uuid>("SELECT id FROM runner_replicas LIMIT 1")
                    .fetch_one(r.h.pool())
                    .await
                    .unwrap(),
            )
            .bind(fk)
            .execute(r.h.pool())
            .await
            .unwrap();
        // The API URL the fake redeems against (served on the same rig).
        let api = api.to_owned();
        let runner_tok = r.tok.clone();
        let rep: Uuid = sqlx::query_scalar("SELECT id FROM runner_replicas LIMIT 1")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
        tokio::spawn(async move {
            #[allow(clippy::result_large_err)] // test plumbing: tungstenite's callback type
            async fn accept_with_auth(
                tcp: tokio::net::TcpStream,
                auth: &std::sync::Arc<std::sync::Mutex<String>>,
            ) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
                let auth2 = auth.clone();
                let callback = move |req: &tokio_tungstenite::tungstenite::http::Request<()>,
                                     resp: tokio_tungstenite::tungstenite::http::Response<()>| {
                    *auth2.lock().unwrap() = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    Ok(resp)
                };
                tokio_tungstenite::accept_hdr_async(tcp, callback)
                    .await
                    .unwrap()
            }
            let (tcp, _) = fkr.accept().await.unwrap();
            // Accept with the request captured (the bearer rides the upgrade headers).
            let auth = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let mut ws = accept_with_auth(tcp, &auth).await;
            // Redeem the captured `auy_` grant at the API exactly as a runner would: the
            // runner authenticates with its own anchor token; the grant rides the body
            // (C-617). 200 proves the upgrade's mint is single-use consumable at the runner.
            let bearer_value = ws_bearer(&auth.lock().unwrap().clone()).map(str::to_owned);
            if let Some(bearer_value) = bearer_value {
                let client = reqwest::Client::new();
                let anchor = runner_tok.clone();
                let _ = client
                    .post(format!("{api}/v1/runner/preview-streams/open"))
                    .header("authorization", format!("Bearer {anchor}"))
                    .header("x-aurora-replica", rep.to_string())
                    .json(&serde_json::json!({"grant": bearer_value, "kind": "terminal"}))
                    .send()
                    .await;
            }
            use tokio_tungstenite::tungstenite::Message;
            while let Some(Ok(m)) = ws.next().await {
                match m {
                    Message::Binary(b) if echo => {
                        let _ = ws.send(Message::Binary(b)).await; // echo
                    }
                    Message::Binary(_) => {}
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        })
    }
}

/// The full leg through the served router: the upgrade completes, bytes flow browser → API →
/// fake runner → browser, the mint happened and was redeemed exactly once (the API's own
/// redemption at the fake runner), and nothing byte-shaped persisted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn served_upgrade_bridges_bytes_and_consumes_the_grant() {
    let r = term_rig().await;
    let (url, api) = served::serve(&r).await;
    served::fake_runner(&r, &api, true).await; // echo + redeeming runner at the replica
    let (agent, _sb, _boot) = sandbox_on(&r, "bridge-sb", "running").await;

    let (mut ws, resp) = served::dial(&url, &r.cookie, &format!("/v1/agents/{agent}/terminal"))
        .await
        .expect("the upgrade must complete");
    assert_eq!(resp.status(), 101);

    // Browser → runner → browser (the fake runner echoes).
    ws.send(tokio_tungstenite::tungstenite::Message::binary(
        b"echo-through".to_vec(),
    ))
    .await
    .unwrap();
    let back = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("echo timed out")
        .unwrap()
        .unwrap();
    assert_eq!(
        back,
        tokio_tungstenite::tungstenite::Message::binary(b"echo-through".to_vec()),
        "{back:?}"
    );

    // The mint happened: one grant, consumed by the API's own redemption at the fake
    // runner's listener (the fake redeems concurrently with the bridge; poll briefly).
    let mut rows = r.grant_rows(agent).await;
    for _ in 0..60 {
        if rows.len() == 1 && rows[0] == (true, true) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        rows = r.grant_rows(agent).await;
    }
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0], (true, true), "minted, redeemed, unexpired");

    // The browser ends the leg with a close; the API passes it upstream and answers.
    ws.close(None).await.unwrap();
    let end = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("close timed out");
    assert!(
        matches!(end, Some(Ok(_)) | Some(Err(_)) | None),
        "the leg ends after the browser's close, got {end:?}"
    );

    // Nothing byte-shaped was persisted: no request-log row, no transcript event.
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM request_log WHERE path LIKE '%terminal%'")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 0, "terminal traffic is never request-logged");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM transcript_events WHERE payload::text LIKE '%echo-through%'",
    )
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0, "terminal bytes never reach the transcript");
}

/// The served refusals: no session cookie → 401, hostile Origin → 403 (both answered as
/// JSON pre-upgrade; tungstenite surfaces the HTTP response).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn served_upgrade_refuses_anonymous_and_cross_origin() {
    let r = term_rig().await;
    let (url, _api) = served::serve(&r).await;
    let (agent, _, _) = sandbox_on(&r, "served-anon", "running").await;
    let path = format!("/v1/agents/{agent}/terminal");

    // No cookie: 401, no upgrade.
    let s = served::status(&url, "aurora_session=aus_bogus", &path, ORIGIN).await;
    assert_eq!(s, 401, "no session, no upgrade");

    // Cross-origin: 403.
    let s = served::status(&url, &r.cookie, &path, "http://evil.example.org").await;
    assert_eq!(s, 403, "cross-origin, no upgrade");
}

fn hex(h: &[u8]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}
