#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P6-TPL-2 (C-1108 step 7, SEC-146): a check's `finish_version` moves only a row that still
//! waits for its package. A check that lost its lease (another check re-claimed the source,
//! the new owner ingested the package and an admin already approved it) must not overwrite
//! the approved version — zero rows match and the caller aborts its transaction.

use aurora_db::PgPool;
use aurora_db::repo::agent_templates as repo;
use serde_json::json;
use uuid::Uuid;

async fn seed_source(pool: &PgPool) -> Uuid {
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, display_name) VALUES (gen_random_uuid(), 'tpl@x.io', 'T') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let s = repo::NewSource {
        id: Uuid::now_v7(),
        name: "CERN agent templates",
        url: "https://git.example/cern/templates",
        reference: None,
        path: "",
        token_ref: None,
        homepage: None,
        contact: None,
        packages_url: "https://cdn.example/{id}-{version}.zip",
        manifest: &json!({"api": 1}),
        update_interval: "1d",
        first_check_in_secs: Some(3600),
        created_by: user,
    };
    repo::insert_source(&mut pool.acquire().await.unwrap(), &s)
        .await
        .unwrap()
        .id
}

async fn seed_template(pool: &PgPool, source: Uuid, template_id: &str) -> Uuid {
    let t = repo::NewTemplate {
        id: Uuid::now_v7(),
        template_id,
        source_id: source,
        dir: template_id,
        name: "Service monitor",
        summary: "s",
        head_version: Some("1.0.0"),
        head_state: "ok",
    };
    repo::upsert_template(&mut pool.acquire().await.unwrap(), &t)
        .await
        .unwrap()
        .expect("the seed source owns the template")
        .id
}

async fn seed_version(pool: &PgPool, template: Uuid, version: &str) -> Uuid {
    let commit = "0".repeat(40);
    let v = repo::NewVersion {
        id: Uuid::now_v7(),
        agent_template_id: template,
        version,
        commit_sha: commit.as_str(),
        package_url: "https://cdn.example/t-1.0.0.zip",
    };
    repo::insert_version(pool, &v).await.unwrap().id
}

/// The row's status and content, for the unchanged assertions.
async fn row_status(pool: &PgPool, id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, content_sha256::text FROM agent_template_versions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn the_package_wait_expiry_is_measured_by_the_database_clock(pool: PgPool) {
    aurora_db::migrate(&pool).await.unwrap();
    let source = seed_source(&pool).await;
    let template = seed_template(&pool, source, "service-monitor").await;
    let v = seed_version(&pool, template, "1.0.0").await;

    // A fresh row is inside the 24 h window. The comparison runs entirely in SQL against
    // the database's `now()` (the row's `created_at` is the database's clock), so a
    // replica's clock skew cannot move the expiry.
    assert!(
        !repo::package_wait_expired(&pool, v, 24 * 3600)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE agent_template_versions SET created_at = now() - interval '25 hours'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        repo::package_wait_expired(&pool, v, 24 * 3600)
            .await
            .unwrap()
    );
    // Two windows (the transient extra retry window) still inside at 25 h.
    assert!(
        !repo::package_wait_expired(&pool, v, 2 * 24 * 3600)
            .await
            .unwrap()
    );
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn a_stale_finish_does_not_change_an_approved_version(pool: PgPool) {
    aurora_db::migrate(&pool).await.unwrap();
    let source = seed_source(&pool).await;
    let template = seed_template(&pool, source, "service-monitor").await;

    // The stale check's row: the version was `awaiting_package` when it read it, but the new
    // check ingested it and the admin approved it meanwhile.
    let stale = seed_version(&pool, template, "1.0.0").await;
    sqlx::query(
        "UPDATE agent_template_versions SET status = 'approved', content = '{\"meta\":{}}'::jsonb,
            content_sha256 = $2, decided_at = now()
         WHERE id = $1",
    )
    .bind(stale)
    .bind("a".repeat(64))
    .execute(&pool)
    .await
    .unwrap();

    // A waiting row for the honest write.
    let waiting = seed_version(&pool, template, "1.1.0").await;

    // The stale finish matches nothing; the honest one matches.
    let mut c = pool.acquire().await.unwrap();
    let moved = repo::finish_version(
        &mut c,
        stale,
        "pending_approval",
        None,
        Some(&json!({})),
        Some(&"a".repeat(64)),
        None,
        Some(1),
        true,
    )
    .await
    .unwrap();
    let again = repo::finish_version(
        &mut c,
        stale,
        "pending_approval",
        None,
        None,
        None,
        None,
        None,
        true,
    )
    .await
    .unwrap();
    let honest = repo::finish_version(
        &mut c,
        waiting,
        "pending_approval",
        None,
        Some(&json!({})),
        Some(&"b".repeat(64)),
        None,
        Some(2),
        true,
    )
    .await
    .unwrap();
    drop(c);

    assert_eq!(
        (moved, again, honest),
        (0, 0, 1),
        "a finished (approved) row is never written again"
    );
    let (status, sha) = row_status(&pool, stale).await;
    assert_eq!(status, "approved");
    assert_eq!(sha.as_deref(), Some("a".repeat(64).as_str()));
}
