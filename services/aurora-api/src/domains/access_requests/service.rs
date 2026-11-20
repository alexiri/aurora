use super::dto::{AccessRequest, STATUSES};
use crate::app::AppState;
use crate::domains::apps::dto::Tool;
use crate::domains::apps::service as apps;
use crate::domains::conversations::service::{self as conv, NewMessage};
use crate::domains::grants::levels::{self, Choice, Level};
use crate::http::auth::Caller;
use aurora_core::{ApiError, limits, sanitize};
use aurora_db::DbError;
use aurora_db::repo::access_requests::{self as repo, RequestRow};
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::{agents, conversations as convs, turns};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

/// Rows a workspace's list returns at most.
const LIST_MAX: i64 = 50;

/// The level a written grant stands for, for the "access was given" notice: `None` for an app that
/// cannot change anything (no choice), the nearest level for a custom grant.
#[must_use]
pub fn level_of(full: bool, allow: &[String], ask: &[String], tools: &[Tool]) -> Option<Level> {
    if !tools.iter().any(|t| t.group == "write") {
        return None;
    }
    Some(match levels::classify(full, allow, ask, tools).choice {
        Choice::Level(l) => l,
        Choice::Custom { base, .. } => base,
    })
}

/// The Done sentence of `14` §3.
#[must_use]
pub fn done_sentence(agent: &str, app: &str, level: Option<Level>) -> String {
    match level {
        Some(Level::Read) => format!("{agent} can now read {app}."),
        Some(Level::Ask) => format!("{agent} can now read {app}, and asks you before each change."),
        Some(Level::Act) => format!("{agent} can now read and change {app} without asking."),
        None => format!("{agent} can now use {app}."),
    }
}

/// (P5-REFINE-6, C-1067, D-419) The Done sentence for a send-only app (the Outgoing webhook),
/// named by its account: there is nothing to read.
#[must_use]
pub fn send_sentence(agent: &str, account: &str, level: Option<Level>) -> String {
    match level {
        Some(Level::Act) => format!("{agent} can now send to {account} without asking."),
        Some(Level::Ask | Level::Read) => {
            format!("{agent} can now send to {account}, and asks you first.")
        }
        None => format!("{agent} can now use {account}."),
    }
}

/// The fixed text of the system turn that restarts the agent (SEC-132): platform words only,
/// nothing the agent wrote.
#[must_use]
pub fn restart_text(app: &str, level: Option<Level>) -> String {
    let given = match level {
        Some(Level::Read) => " (Read only)",
        Some(Level::Ask) => " (Ask before changes)",
        Some(Level::Act) => " (Change without asking)",
        None => "",
    };
    format!("Access to {app} was given{given}. Continue with what you were doing.")
}

/// [`restart_text`] for a send-only app (C-1067).
#[must_use]
pub fn send_restart_text(account: &str, level: Option<Level>) -> String {
    let given = match level {
        Some(Level::Act) => " (Send without asking)",
        Some(Level::Ask | Level::Read) => " (Ask before each send)",
        None => "",
    };
    format!("Access to {account} was given{given}. Continue with what you were doing.")
}

/// What a grant gave, for the notice: the account's name is used for a send-only app (C-1067).
pub struct Given<'a> {
    pub app_id: &'a str,
    pub account: &'a str,
    pub send_only: bool,
    pub level: Option<Level>,
}

async fn app_name(state: &AppState, app_id: &str) -> String {
    match apps::snapshot(state).await {
        Ok(Some(s)) => apps::find_app(&s, app_id)
            .map(|(a, _)| sanitize::label(&a.about.name))
            .unwrap_or_else(|_| sanitize::label(app_id)),
        _ => sanitize::label(app_id),
    }
}

/// Called by the writer of connection grants inside its transaction, after a grant of `agent` to
/// an account of `app_id` was written: resolves the agent's open request for that app, tells the
/// conversation, and, when the asking turn has ended, queues one system turn (SEC-132). Does
/// nothing when the agent has no open request for the app.
pub async fn on_access_given(
    tx: &mut PgConnection,
    state: &AppState,
    agent: &AgentRow,
    given: &Given<'_>,
) -> Result<(), ApiError> {
    let Some(req) = repo::mark_given_for_app(&mut *tx, agent.id, given.app_id).await? else {
        return Ok(());
    };
    let level = given.level;
    let app = app_name(state, given.app_id).await;
    let who = sanitize::label(&agent.name);
    let (sentence, restart) = if given.send_only {
        let account = sanitize::label(given.account);
        (
            send_sentence(&who, &account, level),
            send_restart_text(&account, level),
        )
    } else {
        (done_sentence(&who, &app, level), restart_text(&app, level))
    };
    report_given(tx, &req, &app, &sentence, &restart, level).await
}

/// Called when a person accepts an agent's proposed website secret (and so grants it to the
/// agent), inside that transaction: resolves the agent's open request for it the same way.
pub async fn on_secret_given(
    tx: &mut PgConnection,
    agent_id: Uuid,
    secret_id: Uuid,
    host: &str,
) -> Result<(), ApiError> {
    let Some(req) = repo::mark_given_for_secret(&mut *tx, agent_id, secret_id).await? else {
        return Ok(());
    };
    let name = agents::get(&mut *tx, agent_id)
        .await?
        .map_or_else(|| "The agent".to_owned(), |a| sanitize::label(&a.name));
    let host = sanitize::label(host);
    report_given(
        tx,
        &req,
        &host,
        &done_sentence(&name, &host, None),
        &restart_text(&host, None),
        None,
    )
    .await
}

/// Writes the `app_access_given` notice and queues the restart turn. Fixed platform text only.
async fn report_given(
    tx: &mut PgConnection,
    req: &RequestRow,
    app: &str,
    sentence: &str,
    restart: &str,
    level: Option<Level>,
) -> Result<(), ApiError> {
    let cid = req.conversation_id;
    if !convs::lock(&mut *tx, cid).await? {
        return Ok(());
    }
    conv::append_event(
        &mut *tx,
        cid,
        None,
        "notice",
        &json!({"code": "app_access_given", "text": sentence, "request_id": req.id,
                "app_id": req.app_id, "app_name": app, "level": level.map(Level::as_str)}),
    )
    .await?;
    let ended = match req.turn_id {
        None => true,
        Some(t) => turns::get(&mut *tx, t)
            .await?
            .is_none_or(|t| matches!(t.status.as_str(), "done" | "failed" | "aborted")),
    };
    if !ended {
        return Ok(());
    }
    let Some(c) = convs::get(&mut *tx, cid).await? else {
        return Ok(());
    };
    conv::create_turn_tx(
        &mut *tx,
        &c,
        &NewMessage {
            door: json!({"kind": "system"}),
            speaker_kind: "system",
            speaker_user_id: None,
            speaker_label: "Access",
            text: restart,
            source_event_id: None,
            attachment_ids: &[],
            reply_to_seq: None,
        },
    )
    .await?;
    Ok(())
}

async fn present(state: &AppState, r: RequestRow) -> Result<AccessRequest, ApiError> {
    let agent = agents::get(&state.pool, r.agent_id).await?;
    let (name, handle) = agent.map_or_else(
        || (String::new(), String::new()),
        |a| (sanitize::label(&a.name), a.handle),
    );
    let app = match &r.app_id {
        Some(id) => Some(app_name(state, id).await),
        None => None,
    };
    let host = match r.secret_id {
        Some(id) => repo::secret_host(&state.pool, id)
            .await?
            .map(|h| sanitize::label(&h)),
        None => None,
    };
    Ok(AccessRequest::new(r, name, handle, app, host))
}

/// A workspace's requests (`open` by default), newest first.
pub async fn list(
    state: &AppState,
    workspace_id: Uuid,
    status: Option<&str>,
) -> Result<Vec<AccessRequest>, ApiError> {
    let status = status.unwrap_or("open");
    if !STATUSES.contains(&status) {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![aurora_core::FieldError::new(
                "/status",
                "must be open, given, dismissed or expired",
            )],
        ));
    }
    let rows = repo::list(&state.pool, workspace_id, status, LIST_MAX).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(present(state, r).await?);
    }
    Ok(out)
}

/// "Not now": folds the card away. `can_own` of the request's workspace.
pub async fn dismiss(state: &AppState, c: &Caller, id: Uuid) -> Result<AccessRequest, ApiError> {
    let Some(existing) = repo::get(&state.pool, id).await? else {
        return Err(ApiError::not_found("Request not found."));
    };
    let auth = c.workspace(state, existing.workspace_id).await?;
    crate::http::auth::require_own(&auth)?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let Some(row) = repo::dismiss(&mut tx, id, Some(c.user_id)).await? else {
        return Err(ApiError::conflict(
            "already_resolved",
            "This request has already been resolved.",
        ));
    };
    if convs::lock(&mut tx, row.conversation_id).await? {
        conv::append_event(
            &mut tx,
            row.conversation_id,
            None,
            "notice",
            &json!({"code": "app_access_request_dismissed", "text": "Dismissed.",
                    "request_id": row.id}),
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    present(state, row).await
}

/// Marks open requests older than `ACCESS_REQUEST_TTL` `expired`.
pub async fn sweep(pool: &aurora_db::PgPool) -> Result<u64, ApiError> {
    Ok(repo::expire_overdue(pool, limits::ACCESS_REQUEST_TTL.as_secs_f64()).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_send_only_app_is_named_by_its_account_and_never_read() {
        assert_eq!(
            send_sentence("Dani", "Ops alerts", Some(Level::Ask)),
            "Dani can now send to Ops alerts, and asks you first."
        );
        assert_eq!(
            send_sentence("Dani", "Ops alerts", Some(Level::Act)),
            "Dani can now send to Ops alerts without asking."
        );
        assert_eq!(
            send_restart_text("Ops alerts", Some(Level::Ask)),
            "Access to Ops alerts was given (Ask before each send). Continue with what you were doing."
        );
        // Other apps are unchanged.
        assert_eq!(
            done_sentence("Dani", "Acme", Some(Level::Read)),
            "Dani can now read Acme."
        );
    }
}
