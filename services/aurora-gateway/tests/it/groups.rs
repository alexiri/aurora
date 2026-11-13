#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-GRP-1 (D-131, SEC-71): the speaker's group ids on the gateway identity and the `user`,
//! `workspace` and `org` invalidations that refresh them.

use aurora_db::PgPool;
use aurora_db::bus::{Channel, notify};
use aurora_gateway::auth::{CredentialResolver, RequestIdentity};
use aurora_gateway::invalidation::Hub;
use aurora_gateway::resolver::DbCredentialResolver;
use aurora_testkit::TestDb;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

struct W {
    org: Uuid,
    ws: Uuid,
    hosted: Uuid,
    byo: Uuid,
    alice: Uuid,
    bob: Uuid,
    nobody: Uuid,
}

async fn user(pool: &PgPool, org: Uuid, role: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1,$2,'U')")
        .bind(id)
        .bind(format!("{}@x.io", id.simple()))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1,$2,$3)")
        .bind(org)
        .bind(id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn world(pool: &PgPool) -> W {
    let (org, ws, hosted, byo) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext')",
    )
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    // A credential resolves only while its org has an active owner.
    user(pool, org, "owner").await;
    let (alice, bob, nobody) = (
        user(pool, org, "member").await,
        user(pool, org, "member").await,
        user(pool, org, "member").await,
    );
    for (id, kind, handle) in [(hosted, "hosted", "h"), (byo, "byo", "b")] {
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,$4,'A',$5)")
            .bind(id)
            .bind(org)
            .bind(ws)
            .bind(handle)
            .bind(kind)
            .execute(pool)
            .await
            .unwrap();
    }
    W {
        org,
        ws,
        hosted,
        byo,
        alice,
        bob,
        nobody,
    }
}

async fn group(pool: &PgPool, org: Uuid, name: &str, org_role: Option<&str>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO groups (id, org_id, name, org_role) VALUES ($1,$2,$3,$4)")
        .bind(id)
        .bind(org)
        .bind(name)
        .bind(org_role)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn member(pool: &PgPool, g: Uuid, org: Uuid, u: Uuid) {
    sqlx::query("INSERT INTO group_members (group_id, org_id, user_id) VALUES ($1,$2,$3)")
        .bind(g)
        .bind(org)
        .bind(u)
        .execute(pool)
        .await
        .unwrap();
}

async fn bind(pool: &PgPool, g: Uuid, ws: Uuid, org: Uuid, role: &str) {
    sqlx::query("INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role) VALUES ($1,$2,$3,$4)")
        .bind(g)
        .bind(ws)
        .bind(org)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

async fn cred(pool: &PgPool, agent: Uuid, token: &str, kind: &str, speaker: Option<Uuid>) {
    sqlx::query(
        "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, speaker_user_id) \
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

async fn live(pool: &PgPool) -> Arc<DbCredentialResolver> {
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
    r
}

async fn resolve(r: &DbCredentialResolver, t: &str) -> RequestIdentity {
    r.resolve(t).await.unwrap().expect(t)
}

async fn until(r: &DbCredentialResolver, t: &str, ok: impl Fn(&RequestIdentity) -> bool) -> bool {
    for _ in 0..100 {
        if ok(&resolve(r, t).await) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn ping(pool: &PgPool, ch: Channel, v: Value) {
    notify(pool, ch, &v).await.unwrap();
}

#[tokio::test]
async fn identity_carries_the_speakers_group_ids_and_none_without_a_speaker() {
    let db = TestDb::new().await;
    let p = db.pool();
    let w = world(p).await;
    sqlx::query("INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1,$2,'member'),($1,$3,'member')")
        .bind(w.ws)
        .bind(w.alice)
        .bind(w.bob)
        .execute(p)
        .await
        .unwrap();
    let g1 = group(p, w.org, "one", None).await;
    let g2 = group(p, w.org, "two", None).await;
    member(p, g1, w.org, w.alice).await;
    member(p, g2, w.org, w.alice).await;
    member(p, g2, w.org, w.bob).await;
    // a group of another org: never in the set
    let other = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o2','O2','ext2')",
    )
    .bind(other)
    .execute(p)
    .await
    .unwrap();
    group(p, other, "foreign", None).await;
    cred(p, w.hosted, "auc_alice", "context", Some(w.alice)).await;
    cred(p, w.hosted, "auc_bob", "context", Some(w.bob)).await;
    // a person who has the groups but no `can_use` through them: only g_nowhere ... none bound
    cred(p, w.hosted, "auc_nobody", "context", Some(w.nobody)).await;
    cred(p, w.hosted, "aub_boot", "boot", None).await;
    cred(p, w.hosted, "aug_agent", "agent", None).await;
    cred(p, w.hosted, "auc_nospeaker", "context", None).await;
    cred(p, w.byo, "auc_byo", "context", Some(w.alice)).await;
    let r = live(p).await;

    let mut alice = resolve(&r, "auc_alice").await.speaker_group_ids;
    alice.sort();
    let mut want = vec![g1, g2];
    want.sort();
    assert_eq!(alice, want);
    assert_eq!(resolve(&r, "auc_bob").await.speaker_group_ids, vec![g2]);
    // no speaker, no groups: boot, BYO agent, BYO context, no-person context
    for t in ["aub_boot", "aug_agent", "auc_nospeaker", "auc_byo"] {
        let id = resolve(&r, t).await;
        assert_eq!(id.speaker_user_id, None, "{t}");
        assert!(id.speaker_group_ids.is_empty(), "{t}");
    }
    // a speaker without `can_use` has no speaker and therefore no group ids, even in groups
    let id = resolve(&r, "auc_nobody").await;
    assert_eq!((id.speaker_user_id, id.speaker_group_ids.len()), (None, 0));
}

#[tokio::test]
async fn user_invalidation_re_resolves_the_group_set_mid_turn() {
    let db = TestDb::new().await;
    let p = db.pool();
    let w = world(p).await;
    sqlx::query(
        "INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1,$2,'member')",
    )
    .bind(w.ws)
    .bind(w.alice)
    .execute(p)
    .await
    .unwrap();
    let g = group(p, w.org, "g", None).await;
    cred(p, w.hosted, "auc_alice", "context", Some(w.alice)).await;
    let r = live(p).await;
    assert!(resolve(&r, "auc_alice").await.speaker_group_ids.is_empty());
    // member added: the cached identity is dropped by the `user` scope and the next request
    // carries the group
    member(p, g, w.org, w.alice).await;
    ping(
        p,
        Channel::GatewayInvalidate,
        json!({"scope": "user", "id": w.alice}),
    )
    .await;
    assert!(
        until(&r, "auc_alice", |i| i.speaker_group_ids == vec![g]).await,
        "added"
    );
    // member removed: the group rule stops matching
    sqlx::query("DELETE FROM group_members WHERE user_id = $1")
        .bind(w.alice)
        .execute(p)
        .await
        .unwrap();
    ping(
        p,
        Channel::GatewayInvalidate,
        json!({"scope": "user", "id": w.alice}),
    )
    .await;
    assert!(
        until(&r, "auc_alice", |i| i.speaker_group_ids.is_empty()).await,
        "removed"
    );
}

#[tokio::test]
async fn workspace_and_org_invalidations_refresh_binding_and_mapping_changes() {
    let db = TestDb::new().await;
    let p = db.pool();
    let w = world(p).await;
    let g = group(p, w.org, "g", None).await;
    member(p, g, w.org, w.alice).await;
    // alice has no access at all: no speaker, no groups
    cred(p, w.hosted, "auc_alice", "context", Some(w.alice)).await;
    let r = live(p).await;
    let id = resolve(&r, "auc_alice").await;
    assert_eq!((id.speaker_user_id, id.speaker_group_ids.len()), (None, 0));
    // a binding gives the group's members `can_use`: workspace scope
    bind(p, g, w.ws, w.org, "member").await;
    ping(
        p,
        Channel::GatewayInvalidate,
        json!({"scope": "workspace", "id": w.ws}),
    )
    .await;
    assert!(
        until(&r, "auc_alice", |i| i.speaker_user_id == Some(w.alice)
            && i.speaker_group_ids == vec![g])
        .await,
        "binding"
    );
    // removing the binding withdraws it
    sqlx::query("DELETE FROM group_workspace_access")
        .execute(p)
        .await
        .unwrap();
    ping(
        p,
        Channel::GatewayInvalidate,
        json!({"scope": "workspace", "id": w.ws}),
    )
    .await;
    assert!(
        until(&r, "auc_alice", |i| i.speaker_user_id.is_none()
            && i.speaker_group_ids.is_empty())
        .await
    );
    // an `admin` org-role mapping gives `can_use` on every workspace: org scope
    sqlx::query("UPDATE groups SET org_role = 'admin' WHERE id = $1")
        .bind(g)
        .execute(p)
        .await
        .unwrap();
    ping(
        p,
        Channel::GatewayInvalidate,
        json!({"scope": "org", "id": w.org}),
    )
    .await;
    assert!(
        until(&r, "auc_alice", |i| i.speaker_user_id == Some(w.alice)
            && i.speaker_group_ids == vec![g])
        .await,
        "mapping"
    );
    // a suspended member derives nothing
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE user_id = $1")
        .bind(w.alice)
        .execute(p)
        .await
        .unwrap();
    ping(
        p,
        Channel::GatewayInvalidate,
        json!({"scope": "org", "id": w.org}),
    )
    .await;
    assert!(
        until(&r, "auc_alice", |i| i.speaker_user_id.is_none()
            && i.speaker_group_ids.is_empty())
        .await
    );
}

#[tokio::test]
async fn a_request_already_in_the_pipeline_keeps_the_identity_it_started_with() {
    let db = TestDb::new().await;
    let p = db.pool();
    let w = world(p).await;
    sqlx::query(
        "INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1,$2,'member')",
    )
    .bind(w.ws)
    .bind(w.alice)
    .execute(p)
    .await
    .unwrap();
    let g = group(p, w.org, "g", None).await;
    member(p, g, w.org, w.alice).await;
    cred(p, w.hosted, "auc_alice", "context", Some(w.alice)).await;
    let r = live(p).await;
    let in_flight = resolve(&r, "auc_alice").await;
    sqlx::query("DELETE FROM group_members")
        .execute(p)
        .await
        .unwrap();
    ping(
        p,
        Channel::GatewayInvalidate,
        json!({"scope": "user", "id": w.alice}),
    )
    .await;
    assert!(until(&r, "auc_alice", |i| i.speaker_group_ids.is_empty()).await);
    assert_eq!(
        in_flight.speaker_group_ids,
        vec![g],
        "the owned identity is unchanged"
    );
}

#[tokio::test]
async fn a_speaker_in_more_than_100_groups_is_refused_never_truncated() {
    // Truncating to 100 ids (ORDER BY group id) would hide the highest-id group from the
    // policy: a block rule naming it would never match and the request would pass.
    let db = TestDb::new().await;
    let p = db.pool();
    let w = world(p).await;
    sqlx::query(
        "INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1,$2,'member')",
    )
    .bind(w.ws)
    .bind(w.alice)
    .execute(p)
    .await
    .unwrap();
    let mut ids = Vec::new();
    for i in 0..100 {
        let g = group(p, w.org, &format!("g{i}"), None).await;
        member(p, g, w.org, w.alice).await;
        ids.push(g);
    }
    cred(p, w.hosted, "auc_alice", "context", Some(w.alice)).await;
    let r = live(p).await;
    // exactly at the limit: resolves with every id
    assert_eq!(resolve(&r, "auc_alice").await.speaker_group_ids.len(), 100);
    // a 101st membership (the cap bypassed, inserted directly) whose id sorts last
    let last = group(p, w.org, "last", None).await;
    member(p, last, w.org, w.alice).await;
    assert!(last > *ids.iter().max().unwrap());
    ping(
        p,
        Channel::GatewayInvalidate,
        json!({"scope": "user", "id": w.alice}),
    )
    .await;
    let mut refused = false;
    for _ in 0..100 {
        if r.resolve("auc_alice").await.is_err() {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(refused, "resolution must fail closed above 100 groups");
}
