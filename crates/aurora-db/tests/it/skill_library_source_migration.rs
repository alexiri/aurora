#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-278: `20261201000292` on a database with a library that has open and decided proposals.
//! Every open proposal is dropped (some may come from a source the library no longer follows),
//! decided ones stay (a rejection is still remembered), and every library is compared and
//! counted again.
use aurora_db::PgPool;
use aurora_testkit::TestDb;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_292;

async fn exec(pool: &PgPool, sql: &str, ids: &[Uuid]) {
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
    for id in ids {
        q = q.bind(*id);
    }
    q.execute(pool).await.unwrap();
}

#[tokio::test]
async fn open_proposals_are_dropped_and_libraries_are_compared_again() {
    let (_db, pool) = TestDb::unmigrated_with_pool().await;
    assert!(aurora_db::MIGRATOR.iter().any(|m| m.version == VERSION));
    let before: Vec<_> = aurora_db::MIGRATOR
        .iter()
        .filter(|m| m.version < VERSION)
        .cloned()
        .collect();
    Migrator::with_migrations(before).run(&pool).await.unwrap();
    let ids: Vec<Uuid> = (0..5).map(|_| Uuid::now_v7()).collect();
    let [org, library, skill, open, rejected] = ids[..] else {
        unreachable!()
    };
    exec(
        &pool,
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, 'o', 'o', 'o')",
        &[org],
    )
    .await;
    exec(
        &pool,
        "INSERT INTO skill_libraries (id, org_id, label, url, repo, path, last_head_sha,
             skills_offered, skills_unavailable, next_refresh_at)
         VALUES ($1, $2, 'l', 'https://git.example/a/b', 'git.example/a/b', '', 'abc', 3, 1,
             now() + interval '1 day')",
        &[library, org],
    )
    .await;
    exec(
        &pool,
        "INSERT INTO skills (id, org_id, name, description, body, created_by_kind)
         VALUES ($1, $2, 'pdf', 'd', 'b', 'user')",
        &[skill, org],
    )
    .await;
    exec(
        &pool,
        "INSERT INTO skill_library_updates (id, skill_id, library_id, kind, status, commit_sha)
         VALUES ($1, $2, $3, 'invalid', 'pending', 'c1')",
        &[open, skill, library],
    )
    .await;
    exec(
        &pool,
        "INSERT INTO skill_library_updates (id, skill_id, library_id, kind, status, commit_sha)
         VALUES ($1, $2, $3, 'changed', 'rejected', 'c0')",
        &[rejected, skill, library],
    )
    .await;

    aurora_db::migrate(&pool).await.unwrap();

    let statuses: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, status, reason_code FROM skill_library_updates ORDER BY commit_sha",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        statuses,
        vec![
            (rejected, "rejected".to_owned(), None),
            (open, "superseded".to_owned(), None),
        ]
    );
    let row: (Option<String>, Option<i32>, i32, bool, Option<String>, i32) = sqlx::query_as(
        "SELECT last_head_sha, skills_offered, head_epoch, next_refresh_at <= now(),
             default_branch, source_epoch
         FROM skill_libraries",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (None, None, 1, true, None, 0));
}
