#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-REFINE-8 (C-1081): `20261201000263_app_setup_request_reason.sql` applies after the newer `…000265`
//! (the order an upgraded database sees) and backfills `closed_by_role`: a decline with a decliner is an
//! organization owner's or admin's, a resolution the system's; open, withdrawn and decliner-less rows stay
//! null. The two free-text columns are bounded to 300 characters.
use aurora_testkit::TestDb;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_263;

#[tokio::test]
async fn the_backfill_names_the_closing_role_and_the_texts_are_bounded() {
    let (_db, pool) = TestDb::unmigrated_with_pool().await;
    let without: Vec<_> = aurora_db::MIGRATOR
        .iter()
        // Everything before it, never a later migration: a later one may need its columns.
        .filter(|m| m.version < VERSION)
        .cloned()
        .collect();
    assert!(
        aurora_db::MIGRATOR.iter().any(|m| m.version == VERSION),
        "the migration exists"
    );
    Migrator::with_migrations(without).run(&pool).await.unwrap();
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $2, 'o', $2)")
        .bind(org)
        .bind(format!("o-{org}"))
        .execute(&pool)
        .await
        .unwrap();
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, display_name) VALUES (gen_random_uuid(), 'a@x.io', 'Adam') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut ids = Vec::new();
    for (i, (status, by)) in [
        ("declined", Some(user)),
        ("declined", None),
        ("resolved", None),
        ("withdrawn", None),
        ("open", None),
    ]
    .into_iter()
    .enumerate()
    {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO app_setup_requests (id, org_id, app_id, status, closed_by, closed_at) \
             VALUES ($1, $2, $3, $4, $5, CASE WHEN $4 = 'open' THEN NULL ELSE now() END)",
        )
        .bind(id)
        .bind(org)
        .bind(format!("app{i}"))
        .bind(status)
        .bind(by)
        .execute(&pool)
        .await
        .unwrap();
        ids.push(id);
    }
    aurora_db::migrate(&pool).await.unwrap();
    let mut roles = Vec::new();
    for id in &ids {
        let r: Option<String> =
            sqlx::query_scalar("SELECT closed_by_role FROM app_setup_requests WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        roles.push(r);
    }
    assert_eq!(
        roles,
        [
            Some("org_admin".to_owned()),
            None,
            Some("system".to_owned()),
            None,
            None
        ]
    );
    // The free texts: 1..300 characters, and only the three roles.
    let set = |sql: &'static str, v: String| {
        let pool = pool.clone();
        let id = ids[0];
        async move {
            sqlx::query(sql)
                .bind(id)
                .bind(v)
                .execute(&pool)
                .await
                .is_ok()
        }
    };
    let msg = "UPDATE app_setup_requests SET closed_message = $2 WHERE id = $1";
    assert!(set(msg, "x".repeat(300)).await);
    assert!(!set(msg, "x".repeat(301)).await);
    assert!(!set(msg, String::new()).await);
    assert!(
        !set(
            "UPDATE app_setup_requests SET closed_by_role = $2 WHERE id = $1",
            "owner".into()
        )
        .await
    );
}
