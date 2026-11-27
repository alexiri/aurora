//! (P5-MEMORY-1, C-1172, SEC-157) What the agent remembered, in the transcript: after a
//! `memory_save` saved or was refused, the API appends `memory.saved` / `memory.refused` to the
//! turn's conversation. Written by the API, so every harness gets it. They are ordinary durable
//! transcript events: readable by whoever may read the conversation, replayed, exported and
//! redacted like the others (a redaction keeps only the ids).

use crate::domains::conversations::service as conv;
use crate::domains::tools::ToolCallContext;
use aurora_core::{ApiError, sanitize};
use aurora_db::DbError;
use aurora_db::repo::memory::MemoryRow;
use serde_json::{Value, json};

const PREVIEW_CHARS: usize = 200;
const TITLE_CHARS: usize = 120;
const KEY_CHARS: usize = 64;

/// The refusal codes of `memory.refused` (an open vocabulary for readers, closed here).
pub fn refusal_code(e: &ApiError) -> Option<&'static str> {
    if !e.status().is_client_error() {
        return None;
    }
    Some(match e.code() {
        "memory_locked" => "memory_locked",
        "memory_limit_reached" => "memory_limit_reached",
        "invalid_request" => "invalid",
        _ => "not_allowed",
    })
}

fn preview(content: &str) -> String {
    sanitize::one_line(content, PREVIEW_CHARS)
}

/// A title as one clean line, or null when there is none.
fn title_of(t: Option<&str>) -> Option<String> {
    t.map(|t| sanitize::one_line(t, TITLE_CHARS))
        .filter(|t| !t.is_empty())
}

/// `memory.saved`: `title` is the memory's title, else its key.
pub async fn saved(ctx: &ToolCallContext, m: &MemoryRow, created: bool) {
    let payload = json!({
        "turn_id": ctx.turn_id,
        "key": m.key,
        "title": title_of(m.title.as_deref()).unwrap_or_else(|| m.key.clone()),
        "preview": preview(&m.content),
        "revision": m.revision,
        "created": created,
        "previous_revision": (!created).then(|| m.revision - 1),
    });
    append(ctx, "memory.saved", payload).await;
}

/// What the agent tried to save and was refused.
pub struct Attempt<'a> {
    pub key: &'a str,
    /// The title of the memory it would have changed, else the one it sent.
    pub title: Option<&'a str>,
    pub content: &'a str,
}

/// `memory.refused`, with a plain-words reason. `agent` is the agent's name.
pub async fn refused(
    ctx: &ToolCallContext,
    agent: &str,
    a: &Attempt<'_>,
    code: &str,
    detail: Option<&str>,
) {
    let key = sanitize::one_line(a.key, KEY_CHARS);
    let title = title_of(a.title);
    let name = title.clone().unwrap_or_else(|| key.clone());
    let agent = sanitize::one_line(agent, TITLE_CHARS);
    let message = match code {
        "memory_locked" => format!("“{name}” is locked, so {agent} can't change it."),
        "memory_limit_reached" => format!(
            "{agent} already has as many memories as it can hold, so it couldn't add “{name}”."
        ),
        "invalid" => match detail {
            Some(d) => format!("“{name}” couldn't be saved: {}.", d.trim_end_matches('.')),
            None => format!("“{name}” couldn't be saved because it isn't a valid memory."),
        },
        _ => format!("{agent} wasn't allowed to save “{name}”."),
    };
    let payload = json!({
        "turn_id": ctx.turn_id,
        "key": key,
        "title": title,
        "preview": preview(a.content),
        "code": code,
        "message": sanitize::one_line(&message, 400),
    });
    append(ctx, "memory.refused", payload).await;
}

/// The first field problem of a validation error, as `field: problem`.
pub fn invalid_detail(e: &ApiError) -> Option<String> {
    let f = e.details()?.get("fields")?.get(0)?;
    let path = f.get("path")?.as_str()?.trim_start_matches('/');
    let message = f.get("message")?.as_str()?;
    Some(format!("{path}: {message}"))
}

/// Best effort: the memory is already saved (or refused), so a transcript that cannot be written
/// is logged and never changes the tool's answer.
async fn append(ctx: &ToolCallContext, ty: &str, payload: Value) {
    let written: Result<(), ApiError> = async {
        let mut tx = ctx.agent.pool.begin().await.map_err(DbError::from)?;
        conv::append_event(&mut tx, ctx.conversation_id, ctx.turn_id, ty, &payload).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
    .await;
    if let Err(e) = written {
        tracing::warn!(error = %e.message(), ty, "appending a memory transcript event failed");
    }
}
