//! Agent memory (docs/10 §Memory, 03 §7): rows, revisions, query-time search.

use crate::DbError;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

const COLS: &str =
    "id, agent_id, key, title, description, content, revision, created_at, updated_at";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemoryRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub key: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: String,
    pub revision: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RevisionRow {
    pub revision: i32,
    pub op: String,
    pub author_kind: String,
    pub author_user_id: Option<Uuid>,
    pub author_label: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: String,
    pub provenance: Value,
    pub redacted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SearchHit {
    pub key: String,
    pub title: Option<String>,
    pub score: f32,
    pub snippet: String,
}

/// Serializes memory writes of one agent for the duration of the transaction.
pub async fn lock_agent(conn: &mut PgConnection, agent: Uuid) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 7))")
        .bind(agent)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn get(
    conn: &mut PgConnection,
    agent: Uuid,
    key: &str,
) -> Result<Option<MemoryRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM memories WHERE agent_id = $1 AND key = $2"
    ))
    .bind(agent)
    .bind(key)
    .fetch_optional(conn)
    .await?)
}

pub async fn list(conn: &mut PgConnection, agent: Uuid) -> Result<Vec<MemoryRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM memories WHERE agent_id = $1 ORDER BY key"
    ))
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

pub async fn count(conn: &mut PgConnection, agent: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM memories WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(conn)
            .await?,
    )
}

pub async fn insert(
    conn: &mut PgConnection,
    agent: Uuid,
    key: &str,
    title: Option<&str>,
    description: Option<&str>,
    content: &str,
) -> Result<MemoryRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO memories (id, agent_id, key, title, description, content, revision)
         VALUES ($1, $2, $3, $4, $5, $6, 1) RETURNING {COLS}"
    ))
    .bind(Uuid::now_v7())
    .bind(agent)
    .bind(key)
    .bind(title)
    .bind(description)
    .bind(content)
    .fetch_one(conn)
    .await?)
}

pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    title: Option<&str>,
    description: Option<&str>,
    content: &str,
) -> Result<MemoryRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE memories SET title = $2, description = $3, content = $4,
                revision = revision + 1, updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(id)
    .bind(title)
    .bind(description)
    .bind(content)
    .fetch_one(conn)
    .await?)
}

pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM memories WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

pub struct NewRevision<'a> {
    pub op: &'a str,
    pub author_kind: &'a str,
    pub author_user_id: Option<Uuid>,
    pub author_label: &'a str,
    pub provenance: &'a Value,
}

/// Snapshot of the memory as it now stands (`m.revision` is the new number).
pub async fn insert_revision(
    conn: &mut PgConnection,
    m: &MemoryRow,
    r: &NewRevision<'_>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO memory_revisions (id, memory_id, revision, op, author_kind, author_user_id,
                author_label, title, description, content, provenance)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(Uuid::now_v7())
    .bind(m.id)
    .bind(m.revision)
    .bind(r.op)
    .bind(r.author_kind)
    .bind(r.author_user_id)
    .bind(r.author_label)
    .bind(&m.title)
    .bind(&m.description)
    .bind(&m.content)
    .bind(r.provenance)
    .execute(conn)
    .await?;
    Ok(())
}

/// Keeps the newest `keep` revisions.
pub async fn prune(conn: &mut PgConnection, memory: Uuid, keep: i32) -> Result<(), DbError> {
    sqlx::query(
        "DELETE FROM memory_revisions WHERE memory_id = $1
           AND revision <= (SELECT revision FROM memories WHERE id = $1) - $2",
    )
    .bind(memory)
    .bind(keep)
    .execute(conn)
    .await?;
    Ok(())
}

const REV_COLS: &str = "revision, op, author_kind, author_user_id, author_label, title, \
                        description, content, provenance, redacted_at, created_at";

/// Newest first.
pub async fn revisions(conn: &mut PgConnection, memory: Uuid) -> Result<Vec<RevisionRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {REV_COLS} FROM memory_revisions WHERE memory_id = $1 ORDER BY revision DESC"
    ))
    .bind(memory)
    .fetch_all(conn)
    .await?)
}

pub async fn revision(
    conn: &mut PgConnection,
    memory: Uuid,
    revision: i32,
) -> Result<Option<RevisionRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {REV_COLS} FROM memory_revisions WHERE memory_id = $1 AND revision = $2"
    ))
    .bind(memory)
    .bind(revision)
    .fetch_optional(conn)
    .await?)
}

/// Scrubs one revision in place; false when it was already redacted.
pub async fn redact(
    conn: &mut PgConnection,
    memory: Uuid,
    revision: i32,
    by: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE memory_revisions SET content = '[redacted]', title = NULL, description = NULL,
                redacted_at = now(), redacted_by = $3
         WHERE memory_id = $1 AND revision = $2 AND redacted_at IS NULL",
    )
    .bind(memory)
    .bind(revision)
    .bind(by)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Bumps the agent's home sync generation so a running sandbox re-renders the projection.
pub async fn bump_agent_sync(conn: &mut PgConnection, agent: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE agents SET home_sync_generation = home_sync_generation + 1 WHERE id = $1")
        .bind(agent)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn language_of_agent(conn: &mut PgConnection, agent: Uuid) -> Result<String, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT w.memory_language FROM agents a JOIN workspaces w ON w.id = a.workspace_id
         WHERE a.id = $1",
    )
    .bind(agent)
    .fetch_optional(conn)
    .await?
    .unwrap_or_else(|| "simple".to_owned()))
}

/// `doc(cfg)` of docs/10 §Search and recall.
fn doc(cfg: &str) -> String {
    format!(
        "(setweight(to_tsvector({cfg}, replace(m.key, '-', ' ') || ' ' || coalesce(m.title, '')), 'A')
          || setweight(to_tsvector({cfg}, coalesce(m.description, '')), 'B')
          || setweight(to_tsvector({cfg}, m.content), 'D'))"
    )
}

/// Query-time search over one agent's memories, in `lang` (the workspace memory language).
///
/// `score = greatest(rank(lang), rank('simple'), 0.5 if key/title ILIKE %q%)`, rank being
/// `ts_rank_cd(doc, websearch_to_tsquery, 32)`. Hits have `score > 0` and `score >= min_score`.
/// `lang` must be a validated text-search configuration name.
pub async fn search(
    conn: &mut PgConnection,
    agent: Uuid,
    lang: &str,
    q: &str,
    min_score: f32,
    limit: i64,
) -> Result<Vec<SearchHit>, DbError> {
    let like = format!(
        "%{}%",
        q.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let (dl, ds) = (doc("$2::regconfig"), doc("'simple'::regconfig"));
    let sql = format!(
        "SELECT key, title, score, snippet FROM (
           SELECT m.key, m.title,
                  greatest(
                    ts_rank_cd({dl}, websearch_to_tsquery($2::regconfig, $3), 32),
                    ts_rank_cd({ds}, websearch_to_tsquery('simple'::regconfig, $3), 32),
                    CASE WHEN m.key ILIKE $4 OR m.title ILIKE $4 THEN 0.5 ELSE 0 END
                  )::real AS score,
                  ts_headline($2::regconfig, m.content, websearch_to_tsquery($2::regconfig, $3),
                              'MaxFragments=1, MaxWords=60, MinWords=15') AS snippet
           FROM memories m WHERE m.agent_id = $1
         ) s WHERE score > 0 AND score >= $5
         ORDER BY score DESC, key LIMIT $6"
    );
    Ok(sqlx::query_as(&sql)
        .bind(agent)
        .bind(lang)
        .bind(q)
        .bind(like)
        .bind(min_score)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}
