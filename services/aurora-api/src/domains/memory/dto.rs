use aurora_db::repo::memory as repo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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
}

impl From<repo::MemoryRow> for Memory {
    fn from(m: repo::MemoryRow) -> Self {
        Self {
            index_line: super::service::index_line(
                &m.key,
                m.title.as_deref(),
                m.description.as_deref(),
                &m.content,
            ),
            key: m.key,
            title: m.title,
            description: m.description,
            content: m.content,
            revision: m.revision,
            updated_at: m.updated_at,
            created_at: m.created_at,
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
}

impl From<repo::SearchHit> for MemorySearchHit {
    fn from(h: repo::SearchHit) -> Self {
        Self {
            key: h.key,
            title: h.title,
            score: h.score,
            snippet: h.snippet,
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
