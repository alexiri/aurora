//! Request-log (ledger) reads and partition maintenance (docs/03 §request_log,
//! docs/05 §Background jobs).

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LogRow {
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub credential_kind: String,
    pub lane: String,
    pub method: String,
    pub host: String,
    pub path: String,
    pub app_id: Option<String>,
    pub tool_id: Option<String>,
    pub connection_id: Option<Uuid>,
    pub secret_id: Option<Uuid>,
    pub decision: String,
    pub rule_logical_id: Option<Uuid>,
    pub rule_name: Option<String>,
    pub upstream_status: Option<i32>,
    pub status: i32,
    pub error_code: Option<String>,
    pub injected: i16,
    pub latency_ms: i32,
    pub bytes_up: i64,
    pub bytes_down: i64,
}

#[derive(Debug, Default, Clone)]
pub struct ActivityFilter {
    pub agent_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub decision: Option<String>,
    pub host: Option<String>,
    pub app_id: Option<String>,
}

/// Workspace activity, `ts desc, id desc`; keyset on `(ts, id)`.
pub async fn list_activity(
    pool: &PgPool,
    ws: Uuid,
    f: &ActivityFilter,
    after: After,
    fetch: i64,
) -> Result<Vec<LogRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(
        "SELECT id, ts, agent_id, conversation_id, turn_id, credential_kind, lane, method, host, \
                path, app_id, tool_id, connection_id, secret_id, decision, rule_logical_id, \
                rule_name, upstream_status, status, error_code, injected, latency_ms, bytes_up, \
                bytes_down \
         FROM request_log WHERE workspace_id = $1 \
           AND ($2::uuid IS NULL OR agent_id = $2) \
           AND ($3::uuid IS NULL OR conversation_id = $3) \
           AND ($4::text IS NULL OR decision = $4) \
           AND ($5::text IS NULL OR host = $5) \
           AND ($6::text IS NULL OR app_id = $6) \
           AND ($7::timestamptz IS NULL OR (ts, id) < ($7, $8)) \
         ORDER BY ts DESC, id DESC LIMIT $9",
    )
    .bind(ws)
    .bind(f.agent_id)
    .bind(f.conversation_id)
    .bind(&f.decision)
    .bind(&f.host)
    .bind(&f.app_id)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(pool)
    .await?)
}

fn month_start(y: i32, m: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(NaiveDate::MIN)
}

fn add_months(d: NaiveDate, n: u32) -> NaiveDate {
    let idx = d.year() * 12 + d.month0() as i32 + n as i32;
    month_start(idx.div_euclid(12), idx.rem_euclid(12) as u32 + 1)
}

fn part_name(d: NaiveDate) -> String {
    format!("request_log_p{:04}{:02}", d.year(), d.month())
}

/// Create the monthly partitions for the month of `now` and the next
/// `ahead` months; idempotent. Rows already in the default partition for a
/// month are moved into the new partition first. Returns created names.
pub async fn ensure_partitions(
    pool: &PgPool,
    now: DateTime<Utc>,
    ahead: u32,
) -> Result<Vec<String>, DbError> {
    let first = month_start(now.year(), now.month());
    let mut created = Vec::new();
    for i in 0..=ahead {
        let from = add_months(first, i);
        let to = add_months(from, 1);
        let name = part_name(from);
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(&name)
            .fetch_one(pool)
            .await?;
        if exists {
            continue;
        }
        // Names and bounds are generated from integers only.
        let (lo, hi) = (format!("{from} 00:00:00+00"), format!("{to} 00:00:00+00"));
        let mut tx = pool.begin().await?;
        sqlx::query(&format!(
            "CREATE TABLE IF NOT EXISTS {name} (LIKE request_log INCLUDING DEFAULTS INCLUDING CONSTRAINTS)"
        ))
        .execute(&mut *tx)
        .await?;
        sqlx::query(&format!(
            "WITH moved AS (DELETE FROM request_log_default WHERE ts >= '{lo}' AND ts < '{hi}' RETURNING *) \
             INSERT INTO {name} SELECT * FROM moved"
        ))
        .execute(&mut *tx)
        .await?;
        sqlx::query(&format!(
            "ALTER TABLE request_log ATTACH PARTITION {name} FOR VALUES FROM ('{lo}') TO ('{hi}')"
        ))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        created.push(name);
    }
    Ok(created)
}

/// Drop monthly partitions that end before `now - retention_days`, and purge
/// equally old rows from the default partition. Returns dropped names.
pub async fn drop_expired(
    pool: &PgPool,
    now: DateTime<Utc>,
    retention_days: u32,
) -> Result<Vec<String>, DbError> {
    let cutoff = now - chrono::Duration::days(i64::from(retention_days));
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid \
         WHERE i.inhparent = 'request_log'::regclass AND c.relname ~ '^request_log_p[0-9]{6}$'",
    )
    .fetch_all(pool)
    .await?;
    let mut dropped = Vec::new();
    for name in names {
        let digits = &name["request_log_p".len()..];
        let (Ok(y), Ok(m)) = (digits[..4].parse::<i32>(), digits[4..].parse::<u32>()) else {
            continue;
        };
        let end = add_months(month_start(y, m), 1);
        if end.and_hms_opt(0, 0, 0).map(|e| e.and_utc()) <= Some(cutoff) {
            sqlx::query(&format!("DROP TABLE IF EXISTS {name}"))
                .execute(pool)
                .await?;
            dropped.push(name);
        }
    }
    sqlx::query("DELETE FROM request_log_default WHERE ts < $1")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(dropped)
}
