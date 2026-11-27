//! (P5-MEMORY-1, C-1169, C-1170, D-497, SEC-157) Who wrote a memory.
//!
//! [`StoredActor`] is what the database keeps: ids only, never a name or an address.
//! [`present`] turns stored actors into the [`MemoryActor`]s of the API: names are looked up at
//! read time (a person by display name, never by e-mail), and a conversation is named only to a
//! caller who may read it (`conversations::reference`).

use super::dto::{
    ActorAgent, ActorApiKey, ActorPerson, ActorTemplate, MemoryActor, MemoryActorKind, MemoryVia,
};
use super::service::{Author, Source};
use crate::app::AppState;
use crate::domains::conversations::reference::ConversationRef;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::memory::{self as repo, RevisionRow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredActor {
    pub kind: MemoryActorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<StoredTemplate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTemplate {
    pub name: String,
    pub version: String,
}

impl StoredActor {
    pub fn of_kind(kind: MemoryActorKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    /// A stored value; anything unreadable is `unknown`.
    pub fn parse(v: &Value) -> Self {
        Self::deserialize(v).unwrap_or_default()
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    /// The actor of a write about to happen.
    pub fn of_write(author: &Author, source: &Source) -> Self {
        if author.kind == "agent" {
            return Self {
                conversation_id: match source {
                    Source::Tool {
                        conversation_id, ..
                    } => Some(*conversation_id),
                    _ => None,
                },
                ..Self::of_kind(MemoryActorKind::Agent)
            };
        }
        let person = Self {
            user_id: author.user_id,
            ..Self::of_kind(MemoryActorKind::Person)
        };
        match source {
            Source::Template { name, version } => Self {
                kind: MemoryActorKind::Template,
                api_key_id: author.api_key_id,
                template: Some(StoredTemplate {
                    name: name.clone(),
                    version: version.clone(),
                }),
                ..person
            },
            _ if author.api_key_id.is_some() => Self {
                kind: MemoryActorKind::ApiKey,
                api_key_id: author.api_key_id,
                ..person
            },
            _ => person,
        }
    }

    /// The actor of a kept revision, including the older ones that recorded no API key id.
    pub fn of_revision(r: &RevisionRow) -> Self {
        let text = |k: &str| r.provenance.get(k).and_then(Value::as_str);
        let source = text("source");
        if r.author_kind == "agent" {
            return Self {
                conversation_id: (source == Some("tool"))
                    .then(|| text("conversation_id"))
                    .flatten()
                    .and_then(|id| Uuid::parse_str(id).ok()),
                ..Self::of_kind(MemoryActorKind::Agent)
            };
        }
        let person = Self {
            user_id: r.author_user_id,
            api_key_id: r.author_api_key_id,
            ..Self::of_kind(MemoryActorKind::Person)
        };
        match source {
            Some("template") => Self {
                kind: MemoryActorKind::Template,
                template: Some(StoredTemplate {
                    name: text("template").unwrap_or_default().to_owned(),
                    version: text("template_version").unwrap_or_default().to_owned(),
                }),
                ..person
            },
            Some("import") => Self {
                kind: MemoryActorKind::Import,
                ..person
            },
            Some("api") => Self {
                kind: MemoryActorKind::ApiKey,
                ..person
            },
            _ if r.author_api_key_id.is_some() => Self {
                kind: MemoryActorKind::ApiKey,
                ..person
            },
            _ if r.author_user_id.is_some() => person,
            _ => Self::of_kind(MemoryActorKind::Unknown),
        }
    }
}

/// `via` of a revision, from its provenance.
pub fn via_of(provenance: &Value) -> MemoryVia {
    match provenance.get("source").and_then(Value::as_str) {
        Some("dashboard") => MemoryVia::Dashboard,
        Some("tool") => MemoryVia::Conversation,
        Some("harvest") => MemoryVia::Harvest,
        Some("template") => MemoryVia::Template,
        Some("restore") => MemoryVia::Restore,
        Some("import") => MemoryVia::Import,
        _ => MemoryVia::Api,
    }
}

/// Everything needed to name the actors of one response, looked up in one go.
pub struct Names {
    agent: (Uuid, String),
    people: HashMap<Uuid, String>,
    keys: HashMap<Uuid, String>,
    pub conversations: HashMap<Uuid, ConversationRef>,
}

/// Looks up the names for `actors` (of `agent`'s memories) and for the extra `conversations`.
pub async fn names<'a>(
    s: &AppState,
    c: &crate::http::auth::Caller,
    agent: &AgentRow,
    actors: impl IntoIterator<Item = &'a StoredActor>,
    conversations: &[Uuid],
) -> Result<Names, ApiError> {
    let (mut people, mut keys, mut convs) = (Vec::new(), Vec::new(), conversations.to_vec());
    for a in actors {
        people.extend(a.user_id);
        keys.extend(a.api_key_id);
        convs.extend(a.conversation_id);
    }
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let people = repo::person_names(&mut conn, &people).await?;
    let keys = repo::api_key_names(&mut conn, &keys).await?;
    drop(conn);
    Ok(Names {
        agent: (agent.id, agent.name.clone()),
        people: people.into_iter().collect(),
        keys: keys.into_iter().collect(),
        conversations: crate::domains::conversations::reference::conversation_refs(s, c, &convs)
            .await?,
    })
}

impl Names {
    pub fn conversation(&self, id: Option<Uuid>) -> Option<ConversationRef> {
        id.and_then(|id| self.conversations.get(&id).cloned())
    }

    pub fn present(&self, a: &StoredActor) -> MemoryActor {
        let person = a.user_id.and_then(|id| {
            self.people.get(&id).map(|name| ActorPerson {
                id,
                display_name: name.clone(),
            })
        });
        MemoryActor {
            kind: a.kind,
            person,
            api_key: a.api_key_id.and_then(|id| {
                self.keys.get(&id).map(|name| ActorApiKey {
                    id,
                    name: name.clone(),
                })
            }),
            agent: (a.kind == MemoryActorKind::Agent).then(|| ActorAgent {
                id: self.agent.0,
                name: self.agent.1.clone(),
            }),
            conversation: self.conversation(a.conversation_id),
            template: a.template.as_ref().map(|t| ActorTemplate {
                name: t.name.clone(),
                version: t.version.clone(),
            }),
        }
    }
}

/// The one-line name of an actor, for the deprecated `author_label`.
pub fn label_of(a: &MemoryActor) -> String {
    a.agent
        .as_ref()
        .map(|a| a.name.clone())
        .or_else(|| a.person.as_ref().map(|p| p.display_name.clone()))
        .unwrap_or_else(|| "Unknown".to_owned())
}
