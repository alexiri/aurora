#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! DB-backed credential resolver, invalidation and instance tests.

use aurora_db::PgPool;
use aurora_db::bus::{Channel, notify};
use aurora_gateway::auth::{CredentialResolver, RequestIdentity};
use aurora_gateway::invalidation::Hub;
use aurora_gateway::resolver::DbCredentialResolver;
use aurora_testkit::TestDb;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

struct W {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    owner: Uuid,
    admin: Uuid,
    bound: Uuid,
    suspended: Uuid,
    unbound: Uuid,
}

async fn user(pool: &PgPool, org: Uuid, role: &str, status: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1,$2,'U')")
        .bind(id)
        .bind(format!("{}@x.io", id.simple()))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO org_members (org_id, user_id, role, status) VALUES ($1,$2,$3,$4)")
        .bind(org)
        .bind(id)
        .bind(role)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn world(pool: &PgPool) -> W {
    let (org, ws, agent) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext')",
    )
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    let owner = user(pool, org, "owner", "active").await;
    let admin = user(pool, org, "admin", "active").await;
    let bound = user(pool, org, "member", "active").await;
    let suspended = user(pool, org, "member", "suspended").await;
    let unbound = user(pool, org, "member", "active").await;
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    for u in [bound, suspended] {
        sqlx::query(
            "INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1,$2,'member')",
        )
        .bind(ws)
        .bind(u)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','A','byo')")
        .bind(agent)
        .bind(org)
        .bind(ws)
        .execute(pool)
        .await
        .unwrap();
    W {
        org,
        ws,
        agent,
        owner,
        admin,
        bound,
        suspended,
        unbound,
    }
}

async fn cred(pool: &PgPool, agent: Uuid, token: &str, kind: &str, expires: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(&format!(
        "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, expires_at) \
         VALUES ($1,$2,$3,'x',$4, {expires})"
    ))
    .bind(id)
    .bind(kind)
    .bind(aurora_core::token::hash(token).to_vec())
    .bind(agent)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn live(pool: &PgPool) -> (Arc<DbCredentialResolver>, Arc<Hub>) {
    let r = Arc::new(DbCredentialResolver::with_ttls(
        pool.clone(),
        Duration::from_secs(60),
        Duration::from_millis(10),
    ));
    let hub = Hub::new();
    hub.register(r.cache());
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(hub.clone().run(pool.clone(), rx));
    std::mem::forget(tx);
    while !hub.is_connected() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (r, hub)
}

async fn resolve(r: &DbCredentialResolver, t: &str) -> Option<RequestIdentity> {
    r.resolve(t).await.unwrap()
}

#[tokio::test]
async fn resolves_without_a_speaker() {
    let db = TestDb::new().await;
    let w = world(db.pool()).await;
    cred(db.pool(), w.agent, "aug_good", "agent", "NULL").await;
    let r = DbCredentialResolver::new(db.pool().clone());
    let id = resolve(&r, "aug_good").await.unwrap();
    assert_eq!(
        (id.org_id, id.workspace_id, id.agent_id),
        (w.org, w.ws, w.agent)
    );
    // RFE-005: the workspace roster is gone; a plain agent credential has no speaker.
    assert_eq!(id.speaker_user_id, None);
    assert!(resolve(&r, "aug_nope").await.is_none());
    // Non-Aurora prefix: no DB call (closed pool would error otherwise).
    db.pool().close().await;
    assert!(resolve(&r, "sk-abc").await.is_none());
    assert!(r.resolve("aug_good").await.is_ok(), "served from cache");
}

#[tokio::test]
async fn rejections() {
    let db = TestDb::new().await;
    let p = db.pool();
    let w = world(p).await;
    let revoked = cred(p, w.agent, "aug_revoked", "agent", "NULL").await;
    sqlx::query("UPDATE gateway_credentials SET revoked_at = now() WHERE id = $1")
        .bind(revoked)
        .execute(p)
        .await
        .unwrap();
    cred(
        p,
        w.agent,
        "auc_expired",
        "context",
        "now() - interval '1 minute'",
    )
    .await;
    cred(
        p,
        w.agent,
        "auc_fresh",
        "context",
        "now() + interval '1 hour'",
    )
    .await;
    let r = DbCredentialResolver::new(p.clone());
    assert!(resolve(&r, "aug_revoked").await.is_none());
    assert!(resolve(&r, "auc_expired").await.is_none());
    assert!(resolve(&r, "auc_fresh").await.is_some());
    // Only owner suspended.
    sqlx::query("UPDATE org_members SET status='suspended' WHERE user_id = $1")
        .bind(w.owner)
        .execute(p)
        .await
        .unwrap();
    let r2 = DbCredentialResolver::new(p.clone());
    assert!(resolve(&r2, "auc_fresh").await.is_none());
    sqlx::query("UPDATE org_members SET status='active' WHERE user_id = $1")
        .bind(w.owner)
        .execute(p)
        .await
        .unwrap();
    cred(p, w.agent, "aub_boot", "boot", "NULL").await;
    sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(w.agent)
        .execute(p)
        .await
        .unwrap();
    let r3 = DbCredentialResolver::new(p.clone());
    assert!(resolve(&r3, "aub_boot").await.is_none());
}

async fn gone_after(
    db: &TestDb,
    r: &DbCredentialResolver,
    token: &str,
    channel: Channel,
    payload: serde_json::Value,
) {
    assert!(resolve(r, token).await.is_some());
    sqlx::query("UPDATE gateway_credentials SET revoked_at = now()")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        resolve(r, token).await.is_some(),
        "still cached before notify"
    );
    notify(db.pool(), channel, &payload).await.unwrap();
    for _ in 0..100 {
        if resolve(r, token).await.is_none() {
            sqlx::query("UPDATE gateway_credentials SET revoked_at = NULL")
                .execute(db.pool())
                .await
                .unwrap();
            // Negative entries are dropped by invalidation too; restore for next case.
            tokio::time::sleep(Duration::from_millis(10)).await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("not invalidated: {payload}");
}

#[tokio::test]
async fn notify_invalidates() {
    let db = TestDb::new().await;
    let w = world(db.pool()).await;
    let cid = cred(db.pool(), w.agent, "aug_good", "agent", "NULL").await;
    let (r, _hub) = live(db.pool()).await;
    let g = Channel::GatewayInvalidate;
    let cases = [
        (g, json!({"scope":"credential","id":cid})),
        (g, json!({"scope":"agent","id":w.agent})),
        (g, json!({"scope":"workspace","id":w.ws})),
        (g, json!({"scope":"org","id":w.org})),
        (g, json!({"scope":"all"})),
    ];
    for (c, p) in cases {
        gone_after(&db, &r, "aug_good", c, p).await;
        // Wait out the negative cache clear: invalidation cleared it, and the
        // final resolve re-cached a negative; a restore needs a fresh read.
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

/// A credential of `kind` on `agent` carrying `speaker` in `gateway_credentials.speaker_user_id`.
async fn speaker_cred(pool: &PgPool, agent: Uuid, token: &str, kind: &str, speaker: Uuid) {
    sqlx::query(
        "INSERT INTO gateway_credentials \
         (id, kind, token_hash, display_prefix, agent_id, speaker_user_id) \
         VALUES ($1,$2,$3,'x',$4,$5)",
    )
    .bind(Uuid::now_v7())
    .bind(kind)
    .bind(aurora_core::token::hash(token).to_vec())
    .bind(agent)
    .bind(speaker)
    .execute(pool)
    .await
    .unwrap();
}

/// RFE-005 (D-49, SEC-44): only a hosted context credential carries the speaker, and only while
/// the speaker holds `can_use`; the cached identity follows `user` invalidations.
#[tokio::test]
async fn speaker_binding_follows_can_use() {
    let db = TestDb::new().await;
    let p = db.pool();
    let w = world(p).await;
    let hosted = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'h','H','hosted')")
        .bind(hosted)
        .bind(w.org)
        .bind(w.ws)
        .execute(p)
        .await
        .unwrap();
    speaker_cred(p, hosted, "auc_bound", "context", w.bound).await;
    speaker_cred(p, hosted, "auc_owner", "context", w.owner).await;
    speaker_cred(p, hosted, "auc_admin", "context", w.admin).await;
    speaker_cred(p, hosted, "auc_unbound", "context", w.unbound).await;
    speaker_cred(p, hosted, "auc_suspended", "context", w.suspended).await;
    // Never trusted: a BYO agent mints its own context credentials; other kinds carry none.
    speaker_cred(p, w.agent, "auc_byo", "context", w.bound).await;
    speaker_cred(p, hosted, "aug_agent", "agent", w.bound).await;
    let (r, _hub) = live(p).await;
    let speaker = |t: &'static str| {
        let r = r.clone();
        async move { resolve(&r, t).await.expect(t).speaker_user_id }
    };
    assert_eq!(speaker("auc_bound").await, Some(w.bound));
    assert_eq!(speaker("auc_owner").await, Some(w.owner));
    assert_eq!(speaker("auc_admin").await, Some(w.admin));
    assert_eq!(speaker("auc_unbound").await, None, "no can_use");
    assert_eq!(speaker("auc_suspended").await, None, "suspended member");
    assert_eq!(speaker("auc_byo").await, None, "BYO context");
    assert_eq!(speaker("aug_agent").await, None, "not a context credential");

    // The speaker loses access mid-turn: the credential stays valid, the speaker is dropped
    // as soon as the `user` invalidation reaches the gateway.
    sqlx::query("DELETE FROM workspace_access WHERE workspace_id = $1 AND user_id = $2")
        .bind(w.ws)
        .bind(w.bound)
        .execute(p)
        .await
        .unwrap();
    notify(
        p,
        Channel::AuthRevoked,
        &json!({"kind":"user","id":w.bound}),
    )
    .await
    .unwrap();
    let mut dropped = false;
    for _ in 0..100 {
        if speaker("auc_bound").await.is_none() {
            dropped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dropped, "speaker kept after losing can_use");
    assert_eq!(
        speaker("auc_owner").await,
        Some(w.owner),
        "others unaffected"
    );
}

#[tokio::test]
async fn instance_registers_and_heartbeats() {
    let db = TestDb::new().await;
    let id = Uuid::now_v7();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let h = tokio::spawn(aurora_gateway::instance::run(
        db.pool().clone(),
        id,
        Duration::from_millis(100),
        rx,
    ));
    let q = "SELECT last_seen_at FROM gateway_instances WHERE id = $1";
    let mut first = None;
    for _ in 0..50 {
        first = sqlx::query_scalar::<_, chrono_ts>(q)
            .bind(id)
            .fetch_optional(db.pool())
            .await
            .unwrap();
        if first.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let first = first.expect("registered");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let later: chrono_ts = sqlx::query_scalar(q)
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(later > first);
    tx.send(true).unwrap();
    h.await.unwrap();
    let stopped: Option<chrono_ts> =
        sqlx::query_scalar("SELECT stopped_at FROM gateway_instances WHERE id = $1")
            .bind(id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(stopped.is_some());
}

/// RFE-006 (SEC-45): a credential of an effectively frozen agent is refused (the broadest flag
/// names the level), the refusal is never cached past an invalidation, and unfreezing restores
/// the very same token at once.
#[tokio::test]
async fn frozen_agents_are_refused_at_every_level_and_unfreeze_is_instant() {
    use aurora_gateway::auth::ResolveError;
    let db = TestDb::new().await;
    let p = db.pool();
    let w = world(p).await;
    let cid = cred(p, w.agent, "aug_f", "agent", "NULL").await;
    cred(p, w.agent, "auc_f", "context", "now() + interval '1 hour'").await;
    let (r, _hub) = live(p).await;
    assert!(resolve(&r, "aug_f").await.is_some());
    let set = |table: &'static str, id: Uuid, on: bool| async move {
        let sql = if on {
            format!(
                "UPDATE {table} SET frozen_at = now(), frozen_reason = 'drill', frozen_authority = 'org_admin' WHERE id = $1"
            )
        } else {
            format!(
                "UPDATE {table} SET frozen_at = NULL, frozen_reason = NULL, frozen_authority = NULL, frozen_by_user_id = NULL WHERE id = $1"
            )
        };
        sqlx::query(&sql).bind(id).execute(p).await.unwrap();
    };
    let wait_frozen = |want: Option<&'static str>| {
        let r = r.clone();
        async move {
            for _ in 0..100 {
                let got = match r.resolve("aug_f").await {
                    Err(ResolveError::Frozen { level }) => Some(level),
                    Ok(Some(_)) => None,
                    other => panic!("{other:?}"),
                };
                if got == want {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("never reached {want:?}");
        }
    };
    for (table, id, level) in [
        ("agents", w.agent, "agent"),
        ("workspaces", w.ws, "workspace"),
        ("organizations", w.org, "org"),
    ] {
        set(table, id, true).await;
        notify(
            p,
            Channel::GatewayInvalidate,
            &json!({"scope": if level == "org" { "org" } else { level }, "id": id}),
        )
        .await
        .unwrap();
        wait_frozen(Some(level)).await;
        // Every credential kind of the agent is refused, also by id (live-tunnel re-checks).
        assert!(matches!(
            r.resolve("auc_f").await,
            Err(ResolveError::Frozen { .. })
        ));
        assert!(matches!(
            r.resolve_credential(cid).await,
            Err(ResolveError::Frozen { .. })
        ));
        set(table, id, false).await;
        notify(
            p,
            Channel::GatewayInvalidate,
            &json!({"scope": level, "id": id}),
        )
        .await
        .unwrap();
        wait_frozen(None).await;
    }
    // Broadest wins: org + agent frozen reports `org`; lifting the org leaves `agent`.
    set("agents", w.agent, true).await;
    set("organizations", w.org, true).await;
    notify(p, Channel::GatewayInvalidate, &json!({"scope": "all"}))
        .await
        .unwrap();
    wait_frozen(Some("org")).await;
    set("organizations", w.org, false).await;
    notify(p, Channel::GatewayInvalidate, &json!({"scope": "all"}))
        .await
        .unwrap();
    wait_frozen(Some("agent")).await;
}

#[allow(non_camel_case_types)]
type chrono_ts = sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>;
