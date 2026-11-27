#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-MEMORY-1 (C-1169, D-497): `20261201000288_memory_provenance.sql` on a database with
//! existing memories: the adder comes from revision 1 only while it is kept, the last changer
//! from the newest revision, every kind of older provenance maps to its actor, what cannot be
//! known stays `unknown`, and the temporary function is gone.
use aurora_testkit::TestDb;
use serde_json::{Value, json};
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_288;

#[tokio::test]
async fn the_backfill_reads_the_oldest_and_newest_kept_revision() {
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

    let (o, w, a, u, conv) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $1::text, 'o', $1::text)")
        .bind(o)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1, $2, $1::text, 'w')")
        .bind(w)
        .bind(o)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','a','hosted')")
        .bind(a)
        .bind(o)
        .bind(w)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, 'p@example.com', 'P')")
        .bind(u)
        .execute(&pool)
        .await
        .unwrap();

    // (key, current revision, kept revisions: (number, author kind, user, provenance))
    type Rev = (i32, &'static str, Option<Uuid>, Value);
    let tool = json!({"source": "tool", "conversation_id": conv, "turn_id": null});
    let dash = json!({"source": "dashboard"});
    let memories: Vec<(&str, i32, Vec<Rev>)> = vec![
        (
            "learned",
            2,
            vec![
                (1, "agent", None, tool.clone()),
                (2, "user", Some(u), dash.clone()),
            ],
        ),
        (
            "by-key",
            1,
            vec![(1, "user", Some(u), json!({"source": "api"}))],
        ),
        (
            "templated",
            1,
            vec![(
                1,
                "user",
                Some(u),
                json!({"source": "template", "template": "ops", "template_version": "1.2.0"}),
            )],
        ),
        (
            "harvested",
            1,
            vec![(1, "agent", None, json!({"source": "harvest"}))],
        ),
        // Revision 1 was pruned: the adder is unknown, the changer known.
        (
            "pruned",
            53,
            vec![
                (4, "user", Some(u), dash.clone()),
                (53, "agent", None, tool.clone()),
            ],
        ),
        ("bare", 1, vec![]),
        (
            "nobody",
            1,
            vec![(1, "user", None, json!({"source": "dashboard"}))],
        ),
    ];
    for (key, revision, revs) in memories {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO memories (id, agent_id, key, content, revision) VALUES ($1,$2,$3,'c',$4)",
        )
        .bind(id)
        .bind(a)
        .bind(key)
        .bind(revision)
        .execute(&pool)
        .await
        .unwrap();
        for (n, kind, user, provenance) in revs {
            sqlx::query(
                "INSERT INTO memory_revisions (id, memory_id, revision, op, author_kind,
                    author_user_id, author_label, content, provenance)
                 VALUES ($1,$2,$3,'save',$4,$5,'p@example.com','c',$6)",
            )
            .bind(Uuid::now_v7())
            .bind(id)
            .bind(n)
            .bind(kind)
            .bind(user)
            .bind(provenance)
            .execute(&pool)
            .await
            .unwrap();
        }
    }

    aurora_db::MIGRATOR.run(&pool).await.unwrap();

    let actor = |key: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (Value, Value, Value, bool)>(
                "SELECT added_by, changed_by, last_used_marks, last_used_at IS NULL
                   FROM memories WHERE key = $1",
            )
            .bind(key)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let unknown = json!({"kind": "unknown"});
    let agent_in_conv = json!({"kind": "agent", "conversation_id": conv});
    let person = json!({"kind": "person", "user_id": u});
    let cases = [
        ("learned", agent_in_conv.clone(), person.clone()),
        (
            "by-key",
            json!({"kind": "api_key", "user_id": u}),
            json!({"kind": "api_key", "user_id": u}),
        ),
        (
            "templated",
            json!({"kind": "template", "user_id": u, "template": {"name": "ops", "version": "1.2.0"}}),
            json!({"kind": "template", "user_id": u, "template": {"name": "ops", "version": "1.2.0"}}),
        ),
        (
            "harvested",
            json!({"kind": "agent"}),
            json!({"kind": "agent"}),
        ),
        ("pruned", unknown.clone(), agent_in_conv),
        ("bare", unknown.clone(), unknown.clone()),
        ("nobody", unknown.clone(), unknown),
    ];
    for (key, added, changed) in cases {
        let (a, c, marks, never_used) = actor(key).await;
        assert_eq!((a, c), (added, changed), "{key}");
        assert_eq!((marks, never_used), (json!({}), true), "{key}");
    }
    let keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_revisions WHERE author_api_key_id IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(keys, 0, "older revisions recorded no key");
    let helper: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_proc WHERE proname = 'memory_revision_actor'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(helper, 0);
}
