//! Agent memory (docs/10 §Memory, 03 §7): rows, revisions, query-time search.

use crate::DbError;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

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
    pub locked: bool,
    pub locked_at: Option<DateTime<Utc>>,
    pub locked_by: Option<Uuid>,
    pub locked_by_name: Option<String>,
    /// (P5-MEMORY-1, C-1169) Stored actors (ids only), see the 000288 migration.
    pub added_by: Value,
    pub changed_by: Value,
    /// The last time the agent used the memory, and in which conversation (dangling ids ok).
    pub last_used_at: Option<DateTime<Utc>>,
    pub last_used_conversation_id: Option<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RevisionRow {
    pub revision: i32,
    pub op: String,
    pub author_kind: String,
    pub author_user_id: Option<Uuid>,
    pub author_label: String,
    /// (P5-MEMORY-1) The API key a revision was written through; null for older revisions.
    pub author_api_key_id: Option<Uuid>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: String,
    pub provenance: Value,
    pub redacted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: Uuid,
    pub key: String,
    pub title: Option<String>,
    pub score: f32,
    /// (P5-MEMORY-1, C-1171) Plain text: no markup, no highlight markers.
    pub snippet: String,
    /// `[start, end)` ranges into `snippet`, in UTF-16 code units.
    pub highlights: Vec<[usize; 2]>,
    /// The same for the title, or for the key when the memory has no title.
    pub title_highlights: Vec<[usize; 2]>,
    /// (P6-MEMLOCK, C-1149) Hits carry the lock; the tool result shows it.
    pub locked: bool,
}

/// A hit as the query returns it: headlines with the highlight sentinels still in them.
#[derive(sqlx::FromRow)]
struct RawHit {
    id: Uuid,
    key: String,
    title: Option<String>,
    score: f32,
    snippet: String,
    title_headline: String,
    locked: bool,
}

/// Serializes memory writes of one agent for the duration of the transaction.
pub async fn lock_agent(conn: &mut PgConnection, agent: Uuid) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 7))")
        .bind(agent)
        .execute(conn)
        .await?;
    Ok(())
}

/// Column list for the read queries: the memory's own columns plus its lock (`C-1148`) and the
/// locker's display name (null when the locker had no user or was later deleted).
const READ_COLS: &str = "m.id, m.agent_id, m.key, m.title, m.description, m.content, m.revision, \
                         m.created_at, m.updated_at, m.locked, m.locked_at, m.locked_by, \
                         u.display_name AS locked_by_name, m.added_by, m.changed_by, \
                         m.last_used_at, m.last_used_conversation_id";
const FROM_MEM: &str = "FROM memories m LEFT JOIN users u ON u.id = m.locked_by";

async fn by_id(conn: &mut PgConnection, id: Uuid) -> Result<MemoryRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {READ_COLS} {FROM_MEM} WHERE m.id = $1"
    )))
    .bind(id)
    .fetch_one(&mut *conn)
    .await?)
}

pub async fn get(
    conn: &mut PgConnection,
    agent: Uuid,
    key: &str,
) -> Result<Option<MemoryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {READ_COLS} {FROM_MEM} WHERE m.agent_id = $1 AND m.key = $2"
    )))
    .bind(agent)
    .bind(key)
    .fetch_optional(conn)
    .await?)
}

pub async fn list(conn: &mut PgConnection, agent: Uuid) -> Result<Vec<MemoryRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {READ_COLS} {FROM_MEM} WHERE m.agent_id = $1 ORDER BY m.key"
    )))
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

/// Inserts a new memory at revision 1. (P6-MEMLOCK, C-1148) `locked`/`locked_by` come only from
/// the create route: a tool write and the harvest pass `false`.
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    conn: &mut PgConnection,
    agent: Uuid,
    key: &str,
    title: Option<&str>,
    description: Option<&str>,
    content: &str,
    // (P6-MEMLOCK, C-1148) `create` may arrive locked; `locked_at`/`locked_by` are set here, and
    // only when it is true: an unlocked create stores no locker.
    locked: bool,
    locked_by: Option<Uuid>,
    // (P5-MEMORY-1) The stored actor: both the adder and the last changer of a new memory.
    actor: &Value,
) -> Result<MemoryRow, DbError> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO memories (id, agent_id, key, title, description, content, revision, locked, \
                locked_at, locked_by, added_by, changed_by)
         VALUES ($1, $2, $3, $4, $5, $6, 1, $7, CASE WHEN $7 THEN now() END, \
                CASE WHEN $7 THEN $8 END, $9, $9) RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(agent)
    .bind(key)
    .bind(title)
    .bind(description)
    .bind(content)
    .bind(locked)
    .bind(locked_by)
    .bind(actor)
    .fetch_one(&mut *conn)
    .await?;
    by_id(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    title: Option<&str>,
    description: Option<&str>,
    content: &str,
    changed_by: &Value,
) -> Result<MemoryRow, DbError> {
    let id: Uuid = sqlx::query_scalar(
        "UPDATE memories SET title = $2, description = $3, content = $4,
                revision = revision + 1, updated_at = now(), changed_by = $5
         WHERE id = $1 RETURNING id",
    )
    .bind(id)
    .bind(title)
    .bind(description)
    .bind(content)
    .bind(changed_by)
    .fetch_one(&mut *conn)
    .await?;
    by_id(conn, id).await
}

/// (P6-MEMLOCK, C-1148) Sets the lock: `locked_at`/`locked_by` set when it becomes true, cleared
/// when it becomes false. The lock columns of a save or restore are never touched here.
pub async fn set_locked(
    conn: &mut PgConnection,
    id: Uuid,
    locked: bool,
    locked_by: Option<Uuid>,
) -> Result<MemoryRow, DbError> {
    let id: Uuid = sqlx::query_scalar(
        "UPDATE memories SET locked = $2, locked_at = CASE WHEN $2 THEN now() END,
                locked_by = CASE WHEN $2 THEN $3 END
         WHERE id = $1 RETURNING id",
    )
    .bind(id)
    .bind(locked)
    .bind(locked_by)
    .fetch_one(&mut *conn)
    .await?;
    by_id(conn, id).await
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
    pub author_api_key_id: Option<Uuid>,
    pub provenance: &'a Value,
}

/// Snapshot of the memory as it now stands (`m.revision` is the new number). A person's stored
/// label is their display name, looked up here — never the e-mail address (SEC-157); an agent's is
/// `r.author_label`.
pub async fn insert_revision(
    conn: &mut PgConnection,
    m: &MemoryRow,
    r: &NewRevision<'_>,
) -> Result<(), DbError> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO memory_revisions (id, memory_id, revision, op, author_kind, author_user_id,
                author_label, title, description, content, provenance, author_api_key_id)
         VALUES ($1, $2, $3, $4, $5, $6,
                COALESCE(CASE WHEN $5 = 'user' THEN
                    (SELECT {PUBLIC_NAME} FROM users u WHERE u.id = $6) END, $7),
                $8, $9, $10, $11, $12)"
    )))
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
    .bind(r.author_api_key_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// A person's name as it may be shown: the display name, or (when an identity provider sent no
/// name and the address stands in for it) the part before the `@` — never the address itself.
const PUBLIC_NAME: &str = "CASE WHEN lower(u.display_name) = lower(u.email) \
                           THEN split_part(u.display_name, '@', 1) ELSE u.display_name END";

/// (SEC-157) [`PUBLIC_NAME`] of each existing user.
pub async fn person_names(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.id, {PUBLIC_NAME} FROM users u WHERE u.id = ANY($1)"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The names of API keys (revoked ones too: their writes stay attributed).
pub async fn api_key_names(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, name FROM api_keys WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// (D-497(b), C-1169) Records that the agent used these memories in `conversation`, at most once
/// per memory, conversation and hour: one conditional update, so a turn that uses what it used
/// within the hour writes nothing. Never touches `updated_at` or the revision.
pub async fn mark_used<'e, E: PgExecutor<'e>>(
    exec: E,
    memories: &[Uuid],
    conversation: Uuid,
) -> Result<u64, DbError> {
    if memories.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query(
        "UPDATE memories SET last_used_at = now(), last_used_conversation_id = $2,
                last_used_marks = COALESCE((
                    SELECT jsonb_object_agg(e.k, e.v) FROM jsonb_each_text(last_used_marks) e(k, v)
                     WHERE e.v::timestamptz > now() - interval '1 hour'), '{}'::jsonb)
                  || jsonb_build_object($2::text, now())
         WHERE id = ANY($1)
           AND COALESCE((last_used_marks ->> $2::text)::timestamptz
                        <= now() - interval '1 hour', true)",
    )
    .bind(memories)
    .bind(conversation)
    .execute(exec)
    .await?
    .rows_affected())
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

const REV_COLS: &str = "revision, op, author_kind, author_user_id, author_label, \
                        author_api_key_id, title, description, content, provenance, redacted_at, \
                        created_at";

/// Newest first.
pub async fn revisions(conn: &mut PgConnection, memory: Uuid) -> Result<Vec<RevisionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {REV_COLS} FROM memory_revisions WHERE memory_id = $1 ORDER BY revision DESC"
    )))
    .bind(memory)
    .fetch_all(conn)
    .await?)
}

pub async fn revision(
    conn: &mut PgConnection,
    memory: Uuid,
    revision: i32,
) -> Result<Option<RevisionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {REV_COLS} FROM memory_revisions WHERE memory_id = $1 AND revision = $2"
    )))
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
    // (C-1171) The sentinel pattern of `repo/search.rs`: `ts_headline` marks matches with two
    // control characters (removed from the stored text first, so content cannot forge them), the
    // `memory_language` headline unless only the `simple` one marked something.
    let sentinels = "'StartSel=' || chr(1) || ', StopSel=' || chr(2) || ', ";
    let content = "translate(m.content, chr(1) || chr(2), '')";
    // The title, or the key (hyphens as spaces: same length, so ranges fit the key) when there
    // is no title.
    let name = "translate(coalesce(nullif(btrim(m.title), ''), replace(m.key, '-', ' ')), \
                          chr(1) || chr(2), '')";
    let headline = |text: &str, opts: &str| {
        let hl = |cfg: &str| {
            format!(
                "ts_headline({cfg}, {text}, websearch_to_tsquery({cfg}, $3), {sentinels}{opts}')"
            )
        };
        let (lang_hl, simple_hl) = (hl("$2::regconfig"), hl("'simple'::regconfig"));
        format!(
            "CASE WHEN position(chr(1) IN {lang_hl}) = 0 AND position(chr(1) IN {simple_hl}) > 0 \
             THEN {simple_hl} ELSE {lang_hl} END"
        )
    };
    let body_hl = headline(content, "MaxFragments=1, MaxWords=60, MinWords=15");
    let name_hl = headline(name, "HighlightAll=true");
    let sql = format!(
        "SELECT id, key, title, score, {body_hl} AS snippet, {name_hl} AS title_headline, locked
         FROM (
           SELECT m.id, m.key, m.title, m.content, m.locked,
                  greatest(
                    ts_rank_cd({dl}, websearch_to_tsquery($2::regconfig, $3), 32),
                    ts_rank_cd({ds}, websearch_to_tsquery('simple'::regconfig, $3), 32),
                    CASE WHEN m.key ILIKE $4 OR m.title ILIKE $4 THEN 0.5 ELSE 0 END
                  )::real AS score
           FROM memories m WHERE m.agent_id = $1
         ) m WHERE score > 0 AND score >= $5
         ORDER BY score DESC, key LIMIT $6"
    );
    let rows: Vec<RawHit> = sqlx::query_as(sqlx::AssertSqlSafe(&*sql))
        .bind(agent)
        .bind(lang)
        .bind(q)
        .bind(like)
        .bind(min_score)
        .bind(limit)
        .fetch_all(conn)
        .await?;
    Ok(rows.into_iter().map(|r| plain_hit(r, q)).collect())
}

fn plain_hit(r: RawHit, q: &str) -> SearchHit {
    let (snippet, highlights) = plain_snippet(&r.snippet);
    let name = r
        .title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(&r.key);
    // The key form kept its hyphens out of the headline: marks only, over the real name.
    let (_, mut title_highlights) = marks_of(&r.title_headline);
    if title_highlights.is_empty() {
        // A key or title match found by substring only (`ILIKE`) marks that substring.
        title_highlights = substring_ranges(name, q);
    }
    SearchHit {
        id: r.id,
        key: r.key,
        title: r.title,
        score: r.score,
        snippet,
        highlights,
        title_highlights,
        locked: r.locked,
    }
}

/// A headline of memory Markdown as one line of plain text with its highlight ranges.
pub fn plain_snippet(raw: &str) -> (String, Vec<[usize; 2]>) {
    let tidied: Vec<String> = raw.lines().filter_map(tidy_line).collect();
    let text = super::search::plain_text(&tidied.join("\n"));
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    super::search::snippet_bounded(
        &one,
        false,
        false,
        aurora_core::limits::MEMORY_SNIPPET_CHARS,
    )
}

/// Drops table rules (`|---|:-:|`) and task-list boxes (`- [x] `), which read as noise in plain
/// text and which the inline reducer would cut in half.
fn tidy_line(line: &str) -> Option<String> {
    let t = line.trim();
    if t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
        return None;
    }
    let mut out = line.to_owned();
    for mark in ["- [ ] ", "- [x] ", "- [X] "] {
        if let Some(rest) = t.strip_prefix(mark) {
            out = rest.to_owned();
        }
    }
    Some(out)
}

/// The ranges a headline marked, without shortening it (a title or key is at most 120 chars).
fn marks_of(headline: &str) -> (String, Vec<[usize; 2]>) {
    super::search::snippet_bounded(headline, false, false, 400)
}

/// Case-insensitive occurrences of `q` in `text`, as UTF-16 ranges.
fn substring_ranges(text: &str, q: &str) -> Vec<[usize; 2]> {
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let hay: Vec<char> = text.chars().collect();
    let needle: Vec<char> = q.chars().map(fold).collect();
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() > hay.len() {
        return out;
    }
    let units = |cs: &[char]| cs.iter().map(|c| c.len_utf16()).sum::<usize>();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()]
            .iter()
            .zip(&needle)
            .all(|(a, b)| fold(*a) == *b)
        {
            let start = units(&hay[..i]);
            out.push([start, start + units(&hay[i..i + needle.len()])]);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{plain_snippet, substring_ranges};

    #[test]
    fn a_headline_becomes_one_line_of_plain_text_with_utf16_ranges() {
        let (text, ranges) = plain_snippet(
            "## \u{1F600} Rota\n\u{1}Rotation\u{2} changes **every** Monday.\n|---|---|\n- [x] Pilot",
        );
        assert_eq!(text, "\u{1F600} Rota Rotation changes every Monday. Pilot");
        // The emoji is two UTF-16 units.
        assert_eq!(ranges, vec![[8, 16]]);
    }

    #[test]
    fn substrings_are_found_whatever_the_case() {
        assert_eq!(substring_ranges("On-call Rotation", "rota"), vec![[8, 12]]);
        assert!(substring_ranges("abc", "abcd").is_empty());
    }
}
