#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-USAGE-1 (D-359, C-1001, C-1002): `20261201000247_usage_ledger.sql` backfills `usage_daily` from the
//! terminal, started turns that exist, with the rule of the live write (`repo::usage::record_turn`), and
//! the runtime rollup (`repo::usage::rollup_runtime`) splits intervals by UTC day, counts an open interval
//! up to the run, is idempotent, catches up, and moves its watermark only when a run succeeds.
use aurora_db::PgPool;
use aurora_db::repo::usage::{record_turn, rollup_runtime, runtime_as_of};
use chrono::{DateTime, Utc};
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_247;

async fn fixture(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let (o, w, a) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $1::text, 'o', $1::text)")
        .bind(o)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1, $2, $1::text, 'w')")
        .bind(w)
        .bind(o)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','a','hosted')")
        .bind(a)
        .bind(o)
        .bind(w)
        .execute(pool)
        .await
        .unwrap();
    (o, w, a)
}

#[allow(clippy::too_many_arguments)]
async fn turn(
    pool: &PgPool,
    ws: Uuid,
    agent: Uuid,
    status: &str,
    started: Option<&str>,
    finished: Option<&str>,
    usage: Option<serde_json::Value>,
    basis: Option<serde_json::Value>,
) -> Uuid {
    let (c, t) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'api',$1::text, now())")
        .bind(c)
        .bind(agent)
        .bind(ws)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text,
                            started_at, finished_at, usage, model_basis)
         VALUES ($1,$2,$3,'turn',$4,'{\"kind\":\"api\"}','system','s','t', $5::timestamptz, $6::timestamptz, $7, $8)",
    )
    .bind(t)
    .bind(c)
    .bind(agent)
    .bind(status)
    .bind(started)
    .bind(finished)
    .bind(usage)
    .bind(basis)
    .execute(pool)
    .await
    .unwrap();
    t
}

type Row = (
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
);

async fn ledger(pool: &PgPool) -> Vec<Row> {
    sqlx::query_as(
        "SELECT provider, endpoint_id, model, day::text, turns, turn_seconds, input_tokens, output_tokens,
                cache_read_tokens, cache_write_tokens
         FROM usage_daily ORDER BY day, provider NULLS FIRST, endpoint_id NULLS FIRST, model NULLS FIRST",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = false)]
async fn the_backfill_equals_what_the_live_rule_writes(pool: PgPool) {
    let before: Vec<_> = aurora_db::MIGRATOR
        .iter()
        .filter(|m| m.version != VERSION)
        .cloned()
        .collect();
    Migrator::with_migrations(before).run(&pool).await.unwrap();
    // `turns` rows of every shape, written before the ledger exists.
    let (_, w, a) = fixture(&pool).await;
    let basis = serde_json::json!({"provider": "litellm", "endpoint_id": "ep", "model": "m-def"});
    let entry = |m: &str, i: i64, o: i64| {
        serde_json::json!({"model": m, "input_tokens": i, "output_tokens": o,
                           "cache_read_tokens": 1, "cache_write_tokens": 2})
    };
    let mut ids = vec![];
    // with `by_model`: two entries
    ids.push(
        turn(
            &pool,
            w,
            a,
            "done",
            Some("2026-11-10 23:59:50+00"),
            Some("2026-11-11 00:00:07.9+00"),
            Some(
                serde_json::json!({"input_tokens": 300, "output_tokens": 30, "provider": "litellm",
                "endpoint_id": "ep", "model": "m-def",
                "by_model": [entry("m-def", 100, 10), entry("m-big", 200, 20)]}),
            ),
            Some(basis.clone()),
        )
        .await,
    );
    // totals only, no breakdown
    ids.push(
        turn(&pool, w, a, "failed", Some("2026-11-11 10:00:00+00"), Some("2026-11-11 10:00:03+00"),
            Some(serde_json::json!({"input_tokens": 5, "output_tokens": 1, "cache_read_tokens": 9.5, "model": "m-old"})),
            None).await,
    );
    // no usage, a basis
    ids.push(
        turn(
            &pool,
            w,
            a,
            "aborted",
            Some("2026-11-11 11:00:00+00"),
            Some("2026-11-11 11:00:01+00"),
            None,
            Some(basis.clone()),
        )
        .await,
    );
    // no usage, no basis (a turn from before the stamps)
    ids.push(
        turn(
            &pool,
            w,
            a,
            "done",
            Some("2026-11-11 12:00:00+00"),
            Some("2026-11-11 12:00:02+00"),
            None,
            None,
        )
        .await,
    );
    // never started: nothing
    ids.push(
        turn(
            &pool,
            w,
            a,
            "aborted",
            None,
            Some("2026-11-11 13:00:00+00"),
            None,
            Some(basis.clone()),
        )
        .await,
    );
    // active: nothing
    ids.push(
        turn(
            &pool,
            w,
            a,
            "running",
            Some("2026-11-11 14:00:00+00"),
            None,
            None,
            Some(basis.clone()),
        )
        .await,
    );

    aurora_db::migrate(&pool).await.unwrap();
    let backfilled = ledger(&pool).await;
    // the first turn ends on the 11th (UTC), its seconds floored, tokens per entry
    let s = |x: &str| Some(x.to_owned());
    // (the first turn's 17.9 s floor to 17, the aborted basis-only turn adds one turn and 1 s)
    assert!(
        backfilled.contains(&(
            s("litellm"),
            s("ep"),
            s("m-def"),
            "2026-11-11".into(),
            2,
            18,
            100,
            10,
            1,
            2
        )),
        "{backfilled:?}"
    );
    assert!(
        backfilled.contains(&(
            s("litellm"),
            s("ep"),
            s("m-big"),
            "2026-11-11".into(),
            0,
            0,
            200,
            20,
            1,
            2
        )),
        "{backfilled:?}"
    );
    // totals under the default model of the claim; a non-integer counter reads as 0
    assert!(
        backfilled.contains(&(
            None,
            None,
            s("m-old"),
            "2026-11-11".into(),
            1,
            3,
            5,
            1,
            0,
            0
        )),
        "{backfilled:?}"
    );
    // a turn with no usage and no basis: only nulls
    assert!(
        backfilled.contains(&(None, None, None, "2026-11-11".into(), 1, 2, 0, 0, 0, 0)),
        "{backfilled:?}"
    );
    let turns: i64 = backfilled.iter().map(|r| r.4).sum();
    assert_eq!(
        turns, 4,
        "the never-started and the active turn count nothing"
    );

    // The live rule, run on the same turns, writes the same ledger.
    sqlx::query("TRUNCATE usage_daily")
        .execute(&pool)
        .await
        .unwrap();
    for id in ids {
        let mut c = pool.acquire().await.unwrap();
        record_turn(&mut c, id).await.unwrap();
    }
    assert_eq!(ledger(&pool).await, backfilled);
    // and a second record of a turn would add again: the fence is the caller's (`turns::finish`)
}

fn at(s: &str) -> DateTime<Utc> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .unwrap()
        .and_utc()
}

async fn interval(
    pool: &PgPool,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    from: &str,
    to: Option<&str>,
    updated: &str,
) {
    sqlx::query(
        "INSERT INTO runtime_intervals (id, org_id, workspace_id, agent_id, sandbox_id, boot_id, started_at, ended_at, updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7::timestamptz,$8::timestamptz,$9::timestamptz)",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(ws)
    .bind(agent)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(format!("{from}+00"))
    .bind(to.map(|t| format!("{t}+00")))
    .bind(format!("{updated}+00"))
    .execute(pool)
    .await
    .unwrap();
}

async fn awake(pool: &PgPool, agent: Uuid) -> Vec<(String, i64)> {
    sqlx::query_as(
        "SELECT day::text, awake_seconds FROM runtime_daily WHERE agent_id = $1 ORDER BY day",
    )
    .bind(agent)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn the_rollup_splits_days_counts_open_intervals_and_is_idempotent(pool: PgPool) {
    let (o, w, a) = fixture(&pool).await;
    // across UTC midnight: 2 h on the 10th, 2.5 h on the 11th
    interval(
        &pool,
        o,
        w,
        a,
        "2026-11-10 22:00:00",
        Some("2026-11-11 02:30:00"),
        "2026-11-11 02:30:00",
    )
    .await;
    // open: from 23:00 on the 11th up to the run at 01:00 on the 12th
    interval(
        &pool,
        o,
        w,
        a,
        "2026-11-11 23:00:00",
        None,
        "2026-11-11 23:00:00",
    )
    .await;
    let mut c = pool.acquire().await.unwrap();
    assert_eq!(runtime_as_of(&mut c).await.unwrap(), None);
    let t1 = at("2026-11-12 01:00:00");
    rollup_runtime(&mut c, t1).await.unwrap();
    let want = vec![
        ("2026-11-10".to_owned(), 7200),
        ("2026-11-11".to_owned(), 9000 + 3600),
        ("2026-11-12".to_owned(), 3600),
    ];
    assert_eq!(awake(&pool, a).await, want);
    assert_eq!(runtime_as_of(&mut c).await.unwrap(), Some(t1));
    // the workspace id is stored for the views
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_daily WHERE workspace_id = $1")
        .bind(w)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 3);
    // the same run again changes nothing
    rollup_runtime(&mut c, t1).await.unwrap();
    assert_eq!(awake(&pool, a).await, want);
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn a_run_after_a_missed_day_catches_up_and_a_failed_run_keeps_the_watermark(pool: PgPool) {
    let (o, w, a) = fixture(&pool).await;
    let mut c = pool.acquire().await.unwrap();
    interval(
        &pool,
        o,
        w,
        a,
        "2026-11-10 08:00:00",
        Some("2026-11-10 09:00:00"),
        "2026-11-10 09:00:00",
    )
    .await;
    let t1 = at("2026-11-10 12:00:00");
    rollup_runtime(&mut c, t1).await.unwrap();
    assert_eq!(awake(&pool, a).await, vec![("2026-11-10".to_owned(), 3600)]);

    // downtime: two intervals end while no run happens
    interval(
        &pool,
        o,
        w,
        a,
        "2026-11-11 08:00:00",
        Some("2026-11-11 08:10:00"),
        "2026-11-11 08:10:00",
    )
    .await;
    interval(
        &pool,
        o,
        w,
        a,
        "2026-11-12 08:00:00",
        Some("2026-11-12 08:20:00"),
        "2026-11-12 08:20:00",
    )
    .await;

    // a run that fails does not move the watermark
    sqlx::query("ALTER TABLE runtime_daily RENAME TO runtime_daily_x")
        .execute(&pool)
        .await
        .unwrap();
    let t3 = at("2026-11-13 12:00:00");
    assert!(rollup_runtime(&mut c, t3).await.is_err());
    sqlx::query("ALTER TABLE runtime_daily_x RENAME TO runtime_daily")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(runtime_as_of(&mut c).await.unwrap(), Some(t1));

    // the next run takes both days
    rollup_runtime(&mut c, t3).await.unwrap();
    assert_eq!(
        awake(&pool, a).await,
        vec![
            ("2026-11-10".to_owned(), 3600),
            ("2026-11-11".to_owned(), 600),
            ("2026-11-12".to_owned(), 1200),
        ]
    );
    assert_eq!(runtime_as_of(&mut c).await.unwrap(), Some(t3));
}
