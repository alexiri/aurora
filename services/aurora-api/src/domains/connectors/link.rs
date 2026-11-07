//! `POST /v1/connector/link-start` (docs/13 §4): a 30-minute single-use link request for a
//! platform user of an integration this replica owns. The `aui_` token is returned only inside
//! the URL; only its hash is stored.

use super::auth::ConnectorCtx;
use super::ops::{invalid, require_active};
use crate::app::AppState;
use crate::http::json::Json;
use aurora_core::ApiError;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::repo::channel_integrations as integrations;
use aurora_db::repo::link_requests;
use aurora_protocol::connector::{LinkStartRequest, LinkStartResponse};
use axum::extract::State;
use axum::routing::post;
use utoipa_axum::router::OpenApiRouter;

const LINK_TTL_SECS: f64 = 30.0 * 60.0;
const USER_REF_MAX_CHARS: usize = 512;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().route("/connector/link-start", post(link_start))
}

async fn link_start(
    c: ConnectorCtx,
    State(s): State<AppState>,
    Json(req): Json<LinkStartRequest>,
) -> Result<Json<LinkStartResponse>, ApiError> {
    require_active(&c)?;
    if req.platform_user_ref.is_empty()
        || req.platform_user_ref.chars().count() > USER_REF_MAX_CHARS
    {
        return Err(invalid("platform_user_ref must be 1-512 characters"));
    }
    integrations::get_owned(&s.pool, &c.connector.id, c.replica, req.integration_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Integration not found."))?;
    let tok = token::generate(TokenPrefix::Invitation);
    let (_, expires) = link_requests::insert(
        &s.pool,
        req.integration_id,
        &req.platform_user_ref,
        &token::hash(tok.expose()),
        LINK_TTL_SECS,
        None,
    )
    .await?;
    Ok(Json(LinkStartResponse {
        url: format!(
            "{}/link/channel?token={}",
            s.config.external_url.trim_end_matches('/'),
            tok.expose()
        ),
        expires_at: expires.to_rfc3339(),
    }))
}
