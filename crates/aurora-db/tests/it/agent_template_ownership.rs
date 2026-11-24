#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The template-id ownership race: `upsert_template` must never move a template (with all
//! its stored versions) from one source to another. The `id_taken_elsewhere` pre-check is
//! an unlocked read, so a second source can pass it and then land its upsert after the
//! first source took the id — the upsert itself must refuse to change the owner and
//! return no row, which the caller reports as `id_taken` exactly like the pre-check.

use aurora_db::PgPool;
use aurora_db::repo::agent_templates as repo;
use uuid::Uuid;

async fn seed_source(pool: &PgPool, name: &str) -> Uuid {
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, display_name) VALUES (gen_random_uuid(), $1, 'T') RETURNING id",
    )
    .bind(format!("{name}.tpl@x.io"))
    .fetch_one(pool)
    .await
    .unwrap();
    let url = format!("https://git.example/{name}/templates");
    let s = repo::NewSource {
        id: Uuid::now_v7(),
        name,
        url: &url,
        reference: None,
        path: "",
        token_ref: None,
        homepage: None,
        contact: None,
        packages_url: "https://cdn.example/{id}-{version}.zip",
        manifest: &serde_json::json!({"api": 1}),
        update_interval: "1d",
        first_check_in_secs: Some(3600),
        created_by: user,
    };
    repo::insert_source(&mut pool.acquire().await.unwrap(), &s)
        .await
        .unwrap()
        .id
}

fn new_template(source: Uuid, template_id: &str) -> repo::NewTemplate<'_> {
    repo::NewTemplate {
        id: Uuid::now_v7(),
        template_id,
        source_id: source,
        dir: template_id,
        name: "Service monitor",
        summary: "s",
        head_version: Some("1.0.0"),
        head_state: "ok",
    }
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn a_lost_ownership_race_reports_id_taken_instead_of_stealing(pool: PgPool) {
    aurora_db::migrate(&pool).await.unwrap();
    let a = seed_source(&pool, "first").await;
    let b = seed_source(&pool, "second").await;

    // Source B's unlocked pre-check runs first and sees the id as free.
    assert!(
        !repo::id_taken_elsewhere(&pool, "service-monitor", b)
            .await
            .unwrap()
    );

    // Source A wins the race: it stores the template with a version, and only then
    // commits (the transaction is held open, as the apply-race tests hold their locks).
    let mut tx = pool.begin().await.unwrap();
    let row = repo::upsert_template(&mut tx, &new_template(a, "service-monitor"))
        .await
        .unwrap()
        .expect("the first source takes the brand-new id");
    repo::insert_version(
        &mut *tx,
        &repo::NewVersion {
            id: Uuid::now_v7(),
            agent_template_id: row.id,
            version: "1.0.0",
            commit_sha: &"0".repeat(40),
            package_url: "https://cdn.example/service-monitor-1.0.0.zip",
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Source B's upsert lands after A's commit: it must get no row (the caller reports
    // `id_taken`), not steal the template with its versions.
    let stolen = repo::upsert_template(
        &mut pool.acquire().await.unwrap(),
        &new_template(b, "service-monitor"),
    )
    .await
    .unwrap();
    assert!(
        stolen.is_none(),
        "the upsert moved the template to source B"
    );

    // Exactly one owner: A keeps the row and its versions.
    let owner: Uuid = sqlx::query_scalar(
        "SELECT source_id FROM agent_templates WHERE template_id = 'service-monitor'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owner, a);
    let versions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_template_versions WHERE agent_template_id = $1",
    )
    .bind(row.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(versions, 1, "the first source's versions must stay");

    // The owner still updates its own row through the same upsert.
    let again = repo::upsert_template(
        &mut pool.acquire().await.unwrap(),
        &new_template(a, "service-monitor"),
    )
    .await
    .unwrap();
    assert_eq!(again.map(|r| r.id), Some(row.id));
}

/// A check extends the lease of the source it is still working on: only the current owner
/// succeeds, and a released (or stolen) lease cannot be extended.
#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn extend_lease_only_succeeds_for_the_current_owner(pool: PgPool) {
    aurora_db::migrate(&pool).await.unwrap();
    let source = seed_source(&pool, "first").await;
    let owner = Uuid::now_v7();
    let other = Uuid::now_v7();

    // The check claims the source, then extends while it works: the lease moves.
    repo::claim_one(&mut pool.acquire().await.unwrap(), source, owner, 300.0)
        .await
        .unwrap()
        .expect("a free source is claimed");
    let epoch = |pool: &PgPool| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, f64>(
                "SELECT EXTRACT(EPOCH FROM check_lease_until)::float8 FROM agent_template_sources
                 WHERE id = $1",
            )
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let before = epoch(&pool).await;
    assert!(
        repo::extend_lease(&mut pool.acquire().await.unwrap(), source, owner, 300.0)
            .await
            .unwrap(),
        "the current owner extends the lease"
    );
    assert!(
        epoch(&pool).await > before,
        "the lease moved into the future"
    );

    // Another replica's extend is refused: the owner (and its lease) stays.
    assert!(
        !repo::extend_lease(&mut pool.acquire().await.unwrap(), source, other, 300.0)
            .await
            .unwrap(),
        "a non-owner extended the lease"
    );
    let lease_owner: Uuid =
        sqlx::query_scalar("SELECT lease_owner FROM agent_template_sources WHERE id = $1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(lease_owner, owner);

    // A released source cannot be extended either.
    repo::release_lease(&mut pool.acquire().await.unwrap(), source, owner)
        .await
        .unwrap();
    assert!(
        !repo::extend_lease(&mut pool.acquire().await.unwrap(), source, owner, 300.0)
            .await
            .unwrap(),
        "extended a lease the source does not hold"
    );
}
