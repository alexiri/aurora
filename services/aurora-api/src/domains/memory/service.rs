use super::dto::MemoryRevision;
use crate::app::AppState;
use aurora_core::{ApiError, FieldError, limits};
use aurora_db::DbError;
use aurora_db::repo::memory::{self as repo, NewRevision};
use aurora_db::repo::{agents, runners::RunnerRow, sandboxes};
use aurora_protocol::runner::{MemoryWriteRequest, MemoryWriteResponse};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// Keys that would collide with the projection (`memory/index.md`) or a route.
const RESERVED_KEYS: [&str; 2] = ["index", "search"];
const SEARCH_Q_MAX: usize = 500;

/// Who wrote a revision.
pub struct Author {
    /// `user` | `agent`.
    pub kind: &'static str,
    pub user_id: Option<Uuid>,
    pub label: String,
}

/// Where a write came from; stored in the revision provenance.
pub enum Source {
    Dashboard,
    Api,
    Tool {
        conversation_id: Uuid,
        turn_id: Option<Uuid>,
    },
    Harvest,
}

impl Source {
    fn provenance(&self) -> Value {
        match self {
            Self::Dashboard => json!({"source": "dashboard"}),
            Self::Api => json!({"source": "api"}),
            Self::Tool {
                conversation_id,
                turn_id,
            } => json!({"source": "tool", "conversation_id": conversation_id, "turn_id": turn_id}),
            Self::Harvest => json!({"source": "harvest"}),
        }
    }
}

/// One memory write (create or replace).
pub struct Write<'a> {
    pub key: &'a str,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub content: &'a str,
    /// `MEMORY_TOOL_CONTENT_MAX` for tools, `MEMORY_CONTENT_MAX` otherwise.
    pub max_content: usize,
    /// Only create (409 `memory_exists` when present).
    pub create_only: bool,
    /// Skip the revision when nothing changed (harvest re-uploads).
    pub skip_unchanged: bool,
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid memory.", vec![FieldError::new(path, msg)])
}

pub fn validate_key(key: &str) -> Result<(), ApiError> {
    let ok = !key.is_empty()
        && key.len() <= 64
        && key.split('-').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        });
    if !ok {
        return Err(bad(
            "/key",
            "lowercase letters and digits joined by single hyphens, at most 64 characters",
        ));
    }
    if RESERVED_KEYS.contains(&key) {
        return Err(bad("/key", "reserved key"));
    }
    Ok(())
}

fn blank_none(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

fn clamp(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// `key: line`, the line being the description, else title, else first content line (<=120 chars).
pub fn index_line(
    key: &str,
    title: Option<&str>,
    description: Option<&str>,
    content: &str,
) -> String {
    let line = blank_none(description)
        .or_else(|| blank_none(title))
        .or_else(|| content.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("");
    let one = line.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{key}: {}", clamp(&one, 120))
}

/// Creates or replaces a memory inside one transaction; returns `(row, created)`.
pub async fn save(
    pool: &PgPool,
    agent: Uuid,
    w: &Write<'_>,
    author: &Author,
    source: &Source,
) -> Result<(repo::MemoryRow, bool), ApiError> {
    validate_key(w.key)?;
    let (title, description) = (blank_none(w.title), blank_none(w.description));
    if title.is_some_and(|t| t.chars().count() > 120) {
        return Err(bad("/title", "at most 120 characters"));
    }
    if description.is_some_and(|d| d.chars().count() > 240) {
        return Err(bad("/description", "at most 240 characters"));
    }
    if w.content.trim().is_empty() {
        return Err(bad("/content", "must not be empty"));
    }
    if w.content.chars().count() > w.max_content {
        return Err(bad(
            "/content",
            &format!("at most {} characters", w.max_content),
        ));
    }
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    repo::lock_agent(&mut tx, agent).await?;
    let existing = repo::get(&mut tx, agent, w.key).await?;
    let (row, created, op) = match existing {
        Some(_) if w.create_only => {
            return Err(ApiError::conflict(
                "memory_exists",
                "A memory with this key already exists.",
            ));
        }
        Some(cur) => {
            if w.skip_unchanged
                && cur.title.as_deref() == title
                && cur.description.as_deref() == description
                && cur.content == w.content
            {
                return Ok((cur, false));
            }
            (
                repo::update(&mut tx, cur.id, title, description, w.content).await?,
                false,
                "save",
            )
        }
        None => {
            if repo::count(&mut tx, agent).await? >= i64::from(limits::MEMORY_MAX_PER_AGENT) {
                return Err(ApiError::unprocessable(
                    "memory_limit_reached",
                    format!(
                        "An agent can hold at most {} memories.",
                        limits::MEMORY_MAX_PER_AGENT
                    ),
                ));
            }
            (
                repo::insert(&mut tx, agent, w.key, title, description, w.content).await?,
                true,
                "save",
            )
        }
    };
    snapshot(&mut tx, &row, op, author, &source.provenance()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok((row, created))
}

async fn snapshot(
    tx: &mut PgConnection,
    row: &repo::MemoryRow,
    op: &str,
    author: &Author,
    provenance: &Value,
) -> Result<(), ApiError> {
    repo::insert_revision(
        tx,
        row,
        &NewRevision {
            op,
            author_kind: author.kind,
            author_user_id: author.user_id,
            author_label: &author.label,
            provenance,
        },
    )
    .await?;
    let keep = i32::try_from(limits::MEMORY_REVISIONS_KEPT).unwrap_or(50);
    repo::prune(tx, row.id, keep).await?;
    repo::bump_agent_sync(tx, row.agent_id).await?;
    Ok(())
}

pub async fn get(pool: &PgPool, agent: Uuid, key: &str) -> Result<repo::MemoryRow, ApiError> {
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    repo::get(&mut conn, agent, key)
        .await?
        .ok_or_else(|| ApiError::not_found("Memory not found."))
}

pub async fn list(pool: &PgPool, agent: Uuid) -> Result<Vec<repo::MemoryRow>, ApiError> {
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    Ok(repo::list(&mut conn, agent).await?)
}

/// Deletes a memory (and its revisions). 404 when absent.
pub async fn delete(pool: &PgPool, agent: Uuid, key: &str) -> Result<(), ApiError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    repo::lock_agent(&mut tx, agent).await?;
    let m = repo::get(&mut tx, agent, key)
        .await?
        .ok_or_else(|| ApiError::not_found("Memory not found."))?;
    repo::delete(&mut tx, m.id).await?;
    repo::bump_agent_sync(&mut tx, agent).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

pub async fn revisions(
    pool: &PgPool,
    agent: Uuid,
    key: &str,
) -> Result<Vec<MemoryRevision>, ApiError> {
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let m = repo::get(&mut conn, agent, key)
        .await?
        .ok_or_else(|| ApiError::not_found("Memory not found."))?;
    Ok(repo::revisions(&mut conn, m.id)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Restores an older revision as a new revision (`op = restore`).
pub async fn restore(
    pool: &PgPool,
    agent: Uuid,
    key: &str,
    revision: i32,
    author: &Author,
) -> Result<repo::MemoryRow, ApiError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    repo::lock_agent(&mut tx, agent).await?;
    let cur = repo::get(&mut tx, agent, key)
        .await?
        .ok_or_else(|| ApiError::not_found("Memory not found."))?;
    let rev = repo::revision(&mut tx, cur.id, revision)
        .await?
        .ok_or_else(|| ApiError::not_found("Revision not found."))?;
    if rev.redacted_at.is_some() {
        return Err(ApiError::unprocessable(
            "revision_redacted",
            "A redacted revision cannot be restored.",
        ));
    }
    let row = repo::update(
        &mut tx,
        cur.id,
        rev.title.as_deref(),
        rev.description.as_deref(),
        &rev.content,
    )
    .await?;
    snapshot(
        &mut tx,
        &row,
        "restore",
        author,
        &json!({"source": "restore", "restored_from": revision}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(row)
}

/// Scrubs one non-latest revision in place (idempotent).
pub async fn redact(
    pool: &PgPool,
    agent: Uuid,
    key: &str,
    revision: i32,
    by: Uuid,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    repo::lock_agent(&mut tx, agent).await?;
    let m = repo::get(&mut tx, agent, key)
        .await?
        .ok_or_else(|| ApiError::not_found("Memory not found."))?;
    if repo::revision(&mut tx, m.id, revision).await?.is_none() {
        return Err(ApiError::not_found("Revision not found."));
    }
    if m.revision == revision {
        return Err(ApiError::unprocessable(
            "cannot_redact_latest",
            "The latest revision cannot be redacted; save a new version first.",
        ));
    }
    repo::redact(&mut tx, m.id, revision, by).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

fn query_text(q: &str) -> Option<String> {
    let q = q.trim();
    (!q.is_empty()).then(|| clamp(q, SEARCH_Q_MAX))
}

/// Top `MEMORY_SEARCH_HITS` by score for `q` in the workspace memory language.
pub async fn search(pool: &PgPool, agent: Uuid, q: &str) -> Result<Vec<repo::SearchHit>, ApiError> {
    let Some(q) = query_text(q) else {
        return Err(bad("/q", "must not be empty"));
    };
    let mut conn = pool.acquire().await.map_err(DbError::from)?;
    let lang = repo::language_of_agent(&mut conn, agent).await?;
    Ok(repo::search(
        &mut conn,
        agent,
        &lang,
        &q,
        0.0,
        i64::from(limits::MEMORY_SEARCH_HITS),
    )
    .await?)
}

/// Turn-start recall snippets (docs/10): up to `MEMORY_SNIPPETS` hits for the incoming message
/// with score >= `MEMORY_SNIPPET_MIN_SCORE`, each `key: text` clamped to `MEMORY_SNIPPET_CHARS`.
pub async fn recall_snippets(
    conn: &mut PgConnection,
    agent: Uuid,
    message: &str,
) -> Result<Vec<String>, ApiError> {
    let Some(q) = query_text(message) else {
        return Ok(vec![]);
    };
    let lang = repo::language_of_agent(conn, agent).await?;
    let hits = repo::search(
        conn,
        agent,
        &lang,
        &q,
        limits::MEMORY_SNIPPET_MIN_SCORE,
        i64::from(limits::MEMORY_SNIPPETS),
    )
    .await?;
    Ok(hits
        .into_iter()
        .map(|h| {
            let text = h.snippet.split_whitespace().collect::<Vec<_>>().join(" ");
            clamp(&format!("{}: {text}", h.key), limits::MEMORY_SNIPPET_CHARS)
        })
        .collect())
}

/// `POST /v1/runner/memory-writes`: harvested agent edits. The agent is derived from the
/// runner-owned sandbox with the current boot, never from the request.
pub async fn memory_write(
    s: &AppState,
    runner: &RunnerRow,
    req: MemoryWriteRequest,
) -> Result<MemoryWriteResponse, ApiError> {
    use aurora_protocol::common::Validate;
    req.write
        .validate()
        .map_err(|e| ApiError::invalid_request(e.to_string(), vec![]))?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let sb = match sandboxes::get(&mut conn, req.sandbox_id).await? {
        Some(sb) if sb.runner_id == Some(runner.id) => sb,
        _ => return Err(ApiError::not_found("Unknown sandbox.")),
    };
    if sb.boot_id != Some(req.boot_id) || !matches!(sb.state.as_str(), "starting" | "running") {
        return Err(ApiError::conflict("fenced", "Sandbox boot is not current."));
    }
    let agent = agents::get(&mut *conn, sb.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Unknown agent."))?;
    drop(conn);
    let w = Write {
        key: &req.write.key,
        title: req.write.title.as_deref(),
        description: req.write.description.as_deref(),
        content: &req.write.content,
        max_content: limits::MEMORY_CONTENT_MAX,
        create_only: false,
        skip_unchanged: true,
    };
    let author = Author {
        kind: "agent",
        user_id: None,
        label: agent.name,
    };
    let (row, _) = save(&s.pool, agent.id, &w, &author, &Source::Harvest).await?;
    Ok(MemoryWriteResponse {
        revision: u64::try_from(row.revision).unwrap_or(0),
    })
}
