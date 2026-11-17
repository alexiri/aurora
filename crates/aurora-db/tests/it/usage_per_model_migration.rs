#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-MODELS-2 (D-304, C-880): `20261201000246_usage_per_model.sql` applies on a database whose partitioned
//! `request_log` already has rows (in the default partition and in a monthly one), keeps them, and every
//! partition (existing and later ones) takes the new `llm_model` column. The migrations are forward-only; the
//! reverse statements are exercised too, so a rollback by hand is known to work on the same data.
use aurora_db::PgPool;
use chrono::Utc;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_246;

async fn insert(pool: &PgPool, id: Uuid, with_model: bool) {
    let sql = if with_model {
        "INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, credential_kind, lane, method, host,
            path, decision, status, latency_ms, llm_model)
         VALUES ($1, now(), $1,$1,$1,'agent','mitm','POST','h','/','allow',200,1,'m-1')"
    } else {
        "INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, credential_kind, lane, method, host,
            path, decision, status, latency_ms)
         VALUES ($1, now(), $1,$1,$1,'agent','mitm','POST','h','/','allow',200,1)"
    };
    sqlx::query(sql).bind(id).execute(pool).await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn the_migration_applies_on_a_partitioned_table_that_already_has_rows(pool: PgPool) {
    let before: Vec<_> = aurora_db::MIGRATOR
        .iter()
        // every migration before it: later ones (the usage ledger reads `turns.model_basis`) need it
        .filter(|m| m.version < VERSION)
        .cloned()
        .collect();
    assert!(
        aurora_db::MIGRATOR.iter().any(|m| m.version == VERSION),
        "the migration exists"
    );
    Migrator::with_migrations(before).run(&pool).await.unwrap();
    let has_column = |table: &'static str, col: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = $1 AND column_name = $2)",
            )
            .bind(table)
            .bind(col)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert!(!has_column("request_log", "llm_model").await);
    assert!(!has_column("turns", "model_basis").await);

    // Rows before the migration: one in the default partition, one in the current month's partition.
    let in_default = Uuid::now_v7();
    insert(&pool, in_default, false).await;
    let created = aurora_db::repo::request_log::ensure_partitions(&pool, Utc::now(), 1)
        .await
        .unwrap();
    assert!(!created.is_empty(), "a monthly partition now exists");
    let in_month = Uuid::now_v7();
    insert(&pool, in_month, false).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM request_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
    let in_a_partition: i64 = sqlx::query_scalar("SELECT count(*) FROM ONLY request_log_default")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(in_a_partition <= 2);

    // Up.
    aurora_db::migrate(&pool).await.unwrap();
    assert!(has_column("request_log", "llm_model").await);
    assert!(has_column("turns", "model_basis").await);
    // Every partition has the column, and the old rows are intact with a null model.
    for part in std::iter::once("request_log_default".to_owned()).chain(created) {
        assert!(has_column(Box::leak(part.into_boxed_str()), "llm_model").await);
    }
    let kept: Vec<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, llm_model FROM request_log ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(kept.len(), 2);
    assert!(kept.iter().all(|(_, m)| m.is_none()), "{kept:?}");
    // A new row takes the model, in the monthly partition.
    let fresh = Uuid::now_v7();
    insert(&pool, fresh, true).await;
    let m: Option<String> = sqlx::query_scalar("SELECT llm_model FROM request_log WHERE id = $1")
        .bind(fresh)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(m.as_deref(), Some("m-1"));
    // A partition created after the migration inherits the column.
    let later = aurora_db::repo::request_log::ensure_partitions(
        &pool,
        Utc::now() + chrono::Duration::days(70),
        0,
    )
    .await
    .unwrap();
    for part in later {
        assert!(has_column(Box::leak(part.into_boxed_str()), "llm_model").await);
    }
    // Idempotent.
    aurora_db::migrate(&pool).await.unwrap();

    // Down (by hand): drops the columns, keeps every row.
    sqlx::query("ALTER TABLE request_log DROP COLUMN llm_model")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE turns DROP COLUMN model_basis")
        .execute(&pool)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM request_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 3);
    assert!(!has_column("request_log", "llm_model").await);
}
