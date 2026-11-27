#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A migration numbered *below* the newest applied one still applies on an already-upgraded
//! database (P4-SSH's `…000200` … `…000205` ride after master's `…000241`/`…000242`, because the
//! numbers were reserved before those landed; `…000202` onwards reference `ssh_keys`, so the six
//! go together). sqlx's migrator must run every unapplied migration whatever its position,
//! never refuse it as "out of order".
use aurora_testkit::TestDb;
use sqlx::migrate::Migrator;

const LATE: [i64; 6] = [
    20_261_201_000_200,
    20_261_201_000_201,
    20_261_201_000_202,
    20_261_201_000_203,
    20_261_201_000_204,
    20_261_201_000_205,
];

#[tokio::test]
async fn older_numbered_migrations_apply_on_an_upgraded_database() {
    let (_db, pool) = TestDb::unmigrated_with_pool().await;
    // The database as it was before these two migrations existed: everything else, applied.
    let without: Vec<_> = aurora_db::MIGRATOR
        .iter()
        .filter(|m| !LATE.contains(&m.version))
        .cloned()
        .collect();
    assert!(
        aurora_db::MIGRATOR
            .iter()
            .any(|m| LATE.contains(&m.version))
            && aurora_db::MIGRATOR
                .iter()
                .any(|m| m.version > LATE[5] && !LATE.contains(&m.version)),
        "the late migrations exist and newer ones follow them"
    );
    Migrator::with_migrations(without).run(&pool).await.unwrap();
    let has = |t: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
                .bind(t)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert!(
        !has("ssh_keys").await && !has("ssh_mint_counters").await && !has("ssh_sessions").await
    );

    // Now the full set: the older-numbered migrations apply, nothing is refused or re-run.
    aurora_db::migrate(&pool).await.unwrap();
    for t in [
        "ssh_keys",
        "ssh_mint_counters",
        "ssh_sessions",
        "ssh_stream_grants",
        "ssh_terminator_replicas",
    ] {
        assert!(has(t).await, "{t}");
    }
    let applied: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM _sqlx_migrations WHERE version = ANY($1) ORDER BY version",
    )
    .bind(&LATE[..])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(applied, LATE);
    // And it is idempotent.
    aurora_db::migrate(&pool).await.unwrap();
}
