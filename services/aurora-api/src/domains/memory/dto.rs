use aurora_db::repo::memory as repo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct Memory {
    pub key: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: String,
    /// `key: line` as shown in the memory index.
    pub index_line: String,
    pub revision: i32,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// (P6-MEMLOCK, C-1148) The agent can read but not change or delete a locked memory.
    pub locked: bool,
    /// Set when `locked` became true, cleared when it became false.
    pub locked_at: Option<DateTime<Utc>>,
    /// The person who locked it; null for an API key without a user and for a person who was
    /// later deleted.
    pub locked_by: Option<MemoryLockedBy>,
}

#[derive(Serialize, ToSchema)]
pub struct MemoryLockedBy {
    pub id: Uuid,
    pub display_name: String,
}

impl From<repo::MemoryRow> for Memory {
    fn from(m: repo::MemoryRow) -> Self {
        Self {
            index_line: aurora_core::memory_index::index_line(
                &m.key,
                m.title.as_deref(),
                m.description.as_deref(),
                &m.content,
                m.locked,
            ),
            key: m.key,
            title: m.title,
            description: m.description,
            content: m.content,
            revision: m.revision,
            updated_at: m.updated_at,
            created_at: m.created_at,
            locked_by: m
                .locked_by
                .zip(m.locked_by_name)
                .map(|(id, display_name)| MemoryLockedBy { id, display_name }),
            locked: m.locked,
            locked_at: m.locked_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct MemoryRevision {
    pub revision: i32,
    /// `user` | `agent` | `system`.
    pub author_kind: String,
    pub author_label: String,
    /// `dashboard` | `api` | `tool` | `harvest` | `restore`.
    pub provenance: String,
    pub redacted: bool,
    pub created_at: DateTime<Utc>,
    /// Null when redacted.
    pub content: Option<String>,
}

impl From<repo::RevisionRow> for MemoryRevision {
    fn from(r: repo::RevisionRow) -> Self {
        let source = r
            .provenance
            .get("source")
            .and_then(|s| s.as_str())
            .unwrap_or("api")
            .to_owned();
        let redacted = r.redacted_at.is_some();
        Self {
            revision: r.revision,
            author_kind: r.author_kind,
            author_label: r.author_label,
            provenance: source,
            redacted,
            created_at: r.created_at,
            content: (!redacted).then_some(r.content),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct MemorySearchHit {
    pub key: String,
    pub title: Option<String>,
    pub score: f32,
    pub snippet: String,
    /// (P6-MEMLOCK, C-1149) The hit's memory is locked.
    pub locked: bool,
}

impl From<repo::SearchHit> for MemorySearchHit {
    fn from(h: repo::SearchHit) -> Self {
        Self {
            key: h.key,
            title: h.title,
            score: h.score,
            snippet: h.snippet,
            locked: h.locked,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct MemorySearchResponse {
    pub items: Vec<MemorySearchHit>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateMemoryRequest {
    /// Immutable kebab-case key, at most 64 characters.
    pub key: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: String,
    /// (P6-MEMLOCK, C-1148) Create the memory locked; default false. `locked_at`/`locked_by` are
    /// set from the caller (null for an API key without a user).
    pub locked: Option<bool>,
}

/// (P6-MEMLOCK, C-1148) `PUT …/memories/{key}/locked` (`setMemoryLocked`): idempotent.
#[derive(Deserialize, ToSchema)]
pub struct SetMemoryLockedRequest {
    pub locked: bool,
}

/// Full replacement of the mutable fields; creates the memory when the key is new.
#[derive(Deserialize, ToSchema)]
pub struct PutMemoryRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: String,
}

#[derive(Deserialize, ToSchema)]
pub struct RestoreMemoryRequest {
    pub revision: i32,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchParams {
    /// Search text (websearch syntax).
    pub q: String,
}
