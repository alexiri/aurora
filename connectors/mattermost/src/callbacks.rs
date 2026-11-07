//! Public callbacks (docs/20 §Public callbacks): `POST /mm/actions` (interactive
//! messages), `POST /mm/commands` (slash command), plus `GET /mm/ready` and,
//! in the test profile only, `POST /mm/test/redeliver`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aurora_connector_sdk::EventAck;
use aurora_protocol::connector::{ActionInvoked, CommandInvoked, EventKind};
use axum::extract::{DefaultBodyLimit, Form, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::connector::Shared;
use crate::events::space;
use crate::integ::Integ;
use crate::render::verify;

/// Callbacks answer within Mattermost's patience even when the core is slow.
const CORE_ACK_WAIT: Duration = Duration::from_secs(5);
const BODY_LIMIT: usize = 64 * 1024;

pub fn router(shared: Arc<Shared>) -> Router {
    let mut r = Router::new()
        .route("/mm/actions", post(actions))
        .route("/mm/commands", post(commands))
        .route("/mm/ready", get(ready));
    if shared.settings.test_control {
        tracing::warn!("test control mounted at /mm/test/*");
        r = r
            .route("/mm/test/redeliver", post(redeliver))
            .route("/mm/test/lose-results", post(lose_results));
    }
    r.layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(shared)
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ActionRequest {
    user_id: String,
    channel_id: String,
    post_id: String,
    trigger_id: String,
    selected_option: String,
    context: HashMap<String, Value>,
}

fn forbidden(msg: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response()
}

fn ctx_str<'a>(m: &'a HashMap<String, Value>, k: &str) -> &'a str {
    m.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Mattermost does not sign callbacks: the connector's HMAC over the action
/// token is the only proof the button came from a card this connector rendered.
async fn actions(State(shared): State<Arc<Shared>>, Json(req): Json<ActionRequest>) -> Response {
    let token = ctx_str(&req.context, "token");
    let sig = ctx_str(&req.context, "sig");
    if token.is_empty() || !verify(&shared.settings.hmac_key, token, sig) {
        tracing::warn!("interactive callback with a bad signature rejected");
        return forbidden("bad signature");
    }
    let integ = match Uuid::parse_str(ctx_str(&req.context, "iid"))
        .ok()
        .and_then(|id| shared.integration(id))
    {
        Some(i) => i,
        None => return forbidden("unknown integration"),
    };
    if req.user_id.is_empty() {
        return forbidden("no clicker");
    }
    // Mattermost reports the chosen option inside the action context (older and
    // newer servers also at the top level).
    let selected = if req.selected_option.is_empty() {
        ctx_str(&req.context, "selected_option").to_owned()
    } else {
        req.selected_option.clone()
    };
    let payload = ActionInvoked {
        token: token.to_owned(),
        clicker_ref: req.user_id.clone(),
        value: (!selected.is_empty()).then_some(selected),
        space_ref: (!req.channel_id.is_empty()).then(|| req.channel_id.clone()),
        message_ref: (!req.post_id.is_empty()).then(|| req.post_id.clone()),
    };
    let id = format!(
        "action:{}:{}:{}",
        req.post_id,
        req.user_id,
        if req.trigger_id.is_empty() {
            Uuid::now_v7().to_string()
        } else {
            req.trigger_id.clone()
        }
    );
    let ack = tokio::time::timeout(
        CORE_ACK_WAIT,
        integ.emit(id, EventKind::ActionInvoked, &payload),
    )
    .await;
    match ack {
        Ok(Some(a)) if !matches!(a, EventAck::Rejected(_)) => Json(json!({})).into_response(),
        Ok(_) => (StatusCode::BAD_GATEWAY, Json(json!({}))).into_response(),
        // The event is queued and retried by the SDK; the click is not lost.
        Err(_) => Json(json!({})).into_response(),
    }
}

fn find_by_team(shared: &Shared, team_id: &str) -> Option<Arc<Integ>> {
    shared.all().into_iter().find(|i| i.team_id == team_id)
}

fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

async fn commands(
    State(shared): State<Arc<Shared>>,
    Form(f): Form<HashMap<String, String>>,
) -> Response {
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
    let Some(integ) = find_by_team(&shared, get("team_id")) else {
        return forbidden("unknown team");
    };
    let expected = integ.snapshot().command_token;
    match expected {
        Some(t) if !get("token").is_empty() && ct_eq(&t, get("token")) => {}
        _ => return forbidden("bad token"),
    }
    let text = get("text").trim();
    let (name, args) = match text.split_once(char::is_whitespace) {
        Some((n, a)) => (n.to_ascii_lowercase(), a.trim().to_owned()),
        None if text.is_empty() => ("help".to_owned(), String::new()),
        None => (text.to_ascii_lowercase(), String::new()),
    };
    let channel = match integ.channel(get("channel_id")).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e.message, "slash command in an unknown channel");
            return forbidden("unknown channel");
        }
    };
    let payload = CommandInvoked {
        name,
        args,
        space: space(&channel),
        thread_ref: None,
        user_ref: get("user_id").to_owned(),
        response_ref: None,
    };
    let id = format!("command:{}", get("trigger_id"));
    let id = if id == "command:" {
        format!("command:{}", Uuid::now_v7())
    } else {
        id
    };
    let _ = tokio::time::timeout(
        CORE_ACK_WAIT,
        integ.emit(id, EventKind::CommandInvoked, &payload),
    )
    .await;
    Json(json!({"response_type": "ephemeral", "text": "Working…"})).into_response()
}

async fn ready(State(shared): State<Arc<Shared>>) -> Response {
    let integrations = shared.all();
    let n = integrations.iter().filter(|i| i.is_ready()).count();
    let code = if n > 0 {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(json!({"ready": n}))).into_response()
}

/// Test profile: re-runs catch-up now (a redelivery of recent platform events).
async fn redeliver(State(shared): State<Arc<Shared>>) -> Response {
    for i in shared.all() {
        i.catch_up().await;
    }
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct LoseQuery {
    n: u32,
}

/// Test profile: drop the results of the next `n` `message.post` ops.
async fn lose_results(
    State(shared): State<Arc<Shared>>,
    axum::extract::Query(q): axum::extract::Query<LoseQuery>,
) -> Response {
    shared
        .lose_results
        .store(q.n, std::sync::atomic::Ordering::SeqCst);
    Json(json!({"ok": true})).into_response()
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert!(!ct_eq("", "x"));
    }
}
