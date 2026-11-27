use super::actor::{self, Names, StoredActor};
use crate::domains::conversations::reference::ConversationRef;
use aurora_db::repo::memory as repo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// (P5-MEMORY-1, C-1169) What kind of writer a [`MemoryActor`] is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryActorKind {
    Person,
    ApiKey,
    Agent,
    Template,
    Import,
    /// Written before this was recorded.
    #[default]
    Unknown,
}

#[derive(Serialize, ToSchema)]
pub struct ActorPerson {
    pub id: Uuid,
    /// Never an e-mail address.
    pub display_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct ActorApiKey {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct ActorAgent {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct ActorTemplate {
    pub name: String,
    pub version: String,
}

/// (P5-MEMORY-1, C-1169, D-497) Who added, changed or wrote a memory. The fields that do not
/// apply to `kind` are null: `person` for `person`, `api_key` (the key's person) and `template`
/// (who created or applied it); `api_key` for `api_key`, null when an older record did not keep
/// which key; `agent` for `agent`.
#[derive(Serialize, ToSchema)]
pub struct MemoryActor {
    pub kind: MemoryActorKind,
    pub person: Option<ActorPerson>,
    pub api_key: Option<ActorApiKey>,
    pub agent: Option<ActorAgent>,
    /// The agent's conversation, only for a caller who may read it (SEC-157).
    pub conversation: Option<ConversationRef>,
    pub template: Option<ActorTemplate>,
}

/// (P5-MEMORY-1, C-1169, D-497(b)) The agent's latest recorded use of a memory.
#[derive(Serialize, ToSchema)]
pub struct MemoryLastUsed {
    pub at: DateTime<Utc>,
    /// Only for a caller who may read it (SEC-157).
    pub conversation: Option<ConversationRef>,
}

/// (P5-MEMORY-1, C-1170) How a revision was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryVia {
    Dashboard,
    Api,
    Conversation,
    Harvest,
    Template,
    Restore,
    Import,
}

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
    /// (P5-MEMORY-1, C-1169) Who created the memory.
    pub added_by: MemoryActor,
    /// Who wrote its current content.
    pub changed_by: MemoryActor,
    /// When the agent last used it; null while it never has (dashboard and API reads do not
    /// count).
    pub last_used: Option<MemoryLastUsed>,
}

#[derive(Serialize, ToSchema)]
pub struct MemoryLockedBy {
    pub id: Uuid,
    pub display_name: String,
}

impl Memory {
    /// The row as the caller may see it; `names` comes from [`actor::names`] over these rows.
    pub(super) fn present(
        m: repo::MemoryRow,
        added: &StoredActor,
        changed: &StoredActor,
        names: &Names,
    ) -> Self {
        Self {
            added_by: names.present(added),
            changed_by: names.present(changed),
            last_used: m.last_used_at.map(|at| MemoryLastUsed {
                at,
                conversation: names.conversation(m.last_used_conversation_id),
            }),
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
    /// Deprecated: use `author`. The author's display name (an agent's name), never an e-mail
    /// address.
    pub author_label: String,
    /// `dashboard` | `api` | `tool` | `harvest` | `template` | `restore`.
    pub provenance: String,
    /// (P5-MEMORY-1, C-1170) Who wrote this revision.
    pub author: MemoryActor,
    /// (C-1170) How it was written.
    pub via: MemoryVia,
    /// (C-1170) For a restore: the revision whose content was brought back.
    pub restored_from: Option<i32>,
    pub redacted: bool,
    pub created_at: DateTime<Utc>,
    /// Null when redacted.
    pub content: Option<String>,
}

impl MemoryRevision {
    pub(super) fn present(r: repo::RevisionRow, author: &StoredActor, names: &Names) -> Self {
        let source = r
            .provenance
            .get("source")
            .and_then(|s| s.as_str())
            .unwrap_or("api")
            .to_owned();
        let restored_from = r
            .provenance
            .get("restored_from")
            .and_then(serde_json::Value::as_i64)
            .and_then(|n| i32::try_from(n).ok());
        let via = actor::via_of(&r.provenance);
        let author = names.present(author);
        let redacted = r.redacted_at.is_some();
        Self {
            revision: r.revision,
            author_kind: r.author_kind,
            author_label: actor::label_of(&author),
            provenance: source,
            author,
            via,
            restored_from,
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
    /// (P5-MEMORY-1, C-1171) Plain text on one line (at most 800 characters), never markup.
    pub snippet: String,
    /// `[start, end)` offsets into `snippet`, in UTF-16 code units.
    pub highlights: Vec<[u32; 2]>,
    /// The same offsets into `title`, or into `key` when the memory has no title.
    pub title_highlights: Vec<[u32; 2]>,
    /// (P6-MEMLOCK, C-1149) The hit's memory is locked.
    pub locked: bool,
}

fn offsets(ranges: Vec<[usize; 2]>) -> Vec<[u32; 2]> {
    ranges
        .into_iter()
        .map(|r| r.map(|n| u32::try_from(n).unwrap_or(u32::MAX)))
        .collect()
}

impl From<repo::SearchHit> for MemorySearchHit {
    fn from(h: repo::SearchHit) -> Self {
        Self {
            key: h.key,
            title: h.title,
            score: h.score,
            snippet: h.snippet,
            highlights: offsets(h.highlights),
            title_highlights: offsets(h.title_highlights),
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
