#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-225 (D-469): `20261201000275_followup_controls.sql` adds the follow-up control columns and the
//! `cancelled` status without touching a row that existed before: every existing status stays valid,
//! the new columns are NULL, and `cancelled` is allowed for follow-ups only.
use aurora_db::PgPool;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_275;

type Snapshot = Vec<(Uuid, String, String, Option<chrono::DateTime<chrono::Utc>>)>;

async fn snapshot(pool: &PgPool) -> Snapshot {
    sqlx::query_as("SELECT id, role, status, steer_delivered_at FROM turns ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn existing_rows_are_untouched_and_cancelled_is_for_followups_only(pool: PgPool) {
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

    let (org, ws, agent, conv, parent) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
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
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,'a','a','hosted','fake')")
        .bind(agent)
        .bind(org)
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'api',$1::text, now())")
        .bind(conv)
        .bind(agent)
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    let insert = "INSERT INTO turns (id, conversation_id, agent_id, role, status, parent_turn_id, door, speaker_kind, speaker_label, text, steer_delivered_at)
                  VALUES ($1,$2,$3,$4,$5,$6,'{}','person','Ann','hi',$7)";
    let delivered: Option<chrono::DateTime<chrono::Utc>> =
        Some("2026-01-01T00:00:00Z".parse().unwrap());
    sqlx::query(insert)
        .bind(parent)
        .bind(conv)
        .bind(agent)
        .bind("turn")
        .bind("running")
        .bind(None::<Uuid>)
        .bind(None::<chrono::DateTime<chrono::Utc>>)
        .execute(&pool)
        .await
        .unwrap();
    let mut followups = Vec::new();
    for status in ["joining", "joined", "promoted"] {
        let id = Uuid::now_v7();
        sqlx::query(insert)
            .bind(id)
            .bind(conv)
            .bind(agent)
            .bind("followup")
            .bind(status)
            .bind(Some(parent))
            .bind(delivered)
            .execute(&pool)
            .await
            .unwrap();
        followups.push(id);
    }
    let rows_before = snapshot(&pool).await;
    assert_eq!(rows_before.len(), 4);

    aurora_db::MIGRATOR.run(&pool).await.unwrap();

    assert_eq!(snapshot(&pool).await, rows_before, "no row changed");
    let nulls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM turns WHERE steer_requested_at IS NULL AND steer_error IS NULL AND edited_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(nulls, 4, "the new columns start empty");

    // `cancelled` is a follow-up status only; `steer_error` is `missed` or `refused`.
    sqlx::query("UPDATE turns SET status = 'cancelled' WHERE id = $1")
        .bind(followups[0])
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE turns SET status = 'cancelled' WHERE id = $1")
            .bind(parent)
            .execute(&pool)
            .await
            .is_err(),
        "a turn cannot be cancelled"
    );
    sqlx::query("UPDATE turns SET steer_error = 'missed' WHERE id = $1")
        .bind(followups[1])
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE turns SET steer_error = 'nope' WHERE id = $1")
            .bind(followups[2])
            .execute(&pool)
            .await
            .is_err()
    );
}
