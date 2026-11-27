//! Workspace usage: tokens and time (P5-USAGE-1, D-359, D-360, C-1000..C-1006).
//!
//! `usage_daily` is a ledger added to in the transaction that ends a turn ([`record_turn`]) and never
//! decremented or deleted; `runtime_daily` is rebuilt from `runtime_intervals` by an hourly job
//! ([`rollup_runtime`]). The read side aggregates both for the workspace, organization and instance
//! views through one [`Scope`] and one grouping function.

use crate::DbError;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

/// (C-1001) Add one finished turn to the ledger. Called inside the transaction of the fenced terminal
/// transition, after `turns.finished_at` is set; a turn that never started writes nothing.
///
/// The turn and its seconds go under the default model's key (`usage.provider/endpoint_id/model`, else
/// `turns.model_basis`, else nulls); tokens go per `usage.by_model` entry, or the totals under the default's
/// key when there is no breakdown. The migration `20261201000247` backfills the ledger with the same
/// statement, restricted to nothing instead of one turn.
pub async fn record_turn(conn: &mut PgConnection, turn: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO usage_daily (org_id, workspace_id, agent_id, day, provider, endpoint_id, model, turns,
                                  turn_seconds, input_tokens, output_tokens, cache_read_tokens,
                                  cache_write_tokens)
         SELECT org_id, workspace_id, agent_id, day, provider, endpoint_id, model, sum(turns), sum(turn_seconds),
                sum(input_tokens), sum(output_tokens), sum(cache_read_tokens), sum(cache_write_tokens)
         FROM (
             SELECT a.org_id, c.workspace_id, t.agent_id,
                    (t.finished_at AT TIME ZONE 'UTC')::date AS day,
                    NULLIF(COALESCE(t.usage ->> 'provider', t.model_basis ->> 'provider'), '') AS provider,
                    NULLIF(COALESCE(t.usage ->> 'endpoint_id', t.model_basis ->> 'endpoint_id'), '') AS endpoint_id,
                    NULLIF(COALESCE(t.usage ->> 'model', t.model_basis ->> 'model'), '') AS model,
                    1::bigint AS turns,
                    GREATEST(floor(extract(epoch FROM t.finished_at - t.started_at)), 0)::bigint AS turn_seconds,
                    CASE WHEN usage_has_breakdown(t.usage)
                         THEN 0 ELSE usage_counter(t.usage -> 'input_tokens') END AS input_tokens,
                    CASE WHEN usage_has_breakdown(t.usage)
                         THEN 0 ELSE usage_counter(t.usage -> 'output_tokens') END AS output_tokens,
                    CASE WHEN usage_has_breakdown(t.usage)
                         THEN 0 ELSE usage_counter(t.usage -> 'cache_read_tokens') END AS cache_read_tokens,
                    CASE WHEN usage_has_breakdown(t.usage)
                         THEN 0 ELSE usage_counter(t.usage -> 'cache_write_tokens') END AS cache_write_tokens
             FROM turns t
             JOIN conversations c ON c.id = t.conversation_id
             JOIN agents a ON a.id = t.agent_id
             WHERE t.id = $1 AND t.role = 'turn' AND t.status IN ('done', 'failed', 'aborted')
               AND t.started_at IS NOT NULL AND t.finished_at IS NOT NULL
             UNION ALL
             SELECT a.org_id, c.workspace_id, t.agent_id,
                    (t.finished_at AT TIME ZONE 'UTC')::date,
                    NULLIF(COALESCE(t.usage ->> 'provider', t.model_basis ->> 'provider'), ''),
                    NULLIF(COALESCE(t.usage ->> 'endpoint_id', t.model_basis ->> 'endpoint_id'), ''),
                    NULLIF(e ->> 'model', ''),
                    0, 0,
                    usage_counter(e -> 'input_tokens'), usage_counter(e -> 'output_tokens'),
                    usage_counter(e -> 'cache_read_tokens'), usage_counter(e -> 'cache_write_tokens')
             FROM turns t
             JOIN conversations c ON c.id = t.conversation_id
             JOIN agents a ON a.id = t.agent_id
             CROSS JOIN LATERAL jsonb_array_elements(
                 CASE WHEN jsonb_typeof(t.usage -> 'by_model') = 'array' THEN t.usage -> 'by_model'
                      ELSE '[]'::jsonb END) AS e
             WHERE t.id = $1 AND t.role = 'turn' AND t.status IN ('done', 'failed', 'aborted')
               AND t.started_at IS NOT NULL AND t.finished_at IS NOT NULL
               AND jsonb_typeof(e) = 'object'
         ) rows
         GROUP BY org_id, workspace_id, agent_id, day, provider, endpoint_id, model
         ON CONFLICT (workspace_id, agent_id, day, provider, endpoint_id, model) DO UPDATE SET
             turns = usage_daily.turns + EXCLUDED.turns,
             turn_seconds = usage_daily.turn_seconds + EXCLUDED.turn_seconds,
             input_tokens = usage_daily.input_tokens + EXCLUDED.input_tokens,
             output_tokens = usage_daily.output_tokens + EXCLUDED.output_tokens,
             cache_read_tokens = usage_daily.cache_read_tokens + EXCLUDED.cache_read_tokens,
             cache_write_tokens = usage_daily.cache_write_tokens + EXCLUDED.cache_write_tokens,
             updated_at = now()",
    )
    .bind(turn)
    .execute(conn)
    .await?;
    Ok(())
}

/// (FIX-253) Add what the idle harness child spent after `turn` ended to the ledger, once per
/// numbered usage event: tokens only (no turn, no seconds), on today's day, under the same keys
/// as [`record_turn`]. `usage` is already reduced by the trust rule. Returns whether it was
/// counted now (`false`: a redelivery). (FIX-257) Each event is remembered, so the order the
/// reports arrive in does not matter. The caller's transaction holds the mark and the ledger row.
pub async fn record_idle(
    conn: &mut PgConnection,
    turn: Uuid,
    n: i64,
    usage: &serde_json::Value,
) -> Result<bool, DbError> {
    let fresh = sqlx::query(
        "INSERT INTO idle_usage_events (turn_id, n)
         SELECT id, $2 FROM turns WHERE id = $1 AND role = 'turn'
         ON CONFLICT (turn_id, n) DO NOTHING",
    )
    .bind(turn)
    .bind(n)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    if !fresh {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO usage_daily (org_id, workspace_id, agent_id, day, provider, endpoint_id, model, turns,
                                  turn_seconds, input_tokens, output_tokens, cache_read_tokens,
                                  cache_write_tokens)
         SELECT a.org_id, c.workspace_id, t.agent_id, (now() AT TIME ZONE 'UTC')::date,
                NULLIF(COALESCE($2::jsonb ->> 'provider', t.model_basis ->> 'provider'), ''),
                NULLIF(COALESCE($2::jsonb ->> 'endpoint_id', t.model_basis ->> 'endpoint_id'), ''),
                NULLIF(COALESCE(e ->> 'model', $2::jsonb ->> 'model', t.model_basis ->> 'model'), ''),
                0, 0,
                sum(usage_counter(e -> 'input_tokens')), sum(usage_counter(e -> 'output_tokens')),
                sum(usage_counter(e -> 'cache_read_tokens')),
                sum(usage_counter(e -> 'cache_write_tokens'))
         FROM turns t
         JOIN conversations c ON c.id = t.conversation_id
         JOIN agents a ON a.id = t.agent_id
         CROSS JOIN LATERAL jsonb_array_elements(
             CASE WHEN usage_has_breakdown($2::jsonb) THEN $2::jsonb -> 'by_model'
                  ELSE jsonb_build_array($2::jsonb) END) AS e
         WHERE t.id = $1 AND jsonb_typeof(e) = 'object'
         GROUP BY 1, 2, 3, 4, 5, 6, 7
         ON CONFLICT (workspace_id, agent_id, day, provider, endpoint_id, model) DO UPDATE SET
             input_tokens = usage_daily.input_tokens + EXCLUDED.input_tokens,
             output_tokens = usage_daily.output_tokens + EXCLUDED.output_tokens,
             cache_read_tokens = usage_daily.cache_read_tokens + EXCLUDED.cache_read_tokens,
             cache_write_tokens = usage_daily.cache_write_tokens + EXCLUDED.cache_write_tokens,
             updated_at = now()",
    )
    .bind(turn)
    .bind(usage)
    .execute(conn)
    .await?;
    Ok(true)
}

/// (C-1002) One run of the runtime rollup at time `at` (the job passes the database clock): recompute
/// `runtime_daily.awake_seconds` for every (agent, UTC day) touched by an interval that is open or changed
/// since the watermark minus five minutes (all intervals when there is no watermark), then move the
/// watermark to `at`, in one transaction. Idempotent; a run after downtime catches up because the watermark
/// did not move. Returns the number of (agent, day) rows written.
pub async fn rollup_runtime(conn: &mut PgConnection, at: DateTime<Utc>) -> Result<u64, DbError> {
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let watermark: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT runtime_watermark FROM usage_rollup_state WHERE singleton FOR UPDATE",
    )
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let written = sqlx::query(
        "WITH touched AS (
             SELECT DISTINCT ON (i.org_id, i.agent_id, d.day) i.org_id, i.workspace_id, i.agent_id, d.day::date AS day
             FROM runtime_intervals i
             CROSS JOIN LATERAL generate_series(
                 (i.started_at AT TIME ZONE 'UTC')::date::timestamp,
                 (COALESCE(i.ended_at, $1) AT TIME ZONE 'UTC')::date::timestamp,
                 interval '1 day') AS d(day)
             WHERE i.ended_at IS NULL OR $2::timestamptz IS NULL OR i.updated_at >= $2 - interval '5 minutes'
         ), sums AS (
             SELECT t.org_id, t.workspace_id, t.agent_id, t.day,
                    COALESCE(floor(sum(GREATEST(0, extract(epoch FROM
                        LEAST(COALESCE(i.ended_at, $1), (t.day + 1)::timestamp AT TIME ZONE 'UTC')
                        - GREATEST(i.started_at, t.day::timestamp AT TIME ZONE 'UTC'))))), 0)::bigint AS awake
             FROM touched t
             LEFT JOIN runtime_intervals i ON i.agent_id = t.agent_id
                 AND i.started_at < (t.day + 1)::timestamp AT TIME ZONE 'UTC'
                 AND COALESCE(i.ended_at, $1) > t.day::timestamp AT TIME ZONE 'UTC'
             GROUP BY t.org_id, t.workspace_id, t.agent_id, t.day
         )
         INSERT INTO runtime_daily (org_id, workspace_id, agent_id, day, awake_seconds)
         SELECT org_id, workspace_id, agent_id, day, awake FROM sums
         ON CONFLICT (org_id, agent_id, day) DO UPDATE SET
             awake_seconds = EXCLUDED.awake_seconds, workspace_id = EXCLUDED.workspace_id, updated_at = now()",
    )
    .bind(at)
    .bind(watermark)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query(
        "UPDATE usage_rollup_state SET runtime_watermark = $1, updated_at = now() WHERE singleton",
    )
    .bind(at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(written)
}

/// The database clock, for the rollup job (the intervals are stamped with the database's `now()`).
pub async fn clock(conn: &mut PgConnection) -> Result<DateTime<Utc>, DbError> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(conn)
        .await?)
}

/// The last successful runtime rollup (`runtime_as_of`, C-1000).
pub async fn runtime_as_of(conn: &mut PgConnection) -> Result<Option<DateTime<Utc>>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT runtime_watermark FROM usage_rollup_state WHERE singleton")
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

/// Which ledger rows a view reads. Every field narrows; the route sets the one that bounds the view
/// (`workspace` for C-1000, `org` for C-1006), so rows outside it are never read.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scope {
    pub org: Option<Uuid>,
    pub workspace: Option<Uuid>,
    pub agent: Option<Uuid>,
}

/// The seven counters of C-1000 `UsageCounters`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct Counters {
    pub turns: i64,
    pub turn_seconds: i64,
    pub awake_seconds: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

impl Counters {
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
    /// What the top-N lists order by.
    pub fn tokens(&self) -> i64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
    pub fn add(&mut self, o: &Counters) {
        self.turns += o.turns;
        self.turn_seconds += o.turn_seconds;
        self.awake_seconds += o.awake_seconds;
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_read_tokens += o.cache_read_tokens;
        self.cache_write_tokens += o.cache_write_tokens;
    }
}

/// What the rows of [`grouped`] are keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dim {
    Day,
    Agent,
    Org,
    Workspace,
}

/// One group: the key columns of its [`Dim`] are set, the others null.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Group {
    pub day: Option<NaiveDate>,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    #[sqlx(flatten)]
    pub counters: Counters,
}

/// Counters per [`Dim`] over `from..=to` (UTC days, inclusive): the ledger's turns, turn time and tokens
/// plus `runtime_daily`'s awake time, narrowed by `scope`.
pub async fn grouped(
    conn: &mut PgConnection,
    scope: Scope,
    from: NaiveDate,
    to: NaiveDate,
    dim: Dim,
) -> Result<Vec<Group>, DbError> {
    let (day, org, ws, agent) = match dim {
        Dim::Day => ("day", "NULL::uuid", "NULL::uuid", "NULL::uuid"),
        Dim::Agent => ("NULL::date", "NULL::uuid", "NULL::uuid", "agent_id"),
        Dim::Org => ("NULL::date", "org_id", "NULL::uuid", "NULL::uuid"),
        Dim::Workspace => ("NULL::date", "org_id", "workspace_id", "NULL::uuid"),
    };
    let group_by = match dim {
        Dim::Day => "day",
        Dim::Agent => "agent_id",
        Dim::Org => "org_id",
        Dim::Workspace => "workspace_id, org_id",
    };
    let filter = "day BETWEEN $4 AND $5 AND ($1::uuid IS NULL OR org_id = $1)
                  AND ($2::uuid IS NULL OR workspace_id = $2) AND ($3::uuid IS NULL OR agent_id = $3)";
    let sql = format!(
        "SELECT {day} AS day, {org} AS org_id, {ws} AS workspace_id, {agent} AS agent_id,
                COALESCE(sum(turns), 0)::bigint AS turns, COALESCE(sum(turn_seconds), 0)::bigint AS turn_seconds,
                COALESCE(sum(awake_seconds), 0)::bigint AS awake_seconds,
                COALESCE(sum(input_tokens), 0)::bigint AS input_tokens,
                COALESCE(sum(output_tokens), 0)::bigint AS output_tokens,
                COALESCE(sum(cache_read_tokens), 0)::bigint AS cache_read_tokens,
                COALESCE(sum(cache_write_tokens), 0)::bigint AS cache_write_tokens
         FROM (
             SELECT day, org_id, workspace_id, agent_id, turns, turn_seconds, 0::bigint AS awake_seconds,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
             FROM usage_daily WHERE {filter}
             UNION ALL
             SELECT day, org_id, workspace_id, agent_id, 0, 0, awake_seconds, 0, 0, 0, 0
             FROM runtime_daily WHERE {filter}
         ) u GROUP BY {group_by}"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(scope.org)
        .bind(scope.workspace)
        .bind(scope.agent)
        .bind(from)
        .bind(to)
        .fetch_all(conn)
        .await?)
}

/// One `by_model` row (C-1000): no awake time.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ModelGroup {
    pub provider: Option<String>,
    pub endpoint_id: Option<String>,
    pub model: Option<String>,
    pub turns: i64,
    pub turn_seconds: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

/// Ledger rows per (provider, endpoint, model) over `from..=to`, narrowed by `scope`.
pub async fn by_model(
    conn: &mut PgConnection,
    scope: Scope,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<ModelGroup>, DbError> {
    Ok(sqlx::query_as(
        "SELECT provider, endpoint_id, model,
                COALESCE(sum(turns), 0)::bigint AS turns, COALESCE(sum(turn_seconds), 0)::bigint AS turn_seconds,
                COALESCE(sum(input_tokens), 0)::bigint AS input_tokens,
                COALESCE(sum(output_tokens), 0)::bigint AS output_tokens,
                COALESCE(sum(cache_read_tokens), 0)::bigint AS cache_read_tokens,
                COALESCE(sum(cache_write_tokens), 0)::bigint AS cache_write_tokens
         FROM usage_daily
         WHERE day BETWEEN $4 AND $5 AND ($1::uuid IS NULL OR org_id = $1)
           AND ($2::uuid IS NULL OR workspace_id = $2) AND ($3::uuid IS NULL OR agent_id = $3)
         GROUP BY provider, endpoint_id, model",
    )
    .bind(scope.org)
    .bind(scope.workspace)
    .bind(scope.agent)
    .bind(from)
    .bind(to)
    .fetch_all(conn)
    .await?)
}

/// Names of the agents that still exist (deleted ones are absent: the route returns `null`).
pub async fn agent_names(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, name FROM agents WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Names of the workspaces that still exist.
pub async fn workspace_names(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, name FROM workspaces WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Names of the organizations that still exist.
pub async fn org_names(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, name FROM organizations WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}
