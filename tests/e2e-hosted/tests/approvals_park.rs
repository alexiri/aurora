#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-011B-5 (D-133, D-134, D-264..D-267; C-820..C-839; SEC-74, SEC-75, SEC-76, SEC-102): approval
//! park-and-resume and Web Push, black box through the real binaries. Gated by AURORA_TEST_PODMAN=1;
//! run via `scripts/e2e/approvals-park.sh`.
//!
//! The fake harness runs the `[[http: …]]` directives of a turn and never repeats a request on its
//! own, so the *test* sends the identical directive as the repeat after an approval. The grant
//! matches agent + conversation + the request itself (C-821), not the turn, so this is the very
//! request a real agent repeats. Parking is switched on through the instance settings API
//! (`approvals.park.enabled`, applied live by the gateway); time is moved with the approval row's
//! own clocks where a real wait would take hours.
mod common;

use aurora_testkit::push::{MockPush, VapidExpect};
use aurora_testkit::wiremock::matchers::method;
use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
use common::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const PUSH_HOST: &str = "fcm.googleapis.com";
const PUSH_SUBJECT: &str = "mailto:ops@e2e.test";
const ADMIN: &str = "placement-admin@e2e.test";

async fn upstream() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("upstream-ok"))
        .mount(&s)
        .await;
    s
}

fn turn_id(v: &Value) -> String {
    v["turn"]["id"].as_str().unwrap().to_owned()
}

/// What every journey starts from: a signed-in owner, a mock upstream, a rule that requires an
/// approval with a two-hour deadline, and a conversation.
struct World {
    st: Stack,
    api: Api,
    up: MockServer,
    host: String,
    conv: String,
}

async fn world(o: Opts, extra_toml: &str) -> World {
    Stack::wipe();
    let st = Stack::start_with(o, extra_toml).await;
    let api = st.signup("owner@e2e.test").await;
    let up = upstream().await;
    let host = up.address().to_string();
    let id = api
        .ws_secret(json!({
            "name": "mock-api", "kind": "generic", "host_pattern": host.clone(),
            "injection": {"kind": "header", "name": "x-api-key", "format": "{value}"},
            "value": "sk-e2e-park-SECRET-value-123"
        }))
        .await;
    api.grant(&api.daneel, &id).await;
    // Parking first: a rule's two-hour deadline is above the hold maximum and only valid while on.
    park_switch(&st, &api, true).await;
    api.set_policy(
        "block",
        json!([{"name": "ask", "action": "allow", "targets": [{"type": "network", "host": host}],
                "require_approval": true, "approval_timeout_secs": 7200}]),
    )
    .await;
    let conv = api.direct().await;
    World {
        st,
        api,
        up,
        host,
        conv,
    }
}

/// Turn the parking switch on or off as the instance administrator.
async fn park_switch(st: &Stack, api: &Api, on: bool) {
    let admin = st.signup(ADMIN).await;
    let (s, cur) = admin.get("/v1/admin/settings/approvals").await;
    assert_eq!(s, 200, "{cur}");
    let (s, v) = admin
        .put(
            "/v1/admin/settings/approvals",
            json!({"generation": cur["generation"], "values": {"approvals.park.enabled": on}}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, lim) = api.get("/v1/policy/approval-limits").await;
    assert_eq!(s, 200, "{lim}");
    assert_eq!(lim["park"]["enabled"], on, "{lim}");
}

impl World {
    async fn park(&self, on: bool) {
        park_switch(&self.st, &self.api, on).await;
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.host)
    }

    /// Send `[[http: GET <path>]]` and wait for the turn to end `done`; returns the turn id.
    async fn request(&self, path: &str) -> String {
        let v = self
            .api
            .say(&self.conv, &format!("[[http: GET {}]]", self.url(path)))
            .await;
        let turn = turn_id(&v);
        let (s, e) = self
            .st
            .wait_turn(&turn, &["done", "failed", "aborted"], 180)
            .await;
        assert_eq!((s.as_str(), e), ("done", None), "{}", self.st.log_tails());
        turn
    }

    async fn transcript(&self) -> String {
        transcript_text(&self.api.events(&self.conv).await)
    }

    /// (P5-UX-16, C-983) The payloads of the transcript events `ty` (`approval.pending`,
    /// `approval.decided`, `approval.expired`) written for this approval, once `n` exist (the API's
    /// bus listener writes them, so this polls with a deadline).
    async fn approval_events(&self, ty: &str, id: &str, n: usize) -> Vec<Value> {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            let rows: Vec<(Value,)> = sqlx::query_as(
                "SELECT payload FROM transcript_events WHERE conversation_id = $1::uuid \
                 AND type = $2 AND payload->>'approval_id' = $3 ORDER BY seq",
            )
            .bind(&self.conv)
            .bind(ty)
            .bind(id)
            .fetch_all(self.st.pool())
            .await
            .unwrap();
            if rows.len() >= n || Instant::now() > end {
                return rows.into_iter().map(|r| r.0).collect();
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    async fn upstream_hits(&self) -> usize {
        self.up.received_requests().await.unwrap().len()
    }

    async fn pending(&self, path: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let (s, v) = self
                .api
                .get(&format!(
                    "/v1/workspaces/{}/approvals?status=pending",
                    self.api.ws
                ))
                .await;
            assert_eq!(s, 200, "{v}");
            if let Some(a) = v["items"]
                .as_array()
                .and_then(|a| a.iter().find(|a| a["path"] == path))
            {
                return a.clone();
            }
            assert!(
                Instant::now() < deadline,
                "no pending approval for {path}: {v}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn decide(&self, id: &str, decision: &str) -> Value {
        let (s, d) = self
            .api
            .post(
                &format!("/v1/approvals/{id}/decision"),
                json!({"decision": decision}),
            )
            .await;
        assert_eq!(s, 200, "{d}");
        d
    }

    async fn row(&self, id: &str) -> (String, Option<String>) {
        sqlx::query_as("SELECT status, resume_state FROM approvals WHERE id = $1::uuid")
            .bind(id)
            .fetch_one(self.st.pool())
            .await
            .unwrap()
    }

    async fn turns(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM turns WHERE conversation_id = $1::uuid")
            .bind(&self.conv)
            .fetch_one(self.st.pool())
            .await
            .unwrap()
    }

    /// The `n`-th system follow-up turn (the resume turn of a decision) has finished; returns its
    /// text and door.
    async fn followup(&self, n: usize) -> (String, Value) {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let rows: Vec<(String, String, Value)> = sqlx::query_as(
                "SELECT status, text, door FROM turns WHERE conversation_id = $1::uuid \
                 AND role = 'turn' AND speaker_kind = 'system' ORDER BY created_at",
            )
            .bind(&self.conv)
            .fetch_all(self.st.pool())
            .await
            .unwrap();
            if let Some((status, text, door)) = rows.get(n - 1) {
                assert_ne!(status, "failed", "{}", self.st.log_tails());
                if status == "done" {
                    return (text.clone(), door.clone());
                }
            }
            assert!(
                Instant::now() < deadline,
                "follow-up {n} never finished: {rows:?}\n{}",
                self.st.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// `(decision, status, resumed, approval_id)` of the request-log rows of `path`, once `n`
    /// are written (best effort rows land a moment after the request).
    async fn log(&self, path: &str, n: usize) -> Vec<(String, i32, bool, Option<String>)> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let rows: Vec<(String, i32, bool, Option<String>)> = sqlx::query_as(
                "SELECT decision, status, resumed, approval_id::text FROM request_log \
                 WHERE host = $1 AND path = $2 ORDER BY ts",
            )
            .bind(self.host.split(':').next().unwrap())
            .bind(path)
            .fetch_all(self.st.pool())
            .await
            .unwrap();
            if rows.len() >= n || Instant::now() > deadline {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

/// The journey: park, the agent's turn ends, the sandbox parks, approve, one system follow-up
/// wakes it, the identical repeat is forwarded exactly once, a second repeat parks anew.
#[tokio::test]
async fn p1_park_approve_resume_consumed_once() {
    if !enabled() {
        return;
    }
    let w = world(
        Opts {
            idle_stop_secs: Some(10),
            ..Opts::default()
        },
        "",
    )
    .await;

    // Parked: 403 approval_parked to the agent, the turn ends done, nothing reached the upstream.
    w.request("/parked").await;
    let a = w.pending("/parked").await;
    let id = a["id"].as_str().unwrap().to_owned();
    assert_eq!(a["parked"], true, "{a}");
    let t = w.transcript().await;
    assert!(t.contains("approval_parked"), "{t}");
    assert_eq!(w.upstream_hits().await, 0, "parked before approval");
    // (C-983) The conversation shows the question where the agent paused: one `approval.pending`,
    // marked parked, with the sentence and when it expires.
    let pending = w.approval_events("approval.pending", &id, 1).await;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["parked"], true, "{pending:?}");
    assert!(
        pending[0]["summary"]
            .as_str()
            .unwrap()
            .contains(" wants to "),
        "{pending:?}"
    );
    assert!(pending[0]["expires_at"].is_string(), "{pending:?}");
    // The identical request while pending is the same approval: nothing new is created.
    w.request("/parked").await;
    let (_, list) = w
        .api
        .get(&format!(
            "/v1/workspaces/{}/approvals?status=pending",
            w.api.ws
        ))
        .await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(w.upstream_hits().await, 0);
    // A parked approval is not a keep-awake reason: the sandbox parks on idle.
    w.st.wait_sandbox(&w.api, "stopped", 120).await;

    // Approve: a ready single-use grant and one system follow-up (door ref `approval:<id>`).
    let d = w.decide(&id, "approve").await;
    assert_eq!(d["resume"]["state"], "ready", "{d}");
    // The answer appears in the conversation, once (the repeat requests above made no second card).
    let decided = w.approval_events("approval.decided", &id, 1).await;
    assert_eq!(decided.len(), 1, "{decided:?}");
    assert_eq!(decided[0]["decision"], "approve", "{decided:?}");
    assert!(decided[0]["resume_expires_at"].is_string(), "{decided:?}");
    assert_eq!(
        w.approval_events("approval.pending", &id, 1).await.len(),
        1,
        "one card however often the request repeated"
    );
    let (text, door) = w.followup(1).await;
    assert!(
        text.contains("a person approved a request you were waiting for"),
        "{text}"
    );
    assert_eq!(door["kind"], "system", "{door}");
    assert_eq!(door["ref"], format!("approval:{id}"), "{door}");
    assert_eq!(w.row(&id).await, ("approved".into(), Some("ready".into())));
    assert_eq!(w.upstream_hits().await, 0, "approval alone sends nothing");

    // The identical repeat goes through, once.
    w.request("/parked").await;
    assert!(w.transcript().await.contains("http 200"));
    assert_eq!(w.upstream_hits().await, 1);
    assert_eq!(
        w.row(&id).await,
        ("approved".into(), Some("consumed".into()))
    );
    let log = w.log("/parked", 3).await;
    assert!(
        log.iter()
            .any(|r| r.0 == "approved" && r.1 == 200 && r.2 && r.3.as_deref() == Some(&id)),
        "approved + resumed row: {log:?}"
    );
    assert_eq!(log.iter().filter(|r| r.2).count(), 1, "{log:?}");

    // Single use: the next identical request parks anew and is not forwarded.
    w.request("/parked").await;
    assert_eq!(w.upstream_hits().await, 1, "a second repeat is refused");
    let again = w.pending("/parked").await;
    assert_ne!(again["id"].as_str().unwrap(), id, "a new approval");
    assert_eq!(again["parked"], true);
    w.st.finish().await;
}

/// Deny: the "denied" follow-up, the cooldown answer to the repeat, no new approval.
#[tokio::test]
async fn p2_deny_follow_up_and_cooldown() {
    if !enabled() {
        return;
    }
    let w = world(Opts::default(), "").await;
    w.request("/denied").await;
    let id = w.pending("/denied").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    w.decide(&id, "deny").await;
    let decided = w.approval_events("approval.decided", &id, 1).await;
    assert_eq!(decided.len(), 1, "{decided:?}");
    assert_eq!(decided[0]["decision"], "deny", "{decided:?}");
    let (text, _) = w.followup(1).await;
    assert!(
        text.contains("a person denied a request you were waiting for"),
        "{text}"
    );
    assert_eq!(w.row(&id).await, ("denied".into(), None));
    // The repeat inside the cooldown is refused with the manual-denial answer; no new approval.
    w.request("/denied").await;
    assert!(w.transcript().await.contains("manual_approval_denied"));
    let (_, list) = w
        .api
        .get(&format!(
            "/v1/workspaces/{}/approvals?status=pending",
            w.api.ws
        ))
        .await;
    assert!(list["items"].as_array().unwrap().is_empty(), "{list}");
    assert_eq!(w.upstream_hits().await, 0);
    let log = w.log("/denied", 2).await;
    assert!(log.iter().any(|r| r.0 == "approval_denied"), "{log:?}");
    w.st.finish().await;
}

/// Expiry ends the story without a turn: a deadline that passes unanswered, an approval whose
/// resume window passes unused. The clocks are moved on the rows; the real jobs do the rest.
#[tokio::test]
async fn p3_deadline_and_resume_window_expire_without_a_turn() {
    if !enabled() {
        return;
    }
    let w = world(Opts::default(), "").await;

    // The deadline passes: `expired`, a transcript event, no turn, nothing sent.
    w.request("/late").await;
    let id = w.pending("/late").await["id"].as_str().unwrap().to_owned();
    let turns = w.turns().await;
    sqlx::query("UPDATE approvals SET deadline = now() - interval '1 second' WHERE id = $1::uuid")
        .bind(&id)
        .execute(w.st.pool())
        .await
        .unwrap();
    let end = Instant::now() + Duration::from_secs(40);
    while w.row(&id).await.0 != "expired" {
        assert!(Instant::now() < end, "the expiry job never ran");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(w.turns().await, turns, "expiry starts no turn");
    assert_eq!(w.upstream_hits().await, 0);
    // The card runs out in the conversation: `approval.expired` for the deadline.
    let expired = w.approval_events("approval.expired", &id, 1).await;
    assert_eq!(expired.len(), 1, "{expired:?}");
    assert_eq!(expired[0]["reason"], "deadline", "{expired:?}");

    // The resume window passes unused: `expired`, a notice, no further turn; the repeat parks anew.
    w.request("/window").await;
    let id = w.pending("/window").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    w.decide(&id, "approve").await;
    w.followup(1).await;
    let turns = w.turns().await;
    sqlx::query(
        "UPDATE approvals SET resume_expires_at = now() - interval '1 second' WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(w.st.pool())
    .await
    .unwrap();
    let end = Instant::now() + Duration::from_secs(40);
    while w.row(&id).await.1.as_deref() != Some("expired") {
        assert!(Instant::now() < end, "the resume-window job never ran");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(w.turns().await, turns, "window expiry starts no turn");
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM transcript_events WHERE conversation_id = $1::uuid \
         AND type = 'notice' AND payload->>'code' = 'approval_window_ended'",
    )
    .bind(&w.conv)
    .fetch_one(w.st.pool())
    .await
    .unwrap();
    assert_eq!(notices, 1);
    // After `approval.pending` and `approval.decided`, the card ends with `resume_window`.
    let expired = w.approval_events("approval.expired", &id, 1).await;
    assert_eq!(expired.len(), 1, "{expired:?}");
    assert_eq!(expired[0]["reason"], "resume_window", "{expired:?}");
    assert_eq!(w.approval_events("approval.pending", &id, 1).await.len(), 1);
    assert_eq!(w.approval_events("approval.decided", &id, 1).await.len(), 1);
    w.request("/window").await;
    assert_eq!(
        w.upstream_hits().await,
        0,
        "an expired grant matches nothing"
    );
    assert_ne!(w.pending("/window").await["id"].as_str().unwrap(), id);
    w.st.finish().await;
}

/// A freeze revokes a ready grant (the repeat is not matched); with the switch turned off a ready
/// grant is still consumed exactly once (D-264(b)).
#[tokio::test]
async fn p4_freeze_revokes_the_grant_and_switch_off_still_consumes_once() {
    if !enabled() {
        return;
    }
    let w = world(Opts::default(), "").await;

    w.request("/frozen").await;
    let id = w.pending("/frozen").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    w.decide(&id, "approve").await;
    w.followup(1).await;
    let (s, b) = w
        .api
        .post(
            &format!("/v1/agents/{}/freeze", w.api.daneel),
            json!({"reason": "e2e drill"}),
        )
        .await;
    assert_eq!(s, 200, "{b}");
    assert_eq!(
        w.row(&id).await,
        ("approved".into(), Some("revoked".into()))
    );
    let (s, b) = w
        .api
        .post(&format!("/v1/agents/{}/unfreeze", w.api.daneel), json!({}))
        .await;
    assert_eq!(s, 200, "{b}");
    // The grant is gone: the identical repeat is not matched and parks anew.
    w.request("/frozen").await;
    assert_eq!(w.upstream_hits().await, 0, "a revoked grant is not matched");
    assert_ne!(w.pending("/frozen").await["id"].as_str().unwrap(), id);

    // Approve another, then switch parking off: a probe request is now held (not parked), which
    // proves the gateway saw the switch; the ready grant is still consumed, once.
    w.request("/switch").await;
    let id = w.pending("/switch").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    w.decide(&id, "approve").await;
    w.followup(2).await;
    w.park(false).await;
    let say = w
        .api
        .say(&w.conv, &format!("[[http: GET {}]]", w.url("/probe")))
        .await;
    let probe_turn = turn_id(&say);
    let held = w.pending("/probe").await;
    assert_eq!(held["parked"], false, "off means held: {held}");
    w.decide(held["id"].as_str().unwrap(), "approve").await;
    let (st, e) = w.st.wait_turn(&probe_turn, &["done", "failed"], 120).await;
    assert_eq!((st.as_str(), e), ("done", None), "{}", w.st.log_tails());
    let hits = w.upstream_hits().await;
    w.request("/switch").await;
    assert_eq!(w.upstream_hits().await, hits + 1, "the grant was consumed");
    assert_eq!(
        w.row(&id).await,
        ("approved".into(), Some("consumed".into()))
    );
    w.request("/switch").await;
    assert_eq!(w.upstream_hits().await, hits + 1, "and only once");
    w.st.finish().await;
}

async fn push_claims(w: &World, approval: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT stage FROM approval_push_claims WHERE approval_id = $1::uuid ORDER BY stage",
    )
    .bind(approval)
    .fetch_all(w.st.pool())
    .await
    .unwrap()
}

/// The push leg: a decider with a registered subscription and no open stream gets one push for a
/// parked approval and one reminder at half time, readable by the mock and free of request text;
/// an open `approvals/stream` suppresses it; sign-out deletes the subscription.
#[tokio::test]
async fn p5_web_push_to_a_decider_without_an_open_tab() {
    if !enabled() {
        return;
    }
    let mock = MockPush::start().await;
    let toml = format!(
        r#"
[push]
enabled = true
allowed_endpoint_hosts = ["{PUSH_HOST}"]

[push.vapid]
subject = "{PUSH_SUBJECT}"

[push.dev_endpoint_overrides]
"{PUSH_HOST}" = "{}"
"#,
        mock.url()
    );
    let w = world(Opts::default(), &toml).await;

    // The decider's browser: its own session, keys made by the test, an allow-listed endpoint.
    let browser = w.api.relogin("owner@e2e.test").await;
    let (s, cfg) = browser.get("/v1/push/config").await;
    assert_eq!(s, 200, "{cfg}");
    assert_eq!(cfg["available"], true, "{cfg}");
    let key = cfg["vapid_public_key"].as_str().unwrap().to_owned();
    mock.expect_vapid(VapidExpect {
        aud: format!("https://{PUSH_HOST}"),
        subject: PUSH_SUBJECT.into(),
        public_key: key.clone(),
    });
    let keys = mock.subscribe("/push/laptop");
    let (s, sub) = browser
        .post(
            "/v1/me/push-subscriptions",
            json!({
                "endpoint": format!("https://{PUSH_HOST}/push/laptop"),
                "keys": {"p256dh": keys.p256dh(), "auth": keys.auth()},
                "label": "Laptop", "show_details": false, "vapid_public_key": key,
            }),
        )
        .await;
    assert_eq!(s, 201, "{sub}");

    // A parked approval whose path and query carry canaries: one push, decryptable, no request text.
    w.request("/CANARYPATH?q=CANARYQUERY").await;
    let a = w.pending("/CANARYPATH").await;
    let id = a["id"].as_str().unwrap().to_owned();
    let got = mock.wait_for(1, Duration::from_secs(60)).await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].vapid, Ok(()), "{:?}", got[0]);
    assert_eq!(got[0].path, "/push/laptop");
    let p = got[0].json().expect("the mock decrypts the payload");
    assert_eq!(p["v"], 1);
    assert_eq!(p["type"], "approval.pending");
    assert_eq!(p["approval_id"], id.as_str());
    assert_eq!(p["parked"], true);
    assert!(
        p["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/approvals?focus={id}")),
        "{p}"
    );
    let plain = String::from_utf8(got[0].plaintext.clone().unwrap()).unwrap();
    assert!(
        !plain.contains("CANARY"),
        "no request text in the push: {plain}"
    );
    assert!(!plain.contains(&w.host), "no host in the push: {plain}");

    // Half the time has passed: one reminder, once (the claim row is the proof it is not repeated).
    sqlx::query(
        "UPDATE approvals SET created_at = now() - interval '2 hours', \
         deadline = now() + interval '1 minute' WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(w.st.pool())
    .await
    .unwrap();
    let got = mock.wait_for(2, Duration::from_secs(60)).await;
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[1].json().unwrap()["type"], "approval.reminder");
    assert_eq!(push_claims(&w, &id).await, ["initial", "reminder"]);
    w.decide(&id, "deny").await;
    w.followup(1).await;

    // An open approvals stream is a live surface: no push while it is open. (The one bounded
    // negative wait of the leg: several sweeps of the delivery job run inside it.)
    let stream = w
        .api
        .raw("/v1/approvals/stream")
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    w.request("/watched").await;
    let watched = w.pending("/watched").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(mock.received().len(), 2, "a watching tab gets no push");
    assert!(push_claims(&w, &watched).await.is_empty());
    w.decide(&watched, "deny").await;
    w.followup(2).await;
    drop(stream);

    // Sign-out of the browser's session deletes its subscription; the next approval sends nothing.
    let (s, out) = browser.post("/v1/auth/logout", json!({})).await;
    assert!(s == 200 || s == 204, "{s} {out}");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM push_subscriptions")
        .fetch_one(w.st.pool())
        .await
        .unwrap();
    assert_eq!(left, 0, "sign-out deleted the subscription");
    w.request("/after-signout").await;
    let later = w.pending("/after-signout").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    tokio::time::sleep(Duration::from_secs(25)).await;
    assert_eq!(mock.received().len(), 2, "nothing after sign-out");
    assert!(push_claims(&w, &later).await.is_empty());
    w.st.finish().await;
}
