//! `POST /v1/connector/events` (docs/13 §4, §6 invariant 3): durable, deduped inbox insert.
//!
//! Each event is validated and checked on its own: its integration must exist, belong to this
//! connector and be owned by the calling replica. Rejections carry a neutral code: `invalid`
//! (bad event), `not_found` (unknown integration, or one of another connector: not
//! distinguished), `forbidden` (owned by another replica). The response is sent after commit.

use super::auth::ConnectorCtx;
use super::ops::{invalid, notify, require_active};
use crate::app::AppState;
use crate::http::json::Json;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::bus::Channel;
use aurora_db::repo::channel_inbox as inbox;
use aurora_protocol::common::Validate;
use aurora_protocol::connector::{
    ErrorCode, EventsRequest, EventsResponse, MAX_EVENTS_PER_BATCH, RejectedEvent,
};
use axum::extract::State;
use axum::routing::post;
use serde_json::json;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().route("/connector/events", post(events))
}

async fn events(
    c: ConnectorCtx,
    State(s): State<AppState>,
    Json(req): Json<EventsRequest>,
) -> Result<Json<EventsResponse>, ApiError> {
    require_active(&c)?;
    if req.events.len() > MAX_EVENTS_PER_BATCH {
        return Err(invalid(format!(
            "at most {MAX_EVENTS_PER_BATCH} events per request"
        )));
    }
    let ids: Vec<Uuid> = req.events.iter().map(|e| e.integration_id).collect();
    let owners: HashMap<Uuid, (String, Option<Uuid>)> = inbox::integration_owners(&s.pool, &ids)
        .await?
        .into_iter()
        .map(|(id, conn, owner)| (id, (conn, owner)))
        .collect();

    let mut out = EventsResponse::default();
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    for e in req.events {
        let reject = |code| RejectedEvent {
            event_id: e.event_id.clone(),
            code,
        };
        if e.validate().is_err() {
            out.rejected.push(reject(ErrorCode::Invalid));
            continue;
        }
        match owners.get(&e.integration_id) {
            Some((conn, _)) if *conn != c.connector.id => {
                out.rejected.push(reject(ErrorCode::NotFound));
                continue;
            }
            None => {
                out.rejected.push(reject(ErrorCode::NotFound));
                continue;
            }
            Some((_, owner)) if *owner != Some(c.replica) => {
                out.rejected.push(reject(ErrorCode::Forbidden));
                continue;
            }
            Some(_) => {}
        }
        // C-326: `api:` is reserved for `Idempotency-Key` values stored in `turns.source_event_id`.
        if e.event_id.starts_with("api:") {
            out.rejected.push(reject(
                ErrorCode::from_wire("reserved_event_id").unwrap_or(ErrorCode::Invalid),
            ));
            continue;
        }
        let fresh = inbox::insert(
            &mut *tx,
            &c.connector.id,
            e.integration_id,
            &e.event_id,
            e.kind.as_str(),
            &e.payload,
        )
        .await?;
        if fresh {
            out.accepted.push(e.event_id);
        } else {
            out.duplicates.push(e.event_id);
        }
    }
    if !out.accepted.is_empty() {
        notify(&mut tx, Channel::Inbox, &json!({})).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(Json(out))
}
