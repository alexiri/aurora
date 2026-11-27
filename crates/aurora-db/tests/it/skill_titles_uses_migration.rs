#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-SKILLS-1 (C-1173..C-1176): `20261201000289` and `20261201000290` on a database that
//! already has skills and libraries. Existing skills keep no title and nothing is off; a title
//! is one line of 1..80 characters; the per-agent switch and the uses go with their agent and
//! skill; a deleted conversation or turn keeps the use; one use per turn and skill.
use aurora_db::PgPool;
use aurora_testkit::TestDb;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const FIRST: i64 = 20_261_201_000_289;
const SECOND: i64 = 20_261_201_000_290;

async fn exec(pool: &PgPool, sql: &str, ids: &[Uuid]) -> Result<u64, sqlx::Error> {
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
    for id in ids {
        q = q.bind(*id);
    }
    q.execute(pool).await.map(|r| r.rows_affected())
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn titles_switches_and_uses_on_an_upgraded_database() {
    let (_db, pool) = TestDb::unmigrated_with_pool().await;
    for v in [FIRST, SECOND] {
        assert!(aurora_db::MIGRATOR.iter().any(|m| m.version == v), "{v}");
    }
    let before: Vec<_> = aurora_db::MIGRATOR
        .iter()
        .filter(|m| m.version != FIRST && m.version != SECOND)
        .cloned()
        .collect();
    Migrator::with_migrations(before).run(&pool).await.unwrap();
    let ids: Vec<Uuid> = (0..6).map(|_| Uuid::now_v7()).collect();
    let [org, ws, agent, skill, conv, turn] = ids[..] else {
        unreachable!()
    };
    exec(
        &pool,
        "INSERT INTO organizations (id, slug, name, external_id)
         VALUES ($1, 'o', 'o', 'o')",
        &[org],
    )
    .await
    .unwrap();
    exec(
        &pool,
        "INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1, $2, 'w', 'w')",
        &[ws, org],
    )
    .await
    .unwrap();
    exec(
        &pool,
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1, $2, $3, 'a', 'a', 'hosted', 'fake')",
        &[agent, org, ws],
    )
    .await
    .unwrap();
    exec(
        &pool,
        "INSERT INTO skills (id, org_id, name, description, body, created_by_kind)
         VALUES ($1, $2, 'pdf', 'd', 'b', 'user')",
        &[skill, org],
    )
    .await
    .unwrap();

    aurora_db::migrate(&pool).await.unwrap();

    assert_eq!(
        count(&pool, "SELECT count(*) FROM skills WHERE title IS NULL").await,
        1
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM agent_skills_off").await,
        0
    );
    for bad in ["''", "repeat('x', 81)", "E'a\\nb'", "E'a\\tb'"] {
        let r = exec(&pool, &format!("UPDATE skills SET title = {bad}"), &[]).await;
        assert!(r.is_err(), "{bad} was stored");
    }
    exec(&pool, "UPDATE skills SET title = repeat('é', 80)", &[])
        .await
        .unwrap();

    exec(
        &pool,
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref,
             last_activity_at)
         VALUES ($1, $2, $3, 'api', 'x', now())",
        &[conv, agent, ws],
    )
    .await
    .unwrap();
    exec(
        &pool,
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind,
             speaker_label, text)
         VALUES ($1, $2, $3, 'turn', 'done', '{}', 'system', 's', 'x')",
        &[turn, conv, agent],
    )
    .await
    .unwrap();
    let use_sql = "INSERT INTO skill_uses (skill_id, agent_id, conversation_id, turn_id)
                   VALUES ($1, $2, $3, $4)";
    exec(&pool, use_sql, &[skill, agent, conv, turn])
        .await
        .unwrap();
    assert!(
        exec(&pool, use_sql, &[skill, agent, conv, turn])
            .await
            .is_err(),
        "one use per turn and skill"
    );
    exec(
        &pool,
        "INSERT INTO agent_skills_off (agent_id, skill_id) VALUES ($1, $2)",
        &[agent, skill],
    )
    .await
    .unwrap();
    // The conversation (and its turn) going keeps the use without the reference.
    exec(&pool, "DELETE FROM conversations WHERE id = $1", &[conv])
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM skill_uses WHERE conversation_id IS NULL AND turn_id IS NULL"
        )
        .await,
        1
    );
    // The skill going takes its switches and uses.
    exec(&pool, "DELETE FROM skills WHERE id = $1", &[skill])
        .await
        .unwrap();
    for t in ["agent_skills_off", "skill_uses"] {
        assert_eq!(
            count(&pool, &format!("SELECT count(*) FROM {t}")).await,
            0,
            "{t}"
        );
    }
}
