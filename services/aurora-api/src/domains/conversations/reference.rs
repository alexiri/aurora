//! (P5-MEMORY-1, C-1169, D-497(c), SEC-157) Naming a conversation only to its readers.
//!
//! Anything that says "this happened in a conversation" (a memory's adder, last changer and last
//! use; skill usage) names it through [`conversation_refs`], and through nothing else: a
//! [`ConversationRef`] exists only for a conversation the caller may read, by the very check the
//! conversation routes use. Everyone else gets `null` and learns only that there was one.

use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::authz::ReadAuth;
use aurora_db::repo::conversations as convs;
use serde::Serialize;
use std::collections::HashMap;
use utoipa::ToSchema;
use uuid::Uuid;

/// A conversation the caller may read.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConversationRef {
    pub id: Uuid,
    /// Null for a conversation nobody has titled.
    pub title: Option<String>,
}

/// The references of the conversations among `ids` that `c` may read; every other id (not
/// readable, unknown, deleted) is absent from the map.
///
/// The check is [`service::load_conversation_read`]'s: [`Caller::workspace_read`] (key
/// confinement, own access, or a session's active emergency grant) and then
/// `can_see_conversation`. A title is conversation content, so naming a conversation to an
/// emergency reader records the view first, like the conversation list (C-1165): a record that
/// cannot be written fails the request instead of leaking the name.
pub async fn conversation_refs(
    s: &AppState,
    c: &Caller,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, ConversationRef>, ApiError> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let mut access: HashMap<Uuid, Option<ReadAuth>> = HashMap::new();
    for row in convs::get_many(&s.pool, &ids).await? {
        let ws = row.workspace_id;
        if !access.contains_key(&ws) {
            let read = match c.workspace_read(s, ws).await {
                Ok(r) => Some(r),
                // No access to the workspace (or a key confined elsewhere): no name.
                Err(e) if e.status().is_client_error() => None,
                Err(e) => return Err(e),
            };
            access.insert(ws, read);
        }
        let Some(Some(read)) = access.get(&ws) else {
            continue;
        };
        if !read.can_see_conversation(c.user_id, &row.kind, row.direct_user_id) {
            continue;
        }
        if let ReadAuth::Emergency(g) = read {
            service::record_emergency_view(s, c, &row, g).await?;
        }
        out.insert(
            row.id,
            ConversationRef {
                id: row.id,
                title: row.title,
            },
        );
    }
    Ok(out)
}
