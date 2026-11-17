//! Audit rows of the user-token feature (C-858, D-284(b)). A row names the token id, the agent,
//! the conversation and the outcome; it never carries the message text, the token, or a platform
//! user id by value.

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_db::repo::users;
use serde_json::{Value, json};
use uuid::Uuid;

/// The owner as an audit actor, for rows the platform writes on the owner's behalf.
pub async fn owner_actor(s: &AppState, user: Uuid) -> AuditActor {
    let label = match users::find_by_id(&s.pool, user).await {
        Ok(Some(u)) => u.email,
        _ => "user".to_owned(),
    };
    AuditActor {
        kind: "user",
        id: Some(user),
        label,
        source: "channel",
        ip: None,
    }
}

/// One `user_token.send` row for an attempted send (best effort, not critical). `meta` is the
/// op's internal `user_meta` (agent, conversation, approval).
pub fn record_send(
    s: &AppState,
    org: Uuid,
    token: Uuid,
    owner: Option<Uuid>,
    meta: &Value,
    ok: bool,
    code: Option<&str>,
) {
    let s = s.clone();
    let meta = meta.clone();
    let code = code.map(str::to_owned);
    let owner = owner.or_else(|| {
        meta.get("owner_id")
            .and_then(Value::as_str)
            .and_then(|o| o.parse().ok())
    });
    tokio::spawn(async move {
        let actor = match owner {
            Some(u) => owner_actor(&s, u).await,
            None => AuditActor::system("user-token"),
        };
        let ev = AuditEvent::new(
            actor,
            "user_token.send",
            if ok {
                Outcome::Success
            } else {
                Outcome::Failure
            },
        )
        .org(org)
        .target("user_token", token)
        .metadata(json!({
            "outcome": if ok { "sent" } else { "failed" },
            "error_code": code,
            "agent_id": meta.get("agent_id"),
            "conversation_id": meta.get("conversation_id"),
            "approval_id": meta.get("approval_id"),
        }));
        if let Err(e) = audit::insert(&s, &ev).await {
            tracing::error!(error = %e, "user token send audit write failed");
            metrics::counter!("aurora_audit_dropped_total").increment(1);
        }
    });
}
