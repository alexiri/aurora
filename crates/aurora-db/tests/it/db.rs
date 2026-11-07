#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
use std::time::Duration;

use aurora_db::bus::{self, BusError, BusEvent, Channel, Listener};
use aurora_db::claim::{ClaimSpec, claim};
use aurora_db::{DbError, PgPool, jobs};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

async fn org(pool: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $2, 'o', $2)")
        .bind(id)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn workspace(pool: &PgPool, org: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1, $2, $3, 'w')")
        .bind(id)
        .bind(org)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn agent(
    pool: &PgPool,
    org: Uuid,
    ws: Uuid,
    handle: &str,
    default: bool,
) -> Result<Uuid, DbError> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, is_default) VALUES ($1,$2,$3,$4,'a','hosted',$5)")
        .bind(id).bind(org).bind(ws).bind(handle).bind(default)
        .execute(pool).await?;
    Ok(id)
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn migrations_apply_and_are_idempotent(pool: PgPool) {
    aurora_db::migrate(&pool).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema='public' AND table_name IN ('users','request_log','request_log_default','gateway_credentials','peer_tasks')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 5);
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn one_default_agent_per_workspace(pool: PgPool) {
    let o = org(&pool).await;
    let w = workspace(&pool, o).await;
    agent(&pool, o, w, "daneel", true).await.unwrap();
    agent(&pool, o, w, "other", false).await.unwrap();
    let err = agent(&pool, o, w, "second", true).await.unwrap_err();
    assert!(matches!(err, DbError::UniqueViolation { .. }), "{err:?}");
    assert_eq!(
        err.constraint(),
        Some("agents_one_default_per_workspace_uq")
    );
    // Duplicate handle is also rejected.
    assert!(matches!(
        agent(&pool, o, w, "other", false).await.unwrap_err(),
        DbError::UniqueViolation { .. }
    ));
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn one_active_plugin_version_per_package(pool: PgPool) {
    async fn pkg(pool: &PgPool, version: &str, status: &str) -> Result<(), DbError> {
        sqlx::query("INSERT INTO plugin_packages (id, package_id, version, contract, publisher, digest, signer_key_id, manifest, permissions, source, status) VALUES ($1,'p',$2,1,'pub',$2,'builtin','{}','{}','builtin',$3)")
            .bind(Uuid::now_v7()).bind(version).bind(status)
            .execute(pool).await?;
        Ok(())
    }
    pkg(&pool, "1.0.0", "active").await.unwrap();
    pkg(&pool, "1.1.0", "pending_approval").await.unwrap();
    let err = pkg(&pool, "1.2.0", "active").await.unwrap_err();
    assert_eq!(err.constraint(), Some("plugin_packages_one_active_uq"));
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn one_draft_and_published_generation_per_scope(pool: PgPool) {
    let o = org(&pool).await;
    let w = workspace(&pool, o).await;
    async fn gen_(
        pool: &PgPool,
        o: Uuid,
        w: Option<Uuid>,
        n: i32,
        status: &str,
    ) -> Result<(), DbError> {
        sqlx::query("INSERT INTO policy_generations (id, org_id, workspace_id, number, status) VALUES ($1,$2,$3,$4,$5)")
            .bind(Uuid::now_v7()).bind(o).bind(w).bind(n).bind(status)
            .execute(pool).await?;
        Ok(())
    }
    // Org scope (workspace_id NULL) must be limited too.
    gen_(&pool, o, None, 1, "draft").await.unwrap();
    assert!(gen_(&pool, o, None, 2, "draft").await.is_err());
    gen_(&pool, o, None, 2, "published").await.unwrap();
    assert!(gen_(&pool, o, None, 3, "published").await.is_err());
    gen_(&pool, o, None, 3, "superseded").await.unwrap();
    // Duplicate number (nulls not distinct).
    assert!(gen_(&pool, o, None, 3, "superseded").await.is_err());
    // Workspace scope independent.
    gen_(&pool, o, Some(w), 1, "draft").await.unwrap();
    assert!(gen_(&pool, o, Some(w), 2, "draft").await.is_err());
    // Uncredentialed default only at workspace scope.
    let err: DbError = sqlx::query("INSERT INTO policy_generations (id, org_id, number, status, uncredentialed_default) VALUES ($1,$2,9,'superseded','block')")
        .bind(Uuid::now_v7()).bind(o).execute(&pool).await.unwrap_err().into();
    assert!(matches!(err, DbError::CheckViolation { .. }));
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn fk_and_one_agent_credential(pool: PgPool) {
    let o = org(&pool).await;
    let w = workspace(&pool, o).await;
    let a = agent(&pool, o, w, "a", false).await.unwrap();
    let cred = |hash: u8| {
        let pool = pool.clone();
        async move {
            sqlx::query("INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id) VALUES ($1,'agent',$2,'p',$3)")
                .bind(Uuid::now_v7()).bind(vec![hash; 32]).bind(a)
                .execute(&pool).await
        }
    };
    cred(1).await.unwrap();
    assert!(cred(2).await.is_err());
    sqlx::query("UPDATE gateway_credentials SET revoked_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    cred(3).await.unwrap();
    // Deferred FK to agents works (grants.agent_id).
    let err: DbError = sqlx::query(
        "INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)",
    )
    .bind(Uuid::now_v7())
    .bind(w)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap_err()
    .into();
    assert!(matches!(err, DbError::ForeignKeyViolation { .. }));
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn request_log_default_partition_accepts_rows(pool: PgPool) {
    sqlx::query("INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, credential_kind, lane, method, host, path, decision, status, latency_ms) VALUES ($1, now(), $1,$1,$1,'agent','mitm','GET','h','/','allow',200,1)")
        .bind(Uuid::now_v7()).execute(&pool).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM request_log_default")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn notify_round_trip_and_resync(pool: PgPool) {
    let mut l = Listener::connect(&pool, &[Channel::Inbox, Channel::Approval])
        .await
        .unwrap();
    assert_eq!(l.next().await, BusEvent::Resync);
    // Malformed payload is skipped, not fatal.
    sqlx::query("SELECT pg_notify('aurora_inbox', 'not json')")
        .execute(&pool)
        .await
        .unwrap();
    bus::notify(
        &pool,
        Channel::Approval,
        &json!({"id": "x", "event": "created"}),
    )
    .await
    .unwrap();
    let ev = tokio::time::timeout(Duration::from_secs(5), l.next())
        .await
        .unwrap();
    assert_eq!(
        ev,
        BusEvent::Notification {
            channel: Channel::Approval,
            payload: json!({"id": "x", "event": "created"})
        }
    );
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn oversize_payload_rejected(pool: PgPool) {
    let big = json!({ "d": "x".repeat(7000) });
    let err = bus::notify(&pool, Channel::Delta, &big).await.unwrap_err();
    assert!(matches!(err, BusError::PayloadTooLarge { .. }));
    assert_eq!(Channel::ALL.len(), 11);
    assert_eq!(Channel::parse("aurora_work"), Some(Channel::Work));
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn singleton_excludes_second_holder(pool: PgPool) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let p2 = pool.clone();
    let holder = tokio::spawn(async move {
        jobs::try_singleton(&p2, "test-job", || async move {
            started_tx.send(()).unwrap();
            rx.await.unwrap();
            7
        })
        .await
    });
    started_rx.await.unwrap();
    let second = jobs::try_singleton(&pool, "test-job", || async { 1 })
        .await
        .unwrap();
    assert_eq!(second, None);
    let other = jobs::try_singleton(&pool, "other-job", || async { 2 })
        .await
        .unwrap();
    assert_eq!(other, Some(2));
    tx.send(()).unwrap();
    assert_eq!(holder.await.unwrap().unwrap(), Some(7));
    // Released afterwards.
    assert_eq!(
        jobs::try_singleton(&pool, "test-job", || async { 3 })
            .await
            .unwrap(),
        Some(3)
    );
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn claim_never_double_hands_rows(pool: PgPool) {
    sqlx::query("CREATE TABLE claim_t (id int PRIMARY KEY, done boolean NOT NULL DEFAULT false, lease timestamptz)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO claim_t (id) SELECT g FROM generate_series(1, 300) g")
        .execute(&pool)
        .await
        .unwrap();
    const SPEC: ClaimSpec = ClaimSpec {
        table: "claim_t",
        lease_col: "lease",
        due: "NOT done",
        order_by: "id",
        returning: "id",
    };
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            let mut got = Vec::new();
            loop {
                let rows = claim(&pool, &SPEC, 7, 60.0).await.unwrap();
                if rows.is_empty() {
                    break;
                }
                got.extend(rows.iter().map(|r| r.get::<i32, _>("id")));
            }
            got
        }));
    }
    let mut all = Vec::new();
    for t in tasks {
        all.extend(t.await.unwrap());
    }
    let set: HashSet<_> = all.iter().copied().collect();
    assert_eq!(all.len(), 300);
    assert_eq!(set.len(), 300);
    // Expired leases are reclaimable.
    sqlx::query("UPDATE claim_t SET lease = now() - interval '1 second' WHERE id <= 5")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(claim(&pool, &SPEC, 100, 60.0).await.unwrap().len(), 5);
}
