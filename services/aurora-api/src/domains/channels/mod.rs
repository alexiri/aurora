//! Channel management API (API-006, docs/13 §5.3, §5.7–§5.9): integrations, presences, space
//! bindings, user links, DM defaults and reach. Connector plumbing (ops, config feed, ownership)
//! lives in [`crate::domains::connectors`].
//!
//! Decisions (BE-074):
//! * the config feed ETag is derived from the rows it serves, so row changes invalidate it
//!   without an extra counter;
//! * agent-side presence changes bump the agent's `home_sync_generation`;
//! * deprovision ops are enqueued with the presence id and handle only; rows of a deleted
//!   integration/org take their ops' `integration_id` FK with them, so those ops are enqueued
//!   without an integration (claimable by any live replica of the connector, best effort);
//! * plaintext credentials sent in a `credentials.validate` op payload are blanked once the op
//!   finished.
pub mod actions;
mod bindings;
pub mod card_requests;
pub mod cards;
pub mod commands;
mod dto;
pub mod inbox;
mod integrations;
mod links;
pub mod mirror;
pub mod presences;
mod reach;
mod routes;

use aurora_core::{ApiError, FieldError, sanitize};
use serde_json::Value;

use crate::domains::connectors::ops::CorrelatedError;

pub use dto::*;
pub use routes::router;

const MESSAGE_MAX: usize = 200;

/// C-38: how a management route reports a correlated op that did not succeed.
pub(crate) fn map_correlated(e: CorrelatedError) -> ApiError {
    match e {
        CorrelatedError::Unavailable => ApiError::unavailable(
            "connector_unavailable",
            "The connector did not answer in time.",
        ),
        CorrelatedError::Failed { code, message } => {
            let message = sanitize::one_line(&message, MESSAGE_MAX);
            let message = if message.is_empty() {
                "The connector rejected the request.".to_owned()
            } else {
                message
            };
            match code.as_str() {
                "invalid" | "forbidden" | "credential_dead" => {
                    ApiError::unprocessable("credentials_invalid", message)
                }
                _ => ApiError::bad_gateway("connector_error", message),
            }
        }
    }
}

/// A JSON object (or `null`, read as `{}`) of at most `max` serialized bytes.
pub(crate) fn object_field(
    v: &Value,
    path: &str,
    max: usize,
    allow_empty: bool,
) -> Result<Value, ApiError> {
    let bad =
        |m: &str| ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, m)]);
    let v = if v.is_null() {
        Value::Object(Default::default())
    } else {
        v.clone()
    };
    let Some(o) = v.as_object() else {
        return Err(bad("must be an object"));
    };
    if o.is_empty() && !allow_empty {
        return Err(bad("must not be empty"));
    }
    if serde_json::to_vec(&v).map_or(usize::MAX, |b| b.len()) > max {
        return Err(bad("too large"));
    }
    Ok(v)
}

/// Whether the connector offers `name` for this integration: the manifest enables it and the
/// integration's `_capabilities` (reported by `credentials.validate`) do not switch it off.
pub(crate) fn capability_enabled(manifest: Option<&Value>, config: &Value, name: &str) -> bool {
    let on = |v: Option<&Value>| match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Object(_)) => true,
        _ => false,
    };
    on(manifest.and_then(|m| m.pointer(&format!("/capabilities/{name}"))))
        && !matches!(
            config.pointer(&format!("/_capabilities/{name}")),
            Some(Value::Bool(false))
        )
}
