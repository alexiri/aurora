#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-REFINE-5 (D-407, C-1064): `20261201000261_display_name_edited.sql` marks a name that looks
//! chosen ("Bob Builder") as edited and leaves the one that is the e-mail's local part ("alice",
//! the bootstrap admin) for the next sign-in to correct. Case-insensitive.
use aurora_db::PgPool;
use sqlx::migrate::Migrator;

const VERSION: i64 = 20_261_201_000_261;

#[sqlx::test(migrations = false)]
async fn the_backfill_keeps_chosen_names_and_frees_the_local_part_names(pool: PgPool) {
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
    for (email, name) in [
        ("alice@example.com", "alice"),
        ("Carol@Example.com", "CAROL"),
        ("bob@example.com", "Bob Builder"),
        ("dev@example.com", "dev-bob"),
    ] {
        sqlx::query(
            "INSERT INTO users (id, email, display_name) VALUES (gen_random_uuid(), $1, $2)",
        )
        .bind(email)
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }
    aurora_db::MIGRATOR.run(&pool).await.unwrap();
    let edited = |email: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT display_name_edited_at IS NOT NULL FROM users WHERE email = $1::citext",
            )
            .bind(email)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert!(
        !edited("alice@example.com").await,
        "the local part is left for the sign-in"
    );
    assert!(!edited("carol@example.com").await, "case-insensitive");
    assert!(edited("bob@example.com").await, "a chosen name is kept");
    assert!(edited("dev@example.com").await);
}
