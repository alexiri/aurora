#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-REFINE-10 (C-1085): `20261201000264_agent_link_direction.sql` keeps every existing link
//! as it was: `direction = both`, created by an agent, no creator person.
use aurora_testkit::TestDb;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_264;

#[tokio::test]
async fn existing_links_stay_both_ways_and_agent_created() {
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
    let mut agents = Vec::new();
    for handle in ["a", "b", "c"] {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
             VALUES ($1,$2,$3,$4,$4,'hosted','fake')",
        )
        .bind(id)
        .bind(org)
        .bind(ws)
        .bind(handle)
        .execute(&pool)
        .await
        .unwrap();
        agents.push(id);
    }
    for (x, y, p) in [
        (agents[0], agents[1], "allow"),
        (agents[1], agents[2], "ask"),
    ] {
        sqlx::query(
            "INSERT INTO agent_links (id, agent_a_id, agent_b_id, policy_a, policy_b)
             VALUES ($1,$2,$3,$4,$4)",
        )
        .bind(Uuid::now_v7())
        .bind(x.min(y))
        .bind(x.max(y))
        .bind(p)
        .execute(&pool)
        .await
        .unwrap();
    }
    aurora_db::MIGRATOR.run(&pool).await.unwrap();
    let rows: Vec<(String, String, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT direction, created_by_kind, created_by_user_id, policy_a FROM agent_links
         ORDER BY policy_a",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for (direction, kind, user, _) in &rows {
        assert_eq!(direction, "both");
        assert_eq!(kind, "agent");
        assert_eq!(*user, None);
    }
    assert_eq!((rows[0].3.as_str(), rows[1].3.as_str()), ("allow", "ask"));
    // The checks hold for new rows.
    let bad = sqlx::query("UPDATE agent_links SET direction = 'sideways'")
        .execute(&pool)
        .await;
    assert!(bad.is_err(), "direction is checked");
}
