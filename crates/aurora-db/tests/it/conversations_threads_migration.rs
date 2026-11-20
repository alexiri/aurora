#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-REFINE-7 (D-409, C-1073, C-1074, C-1079): `20261201000262_conversations_threads.sql` on a
//! database with existing direct, schedule and webhook conversations: the one-chat unique index
//! goes (a second direct chat is possible), the backfill folds `schedule:<id>` / `webhook:<id>`
//! under their automation with its current name, a gone automation is marked deleted, other
//! conversations are untouched, and `webhook_deliveries` accepts `duplicate`. The old creators'
//! insert (no new columns) still works afterwards.
use aurora_db::PgPool;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_262;

type Folded = (Option<String>, Option<Uuid>, bool, String, Option<String>);

#[sqlx::test(migrations = false)]
async fn the_backfill_folds_existing_threads_and_the_one_chat_rule_goes(pool: PgPool) {
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

    let (o, w, a, u) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let q = |sql: &'static str| sqlx::query(sql);
    q("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $1::text, 'o', $1::text)")
        .bind(o)
        .execute(&pool)
        .await
        .unwrap();
    q("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1, $2, $1::text, 'w')")
        .bind(w)
        .bind(o)
        .execute(&pool)
        .await
        .unwrap();
    q("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','a','hosted')")
        .bind(a)
        .bind(o)
        .bind(w)
        .execute(&pool)
        .await
        .unwrap();
    q("INSERT INTO users (id, email, display_name) VALUES ($1, 'p@example.com', 'P')")
        .bind(u)
        .execute(&pool)
        .await
        .unwrap();
    let (live_s, gone_s, live_w, gone_w) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    q("INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone) VALUES ($1,$2,'Digest (renamed)','p','0 9 * * *','UTC')")
        .bind(live_s)
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    q("INSERT INTO webhooks (id, agent_id, name, instructions, token_hash, display_prefix) VALUES ($1,$2,'Deploy','i',$3,'auw_x')")
        .bind(live_w)
        .bind(a)
        .bind(vec![7u8; 32])
        .execute(&pool)
        .await
        .unwrap();
    let conv =
        |kind: &'static str, ext: String, direct: Option<Uuid>, title: Option<&'static str>| {
            let pool = pool.clone();
            async move {
                let id = Uuid::now_v7();
                sqlx::query(
                    "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref,
                    direct_user_id, title, last_activity_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7, now())",
                )
                .bind(id)
                .bind(a)
                .bind(w)
                .bind(kind)
                .bind(ext)
                .bind(direct)
                .bind(title)
                .execute(&pool)
                .await
                .unwrap();
                id
            }
        };
    let direct = conv("direct", format!("user:{u}"), Some(u), None).await;
    let c_live_s = conv(
        "schedule",
        format!("schedule:{live_s}"),
        None,
        Some("Digest"),
    )
    .await;
    let c_gone_s = conv("schedule", format!("schedule:{gone_s}"), None, Some("Old")).await;
    let c_live_w = conv("webhook", format!("webhook:{live_w}"), None, Some("Deploy")).await;
    let c_gone_w = conv(
        "webhook",
        format!("webhook:{gone_w}"),
        None,
        Some("Gone hook"),
    )
    .await;
    let c_api = conv("api", "schedule:not-a-uuid".into(), None, Some("T")).await;
    // Before: the one-chat rule refuses a second direct conversation.
    let second = format!("user:{u}:{}", Uuid::now_v7());
    assert!(
        sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id, last_activity_at) VALUES (gen_random_uuid(),$1,$2,'direct',$3,$4, now())")
            .bind(a).bind(w).bind(&second).bind(u)
            .execute(&pool).await.is_err()
    );

    aurora_db::MIGRATOR.run(&pool).await.unwrap();

    let folded = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, Folded>(
                "SELECT automation_kind, automation_id, automation_deleted_at IS NOT NULL,
                        title_source, title
                 FROM conversations WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        folded(c_live_s).await,
        (
            Some("schedule".into()),
            Some(live_s),
            false,
            "automation".into(),
            Some("Digest (renamed)".into())
        ),
        "folded under the schedule, titled with its current name"
    );
    assert_eq!(
        folded(c_gone_s).await,
        (
            Some("schedule".into()),
            Some(gone_s),
            true,
            "automation".into(),
            Some("Old".into())
        )
    );
    assert_eq!(
        folded(c_live_w).await,
        (
            Some("webhook".into()),
            Some(live_w),
            false,
            "automation".into(),
            Some("Deploy".into())
        )
    );
    assert!(folded(c_gone_w).await.2, "a gone webhook's thread is marked");
    assert_eq!(
        folded(direct).await,
        (None, None, false, "auto".into(), None)
    );
    assert_eq!(
        folded(c_api).await,
        (None, None, false, "auto".into(), Some("T".into()))
    );
    // After: a second direct chat, and the old creators' insert (no new columns) still work.
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id, last_activity_at) VALUES (gen_random_uuid(),$1,$2,'direct',$3,$4, now())")
        .bind(a).bind(w).bind(&second).bind(u)
        .execute(&pool).await.unwrap();
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes WHERE tablename = 'conversations' ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!indexes.contains(&"conversations_direct_user_uq".to_owned()));
    assert!(indexes.contains(&"conversations_direct_person_idx".to_owned()));
    assert!(indexes.contains(&"conversations_automation_idx".to_owned()));
    // Both or neither of the automation pair.
    assert!(
        sqlx::query("UPDATE conversations SET automation_kind = 'schedule' WHERE id = $1")
            .bind(c_api)
            .execute(&pool)
            .await
            .is_err()
    );
    // `duplicate` deliveries are recorded.
    sqlx::query(
        "INSERT INTO webhook_deliveries (id, webhook_id, status, size_bytes, reason)
         VALUES (gen_random_uuid(), $1, 'duplicate', 1, $2)",
    )
    .bind(live_w)
    .bind(Uuid::now_v7().to_string())
    .execute(&pool)
    .await
    .unwrap();
}
