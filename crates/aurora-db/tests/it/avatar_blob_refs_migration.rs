#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-223 (D-466): `20261201000274_avatar_blob_refs.sql` clears only the avatar pointers whose blob
//! no longer exists (the orphan sweep deleted them), keeps the pointer whose blob is there, leaves
//! `updated_at` alone, adds the sweep's index, and is idempotent.
use aurora_db::PgPool;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_274;

type AvatarRow = (Option<String>, Option<String>, bool);

async fn avatar(
    pool: &PgPool,
    id: Uuid,
    original_updated_at: chrono::DateTime<chrono::Utc>,
) -> AvatarRow {
    sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<String>,
            chrono::DateTime<chrono::Utc>,
        ),
    >("SELECT avatar_blob_key, avatar_public_key, updated_at FROM agents WHERE id = $1")
    .bind(id)
    .fetch_one(pool)
    .await
    .map(|(b, p, u)| (b, p, u == original_updated_at))
    .unwrap()
}

#[sqlx::test(migrations = false)]
async fn only_pointers_to_missing_blobs_are_cleared(pool: PgPool) {
    let before: Vec<_> = aurora_db::MIGRATOR
        .iter()
        .filter(|m| m.version < VERSION)
        .cloned()
        .collect();
    assert!(
        aurora_db::MIGRATOR.iter().any(|m| m.version == VERSION),
        "the migration exists"
    );
    Migrator::with_migrations(before).run(&pool).await.unwrap();

    let (org, ws) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','o','o')")
        .bind(org)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','w')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await
        .unwrap();
    // gone: the blob was swept; kept: the blob is there; none: no avatar at all.
    let mut ids = Vec::new();
    for (handle, blob_key, public_key) in [
        ("gone", Some("blob-gone"), Some("pub-gone")),
        ("kept", Some("blob-kept"), Some("pub-kept")),
        ("none", None, None),
    ] {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, avatar_blob_key, avatar_public_key, updated_at)
             VALUES ($1,$2,$3,$4,$4,'hosted','fake',$5,$6, '2026-01-01T00:00:00Z')",
        )
        .bind(id)
        .bind(org)
        .bind(ws)
        .bind(handle)
        .bind(blob_key)
        .bind(public_key)
        .execute(&pool)
        .await
        .unwrap();
        ids.push(id);
    }
    sqlx::query("INSERT INTO blobs (key, content) VALUES ('blob-kept', 'png')")
        .execute(&pool)
        .await
        .unwrap();
    let t0: chrono::DateTime<chrono::Utc> = "2026-01-01T00:00:00Z".parse().unwrap();

    // Before: both avatar rows still point at a key.
    assert_eq!(
        avatar(&pool, ids[0], t0).await,
        (Some("blob-gone".into()), Some("pub-gone".into()), true)
    );

    aurora_db::MIGRATOR.run(&pool).await.unwrap();

    assert_eq!(
        avatar(&pool, ids[0], t0).await,
        (None, None, true),
        "the dangling pointer is cleared, updated_at untouched"
    );
    assert_eq!(
        avatar(&pool, ids[1], t0).await,
        (Some("blob-kept".into()), Some("pub-kept".into()), true),
        "a pointer whose blob exists is kept"
    );
    assert_eq!(avatar(&pool, ids[2], t0).await, (None, None, true));
    let index: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_indexes WHERE indexname = 'agents_avatar_blob_key_idx'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(index, 1, "the sweep's index exists");
}
