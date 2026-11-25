//! Message search (P5-CHAT-4 part A, D-229, C-727): `GET /v1/workspaces/{ws}/messages/search`
//! and the background backfill of the index. The query builder and the visibility predicate live
//! in `aurora_db::repo::search` (one module, two callers: this route and the agent tools).

use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::search::{self, Archived, AuthorFilter, Cursor, Params, Scope};
use aurora_db::repo::workspaces;
use axum::extract::State;
use base64::Engine as _;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

const LIMIT_DEFAULT: u32 = 20;
const LIMIT_MAX: u32 = 50;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(search_messages))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    /// Words (the last one prefix-matched), `"phrases"`, `-word` to exclude, `OR` between terms.
    /// 1-200 characters, at most 12 terms.
    pub q: String,
    pub agent_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    /// `me` | `agent` | `others`
    pub author: Option<String>,
    /// `file`
    pub has: Option<String>,
    /// RFC 3339 or `YYYY-MM-DD` (UTC); half-open `[after, before)`.
    pub after: Option<String>,
    pub before: Option<String>,
    /// `include` (default) | `exclude` | `only`
    pub archived: Option<String>,
    /// `recent` (default) | `relevance`
    pub sort: Option<String>,
    /// 1-50, default 20.
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SearchConversation {
    pub id: Uuid,
    pub title: Option<String>,
    pub kind: String,
    pub archived: bool,
}

#[derive(Serialize, ToSchema)]
pub struct SearchAgent {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct SearchAuthor {
    /// `person` | `agent` | `other`
    pub kind: String,
    pub label: String,
}

#[derive(Serialize, ToSchema)]
pub struct SearchItem {
    pub conversation: SearchConversation,
    pub agent: SearchAgent,
    pub seq: i64,
    /// `turn.queued` | `text` | `attachment`
    #[serde(rename = "type")]
    pub ty: String,
    pub author: SearchAuthor,
    pub at: DateTime<Utc>,
    /// Plain text (at most 300 characters), never markup.
    pub snippet: String,
    /// `[start, end)` offsets into `snippet`, in UTF-16 code units.
    pub highlights: Vec<[u32; 2]>,
}

#[derive(Serialize, ToSchema)]
pub struct SearchResponse {
    pub items: Vec<SearchItem>,
    pub next_cursor: Option<String>,
    /// `false` while the index of older messages is still being built.
    pub index_complete: bool,
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_field("Invalid request.", path, msg)
}

/// 422 `search_query_invalid` (also the tools' `search_query_invalid`).
pub fn query_invalid(e: &search::QueryError) -> ApiError {
    let msg = match e {
        search::QueryError::Empty => "Enter at least one word to search for.",
        search::QueryError::TooLong => "The search is too long (at most 200 characters).",
        search::QueryError::TooManyTerms => "The search has too many terms (at most 12).",
    };
    ApiError::unprocessable("search_query_invalid", msg)
        .with_details(serde_json::json!({"reason": e.reason()}))
}

/// RFC 3339 or `YYYY-MM-DD` (midnight UTC).
pub fn parse_when(path: &str, s: &str) -> Result<DateTime<Utc>, ApiError> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc())
        .ok_or_else(|| bad(path, "must be an RFC 3339 time or a YYYY-MM-DD date"))
}

#[derive(Serialize, Deserialize)]
struct CursorWire {
    at: DateTime<Utc>,
    c: Uuid,
    s: i64,
    r: Option<f64>,
    /// The sort the cursor belongs to.
    rel: bool,
}

fn encode_cursor(c: &Cursor, relevance: bool) -> String {
    let wire = CursorWire {
        at: c.at,
        c: c.c,
        s: c.s,
        r: c.r,
        rel: relevance,
    };
    let json = serde_json::to_vec(&wire).unwrap_or_default();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

fn decode_cursor(s: &str, relevance: bool) -> Result<Cursor, ApiError> {
    let wire: CursorWire = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|w: &CursorWire| w.rel == relevance)
        .ok_or_else(|| bad("/cursor", "invalid cursor"))?;
    Ok(Cursor {
        at: wire.at,
        c: wire.c,
        s: wire.s,
        r: wire.r,
    })
}

/// A statement timeout of the search is a 503, never a 500 (`57014` query_canceled).
pub fn map_db(e: DbError) -> ApiError {
    if search::is_timeout(&e) {
        return ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "search_timeout",
            "The search took too long. Narrow it down and try again.",
        );
    }
    ApiError::from(e)
}

/// Search the messages of a workspace.
#[utoipa::path(get, path = "/workspaces/{ws}/messages/search", operation_id = "searchMessages", tag = "conversations",
    params(("ws" = Uuid, Path, description = "Workspace id"), SearchQuery),
    responses((status = 200, body = SearchResponse), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 422, description = "search_query_invalid (details.reason: empty | too_long | too_many_terms)", body = ErrorEnvelope),
        (status = 503, description = "search_timeout", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn search_messages(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    Ok(Json(search_workspace(&s, &c, ws, q).await?))
}

pub async fn search_workspace(
    s: &AppState,
    c: &Caller,
    ws: Uuid,
    q: SearchQuery,
) -> Result<SearchResponse, ApiError> {
    // (FIX-236, C-1165) A read route: the caller's own access, or an active emergency grant
    // (which searches like an owner — every conversation, direct ones included).
    let read = c.workspace_read(s, ws).await?;
    let query = search::Query::parse(&q.q).map_err(|e| query_invalid(&e))?;
    let limit = q.limit.unwrap_or(LIMIT_DEFAULT);
    if !(1..=LIMIT_MAX).contains(&limit) {
        return Err(bad("/limit", "must be between 1 and 50"));
    }
    let author = match q.author.as_deref() {
        None => None,
        Some("me") => Some(AuthorFilter::Speaker(c.user_id)),
        Some("agent") => Some(AuthorFilter::Agent),
        Some("others") => Some(AuthorFilter::Others(Some(c.user_id))),
        Some(_) => return Err(bad("/author", "must be me, agent or others")),
    };
    let has_file = match q.has.as_deref() {
        None => false,
        Some("file") => true,
        Some(_) => return Err(bad("/has", "must be file")),
    };
    let archived = match q.archived.as_deref() {
        None | Some("include") => Archived::Include,
        Some("exclude") => Archived::Exclude,
        Some("only") => Archived::Only,
        Some(_) => return Err(bad("/archived", "must be include, exclude or only")),
    };
    let relevance = match q.sort.as_deref() {
        None | Some("recent") => false,
        Some("relevance") => true,
        Some(_) => return Err(bad("/sort", "must be recent or relevance")),
    };
    let after = q
        .after
        .as_deref()
        .map(|v| parse_when("/after", v))
        .transpose()?;
    let before = q
        .before
        .as_deref()
        .map(|v| parse_when("/before", v))
        .transpose()?;
    // A conversation the caller cannot see (or of another workspace) is a 404 like every
    // `conversation=` filter; it never leaks as an empty page.
    if let Some(id) = q.conversation_id {
        let row = service::load_conversation(s, c, id).await?;
        if row.workspace_id != ws {
            return Err(ApiError::not_found("Conversation not found."));
        }
    }
    let cursor = q
        .cursor
        .as_deref()
        .map(|v| decode_cursor(v, relevance))
        .transpose()?;
    let language = workspaces::get(&s.pool, ws)
        .await?
        .map(|w| w.memory_language)
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
    let page = search::search(
        &s.pool,
        &Params {
            workspace_id: ws,
            language: &language,
            query: &query,
            scope: Scope::Caller {
                user_id: c.user_id,
                owner: read.can_own(),
            },
            conversation_id: q.conversation_id,
            agent_id: q.agent_id,
            author,
            has_file,
            after,
            before,
            archived,
            relevance,
            limit: i64::from(limit),
            cursor,
        },
    )
    .await
    .map_err(map_db)?;
    let index_complete = search::index_complete(&s.pool).await?;
    let next_cursor = page.next.as_ref().map(|n| encode_cursor(n, relevance));
    let items = page
        .hits
        .into_iter()
        .map(|h| {
            let (snippet, ranges) = search::snippet(&h.raw_snippet, false, false);
            let author = if h.author_kind == "agent" {
                h.agent_name.clone()
            } else {
                h.author_label.clone()
            };
            let item = SearchItem {
                conversation: SearchConversation {
                    id: h.conversation_id,
                    title: h.conversation_title,
                    kind: h.conversation_kind,
                    archived: h.archived,
                },
                agent: SearchAgent {
                    id: h.agent_id,
                    name: h.agent_name,
                },
                seq: h.seq,
                ty: h.ty,
                author: SearchAuthor {
                    kind: h.author_kind,
                    label: aurora_core::sanitize::label(&author),
                },
                at: h.at,
                snippet,
                highlights: ranges
                    .into_iter()
                    .map(|[a, b]| {
                        [
                            u32::try_from(a).unwrap_or(u32::MAX),
                            u32::try_from(b).unwrap_or(u32::MAX),
                        ]
                    })
                    .collect(),
            };
            (h.conversation_id, item)
        })
        .collect::<Vec<(Uuid, SearchItem)>>();
    // (FIX-236, C-1165) The snippets are conversation content: the emergency reader's search
    // records every conversation it touched, before the body (503 on a failed write).
    if let aurora_db::authz::ReadAuth::Emergency(g) = &read {
        let actor = crate::audit::AuditActor::from_caller(c);
        let mut seen = std::collections::HashSet::new();
        for (conv, _) in &items {
            if seen.insert(*conv) {
                service::record_emergency_view_id(s, &actor, *conv, g).await?;
            }
        }
    }
    Ok(SearchResponse {
        items: items.into_iter().map(|(_, i)| i).collect(),
        next_cursor,
        index_complete,
    })
}

// ------------------------------------------------------------------ backfill

/// Index pre-existing events in the background (batches of 1 000 by key, persisted cursor); the
/// route answers `index_complete: false` until it finishes. Runs on every replica, one at a time
/// (advisory lock per step); stops once complete.
pub fn spawn_backfill(s: AppState) {
    tokio::spawn(async move {
        loop {
            let step =
                aurora_db::jobs::try_singleton(&s.pool, "transcript_search_backfill", || {
                    search::backfill_step(&s.pool, search::BACKFILL_BATCH)
                })
                .await;
            match step {
                Ok(Some(Ok(st))) => {
                    if st.scanned > 0 {
                        tracing::info!(
                            scanned = st.scanned,
                            complete = st.complete,
                            "message search backfill"
                        );
                    }
                    if st.complete {
                        return;
                    }
                    // Next batch right away; the lock is released between batches.
                    tokio::task::yield_now().await;
                }
                Ok(Some(Err(e))) => {
                    tracing::warn!(error = %e, "message search backfill failed");
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
                Ok(None) => tokio::time::sleep(std::time::Duration::from_secs(30)).await,
                Err(e) => {
                    tracing::warn!(error = %e, "message search backfill lock failed");
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            }
        }
    });
}
