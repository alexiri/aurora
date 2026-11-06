//! Reach: which spaces/people an agent may talk to through a shared bot (docs/13; schema
//! `reach_grants`). Decisions use compare-and-set on `revision`.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReachRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub integration_id: Uuid,
    pub subject_kind: String,
    pub subject_ref: String,
    pub subject_label: String,
    pub state: String,
    pub listen: String,
    pub revision: i64,
    pub decided_by_user_id: Option<Uuid>,
    pub decided_by_name: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const SELECT: &str =
    "SELECT g.id, g.agent_id, g.integration_id, g.subject_kind, g.subject_ref, g.subject_label,
        g.state, g.listen, g.revision, g.decided_by_user_id, u.display_name AS decided_by_name,
        g.decided_at, g.expires_at, g.created_at
    FROM reach_grants g LEFT JOIN users u ON u.id = g.decided_by_user_id";

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<ReachRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE g.id=$1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<ReachRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("{SELECT} WHERE g.id=$1 FOR UPDATE OF g"))
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// The grant for a subject (`kind`: `space` | `person`), whatever its state.
pub async fn reach_for<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    integration: Uuid,
    subject_kind: &str,
    subject_ref: &str,
) -> Result<Option<ReachRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE g.agent_id=$1 AND g.integration_id=$2 AND g.subject_kind=$3 AND g.subject_ref=$4"
    ))
    .bind(agent)
    .bind(integration)
    .bind(subject_kind)
    .bind(subject_ref)
    .fetch_optional(exec)
    .await?)
}

/// Keyset page of an agent's grants, newest first.
pub async fn page_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> Result<Vec<ReachRow>, DbError> {
    let (t, id) = after.unzip();
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE g.agent_id=$1 AND ($2::timestamptz IS NULL OR (g.created_at, g.id) < ($2, $3))
         ORDER BY g.created_at DESC, g.id DESC LIMIT $4"
    ))
    .bind(agent)
    .bind(t)
    .bind(id)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// Insert a `pending` grant, or return the existing one unchanged.
pub async fn ensure_pending<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    integration: Uuid,
    subject_kind: &str,
    subject_ref: &str,
    subject_label: &str,
) -> Result<Uuid, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO reach_grants (id, agent_id, integration_id, subject_kind, subject_ref,
             subject_label, state)
         VALUES ($1,$2,$3,$4,$5,$6,'pending')
         ON CONFLICT (agent_id, integration_id, subject_kind, subject_ref)
         DO UPDATE SET subject_label = EXCLUDED.subject_label
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(agent)
    .bind(integration)
    .bind(subject_kind)
    .bind(subject_ref)
    .bind(subject_label)
    .fetch_one(exec)
    .await?)
}

/// Record a decision if `revision` still matches; bumps the revision. `None` = stale.
pub async fn decide<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    expected_revision: i64,
    state: &str,
    listen: &str,
    by: Uuid,
) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE reach_grants
            SET state=$3, listen=$4, decided_by_user_id=$5, decided_at=now(),
                revision=revision+1, updated_at=now()
          WHERE id=$1 AND revision=$2
          RETURNING revision",
    )
    .bind(id)
    .bind(expected_revision)
    .bind(state)
    .bind(listen)
    .bind(by)
    .fetch_optional(exec)
    .await?)
}

/// Whether any of `person_refs` has an approved person grant for `agent` on `integration`.
pub async fn approved_person_reach<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    integration: Uuid,
    person_refs: &[String],
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM reach_grants
          WHERE agent_id=$1 AND integration_id=$2 AND subject_kind='person'
            AND subject_ref = ANY($3) AND state='approved'
            AND (expires_at IS NULL OR expires_at > now()))",
    )
    .bind(agent)
    .bind(integration)
    .bind(person_refs)
    .fetch_one(exec)
    .await?)
}
