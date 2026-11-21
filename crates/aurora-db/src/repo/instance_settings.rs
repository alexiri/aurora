//! Instance settings rows (03 §`instance_settings`, SET-1, D-236): one row per section, a flat
//! jsonb object of stored values and a generation that every write bumps (compare-and-set).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// The advisory-lock key of Helm mirroring at API start (D-236(c)): one replica at a time.
pub const MIRROR_LOCK_KEY: i64 = 0x5E77_1146_0000_0001;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SettingsRow {
    pub section: String,
    pub schema_version: i32,
    pub value: serde_json::Value,
    pub generation: i64,
    pub updated_by: Option<Uuid>,
    pub updated_by_label: Option<String>,
    /// (C-1102) The writer's display name now; null when the user is gone or unknown. Filled only by
    /// [`list_all_named`] (the API): the other services' roles do not read `users`.
    #[sqlx(default)]
    pub updated_by_name: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// The columns every service may read: `instance_settings` only. The gateway, the runner and the
/// other service roles have no grant on `users` (least privilege), so no join or subselect here.
const COLS: &str =
    "section, schema_version, value, generation, updated_by, updated_by_label, updated_at";

pub async fn list_all<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<SettingsRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM instance_settings ORDER BY section"
    )))
    .fetch_all(exec)
    .await?)
}

/// [`list_all`] with the writer's display name (C-1102), for the API's role, which reads `users`.
pub async fn list_all_named<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<SettingsRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS}, (SELECT u.display_name FROM users u WHERE u.id = instance_settings.updated_by) \
         AS updated_by_name FROM instance_settings ORDER BY section"
    )))
    .fetch_all(exec)
    .await?)
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    section: &str,
) -> Result<Option<SettingsRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM instance_settings WHERE section = $1"
    )))
    .bind(section)
    .fetch_optional(exec)
    .await?)
}

/// `(section, generation)` of every row: the reload poll's cheap comparison.
pub async fn generations<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<(String, i64)>, DbError> {
    Ok(
        sqlx::query_as("SELECT section, generation FROM instance_settings")
            .fetch_all(exec)
            .await?,
    )
}

/// Row-lock a section for the read-modify-write of a transaction (absent row: nothing locked,
/// the insert's conflict check then settles a race).
pub async fn get_for_update(
    conn: &mut PgConnection,
    section: &str,
) -> Result<Option<SettingsRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM instance_settings WHERE section = $1 FOR UPDATE"
    )))
    .bind(section)
    .fetch_optional(conn)
    .await?)
}

/// Compare-and-set write: `expected` is the generation the writer read (0 = no row yet).
/// Returns the new generation, or `None` when the row moved (409 `stale_revision`).
pub async fn cas_write(
    conn: &mut PgConnection,
    section: &str,
    expected: i64,
    value: &serde_json::Value,
    by: Option<Uuid>,
    label: &str,
) -> Result<Option<i64>, DbError> {
    let gen_: Option<(i64,)> = if expected == 0 {
        sqlx::query_as(
            "INSERT INTO instance_settings (section, value, generation, updated_by, updated_by_label)
             VALUES ($1, $2, 1, $3, $4)
             ON CONFLICT (section) DO NOTHING
             RETURNING generation",
        )
        .bind(section)
        .bind(value)
        .bind(by)
        .bind(label)
        .fetch_optional(conn)
        .await?
    } else {
        sqlx::query_as(
            "UPDATE instance_settings
                SET value = $3, generation = generation + 1, updated_by = $4,
                    updated_by_label = $5, updated_at = now()
              WHERE section = $1 AND generation = $2
              RETURNING generation",
        )
        .bind(section)
        .bind(expected)
        .bind(value)
        .bind(by)
        .bind(label)
        .fetch_optional(conn)
        .await?
    };
    Ok(gen_.map(|(g,)| g))
}

/// `pg_advisory_xact_lock` for Helm mirroring: held until the transaction ends.
pub async fn lock_mirroring(conn: &mut PgConnection) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIRROR_LOCK_KEY)
        .execute(conn)
        .await?;
    Ok(())
}

/// `NOTIFY aurora_settings` with the `{section, generation}` payload (after commit).
pub async fn notify<'e, E: PgExecutor<'e>>(
    exec: E,
    section: &str,
    generation: i64,
) -> Result<(), DbError> {
    crate::bus::notify(
        exec,
        crate::bus::Channel::Settings,
        &serde_json::json!({ "section": section, "generation": generation }),
    )
    .await
    .map_err(|e| match e {
        crate::bus::BusError::Db(e) => e.into(),
        other => DbError::Other(sqlx::Error::Protocol(other.to_string())),
    })
}

/// Instance admins that may be e-mailed about a change (not disabled).
pub async fn admin_emails<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<String>, DbError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT email FROM users WHERE is_instance_admin AND disabled_at IS NULL ORDER BY email",
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|(e,)| e).collect())
}
