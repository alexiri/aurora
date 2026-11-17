#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-UTOK (D-281..D-284, SEC-112, SEC-113; docs/18 §User tokens): a person's chat-platform token
//! and `send_as_me`. The connector is played over the real connector plane (`/v1/connector/ops/…`
//! with a connector token and a replica), so the claim path, the per-op copy and the terminal
//! discard run for real; the secret store is a memory store, so "no version left" is countable.
//!
//! The pasted value used here is a canary: it must appear in no response, audit row, queued op or
//! stored row, only inside the store and in the claimed operation's `user_credential`.

use crate::common::*;
use aurora_api::domains::action_approvals::service as approvals;
use aurora_api::domains::tools::{ToolCallContext, ToolContext, registry};
use aurora_core::token::{self, TokenPrefix};
use aurora_secrets::{MemoryStore, SecretVault, StoreRegistry};
use http::Method;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const CANARY: &str = "canary-pat-4f9c1e7a3b2d4c6f8e0a1b3c5d7e9f02";
const OTHER_CANARY: &str = "canary-pat-other-0a1b2c3d4e5f60718293a4b5c6d7";

struct W {
    h: Harness,
    mem: Arc<MemoryStore>,
    org: Uuid,
    ws: Uuid,
    integ: Uuid,
    agent: Uuid,
    owner: Uuid,
    owner_cookie: String,
    approver: Uuid,
    approver_cookie: String,
    conn_tok: String,
    replica: Uuid,
    conv: Uuid,
    link: Uuid,
    turn: Uuid,
}

fn hd(c: &str) -> [(&str, &str); 2] {
    [("cookie", c), ("origin", ORIGIN)]
}

fn base64_key() -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [b'k'; 32])
}

fn caps_manifest(user_tokens: bool) -> Value {
    json!({"capabilities": {"threads": true, "user_tokens": user_tokens}})
}

impl W {
    async fn new() -> W {
        Self::with(true).await
    }

    async fn with(user_tokens: bool) -> W {
        let mut h = Harness::new().await;
        let keyring =
            Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{}", base64_key()), "k1").unwrap());
        let mut sr = StoreRegistry::from_config(&h.state.config.secrets, keyring).unwrap();
        let mem = Arc::new(MemoryStore::new("memory"));
        sr.register(mem.clone());
        sr.set_default("memory").unwrap();
        h.state = h.state.clone().with_vault(Arc::new(SecretVault::new(sr)));
        h.app = test_router(h.state.clone());
        let org = h.org().await;
        let ws = h.workspace(org).await;
        let owner = h.user("owner@x.io", false).await;
        let approver = h.user("approver@x.io", false).await;
        for (u, role) in [(owner, "member"), (approver, "approver")] {
            h.member(org, u, "member", "active").await;
            h.bind_ws(ws, u, role).await;
        }
        let owner_cookie = cookie(&h.good_session(owner).await.1);
        let approver_cookie = cookie(&h.good_session(approver).await.1);
        let p = h.pool();
        let conn_tok = token::generate(TokenPrefix::Connector).expose().clone();
        sqlx::query("INSERT INTO connectors (id, token_hash, display_name, manifest, status) VALUES ('chat', $1, 'chat', $2, 'active')")
            .bind(&token::hash(&conn_tok)[..])
            .bind(caps_manifest(user_tokens))
            .execute(p)
            .await
            .unwrap();
        let replica = Uuid::now_v7();
        sqlx::query("INSERT INTO connector_replicas (id, connector_id, version, last_seen_at) VALUES ($1,'chat','1', now())")
            .bind(replica)
            .execute(p)
            .await
            .unwrap();
        let integ = Uuid::now_v7();
        sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status, owner_replica_id, owner_lease_until) VALUES ($1,$2,'chat','T1','Team One','{}','x','active',$3, now() + interval '1 hour')")
            .bind(integ).bind(org).bind(replica)
            .execute(p).await.unwrap();
        let agent = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief, proactive_messaging) VALUES ($1,$2,$3,'bot','Bot','byo','medium','','ask')")
            .bind(agent).bind(org).bind(ws)
            .execute(p).await.unwrap();
        sqlx::query("INSERT INTO channel_presences (id, agent_id, integration_id, mode, status) VALUES ($1,$2,$3,'shared','active')")
            .bind(Uuid::now_v7()).bind(agent).bind(integ)
            .execute(p).await.unwrap();
        let link = Uuid::now_v7();
        sqlx::query("INSERT INTO user_links (id, integration_id, platform_user_ref, user_id, method) VALUES ($1,$2,'U_OWNER',$3,'admin')")
            .bind(link).bind(integ).bind(owner)
            .execute(p).await.unwrap();
        sqlx::query("INSERT INTO user_links (id, integration_id, platform_user_ref, user_id, method) VALUES ($1,$2,'U_APPROVER',$3,'admin')")
            .bind(Uuid::now_v7()).bind(integ).bind(approver)
            .execute(p).await.unwrap();
        let mut w = W {
            h,
            mem,
            org,
            ws,
            integ,
            agent,
            owner,
            owner_cookie,
            approver,
            approver_cookie,
            conn_tok,
            replica,
            conv: Uuid::nil(),
            link: Uuid::nil(),
            turn: Uuid::nil(),
        };
        // The owner's own direct conversation with the agent, reached through a DM thread, and a
        // running turn the owner wrote.
        let (conv, link, turn) = w
            .thread("direct", "DM_OWNER", "", w.owner, json!({"kind": "thread"}))
            .await;
        w.conv = conv;
        w.link = link;
        w.turn = turn;
        w
    }

    /// A conversation of `kind` with a thread link and one running turn spoken by `speaker`
    /// through `door` (the thread link id is added for a `thread` door).
    async fn thread(
        &self,
        kind: &str,
        space: &str,
        thread: &str,
        speaker: Uuid,
        door: Value,
    ) -> (Uuid, Uuid, Uuid) {
        self.thread_for(self.agent, kind, space, thread, speaker, door)
            .await
    }

    async fn thread_for(
        &self,
        agent: Uuid,
        kind: &str,
        space: &str,
        thread: &str,
        speaker: Uuid,
        mut door: Value,
    ) -> (Uuid, Uuid, Uuid) {
        let p = self.h.pool();
        let conv = Uuid::now_v7();
        let direct = (kind == "direct").then_some(speaker);
        sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id, last_activity_at) VALUES ($1,$2,$3,$4,$5,$6, now())")
            .bind(conv).bind(agent).bind(self.ws).bind(kind).bind(conv.to_string()).bind(direct)
            .execute(p).await.unwrap();
        let link = Uuid::now_v7();
        sqlx::query("INSERT INTO thread_links (id, integration_id, agent_id, conversation_id, kind, space_ref, thread_ref, sender) VALUES ($1,$2,$3,$4,$5,$6,$7,'\"shared\"')")
            .bind(link).bind(self.integ).bind(agent).bind(conv)
            .bind(if kind == "direct" { "direct" } else { "group" })
            .bind(space).bind(thread)
            .execute(p).await.unwrap();
        if door["kind"] == "thread" {
            door["thread_link_id"] = json!(link);
        }
        let turn = self.add_turn(agent, conv, speaker, "person", door).await;
        (conv, link, turn)
    }

    async fn add_turn(
        &self,
        agent: Uuid,
        conv: Uuid,
        speaker: Uuid,
        kind: &str,
        door: Value,
    ) -> Uuid {
        let turn = Uuid::now_v7();
        sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_user_id, speaker_label, text) VALUES ($1,$2,$3,'turn','done',$4,$5,$6,'x','post this')")
            .bind(turn).bind(conv).bind(agent).bind(door).bind(kind).bind(speaker)
            .execute(self.h.pool()).await.unwrap();
        turn
    }

    // ---- the connector plane ----

    async fn plane(&self, path: &str, body: Value) -> (u16, Value) {
        let b = bearer(&self.conn_tok);
        let r = self.replica.to_string();
        let (s, _, v) = self
            .h
            .send(
                Method::POST,
                &format!("/v1/connector{path}"),
                &[
                    ("authorization", b.as_str()),
                    ("x-aurora-replica", r.as_str()),
                ],
                Some(body),
            )
            .await;
        (s.as_u16(), v)
    }

    async fn claim(&self) -> Vec<Value> {
        let (s, v) = self
            .plane("/ops/claim", json!({"max": 20, "wait_secs": 0}))
            .await;
        assert_eq!(s, 200, "{v}");
        v["ops"].as_array().unwrap().clone()
    }

    async fn report(&self, op: &Value, ok: bool, data: Value, code: &str) {
        let res = if ok {
            json!({"op_id": op["op_id"], "outcome": "ok", "data": data})
        } else {
            json!({"op_id": op["op_id"], "outcome": "error",
                   "error": {"code": code, "message": "no"}})
        };
        let (s, v) = self.plane("/ops/results", json!({"results": [res]})).await;
        assert_eq!(s, 200, "{v}");
    }

    /// Plays the connector for one verify: answers `user_token.verify` ops until `n` were served.
    /// `who` maps a token to the platform user it belongs to (`None`: the platform rejects it).
    /// Returns the claimed operations (their `user_credential` is what the connector saw).
    async fn serve_verify(&self, n: usize, who: &[(&str, Option<(&str, &str)>)]) -> Vec<Value> {
        let deadline = std::time::Instant::now() + Duration::from_secs(12);
        let mut seen = Vec::new();
        while seen.len() < n && std::time::Instant::now() < deadline {
            for op in self.claim().await {
                assert_eq!(op["kind"], "user_token.verify", "{op}");
                let tok = op["user_credential"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                match who.iter().find(|(t, _)| *t == tok) {
                    Some((_, Some((r, h)))) => {
                        self.report(&op, true, json!({"platform_user_ref": r, "handle": h}), "")
                            .await;
                    }
                    _ => {
                        self.report(&op, false, Value::Null, "credential_dead")
                            .await
                    }
                }
                seen.push(op);
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        seen
    }

    // ---- the routes ----

    async fn call(&self, m: Method, uri: &str, cookie: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self.h.send(m, uri, &hd(cookie), body).await;
        (s.as_u16(), v)
    }

    /// Add `token` for the owner while the connector answers `who`.
    async fn add(&self, token: &str, who: &[(&str, Option<(&str, &str)>)]) -> (u16, Value) {
        let body = json!({"integration_id": self.integ, "token": token, "label": "work"});
        let (res, _) = tokio::join!(
            self.call(
                Method::POST,
                "/v1/me/user-tokens",
                &self.owner_cookie,
                Some(body)
            ),
            self.serve_verify(1, who)
        );
        res
    }

    async fn add_owner_token(&self) -> Uuid {
        let (s, v) = self
            .add(CANARY, &[(CANARY, Some(("U_OWNER", "owner-handle")))])
            .await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().unwrap().parse().unwrap()
    }

    async fn grant(&self, token: Uuid, agent: Uuid) -> u16 {
        self.call(
            Method::PUT,
            &format!("/v1/me/user-tokens/{token}/agents/{agent}"),
            &self.owner_cookie,
            None,
        )
        .await
        .0
    }

    /// A token with the grant of the main agent, ready to be used.
    async fn ready(&self) -> Uuid {
        let t = self.add_owner_token().await;
        assert_eq!(self.grant(t, self.agent).await, 204);
        t
    }

    // ---- the tool ----

    fn ctx(&self, agent: Uuid, conv: Uuid, turn: Option<Uuid>, call: &str) -> ToolCallContext {
        ToolCallContext {
            agent: ToolContext {
                pool: self.h.pool().clone(),
                agent_id: agent,
                workspace_id: self.ws,
                org_id: self.org,
            },
            conversation_id: conv,
            turn_id: turn,
            call_id: call.into(),
            state: self.h.state.clone(),
        }
    }

    async fn send_as_me_at(
        &self,
        agent: Uuid,
        conv: Uuid,
        turn: Option<Uuid>,
        text: &str,
    ) -> Value {
        let r = registry()
            .call(
                &self.ctx(agent, conv, turn, &Uuid::now_v7().to_string()),
                "send_as_me",
                json!({"to": "here", "text": text}),
                Duration::from_secs(20),
            )
            .await;
        serde_json::to_value(&r).unwrap()
    }

    async fn send_as_me(&self, text: &str) -> Value {
        self.send_as_me_at(self.agent, self.conv, Some(self.turn), text)
            .await
    }

    /// The owner's `user_links` row on the integration.
    async fn link_id(&self) -> Uuid {
        sqlx::query_scalar("SELECT id FROM user_links WHERE user_id = $1")
            .bind(self.owner)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn approvals(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM action_approvals WHERE action_key = 'send_as_me'")
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn posts(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM channel_ops WHERE kind = 'message.post'")
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn decide(&self, approval: Uuid, cookie: &str, decision: &str) -> (u16, Value) {
        self.call(
            Method::POST,
            &format!("/v1/action-approvals/{approval}/decision"),
            cookie,
            Some(json!({"decision": decision})),
        )
        .await
    }

    /// Ask for a post and return the approval id.
    async fn propose(&self, text: &str) -> Uuid {
        let v = self.send_as_me(text).await;
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(
            v["content"][0]["value"]["status"], "pending_approval",
            "{v}"
        );
        v["content"][0]["value"]["approval_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    /// Propose and approve as the owner: queues the post.
    async fn approved(&self, text: &str) -> Uuid {
        let a = self.propose(text).await;
        let (s, v) = self.decide(a, &self.owner_cookie, "approve").await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["status"], "executed", "{v}");
        a
    }

    async fn status(&self, token: Uuid) -> (String, Option<String>) {
        sqlx::query_as("SELECT status, token_ref FROM user_tokens WHERE id = $1")
            .bind(token)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn audit(&self, action: &str) -> Vec<(Value, bool)> {
        sqlx::query_as::<_, (Value, bool)>(
            "SELECT metadata, critical FROM audit_log WHERE action = $1 ORDER BY created_at",
        )
        .bind(action)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }

    /// Wait for best-effort audit rows (`user_token.send`, `.grant`, …) written by a spawned task.
    async fn wait_audit(&self, action: &str, n: usize) -> Vec<(Value, bool)> {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let rows = self.audit(action).await;
            if rows.len() >= n || std::time::Instant::now() > deadline {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

fn refusal(v: &Value) -> String {
    assert_eq!(v["ok"], false, "{v}");
    v["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

// ===================================================================== add and verify

#[tokio::test]
async fn add_verifies_against_the_callers_own_link_and_never_returns_the_value() {
    let w = W::new().await;
    let (s, v) = w
        .add(CANARY, &[(CANARY, Some(("U_OWNER", "owner-handle")))])
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "active");
    assert_eq!(v["label"], "work");
    assert_eq!(v["platform_user_label"], "owner-handle");
    assert_eq!(v["integration"]["id"], json!(w.integ));
    assert_eq!(v["agents"], json!([]));
    assert!(
        !v.to_string().contains(CANARY),
        "the value is never returned"
    );
    // The listing shows the same, without the value.
    let (s, l) = w
        .call(Method::GET, "/v1/me/user-tokens", &w.owner_cookie, None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(l["tokens"].as_array().unwrap().len(), 1);
    assert!(!l.to_string().contains(CANARY));
    // Not in the row, a queued op's payload or an audit row: only inside the store.
    let (status, token_ref) = w.status(v["id"].as_str().unwrap().parse().unwrap()).await;
    assert_eq!(status, "active");
    assert!(!token_ref.unwrap().contains(CANARY));
    let ops: String = sqlx::query_scalar("SELECT coalesce(string_agg(payload::text || coalesce(result::text, ''), ' '), '') FROM channel_ops")
        .fetch_one(w.h.pool()).await.unwrap();
    assert!(!ops.contains(CANARY), "no plaintext in a queued op");
    let audit: String = sqlx::query_scalar(
        "SELECT coalesce(string_agg(metadata::text || target_id, ' '), '') FROM audit_log",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(!audit.contains(CANARY));
    assert_eq!(w.wait_audit("user_token.create", 1).await.len(), 1);
    // The verify op's copy is gone at its terminal state: one version (the token) is left.
    assert_eq!(w.mem.managed_count(), 1);
}

#[tokio::test]
async fn the_connector_saw_the_value_only_in_user_credential() {
    let w = W::new().await;
    let body = json!({"integration_id": w.integ, "token": CANARY});
    let (res, seen) = tokio::join!(
        w.call(
            Method::POST,
            "/v1/me/user-tokens",
            &w.owner_cookie,
            Some(body)
        ),
        w.serve_verify(1, &[(CANARY, Some(("U_OWNER", "o")))])
    );
    assert_eq!(res.0, 201, "{:?}", res.1);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["user_credential"], CANARY);
    // The stored payload keys never reach the connector.
    for k in ["user_credential_ref", "user_org_id", "user_meta"] {
        assert!(
            seen[0]["payload"].get(k).is_none(),
            "{k} leaked to the connector"
        );
    }
    assert!(!seen[0]["payload"].to_string().contains(CANARY));
}

#[tokio::test]
async fn another_platform_user_is_refused_and_the_sealed_value_discarded() {
    let w = W::new().await;
    let (s, v) = w
        .add(
            OTHER_CANARY,
            &[(OTHER_CANARY, Some(("U_SOMEONE_ELSE", "x")))],
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(code(&v), "user_token_mismatch");
    // Another *Aurora* user's own platform account is no better.
    let (s, v) = w
        .add(OTHER_CANARY, &[(OTHER_CANARY, Some(("U_APPROVER", "x")))])
        .await;
    assert_eq!((s, code(&v)), (422, "user_token_mismatch"), "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM user_tokens")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "nothing stored");
    assert_eq!(w.mem.managed_count(), 0, "no version left in the store");
    assert!(!v.to_string().contains(OTHER_CANARY));
}

#[tokio::test]
async fn a_token_the_platform_rejects_is_refused() {
    let w = W::new().await;
    let (s, v) = w.add(CANARY, &[(CANARY, None)]).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(code(&v), "user_token_invalid");
    assert_eq!(w.mem.managed_count(), 0);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM user_tokens")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn the_sixth_token_is_refused_and_a_down_store_stores_nothing() {
    let w = W::new().await;
    for i in 0..5 {
        sqlx::query("INSERT INTO user_tokens (id, user_id, integration_id, platform_user_ref, platform_user_label, token_ref, status) VALUES ($1,$2,$3,$4,'x','r','active')")
            .bind(Uuid::now_v7()).bind(w.owner).bind(w.integ).bind(format!("U{i}"))
            .execute(w.h.pool()).await.unwrap();
    }
    let (s, v) = w.add(CANARY, &[(CANARY, Some(("U_OWNER", "o")))]).await;
    assert_eq!((s, code(&v)), (409, "user_token_limit"), "{v}");
    assert_eq!(w.mem.managed_count(), 0);

    let w = W::new().await;
    w.mem.set_unavailable(true);
    let body = json!({"integration_id": w.integ, "token": CANARY});
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/me/user-tokens",
            &w.owner_cookie,
            Some(body),
        )
        .await;
    assert_eq!((s, code(&v)), (503, "secret_store_unavailable"), "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM user_tokens")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn unsupported_unlinked_and_foreign_requests_are_refused() {
    // The connector does not declare `user_tokens`.
    let w = W::with(false).await;
    let body = json!({"integration_id": w.integ, "token": CANARY});
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/me/user-tokens",
            &w.owner_cookie,
            Some(body),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "user_tokens_unsupported"), "{v}");
    // No link on the integration: nothing to compare the token with.
    let w = W::new().await;
    sqlx::query("DELETE FROM user_links WHERE user_id = $1")
        .bind(w.owner)
        .execute(w.h.pool())
        .await
        .unwrap();
    let body = json!({"integration_id": w.integ, "token": CANARY});
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/me/user-tokens",
            &w.owner_cookie,
            Some(body),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "user_link_required"), "{v}");
    // Empty and over-long values are bad requests; an API key cannot add (session only).
    let w = W::new().await;
    let body = json!({"integration_id": w.integ, "token": "x".repeat(257)});
    let (s, _) = w
        .call(
            Method::POST,
            "/v1/me/user-tokens",
            &w.owner_cookie,
            Some(body),
        )
        .await;
    assert_eq!(s, 400);
    // Another person's token is 404 everywhere.
    let t = w.ready().await;
    for (m, uri) in [
        (Method::PATCH, format!("/v1/me/user-tokens/{t}")),
        (Method::DELETE, format!("/v1/me/user-tokens/{t}")),
        (
            Method::PUT,
            format!("/v1/me/user-tokens/{t}/agents/{}", w.agent),
        ),
        (
            Method::DELETE,
            format!("/v1/me/user-tokens/{t}/agents/{}", w.agent),
        ),
    ] {
        let body = (m == Method::PATCH).then(|| json!({"label": "x"}));
        let (s, v) = w.call(m.clone(), &uri, &w.approver_cookie, body).await;
        assert_eq!(s, 404, "{m} {uri}: {v}");
    }
    let (_, l) = w
        .call(Method::GET, "/v1/me/user-tokens", &w.approver_cookie, None)
        .await;
    assert_eq!(l["tokens"], json!([]));
}

#[tokio::test]
async fn rename_and_grants_are_idempotent_and_audited() {
    let w = W::new().await;
    let t = w.add_owner_token().await;
    let (s, v) = w
        .call(
            Method::PATCH,
            &format!("/v1/me/user-tokens/{t}"),
            &w.owner_cookie,
            Some(json!({"label": "  renamed  "})),
        )
        .await;
    assert_eq!((s, v["label"].as_str()), (200, Some("renamed")), "{v}");
    assert_eq!(w.grant(t, w.agent).await, 204);
    assert_eq!(w.grant(t, w.agent).await, 204, "idempotent");
    let (_, l) = w
        .call(Method::GET, "/v1/me/user-tokens", &w.owner_cookie, None)
        .await;
    assert_eq!(l["tokens"][0]["agents"][0]["id"], json!(w.agent));
    // An agent in a workspace the owner cannot see is 404.
    let other_org = w.h.org().await;
    let other_ws = w.h.workspace(other_org).await;
    let stranger = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'s','S','byo','medium','')")
        .bind(stranger).bind(other_org).bind(other_ws).execute(w.h.pool()).await.unwrap();
    assert_eq!(w.grant(t, stranger).await, 404);
    let del = w
        .call(
            Method::DELETE,
            &format!("/v1/me/user-tokens/{t}/agents/{}", w.agent),
            &w.owner_cookie,
            None,
        )
        .await
        .0;
    assert_eq!(del, 204);
    assert_eq!(
        w.call(
            Method::DELETE,
            &format!("/v1/me/user-tokens/{t}/agents/{}", w.agent),
            &w.owner_cookie,
            None
        )
        .await
        .0,
        204,
        "idempotent"
    );
    assert_eq!(
        w.wait_audit("user_token.grant", 1).await.len(),
        1,
        "one row for the first grant"
    );
    assert_eq!(w.wait_audit("user_token.ungrant", 1).await.len(), 1);
}

// ===================================================================== eligibility (SEC-113)

#[tokio::test]
async fn the_owners_own_direct_conversation_and_own_thread_reach_the_approval_step() {
    let w = W::new().await;
    w.ready().await;
    let v = w.send_as_me("hello team").await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["content"][0]["value"]["status"], "pending_approval");
    assert_eq!(w.approvals().await, 1);
    // A group thread the owner started, spoken by the owner.
    let (conv, _, turn) = w
        .thread("group", "CHAN", "T9", w.owner, json!({"kind": "thread"}))
        .await;
    let v = w
        .send_as_me_at(w.agent, conv, Some(turn), "in my thread")
        .await;
    assert_eq!(
        v["content"][0]["value"]["status"], "pending_approval",
        "{v}"
    );
    assert_eq!(w.approvals().await, 2);
    // The summary names the conversation and the length, never the text.
    let (summary, payload): (String, Value) =
        sqlx::query_as("SELECT summary, payload FROM action_approvals ORDER BY created_at LIMIT 1")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(summary.starts_with("Post as "), "{summary}");
    assert!(summary.contains("(10 characters)"), "{summary}");
    assert!(!summary.contains("hello team"));
    assert_eq!(payload["text"], "hello team");
    assert_eq!(payload["owner_id"], json!(w.owner));
}

#[tokio::test]
async fn nobody_else_and_no_other_door_can_start_a_send() {
    let w = W::new().await;
    w.ready().await;
    // Another speaker in a channel thread the owner started: Bob's prompt, Alice's identity.
    let (conv, _, _) = w
        .thread("group", "CHAN", "T1", w.owner, json!({"kind": "thread"}))
        .await;
    let bob_turn = {
        let link: Uuid =
            sqlx::query_scalar("SELECT id FROM thread_links WHERE conversation_id = $1")
                .bind(conv)
                .fetch_one(w.h.pool())
                .await
                .unwrap();
        w.add_turn(
            w.agent,
            conv,
            w.approver,
            "person",
            json!({"kind": "thread", "thread_link_id": link}),
        )
        .await
    };
    let v = w
        .send_as_me_at(w.agent, conv, Some(bob_turn), "bob says")
        .await;
    assert!(refusal(&v).starts_with("not_eligible"), "{v}");
    // A group thread another person started, even with the owner speaking in it.
    let (conv2, link2, _) = w
        .thread(
            "group",
            "CHAN2",
            "T2",
            w.approver,
            json!({"kind": "thread"}),
        )
        .await;
    let owner_turn = w
        .add_turn(
            w.agent,
            conv2,
            w.owner,
            "person",
            json!({"kind": "thread", "thread_link_id": link2}),
        )
        .await;
    let v = w
        .send_as_me_at(w.agent, conv2, Some(owner_turn), "not my thread")
        .await;
    assert!(refusal(&v).starts_with("not_eligible"), "{v}");
    // No door but a chat thread qualifies, whoever the speaker.
    for door in [
        "schedule", "webhook", "watch", "peer", "system", "web", "api",
    ] {
        let t = w
            .add_turn(w.agent, w.conv, w.owner, "person", json!({"kind": door}))
            .await;
        let v = w.send_as_me_at(w.agent, w.conv, Some(t), "x").await;
        assert!(refusal(&v).starts_with("not_eligible"), "{door}: {v}");
    }
    // A non-person speaker, and a call outside any turn.
    let sys = w
        .add_turn(
            w.agent,
            w.conv,
            w.owner,
            "system",
            json!({"kind": "thread", "thread_link_id": w.link}),
        )
        .await;
    let v = w.send_as_me_at(w.agent, w.conv, Some(sys), "x").await;
    assert!(refusal(&v).starts_with("not_eligible"), "{v}");
    let v = w.send_as_me_at(w.agent, w.conv, None, "x").await;
    assert!(refusal(&v).starts_with("not_eligible"), "{v}");
    assert_eq!(w.approvals().await, 0, "a refusal creates no approval");
    assert_eq!(w.posts().await, 0);
}

#[tokio::test]
async fn a_grant_of_another_agent_an_inactive_token_and_an_inactive_link_are_refused() {
    let w = W::new().await;
    let t = w.add_owner_token().await;
    // No grant at all: the tool is not even offered to the agent.
    let v = w.send_as_me("x").await;
    assert_eq!(code(&v), "tool_unavailable", "{v}");
    // The agent holds a grant of someone else's token: offered, but not this owner's.
    let theirs = Uuid::now_v7();
    sqlx::query("INSERT INTO user_tokens (id, user_id, integration_id, platform_user_ref, platform_user_label, token_ref, status) VALUES ($1,$2,$3,'U_APPROVER','a','r','active')")
        .bind(theirs).bind(w.approver).bind(w.integ).execute(w.h.pool()).await.unwrap();
    sqlx::query("INSERT INTO user_token_agents (user_token_id, agent_id) VALUES ($1,$2)")
        .bind(theirs)
        .bind(w.agent)
        .execute(w.h.pool())
        .await
        .unwrap();
    let v = w.send_as_me("x").await;
    assert!(refusal(&v).starts_with("not_granted"), "{v}");
    // A grant of another agent is no grant of this one.
    let other = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'o','Other','byo','medium','')")
        .bind(other).bind(w.org).bind(w.ws).execute(w.h.pool()).await.unwrap();
    assert_eq!(w.grant(t, other).await, 204);
    let v = w.send_as_me("x").await;
    assert!(refusal(&v).starts_with("not_granted"), "{v}");
    // The granted agent, with the token invalid, then with the link gone.
    assert_eq!(w.grant(t, w.agent).await, 204);
    sqlx::query("UPDATE user_tokens SET status = 'invalid' WHERE id = $1")
        .bind(t)
        .execute(w.h.pool())
        .await
        .unwrap();
    let v = w.send_as_me("x").await;
    assert!(refusal(&v).starts_with("token_inactive"), "{v}");
    sqlx::query("UPDATE user_tokens SET status = 'active' WHERE id = $1")
        .bind(t)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM user_links WHERE user_id = $1")
        .bind(w.owner)
        .execute(w.h.pool())
        .await
        .unwrap();
    let v = w.send_as_me("x").await;
    assert!(refusal(&v).starts_with("token_inactive"), "{v}");
    assert_eq!(w.approvals().await, 0);
}

#[tokio::test]
async fn the_tool_is_offered_only_to_an_agent_that_holds_a_grant() {
    let w = W::new().await;
    let names = |c: Vec<aurora_protocol::tools::ToolDescriptor>| -> Vec<String> {
        c.into_iter().map(|t| t.name).collect()
    };
    let before = names(
        aurora_api::domains::tools::catalogue(w.h.pool(), w.agent)
            .await
            .unwrap(),
    );
    assert!(!before.contains(&"send_as_me".to_owned()), "{before:?}");
    w.ready().await;
    let after = names(
        aurora_api::domains::tools::catalogue(w.h.pool(), w.agent)
            .await
            .unwrap(),
    );
    assert!(after.contains(&"send_as_me".to_owned()), "{after:?}");
}

// ===================================================================== approval

#[tokio::test]
async fn only_the_owner_decides_even_an_approver_and_there_is_no_allow_mode() {
    let w = W::new().await;
    w.ready().await;
    // The post comes from a channel thread the owner started, so the approver sees the approval.
    let (conv, _, turn) = w
        .thread("group", "CHAN", "T7", w.owner, json!({"kind": "thread"}))
        .await;
    let v = w
        .send_as_me_at(w.agent, conv, Some(turn), "approve me")
        .await;
    let a: Uuid = v["content"][0]["value"]["approval_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // An approver (can_approve) is refused: the decision is the owner's alone.
    let (s, v) = w.decide(a, &w.approver_cookie, "approve").await;
    assert_eq!((s, code(&v)), (403, "not_token_owner"), "{v}");
    let (s, _) = w.decide(a, &w.approver_cookie, "deny").await;
    assert_eq!(s, 403);
    assert_eq!(w.posts().await, 0);
    // In the owner's direct conversation the approval does not even exist for an approver.
    let dm = w.propose("dm text").await;
    let (s, _) = w.decide(dm, &w.approver_cookie, "approve").await;
    assert_eq!(s, 404);
    // The workspace list carries the row for approvers but never the text.
    let (s, l) = w
        .call(
            Method::GET,
            &format!("/v1/workspaces/{}/action-approvals", w.ws),
            &w.approver_cookie,
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert!(!l.to_string().contains("approve me"), "{l}");
    // The owner sees the full text on their own list.
    let (s, own) = w
        .call(
            Method::GET,
            "/v1/me/user-token-approvals?status=pending",
            &w.owner_cookie,
            None,
        )
        .await;
    assert_eq!(s, 200, "{own}");
    let texts: Vec<&str> = own["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["text"].as_str())
        .collect();
    assert!(
        texts.contains(&"approve me") && texts.contains(&"dm text"),
        "{own}"
    );
    let (_, theirs) = w
        .call(
            Method::GET,
            "/v1/me/user-token-approvals",
            &w.approver_cookie,
            None,
        )
        .await;
    assert_eq!(theirs["items"], json!([]), "nobody else's text");
    // `proactive_messaging = allow` creates no send path: still a held approval, nothing queued.
    sqlx::query("UPDATE agents SET proactive_messaging = 'allow' WHERE id = $1")
        .bind(w.agent)
        .execute(w.h.pool())
        .await
        .unwrap();
    let again = w.send_as_me("second").await;
    assert_eq!(
        again["content"][0]["value"]["status"], "pending_approval",
        "{again}"
    );
    assert_eq!(
        w.posts().await,
        0,
        "nothing is ever sent without the owner's decision"
    );
    // The owner approves: the post is queued with the sender and its own copy.
    let (s, v) = w.decide(a, &w.owner_cookie, "approve").await;
    assert_eq!((s, v["status"].as_str()), (200, Some("executed")), "{v}");
    let payload: Value =
        sqlx::query_scalar("SELECT payload FROM channel_ops WHERE kind = 'message.post'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(payload["sender"]["user_token"].is_string(), "{payload}");
    assert_eq!(payload["target"]["space_ref"], "CHAN");
    assert_eq!(payload["target"]["thread_ref"], "T7");
    assert!(payload["user_credential_ref"].is_string());
    assert!(!payload.to_string().contains(CANARY));
    assert_eq!(
        payload["content"]["text"], "approve me",
        "the footer is the connector's, not stored"
    );
}

#[tokio::test]
async fn deny_and_expiry_send_nothing_and_the_sixth_pending_send_is_refused() {
    let w = W::new().await;
    w.ready().await;
    let a = w.propose("deny me").await;
    let (s, v) = w.decide(a, &w.owner_cookie, "deny").await;
    assert_eq!((s, v["status"].as_str()), (200, Some("rejected")), "{v}");
    let b = w.propose("expire me").await;
    sqlx::query(
        "UPDATE action_approvals SET expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(b)
    .execute(w.h.pool())
    .await
    .unwrap();
    assert_eq!(approvals::sweep(w.h.pool()).await.unwrap(), 1);
    let (st, _): (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM action_approvals WHERE id = $1")
            .bind(b)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "expired");
    let (s, v) = w.decide(b, &w.owner_cookie, "approve").await;
    assert_eq!((s, code(&v)), (409, "already_decided"), "{v}");
    assert_eq!(w.posts().await, 0);
    for i in 0..5 {
        w.propose(&format!("pending {i}")).await;
    }
    let v = w.send_as_me("one too many").await;
    assert!(refusal(&v).starts_with("pending_limit"), "{v}");
}

// ===================================================================== custody (SEC-112)

#[tokio::test]
async fn the_copy_lives_from_approval_to_the_terminal_state_and_leaves_no_version() {
    let w = W::new().await;
    let t = w.ready().await;
    assert_eq!(w.mem.managed_count(), 1);
    w.approved("the post").await;
    assert_eq!(w.mem.managed_count(), 2, "the token and one per-op copy");
    let ops = w.claim().await;
    assert_eq!(ops.len(), 1, "{ops:?}");
    let op = &ops[0];
    assert_eq!(op["kind"], "message.post");
    assert_eq!(
        op["user_credential"], CANARY,
        "opened at claim, delivered to the connector"
    );
    assert_eq!(op["payload"]["sender"]["user_token"], json!(t));
    assert_eq!(op["payload"]["user_footer"]["agent_name"], "Bot");
    for k in ["user_credential_ref", "user_org_id", "user_meta"] {
        assert!(op["payload"].get(k).is_none(), "{k} is internal");
    }
    assert!(!op["payload"].to_string().contains(CANARY));
    // Until the terminal state the copy is referenced; then null and gone from the store.
    let before: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'user_credential_ref' FROM channel_ops WHERE kind = 'message.post'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(before.is_some());
    w.report(op, true, json!({"message_ref": "m1", "thread_ref": ""}), "")
        .await;
    let after: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'user_credential_ref' FROM channel_ops WHERE kind = 'message.post'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(after, None, "null at the terminal state");
    assert_eq!(
        w.mem.managed_count(),
        1,
        "no store version of the copy is left"
    );
    // One audit row for the send: outcome, agent, token, conversation; no text, no token.
    let rows = w.wait_audit("user_token.send", 1).await;
    assert_eq!(rows.len(), 1);
    let m = rows[0].0.to_string();
    assert!(
        m.contains(&w.agent.to_string()) && m.contains(&w.conv.to_string()),
        "{m}"
    );
    assert!(
        !m.contains("the post") && !m.contains(CANARY) && !m.contains("U_OWNER"),
        "{m}"
    );
    let last_used: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_used_at FROM user_tokens WHERE id = $1")
            .bind(t)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(last_used.is_some());
}

#[tokio::test]
async fn a_claim_after_revoke_finds_no_copy_and_fails_the_op() {
    let w = W::new().await;
    let t = w.ready().await;
    w.approved("never sent").await;
    // Revoke before the connector claims: the open send fails and its copy is gone.
    let (s, _) = w
        .call(
            Method::DELETE,
            &format!("/v1/me/user-tokens/{t}"),
            &w.owner_cookie,
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(w.claim().await.len(), 0, "nothing to hand out");
    let (st, code): (String, Option<String>) =
        sqlx::query_as("SELECT status, error_code FROM channel_ops WHERE kind = 'message.post'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (st.as_str(), code.as_deref()),
        ("failed", Some("credential_revoked"))
    );
    assert_eq!(
        w.mem.managed_count(),
        0,
        "the token and the copy are both discarded"
    );
    // A copy that survived (revoked at the database level only) is refused at the claim itself.
    let w = W::new().await;
    let t = w.ready().await;
    w.approved("never sent").await;
    sqlx::query("UPDATE user_tokens SET status = 'revoked' WHERE id = $1")
        .bind(t)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(w.claim().await.len(), 0);
    let code: Option<String> =
        sqlx::query_scalar("SELECT error_code FROM channel_ops WHERE kind = 'message.post'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(code.as_deref(), Some("credential_revoked"));
}

#[tokio::test]
async fn a_platform_401_marks_the_token_invalid_and_leaves_the_presence() {
    let w = W::new().await;
    let t = w.ready().await;
    let pending = w.propose("queued behind").await;
    w.approved("the send").await;
    let ops = w.claim().await;
    w.report(&ops[0], false, Value::Null, "credential_dead")
        .await;
    let (status, _) = w.status(t).await;
    assert_eq!(status, "invalid");
    let presence: String =
        sqlx::query_scalar("SELECT status FROM channel_presences WHERE agent_id = $1")
            .bind(w.agent)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(presence, "active", "the presence is untouched");
    let integ: String = sqlx::query_scalar("SELECT status FROM channel_integrations WHERE id = $1")
        .bind(w.integ)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(integ, "active");
    let (st, err): (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM action_approvals WHERE id = $1")
            .bind(pending)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (st.as_str(), err.as_deref()),
        ("rejected", Some("credential_revoked"))
    );
    let rows = w.audit("user_token.invalid").await;
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].1, "system row, not critical");
    // The sweep discards the value the invalid token still referenced... it was discarded at once.
    assert_eq!(w.mem.managed_count(), 0);
    let ref_left: Option<String> =
        sqlx::query_scalar("SELECT token_ref FROM user_tokens WHERE id = $1")
            .bind(t)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(ref_left, None);
}

// ===================================================================== revocation cascades

/// A ready token with a pending approval and an approved, unclaimed send.
async fn busy(w: &W) -> (Uuid, Uuid) {
    let t = w.ready().await;
    let pending = w.propose("pending one").await;
    w.approved("queued one").await;
    assert_eq!(w.mem.managed_count(), 2);
    (t, pending)
}

async fn assert_ended(w: &W, t: Uuid, pending: Uuid, token_row_survives: bool) {
    if token_row_survives {
        let (status, token_ref) = w.status(t).await;
        assert_eq!(status, "revoked");
        assert_eq!(token_ref, None, "the sealed reference is cleared");
    } else {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM user_tokens WHERE id = $1")
            .bind(t)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
    let (st, err): (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM action_approvals WHERE id = $1")
            .bind(pending)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (st.as_str(), err.as_deref()),
        ("rejected", Some("credential_revoked"))
    );
    let (op, code): (String, Option<String>) =
        sqlx::query_as("SELECT status, error_code FROM channel_ops WHERE kind = 'message.post'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (op.as_str(), code.as_deref()),
        ("failed", Some("credential_revoked"))
    );
    // The store keeps no version of the token or of the queued send's copy.
    assert_eq!(w.mem.managed_count(), 0, "every sealed value is discarded");
    let rows = w.audit("user_token.revoke").await;
    assert_eq!(rows.len(), 1, "one audit row: {rows:?}");
    assert!(rows[0].1, "user_token.revoke is critical");
    assert!(!rows[0].0.to_string().contains(CANARY));
}

#[tokio::test]
async fn the_owner_revokes() {
    let w = W::new().await;
    let (t, pending) = busy(&w).await;
    let (s, _) = w
        .call(
            Method::DELETE,
            &format!("/v1/me/user-tokens/{t}"),
            &w.owner_cookie,
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_ended(&w, t, pending, true).await;
    assert_eq!(w.audit("user_token.revoke").await[0].0["reason"], "owner");
    // Revoking again finds nothing.
    let (s, _) = w
        .call(
            Method::DELETE,
            &format!("/v1/me/user-tokens/{t}"),
            &w.owner_cookie,
            None,
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn removing_the_link_ends_the_token() {
    let w = W::new().await;
    let (t, pending) = busy(&w).await;
    let (s, _) = w
        .call(
            Method::DELETE,
            &format!("/v1/me/channel-links/{}", w.link_id().await),
            &w.owner_cookie,
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_ended(&w, t, pending, true).await;
    assert_eq!(
        w.audit("user_token.revoke").await[0].0["reason"],
        "link_removed"
    );
}

#[tokio::test]
async fn deleting_the_owner_ends_the_token() {
    let w = W::new().await;
    let (t, pending) = busy(&w).await;
    let (s, v) = w
        .call(Method::DELETE, "/v1/me", &w.owner_cookie, None)
        .await;
    assert_eq!(s, 204, "{v}");
    // The approvals and the op may go with the user's conversations; what must hold is the store.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM user_tokens WHERE id = $1")
        .bind(t)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(w.mem.managed_count(), 0, "every sealed value is discarded");
    let rows = w.audit("user_token.revoke").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].1);
    assert_eq!(rows[0].0["reason"], "owner_deleted");
    let _ = pending;
}

#[tokio::test]
async fn deleting_the_integration_ends_the_token() {
    let w = W::new().await;
    let (t, pending) = busy(&w).await;
    let admin = w.h.user("admin@x.io", false).await;
    w.h.member(w.org, admin, "owner", "active").await;
    let ac = cookie(&w.h.good_session(admin).await.1);
    let (s, v) = w
        .call(
            Method::DELETE,
            &format!("/v1/channel-integrations/{}", w.integ),
            &ac,
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(w.mem.managed_count(), 0);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM user_tokens WHERE id = $1")
        .bind(t)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    let rows = w.audit("user_token.revoke").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0["reason"], "integration_deleted");
    let _ = pending;
}

#[tokio::test]
async fn a_directory_deprovisioning_ends_the_token_and_the_sweep_discards_its_value() {
    let w = W::new().await;
    let (t, pending) = busy(&w).await;
    let mut tx = w.h.pool().begin().await.unwrap();
    aurora_api::domains::scim::deprovision::end_user_tokens(
        &mut tx,
        w.org,
        w.owner,
        &aurora_api::audit::AuditActor::system("scim:test"),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // In the same transaction the token is unusable; its sealed value is discarded by the sweep.
    let (status, token_ref) = w.status(t).await;
    assert_eq!(status, "revoked");
    assert!(
        token_ref.is_some(),
        "kept only until the sweep discards the value"
    );
    assert!(
        aurora_api::domains::user_tokens::custody::sweep(&w.h.state)
            .await
            .unwrap()
            >= 1
    );
    assert_eq!(w.status(t).await.1, None);
    assert_ended(&w, t, pending, true).await;
    assert_eq!(
        w.audit("user_token.revoke").await[0].0["reason"],
        "scim_deprovision"
    );
}

#[tokio::test]
async fn removing_a_grant_or_deleting_the_agent_ends_that_agents_posts_only() {
    let w = W::new().await;
    let t = w.ready().await;
    let pending = w.propose("pending one").await;
    w.approved("queued one").await;
    let n = w
        .call(
            Method::DELETE,
            &format!("/v1/me/user-tokens/{t}/agents/{}", w.agent),
            &w.owner_cookie,
            None,
        )
        .await
        .0;
    assert_eq!(n, 204);
    let (st, err): (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM action_approvals WHERE id = $1")
            .bind(pending)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (st.as_str(), err.as_deref()),
        ("rejected", Some("credential_revoked"))
    );
    let (op, code): (String, Option<String>) =
        sqlx::query_as("SELECT status, error_code FROM channel_ops WHERE kind = 'message.post'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (op.as_str(), code.as_deref()),
        ("failed", Some("credential_revoked"))
    );
    assert_eq!(
        w.status(t).await.0,
        "active",
        "the token itself stays for other agents"
    );
    assert_eq!(w.mem.managed_count(), 1, "only the token itself");
    // The agent's deletion ends its grants and writes one ungrant row.
    assert_eq!(w.grant(t, w.agent).await, 204);
    let pending2 = w.propose("again").await;
    let del = w
        .call(
            Method::DELETE,
            &format!("/v1/agents/{}", w.agent),
            &w.approver_cookie,
            None,
        )
        .await;
    assert!(matches!(del.0, 204 | 200), "{del:?}");
    let _ = pending2;
    assert_eq!(w.status(t).await.0, "active");
    let grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_token_agents WHERE user_token_id = $1")
            .bind(t)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(grants, 0);
    assert!(w.wait_audit("user_token.ungrant", 2).await.len() >= 2);
}

// ===================================================================== static checks

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn the_token_purposes_are_used_only_by_the_custody_module() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let allowed = ["domains/user_tokens/custody.rs", "secrets_migrate.rs"];
    for f in files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&f).unwrap();
        for needle in ["Purpose::UserToken", "Purpose::ChannelOpUserToken"] {
            if text.contains(needle) {
                assert!(
                    allowed.contains(&rel.as_str()),
                    "{rel} uses {needle}: a person's token is sealed, opened and discarded only in the custody module"
                );
            }
        }
    }
}

#[test]
fn no_gateway_runner_or_sandbox_code_knows_a_user_token() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for dir in [
        "services/aurora-gateway/src",
        "services/aurora-supervisor/src",
        "services/aurora-runner/src",
        "services/aurora-hookhost/src",
        "services/aurora-terminator/src",
        "crates/aurora-policy/src",
    ] {
        let p = repo.join(dir);
        if !p.exists() {
            continue;
        }
        let mut files = Vec::new();
        rust_files(&p, &mut files);
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap().to_lowercase();
            assert!(
                !text.contains("user_token") && !text.contains("usertoken"),
                "{} mentions a person's token",
                f.display()
            );
        }
    }
}
