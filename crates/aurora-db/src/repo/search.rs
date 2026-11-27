//! Message search (P5-CHAT-4, D-229, D-230, C-727, C-728): the `transcript_search` index, its write
//! path (called by `transcript::append_durable_event`), the resumable backfill, and the one query
//! builder behind both the dashboard route and the agent tools.
//!
//! Security rules, enforced here so no caller can forget them:
//! * user text never reaches `to_tsquery` raw — [`Query::parse`] splits it into terms and every
//!   term is a bound parameter of `plainto_tsquery` / `phraseto_tsquery` (the one `to_tsquery`
//!   use takes a prefix word made only of letters, digits and `_`);
//! * visibility is a predicate of the query ([`Scope`]), never a post-filter;
//! * redacted events have no row (redaction deletes it in its transaction; the backfill locks the
//!   event rows it reads so a concurrent redaction cannot be overtaken).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

/// `body` bound (bytes): the rest of a longer message is not searchable.
pub const BODY_MAX_BYTES: usize = 32 * 1024;
/// Longest accepted `q` (characters), and most terms.
pub const QUERY_MAX_CHARS: usize = 200;
pub const QUERY_MAX_TERMS: usize = 12;
/// Statement timeout of one search (milliseconds).
pub const STATEMENT_TIMEOUT_MS: u32 = 2000;
/// Backfill batch size (rows).
pub const BACKFILL_BATCH: i64 = 1000;
/// Snippet bound (characters).
pub const SNIPPET_MAX_CHARS: usize = 300;

const START: char = '\u{1}';
const STOP: char = '\u{2}';

// ------------------------------------------------------------------ text reduction

/// The D-227 Markdown of an agent message reduced to plain text: code kept (fences dropped),
/// link text kept, URLs kept, emphasis and structure marks dropped.
pub fn plain_text(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    for line in md.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            continue;
        }
        let mut t = t;
        while let Some(rest) = t.strip_prefix('>') {
            t = rest.trim_start();
        }
        let t = t.trim_start_matches('#').trim_start();
        let t = ["- ", "* ", "+ "]
            .iter()
            .find_map(|m| t.strip_prefix(m))
            .unwrap_or(t);
        out.push_str(&inline(t));
        out.push('\n');
    }
    out.trim().to_owned()
}

/// Inline Markdown: `[text](url)` → `text url`, `![alt](url)` → `alt url`, `<url>` → `url`,
/// other tags dropped, `*`, `` ` ``, `|` and `~~` removed.
fn inline(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '!' if c.get(i + 1) == Some(&'[') => i += 1,
            '[' => {
                // [text](url)
                if let Some(close) = c[i + 1..].iter().position(|x| *x == ']').map(|p| p + i + 1)
                    && c.get(close + 1) == Some(&'(')
                    && let Some(end) = c[close + 2..]
                        .iter()
                        .position(|x| *x == ')')
                        .map(|p| p + close + 2)
                {
                    out.extend(&c[i + 1..close]);
                    out.push(' ');
                    out.extend(&c[close + 2..end]);
                    i = end + 1;
                    continue;
                }
                i += 1;
            }
            '<' => {
                if let Some(end) = c[i + 1..].iter().position(|x| *x == '>').map(|p| p + i + 1) {
                    let inner: String = c[i + 1..end].iter().collect();
                    if inner.contains("://") || inner.contains('@') {
                        out.push_str(&inner);
                    } else if inner
                        .chars()
                        .all(|x| x.is_alphanumeric() || "/ =\"'-".contains(x))
                    {
                        out.push(' ');
                    } else {
                        out.push('<');
                        i += 1;
                        continue;
                    }
                    i = end + 1;
                } else {
                    out.push('<');
                    i += 1;
                }
            }
            '*' | '`' | '|' => {
                out.push(if c[i] == '|' { ' ' } else { '\0' });
                i += 1;
            }
            '~' if c.get(i + 1) == Some(&'~') => i += 2,
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out.replace('\0', "")
}

/// Bound a body: control characters (the highlight sentinels among them) become spaces, at most
/// [`BODY_MAX_BYTES`] bytes.
pub fn clean_body(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c == '\n' || c == '\t' || !c.is_control() {
                c
            } else {
                ' '
            }
        })
        .collect();
    if out.len() > BODY_MAX_BYTES {
        let mut end = BODY_MAX_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out.trim().to_owned()
}

// ------------------------------------------------------------------ query syntax

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    Empty,
    TooLong,
    TooManyTerms,
}

impl QueryError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLong => "too_long",
            Self::TooManyTerms => "too_many_terms",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// A word (`prefix`: the last word of the query, matched as a prefix).
    Word {
        text: String,
        prefix: bool,
    },
    Phrase(String),
}

/// A parsed `q`: AND of groups; a group is the OR of its terms; `excluded` terms are negated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub groups: Vec<Vec<Term>>,
    pub excluded: Vec<Term>,
}

impl Query {
    /// Words (the last one prefix-matched), `"phrases"`, `-word` excludes, `OR` between terms;
    /// anything else is a word (C-727).
    pub fn parse(q: &str) -> Result<Self, QueryError> {
        let q = q.trim();
        if q.is_empty() {
            return Err(QueryError::Empty);
        }
        if q.chars().count() > QUERY_MAX_CHARS {
            return Err(QueryError::TooLong);
        }
        let chars: Vec<char> = q
            .chars()
            .filter(|c| !c.is_control() || c.is_whitespace())
            .collect();
        let mut tokens: Vec<(bool, bool, String, bool)> = Vec::new(); // (negated, phrase, text, is_or)
        let mut i = 0;
        while i < chars.len() {
            if chars[i].is_whitespace() {
                i += 1;
                continue;
            }
            let mut neg = false;
            if chars[i] == '-'
                && chars
                    .get(i + 1)
                    .is_some_and(|c| !c.is_whitespace() && *c != '-')
            {
                neg = true;
                i += 1;
            }
            if chars[i] == '"' {
                let end = chars[i + 1..]
                    .iter()
                    .position(|c| *c == '"')
                    .map_or(chars.len(), |p| p + i + 1);
                let text: String = chars[i + 1..end].iter().collect();
                tokens.push((neg, true, text, false));
                i = (end + 1).min(chars.len());
            } else {
                let end = chars[i..]
                    .iter()
                    .position(|c| c.is_whitespace())
                    .map_or(chars.len(), |p| p + i);
                let text: String = chars[i..end].iter().collect();
                let is_or = !neg && text == "OR";
                tokens.push((neg, false, text, is_or));
                i = end;
            }
        }
        // Terms that carry no letter or digit match nothing and are dropped.
        let usable = |t: &str| t.chars().any(char::is_alphanumeric);
        let mut groups: Vec<Vec<Term>> = Vec::new();
        let mut excluded = Vec::new();
        let mut or_next = false;
        let mut n_terms = 0;
        for (neg, phrase, text, is_or) in tokens {
            if is_or {
                or_next = true;
                continue;
            }
            if !usable(&text) {
                continue;
            }
            n_terms += 1;
            let term = if phrase {
                Term::Phrase(text.trim().to_owned())
            } else {
                Term::Word {
                    text,
                    prefix: false,
                }
            };
            if neg {
                excluded.push(term);
                or_next = false;
            } else if or_next && let Some(g) = groups.last_mut() {
                g.push(term);
                or_next = false;
            } else {
                groups.push(vec![term]);
                or_next = false;
            }
        }
        if n_terms > QUERY_MAX_TERMS {
            return Err(QueryError::TooManyTerms);
        }
        if groups.is_empty() {
            return Err(QueryError::Empty);
        }
        // The last positive word is matched as a prefix.
        if let Some(Term::Word { text, prefix }) = groups.last_mut().and_then(|g| g.last_mut())
            && text.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            *prefix = true;
        }
        Ok(Self { groups, excluded })
    }
}

// ------------------------------------------------------------------ SQL building

#[derive(Debug, Clone)]
enum B {
    Text(String),
    Uuid(Uuid),
    I64(i64),
    Ts(DateTime<Utc>),
    F64(f64),
}

#[derive(Default)]
struct Binds(Vec<B>);

impl Binds {
    fn push(&mut self, b: B, cast: &str) -> String {
        self.0.push(b);
        format!("${}{cast}", self.0.len())
    }
}

fn leaf(t: &Term, lang: &str, binds: &mut Binds) -> String {
    let (func, text) = match t {
        Term::Word { text, prefix: true } => ("to_tsquery", format!("{text}:*")),
        Term::Word { text, .. } => ("plainto_tsquery", text.clone()),
        Term::Phrase(p) => ("phraseto_tsquery", p.clone()),
    };
    let p = binds.push(B::Text(text), "::text");
    format!("({func}({lang}, {p}) || {func}('simple'::regconfig, {p}))")
}

/// The tsquery expression of `q` for the workspace language `lang` (a `$n::regconfig` SQL text).
fn tsquery_sql(q: &Query, lang: &str, binds: &mut Binds) -> String {
    let mut parts: Vec<String> = q
        .groups
        .iter()
        .map(|g| {
            let leaves: Vec<String> = g.iter().map(|t| leaf(t, lang, binds)).collect();
            format!("({})", leaves.join(" || "))
        })
        .collect();
    for t in &q.excluded {
        parts.push(format!("!!{}", leaf(t, lang, binds)));
    }
    format!("({})", parts.join(" && "))
}

// ------------------------------------------------------------------ the search

/// Who is searching, and therefore which conversations are in scope (applied in the query).
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    /// A dashboard caller: non-direct conversations, their own direct ones, and every direct one
    /// when `owner` (`can_see_conversation`).
    Caller { user_id: Uuid, owner: bool },
    /// An agent tool call (C-728): the agent's conversations that are the current one, the
    /// speaker's own direct one, or non-direct and visible to the speaker (a speaker who lost
    /// workspace access sees none of them); a speakerless turn gets the current conversation and
    /// every non-direct one. Never another person's direct conversation.
    Agent {
        agent_id: Uuid,
        current_conversation: Uuid,
        speaker: Option<Uuid>,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum AuthorFilter {
    /// Events of turns whose speaker is this user.
    Speaker(Uuid),
    /// Agent `text` and `attachment` events.
    Agent,
    /// Everything that is neither the agent nor this user (`None`: nor any user filter).
    Others(Option<Uuid>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archived {
    Include,
    Exclude,
    Only,
}

/// Keyset position of the last row of a page.
#[derive(Debug, Clone)]
pub struct Cursor {
    pub at: DateTime<Utc>,
    pub c: Uuid,
    pub s: i64,
    /// Relevance sort only.
    pub r: Option<f64>,
}

pub struct Params<'a> {
    pub workspace_id: Uuid,
    /// The workspace's memory language (a text-search configuration name).
    pub language: &'a str,
    pub query: &'a Query,
    pub scope: Scope,
    pub conversation_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub author: Option<AuthorFilter>,
    pub has_file: bool,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub archived: Archived,
    pub relevance: bool,
    pub limit: i64,
    pub cursor: Option<Cursor>,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub conversation_id: Uuid,
    pub conversation_title: Option<String>,
    pub conversation_kind: String,
    pub archived: bool,
    pub agent_id: Uuid,
    pub agent_name: String,
    pub seq: i64,
    pub ty: String,
    pub author_kind: String,
    pub author_label: String,
    pub at: DateTime<Utc>,
    /// `ts_headline` output with the control sentinels; see [`snippet`].
    pub raw_snippet: String,
    pub rank: f64,
}

pub struct Page {
    pub hits: Vec<Hit>,
    pub next: Option<Cursor>,
}

/// The visibility predicate over `conversations c` (the one expression every search and read
/// uses; keep in step with `authz::can_see_conversation`).
fn scope_sql(scope: Scope, b: &mut Binds) -> Vec<String> {
    match scope {
        Scope::Caller { user_id, owner } => {
            let u = b.push(B::Uuid(user_id), "::uuid");
            vec![if owner {
                "TRUE".to_owned()
            } else {
                format!("(c.kind <> 'direct' OR c.direct_user_id = {u})")
            }]
        }
        Scope::Agent {
            agent_id,
            current_conversation,
            speaker,
        } => {
            let a = b.push(B::Uuid(agent_id), "::uuid");
            let cur = b.push(B::Uuid(current_conversation), "::uuid");
            let own = format!("c.agent_id = {a}");
            let vis = match speaker {
                Some(sp) => {
                    let u = b.push(B::Uuid(sp), "::uuid");
                    format!(
                        "(c.id = {cur} OR (c.kind = 'direct' AND c.direct_user_id = {u}) \
                         OR (c.kind <> 'direct' AND EXISTS (SELECT 1 FROM effective_workspace_access ea \
                             WHERE ea.workspace_id = c.workspace_id AND ea.user_id = {u})))"
                    )
                }
                None => format!("(c.id = {cur} OR c.kind <> 'direct')"),
            };
            vec![own, vis]
        }
    }
}

/// One message of a [`read_window`] (`text: None`: redacted).
#[derive(Debug, Clone)]
pub struct WindowMessage {
    pub seq: i64,
    pub at: DateTime<Utc>,
    pub author_kind: &'static str,
    pub author_label: String,
    pub text: Option<String>,
}

pub struct Window {
    pub conversation_title: Option<String>,
    pub conversation_kind: String,
    pub agent_name: String,
    pub messages: Vec<WindowMessage>,
}

/// `conversation_read` (C-728): the searchable events around `seq` of `conversation_id`, only
/// when the conversation is inside `scope` (same predicate as the search); `None` otherwise, which
/// callers must not distinguish from an unknown conversation. Redacted events come back with
/// `text: None`.
#[allow(clippy::too_many_arguments)]
pub async fn read_window(
    pool: &PgPool,
    workspace_id: Uuid,
    scope: Scope,
    conversation_id: Uuid,
    seq: i64,
    before: i64,
    after: i64,
    text_max_chars: usize,
) -> Result<Option<Window>, DbError> {
    let mut b = Binds::default();
    let ws = b.push(B::Uuid(workspace_id), "::uuid");
    let conv = b.push(B::Uuid(conversation_id), "::uuid");
    let mut w = vec![format!("c.workspace_id = {ws}"), format!("c.id = {conv}")];
    w.extend(scope_sql(scope, &mut b));
    let head = format!(
        "SELECT c.title, c.kind, a.name FROM conversations c JOIN agents a ON a.id = c.agent_id WHERE {}",
        w.join(" AND ")
    );
    let mut q = sqlx::query_as::<_, (Option<String>, String, String)>(sqlx::AssertSqlSafe(&*head));
    for v in &b.0 {
        q = match v {
            B::Text(s) => q.bind(s.clone()),
            B::Uuid(u) => q.bind(*u),
            B::I64(n) => q.bind(*n),
            B::Ts(t) => q.bind(*t),
            B::F64(f) => q.bind(*f),
        };
    }
    let Some((title, kind, agent_name)) = q.fetch_optional(pool).await? else {
        return Ok(None);
    };
    let cols = "SELECT e.seq, e.type, e.payload, e.created_at, e.redacted_at, t.speaker_kind, t.speaker_label
         FROM transcript_events e LEFT JOIN turns t ON t.id = e.turn_id
         WHERE e.conversation_id = $1 AND e.type IN ('turn.queued', 'text', 'attachment')";
    let mut rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{cols} AND e.seq <= $2 ORDER BY e.seq DESC LIMIT $3"
    )))
    .bind(conversation_id)
    .bind(seq)
    .bind(before.clamp(0, 20) + 1)
    .fetch_all(pool)
    .await?;
    rows.reverse();
    rows.extend(
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{cols} AND e.seq > $2 ORDER BY e.seq LIMIT $3"
        )))
        .bind(conversation_id)
        .bind(seq)
        .bind(after.clamp(0, 20))
        .fetch_all(pool)
        .await?,
    );
    let mut messages = Vec::new();
    for r in rows {
        let ty: String = r.try_get("type")?;
        let payload: serde_json::Value = r.try_get("payload")?;
        let redacted: Option<DateTime<Utc>> = r.try_get("redacted_at")?;
        let speaker_kind: Option<String> = r.try_get("speaker_kind")?;
        let (author_kind, author_label) = if ty == "turn.queued" {
            (
                if speaker_kind.as_deref() == Some("person") {
                    "person"
                } else {
                    "other"
                },
                r.try_get::<Option<String>, _>("speaker_label")?
                    .unwrap_or_default(),
            )
        } else {
            ("agent", agent_name.clone())
        };
        let text = if redacted.is_some() || payload.get("redacted").is_some() {
            None
        } else {
            let s = |k: &str| {
                payload
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned()
            };
            let t = match ty.as_str() {
                "attachment" => format!("[file] {} {}", s("filename"), s("caption")),
                _ => s("text"),
            };
            Some(t.trim().chars().take(text_max_chars).collect::<String>())
        };
        messages.push(WindowMessage {
            seq: r.try_get("seq")?,
            at: r.try_get("created_at")?,
            author_kind,
            author_label,
            text,
        });
    }
    Ok(Some(Window {
        conversation_title: title,
        conversation_kind: kind,
        agent_name,
        messages,
    }))
}

/// True when `e` is a statement timeout (`57014`).
pub fn is_timeout(e: &DbError) -> bool {
    matches!(e, DbError::Other(sqlx::Error::Database(d)) if d.code().as_deref() == Some("57014"))
}

/// Run one search. At most `limit` hits; `next` when more exist.
pub async fn search(pool: &PgPool, p: &Params<'_>) -> Result<Page, DbError> {
    let mut b = Binds::default();
    let lang = b.push(B::Text(p.language.to_owned()), "::text::regconfig");
    let q = tsquery_sql(p.query, &lang, &mut b);
    let ws = b.push(B::Uuid(p.workspace_id), "::uuid");
    let mut w: Vec<String> = vec![format!("s.workspace_id = {ws}"), format!("s.doc @@ {q}")];
    w.extend(scope_sql(p.scope, &mut b));
    if let Some(c) = p.conversation_id {
        let c = b.push(B::Uuid(c), "::uuid");
        w.push(format!("s.conversation_id = {c}"));
    }
    if let Some(a) = p.agent_id {
        let a = b.push(B::Uuid(a), "::uuid");
        w.push(format!("s.agent_id = {a}"));
    }
    match p.author {
        Some(AuthorFilter::Speaker(u)) => {
            let u = b.push(B::Uuid(u), "::uuid");
            w.push(format!("s.speaker_user_id = {u}"));
        }
        Some(AuthorFilter::Agent) => w.push("s.author_kind = 'agent'".to_owned()),
        Some(AuthorFilter::Others(u)) => {
            w.push("s.author_kind <> 'agent'".to_owned());
            if let Some(u) = u {
                let u = b.push(B::Uuid(u), "::uuid");
                w.push(format!("s.speaker_user_id IS DISTINCT FROM {u}"));
            }
        }
        None => {}
    }
    if p.has_file {
        w.push("s.has_file".to_owned());
    }
    if let Some(t) = p.after {
        let t = b.push(B::Ts(t), "::timestamptz");
        w.push(format!("s.at >= {t}"));
    }
    if let Some(t) = p.before {
        let t = b.push(B::Ts(t), "::timestamptz");
        w.push(format!("s.at < {t}"));
    }
    match p.archived {
        Archived::Include => {}
        Archived::Exclude => w.push("c.archived_at IS NULL".to_owned()),
        Archived::Only => w.push("c.archived_at IS NOT NULL".to_owned()),
    }
    let rank = if p.relevance {
        format!("ts_rank_cd(s.doc, {q})::float8")
    } else {
        "0::float8".to_owned()
    };
    let mut keyset = String::new();
    if let Some(cur) = &p.cursor {
        let (at, c, s) = (
            b.push(B::Ts(cur.at), "::timestamptz"),
            b.push(B::Uuid(cur.c), "::uuid"),
            b.push(B::I64(cur.s), "::bigint"),
        );
        let pos = format!("(m.at, m.conversation_id, m.seq) < ({at}, {c}, {s})");
        keyset = if p.relevance {
            let r = b.push(B::F64(cur.r.unwrap_or(0.0)), "::float8");
            format!("WHERE (m.rank < {r} OR (m.rank = {r} AND {pos}))")
        } else {
            format!("WHERE {pos}")
        };
    }
    let order = if p.relevance {
        "m.rank DESC, m.at DESC, m.conversation_id DESC, m.seq DESC"
    } else {
        "m.at DESC, m.conversation_id DESC, m.seq DESC"
    };
    let limit = p.limit.clamp(1, 50) + 1;
    let hl_opts = format!(
        "'StartSel=' || chr({}) || ', StopSel=' || chr({}) || ', MaxFragments=1, MaxWords=30, MinWords=12'",
        START as u32, STOP as u32
    );
    let sql = format!(
        "WITH m AS (
            SELECT s.conversation_id, s.seq, s.type, s.author_kind, s.author_label, s.at,
                   s.agent_id, a.name AS agent_name, c.title, c.kind, (c.archived_at IS NOT NULL) AS archived,
                   {rank} AS rank
            FROM transcript_search s
              JOIN conversations c ON c.id = s.conversation_id
              JOIN agents a ON a.id = s.agent_id
            WHERE {where_}
         ), page AS (
            SELECT * FROM m {keyset} ORDER BY {order} LIMIT {limit}
         )
         SELECT m.*, CASE WHEN position(chr({start}) IN h1) > 0 THEN h1 ELSE h2 END AS snip
         FROM (SELECT page.*, ts_headline({lang}, b.body, {q}, {hl_opts}) AS h1,
                      ts_headline('simple'::regconfig, b.body, {q}, {hl_opts}) AS h2
               FROM page JOIN transcript_search b USING (conversation_id, seq)) m
         ORDER BY {order}",
        where_ = w.join(" AND "),
        start = START as u32,
    );
    let mut tx = pool.begin().await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = {STATEMENT_TIMEOUT_MS}"
    )))
    .execute(&mut *tx)
    .await?;
    let mut query = sqlx::query(sqlx::AssertSqlSafe(&*sql));
    for v in &b.0 {
        query = match v {
            B::Text(s) => query.bind(s),
            B::Uuid(u) => query.bind(*u),
            B::I64(n) => query.bind(*n),
            B::Ts(t) => query.bind(*t),
            B::F64(f) => query.bind(*f),
        };
    }
    let rows = query.fetch_all(&mut *tx).await?;
    tx.rollback().await?;
    let mut hits: Vec<Hit> = rows
        .into_iter()
        .map(|r| -> Result<Hit, sqlx::Error> {
            Ok(Hit {
                conversation_id: r.try_get("conversation_id")?,
                conversation_title: r.try_get("title")?,
                conversation_kind: r.try_get("kind")?,
                archived: r.try_get("archived")?,
                agent_id: r.try_get("agent_id")?,
                agent_name: r.try_get("agent_name")?,
                seq: r.try_get("seq")?,
                ty: r.try_get("type")?,
                author_kind: r.try_get("author_kind")?,
                author_label: r.try_get("author_label")?,
                at: r.try_get("at")?,
                raw_snippet: r.try_get("snip")?,
                rank: r.try_get("rank")?,
            })
        })
        .collect::<Result<_, _>>()?;
    let more = hits.len() as i64 > limit - 1;
    hits.truncate((limit - 1) as usize);
    let next = more.then(|| hits.last()).flatten().map(|h| Cursor {
        at: h.at,
        c: h.conversation_id,
        s: h.seq,
        r: p.relevance.then_some(h.rank),
    });
    Ok(Page { hits, next })
}

/// Convert `ts_headline` output into a plain snippet (≤ [`SNIPPET_MAX_CHARS`] characters, no
/// markup, no control characters) and highlight ranges as UTF-16 code-unit offsets into it.
pub fn snippet(
    raw: &str,
    body_start_is_cut: bool,
    body_end_is_cut: bool,
) -> (String, Vec<[usize; 2]>) {
    snippet_bounded(raw, body_start_is_cut, body_end_is_cut, SNIPPET_MAX_CHARS)
}

/// [`snippet`] with another length bound (memory search and recall, C-1171).
pub fn snippet_bounded(
    raw: &str,
    body_start_is_cut: bool,
    body_end_is_cut: bool,
    max_chars: usize,
) -> (String, Vec<[usize; 2]>) {
    let mut text = String::new();
    let mut units = 0usize;
    let mut ranges: Vec<[usize; 2]> = Vec::new();
    let mut open: Option<usize> = None;
    if body_start_is_cut {
        text.push('…');
        units += 1;
    }
    let mut chars = 0usize;
    for c in raw.chars() {
        match c {
            START => open = Some(units),
            STOP => {
                if let Some(s) = open.take()
                    && units > s
                {
                    ranges.push([s, units]);
                }
            }
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => {
                if chars >= max_chars {
                    break;
                }
                let c = if c == '\n' || c == '\t' { ' ' } else { c };
                text.push(c);
                units += c.len_utf16();
                chars += 1;
            }
        }
    }
    if let Some(s) = open.take()
        && units > s
    {
        ranges.push([s, units]);
    }
    if body_end_is_cut {
        text.push('…');
    }
    (text, ranges)
}

// ------------------------------------------------------------------ the index

/// One indexed message.
#[derive(Debug, Clone)]
pub struct IndexRow {
    pub conversation_id: Uuid,
    pub seq: i64,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub language: String,
    pub ty: String,
    pub author_kind: &'static str,
    pub author_label: String,
    pub speaker_user_id: Option<Uuid>,
    pub has_file: bool,
    pub at: DateTime<Utc>,
    pub body: String,
}

/// What an event contributes to the index: `(body, has_file)`, or `None` when it is not
/// searchable (other types, redacted markers, empty text).
pub fn event_body(
    ty: &str,
    payload: &serde_json::Value,
    filenames: &[String],
) -> Option<(String, bool)> {
    if payload.get("redacted").is_some() {
        return None;
    }
    let s = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let (body, has_file) = match ty {
        "turn.queued" => {
            let mut b = s("text").to_owned();
            for f in filenames {
                b.push('\n');
                b.push_str(f);
            }
            (b, !filenames.is_empty())
        }
        "text" => (plain_text(s("text")), false),
        "attachment" => {
            let mut b = s("filename").to_owned();
            if !s("caption").is_empty() {
                b.push('\n');
                b.push_str(s("caption"));
            }
            (b, true)
        }
        _ => return None,
    };
    let body = clean_body(&body);
    (!body.is_empty()).then_some((body, has_file))
}

async fn insert_row(conn: &mut PgConnection, r: &IndexRow) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO transcript_search (conversation_id, seq, workspace_id, agent_id, type, author_kind,
            author_label, speaker_user_id, has_file, at, body, doc)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,
            setweight(to_tsvector($12::text::regconfig, $11), 'A') || setweight(to_tsvector('simple'::regconfig, $11), 'B'))
         ON CONFLICT (conversation_id, seq) DO NOTHING",
    )
    .bind(r.conversation_id)
    .bind(r.seq)
    .bind(r.workspace_id)
    .bind(r.agent_id)
    .bind(&r.ty)
    .bind(r.author_kind)
    .bind(&r.author_label)
    .bind(r.speaker_user_id)
    .bind(r.has_file)
    .bind(r.at)
    .bind(&r.body)
    .bind(&r.language)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn filenames_of(
    conn: &mut PgConnection,
    payload: &serde_json::Value,
) -> Result<Vec<String>, DbError> {
    let ids: Vec<Uuid> = payload
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().and_then(|s| s.parse().ok()))
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT filename FROM attachments WHERE id = ANY($1) ORDER BY created_at, id",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?)
}

/// The write path: index the durable event just appended, in the same transaction. No-op for
/// events that are not searchable.
pub async fn index_event(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    seq: i64,
    turn_id: Option<Uuid>,
    ty: &str,
    payload: &serde_json::Value,
) -> Result<(), DbError> {
    if !matches!(ty, "turn.queued" | "text" | "attachment") {
        return Ok(());
    }
    let names = if ty == "turn.queued" {
        filenames_of(&mut *conn, payload).await?
    } else {
        Vec::new()
    };
    let Some((body, has_file)) = event_body(ty, payload, &names) else {
        return Ok(());
    };
    let Some((workspace_id, agent_id, language, at)): Option<(Uuid, Uuid, String, DateTime<Utc>)> =
        sqlx::query_as(
            "SELECT c.workspace_id, c.agent_id, w.memory_language, e.created_at
             FROM transcript_events e JOIN conversations c ON c.id = e.conversation_id
               JOIN workspaces w ON w.id = c.workspace_id
             WHERE e.conversation_id = $1 AND e.seq = $2",
        )
        .bind(conversation_id)
        .bind(seq)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Ok(());
    };
    let (author_kind, author_label, speaker_user_id) = if ty == "turn.queued" {
        let t: Option<(String, Option<Uuid>, String)> =
            match turn_id {
                Some(t) => sqlx::query_as(
                    "SELECT speaker_kind, speaker_user_id, speaker_label FROM turns WHERE id = $1",
                )
                .bind(t)
                .fetch_optional(&mut *conn)
                .await?,
                None => None,
            };
        match t {
            Some((k, u, l)) => (if k == "person" { "person" } else { "other" }, l, u),
            None => ("other", String::new(), None),
        }
    } else {
        ("agent", String::new(), None)
    };
    insert_row(
        conn,
        &IndexRow {
            conversation_id,
            seq,
            workspace_id,
            agent_id,
            language,
            ty: ty.to_owned(),
            author_kind,
            author_label,
            speaker_user_id,
            has_file,
            at,
            body,
        },
    )
    .await
}

/// Redaction: the event's row goes in the redaction's transaction.
pub async fn remove_event(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    seq: i64,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM transcript_search WHERE conversation_id = $1 AND seq = $2")
        .bind(conversation_id)
        .bind(seq)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

// ------------------------------------------------------------------ backfill

/// `false` while the backfill of pre-existing events is still running (`index_complete`).
pub async fn index_complete(pool: &PgPool) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT complete FROM transcript_search_backfill")
            .fetch_one(pool)
            .await?,
    )
}

/// What one backfill step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub scanned: usize,
    pub complete: bool,
}

/// Index the next batch of existing events (key order, resumable from the persisted cursor).
/// The event rows are share-locked and re-checked for `redacted_at`, so a concurrent redaction
/// either finishes first (its event is skipped) or waits for this transaction and then deletes
/// the row it created. Rows already indexed by the write path are left alone.
pub async fn backfill_step(pool: &PgPool, batch: i64) -> Result<Step, DbError> {
    let mut tx = pool.begin().await?;
    let (cc, cs, done): (Option<Uuid>, Option<i64>, bool) = sqlx::query_as(
        "SELECT cursor_conversation, cursor_seq, complete FROM transcript_search_backfill FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await?;
    if done {
        return Ok(Step {
            scanned: 0,
            complete: true,
        });
    }
    let select = "SELECT e.conversation_id, e.seq, e.turn_id, e.type, e.payload, e.created_at,
                c.workspace_id, c.agent_id, w.memory_language,
                t.speaker_kind, t.speaker_user_id, t.speaker_label
         FROM transcript_events e JOIN conversations c ON c.id = e.conversation_id
           JOIN workspaces w ON w.id = c.workspace_id LEFT JOIN turns t ON t.id = e.turn_id
         WHERE e.type IN ('turn.queued', 'text', 'attachment') AND e.redacted_at IS NULL";
    let rows = match (cc, cs) {
        (Some(c), Some(s)) => {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "{select} AND (e.conversation_id, e.seq) > ($1, $2)
                 ORDER BY e.conversation_id, e.seq LIMIT $3 FOR SHARE OF e"
            )))
            .bind(c)
            .bind(s)
            .bind(batch)
            .fetch_all(&mut *tx)
            .await?
        }
        _ => {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "{select} ORDER BY e.conversation_id, e.seq LIMIT $1 FOR SHARE OF e"
            )))
            .bind(batch)
            .fetch_all(&mut *tx)
            .await?
        }
    };
    let mut last: Option<(Uuid, i64)> = None;
    let mut indexed = 0i64;
    for r in &rows {
        let conversation_id: Uuid = r.try_get("conversation_id")?;
        let seq: i64 = r.try_get("seq")?;
        last = Some((conversation_id, seq));
        let ty: String = r.try_get("type")?;
        let payload: serde_json::Value = r.try_get("payload")?;
        let names = if ty == "turn.queued" {
            filenames_of(&mut tx, &payload).await?
        } else {
            Vec::new()
        };
        let Some((body, has_file)) = event_body(&ty, &payload, &names) else {
            continue;
        };
        let speaker_kind: Option<String> = r.try_get("speaker_kind")?;
        let (author_kind, author_label, speaker_user_id) = if ty == "turn.queued" {
            (
                if speaker_kind.as_deref() == Some("person") {
                    "person"
                } else {
                    "other"
                },
                r.try_get::<Option<String>, _>("speaker_label")?
                    .unwrap_or_default(),
                r.try_get("speaker_user_id")?,
            )
        } else {
            ("agent", String::new(), None)
        };
        insert_row(
            &mut tx,
            &IndexRow {
                conversation_id,
                seq,
                workspace_id: r.try_get("workspace_id")?,
                agent_id: r.try_get("agent_id")?,
                language: r.try_get("memory_language")?,
                ty,
                author_kind,
                author_label,
                speaker_user_id,
                has_file,
                at: r.try_get("created_at")?,
                body,
            },
        )
        .await?;
        indexed += 1;
    }
    let complete = (rows.len() as i64) < batch;
    sqlx::query(
        "UPDATE transcript_search_backfill SET cursor_conversation = COALESCE($1, cursor_conversation),
            cursor_seq = COALESCE($2, cursor_seq), complete = $3, indexed = indexed + $4, updated_at = now()",
    )
    .bind(last.map(|l| l.0))
    .bind(last.map(|l| l.1))
    .bind(complete)
    .bind(indexed)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Step {
        scanned: rows.len(),
        complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_reduced_to_plain_text() {
        let md = "# Title\n**bold** and `code`\n- item [docs](https://x.io/a) ![pic](https://x.io/p.png)\n```rust\nlet x = 1;\n```\n> quoted <https://y.io>";
        let t = plain_text(md);
        assert!(t.contains("Title"));
        assert!(t.contains("bold and code"));
        assert!(t.contains("item docs https://x.io/a pic https://x.io/p.png"));
        assert!(t.contains("let x = 1;"));
        assert!(!t.contains("```"));
        assert!(t.contains("quoted https://y.io"));
    }

    #[test]
    fn parses_words_phrases_excludes_and_or() {
        let q = Query::parse("deploy \"blue green\" -rollback staging OR prod").unwrap();
        assert_eq!(q.groups.len(), 3, "deploy, the phrase, and staging OR prod");
        assert_eq!(q.groups[2].len(), 2, "OR joins staging and prod");
        assert_eq!(
            q.excluded,
            vec![Term::Word {
                text: "rollback".into(),
                prefix: false
            }]
        );
        assert!(matches!(&q.groups[2][1], Term::Word { text, prefix: true } if text == "prod"));
    }

    #[test]
    fn hostile_input_is_data() {
        for q in [
            "a & b | !c",
            "'; DROP TABLE x; --",
            "\\\\ \\ ",
            "(((",
            "a:*b <-> c",
            "\"unterminated",
        ] {
            let _ = Query::parse(q);
        }
        assert_eq!(Query::parse("   ").unwrap_err(), QueryError::Empty);
        assert_eq!(Query::parse("& | !").unwrap_err(), QueryError::Empty);
        assert_eq!(
            Query::parse(&"a".repeat(201)).unwrap_err(),
            QueryError::TooLong
        );
        let many = (0..13)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(Query::parse(&many).unwrap_err(), QueryError::TooManyTerms);
        // The prefix form is only ever built from letters, digits and underscores.
        let q = Query::parse("foo:*bar").unwrap();
        assert!(matches!(&q.groups[0][0], Term::Word { prefix: false, .. }));
    }

    #[test]
    fn snippet_markers_become_utf16_ranges() {
        let raw = format!("a 😀 {START}hit{STOP} b");
        let (t, r) = snippet(&raw, false, false);
        assert_eq!(t, "a 😀 hit b");
        assert_eq!(r, vec![[5, 8]], "the emoji is two UTF-16 units");
        let (t, r) = snippet(&format!("{START}x{STOP}"), true, true);
        assert_eq!((t.as_str(), r), ("…x…", vec![[1, 2]]));
    }

    #[test]
    fn bodies_are_cleaned_and_bounded() {
        assert_eq!(clean_body("a\u{1}b"), "a b");
        assert!(clean_body(&"é".repeat(40_000)).len() <= BODY_MAX_BYTES);
        assert!(event_body("tool.started", &serde_json::json!({"text": "x"}), &[]).is_none());
        assert!(event_body("text", &serde_json::json!({"redacted": true}), &[]).is_none());
        let (b, f) = event_body(
            "turn.queued",
            &serde_json::json!({"text": "hi"}),
            &["a.txt".into()],
        )
        .unwrap();
        assert_eq!((b.as_str(), f), ("hi\na.txt", true));
    }
}
