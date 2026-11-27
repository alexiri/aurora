#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-SKLIB-2 (D-449, C-1128): `20261201000271_skill_library_instance.sql` on a database that
//! already has libraries. A library that was `over_budget` becomes `active` with a
//! `skill_library_org_state` row for its organization; `org_id` may be null (an instance library);
//! labels and addresses are unique per organization and among the instance libraries (null is one
//! scope, not "distinct nulls"); the state rows go with their library or organization.
use aurora_db::PgPool;
use aurora_testkit::TestDb;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_271;

async fn org(pool: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $2, 'o', $2)")
        .bind(id)
        .bind(format!("o-{id}"))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn lib(
    pool: &PgPool,
    org: Option<Uuid>,
    label: &str,
    url: &str,
    status: &str,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skill_libraries (id, org_id, label, url, repo, status) VALUES ($1,$2,$3,$4,$4,$5)",
    )
    .bind(id)
    .bind(org)
    .bind(label)
    .bind(url)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(id)
}

#[tokio::test]
async fn an_over_budget_library_gets_its_state_row_and_instance_libraries_are_one_scope() {
    let (_db, pool) = TestDb::unmigrated_with_pool().await;
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
    let (a, b) = (org(&pool).await, org(&pool).await);
    let over = lib(
        &pool,
        Some(a),
        "over",
        "https://git.example/a/over",
        "over_budget",
    )
    .await
    .unwrap();
    let fine = lib(
        &pool,
        Some(b),
        "fine",
        "https://git.example/b/fine",
        "active",
    )
    .await
    .unwrap();
    let paused = lib(
        &pool,
        Some(b),
        "paused",
        "https://git.example/b/paused",
        "paused",
    )
    .await
    .unwrap();
    // before: org_id is required
    assert!(
        lib(&pool, None, "inst", "https://git.example/i/one", "active")
            .await
            .is_err()
    );

    aurora_db::migrate(&pool).await.unwrap();

    let status = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT status FROM skill_libraries WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        status(over).await,
        "active",
        "the library's own status no longer says over_budget"
    );
    assert_eq!(status(fine).await, "active");
    assert_eq!(status(paused).await, "paused");
    let states: Vec<(Uuid, Uuid)> =
        sqlx::query_as("SELECT library_id, org_id FROM skill_library_org_state")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        states,
        [(over, a)],
        "only the over-budget library, for its organization"
    );

    // an instance library (org_id null) is allowed, and null is one scope for the unique indexes
    let inst = lib(&pool, None, "shared", "https://git.example/i/one", "active")
        .await
        .unwrap();
    assert!(
        lib(&pool, None, "shared", "https://git.example/i/two", "active")
            .await
            .is_err(),
        "same label"
    );
    assert!(
        lib(&pool, None, "other", "https://git.example/i/one", "active")
            .await
            .is_err(),
        "same address"
    );
    // an organization may reuse the label and the address
    lib(
        &pool,
        Some(a),
        "shared",
        "https://git.example/i/one",
        "active",
    )
    .await
    .unwrap();
    // and the per-organization rule is unchanged
    assert!(
        lib(
            &pool,
            Some(a),
            "shared",
            "https://git.example/i/three",
            "active"
        )
        .await
        .is_err()
    );

    // state rows go with the library and with the organization
    sqlx::query("INSERT INTO skill_library_org_state (library_id, org_id) VALUES ($1, $2)")
        .bind(inst)
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM skill_libraries WHERE id = $1")
        .bind(inst)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM skill_library_org_state")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
