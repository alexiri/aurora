//! Connector plane `/v1/connector/*` (docs/13 §4): register, config feed, credential write-back,
//! retire. Machine plane: authenticated with the connector anchor token `aun_…` (+
//! `X-Aurora-Replica`), deliberately **not** part of the dashboard OpenAPI document (routes are
//! added with `OpenApiRouter::route`, which carries no schema).
//!
//! Decisions:
//! * the config feed serves only integrations in status `active` (a `validating` row is checked
//!   by a correlated `credentials.validate` op that carries its own credentials, so no socket is
//!   needed for it) and only `active` presences;
//! * a connector that is not `active` (e.g. `pending_approval`) gets an empty feed;
//! * a replica is live when seen within 2x the advertised config poll interval.

use super::auth::{ConnectorCtx, ConnectorToken};
use super::manifest;
use super::{integration_seal_ctx, presence_seal_ctx};
use crate::app::AppState;
use crate::http::json::Json;
use aurora_core::{ApiError, Secret};
use aurora_db::DbError;
use aurora_db::repo::channel_integrations as integrations;
use aurora_db::repo::connectors::{self, ConnectorRow, Registration};
use aurora_protocol::common::Validate;
use aurora_protocol::connector::{
    AgentRef, Binding, ConfigResponse, ConnectorStatus as WireStatus, CredentialsRequest,
    CredentialsResponse, DEFAULT_CONFIG_SECS, Integration, ListenMode, OkResponse, PollConfig,
    Presence, PresenceMode, RegisterRequest, RegisterResponse, SharedBot,
};
use aurora_registry::signature::verify_digest;
use aurora_secrets::{SecretRef, ValueSource};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use sha2::{Digest, Sha256};
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

/// Config poll interval advertised by `register`.
const CONFIG_SECS: u32 = DEFAULT_CONFIG_SECS;
/// Ownership lease granted by each config poll (three poll intervals).
const LEASE_SECS: u32 = CONFIG_SECS * 3;
/// A replica is live when seen within 2x the config poll interval.
const LIVE_SECS: f64 = (CONFIG_SECS * 2) as f64;
/// Cap on a credential document written back by a connector.
const CREDENTIALS_MAX_BYTES: usize = 64 * 1024;

/// Mounted under `/v1`; plain routes, so excluded from the dashboard OpenAPI document.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .route("/connector/register", post(register))
        .route("/connector/config", get(config))
        .route("/connector/credentials", post(credentials))
        .route("/connector/retire", post(retire))
}

async fn register(
    t: ConnectorToken,
    State(s): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, ApiError> {
    Ok(Json(do_register(&s, &t.0, req).await?))
}

async fn config(
    c: ConnectorCtx,
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let inm = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    do_config(&s, &c, inm).await
}

async fn credentials(
    c: ConnectorCtx,
    State(s): State<AppState>,
    Json(req): Json<CredentialsRequest>,
) -> Result<Json<CredentialsResponse>, ApiError> {
    Ok(Json(do_credentials(&s, &c, req).await?))
}

async fn retire(c: ConnectorCtx, State(s): State<AppState>) -> Result<Json<OkResponse>, ApiError> {
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    connectors::retire_replica(&mut *tx, &c.connector.id, c.replica).await?;
    integrations::release_replica(&mut *tx, &c.connector.id, c.replica).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Json(OkResponse { ok: true }))
}

// -------------------------------------------------------------------- register

fn invalid(e: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_request(e.to_string(), vec![])
}

async fn do_register(
    s: &AppState,
    conn: &ConnectorRow,
    req: RegisterRequest,
) -> Result<RegisterResponse, ApiError> {
    req.validate().map_err(invalid)?;
    let m = &req.manifest;
    if m.package.id != conn.id {
        return Err(ApiError::unprocessable(
            "connector_id_mismatch",
            "The manifest package id does not match this connector.",
        ));
    }
    let errs = manifest::validate(m);
    if !errs.is_empty() {
        return Err(ApiError::invalid_request("Invalid manifest.", errs));
    }

    // Trust: the detached signature covers sha256 of the canonical manifest JSON.
    let digest = manifest::digest(m).map_err(|e| ApiError::internal(e.to_string()))?;
    let trust = crate::domains::plugins::keys::trust_store(&s.pool).await?;
    verify_digest(
        req.signature.as_bytes(),
        &m.package.publisher,
        &digest,
        &trust,
    )
    .map_err(|v| ApiError::unprocessable(v.code, v.message))?;
    let digest_hex = manifest::hex(&digest);

    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let row = connectors::get_for_update(&mut *tx, &conn.id)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("Unknown connector token."))?;
    if row.status == "disabled" {
        return Err(ApiError::forbidden("This connector is disabled."));
    }
    let baseline = row.manifest.as_ref().and_then(manifest::baseline);
    let mut widening = manifest::widening(baseline.as_ref(), m);
    let accepted = widening.is_empty() || s.config.plugins.allow_unsigned;
    if accepted && !widening.is_empty() {
        // `plugins.allow_unsigned` (dev only, docs/15): auto-approve instead of holding.
        tracing::warn!(
            connector = %conn.id,
            reasons = ?widening,
            "plugins.allow_unsigned: connector manifest auto-approved without review"
        );
        widening.clear();
    }
    let (status, wire, approved_digest, keep_baseline) = if accepted {
        // Unchanged or narrowed relative to the approved manifest: accept automatically.
        (
            "active",
            WireStatus::Active,
            Some(digest_hex.as_str()),
            Some(m),
        )
    } else {
        (
            "pending_approval",
            WireStatus::PendingApproval,
            row.approved_manifest_digest.as_deref(),
            baseline.as_ref(),
        )
    };
    let stored = manifest::to_stored(m, keep_baseline, &widening)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    connectors::set_registration(
        &mut *tx,
        &conn.id,
        &Registration {
            version: &req.version,
            contract: i32::try_from(m.package.contract).unwrap_or(1),
            manifest: &stored,
            manifest_digest: &digest_hex,
            status,
            approved_manifest_digest: approved_digest,
        },
    )
    .await?;
    let replica =
        connectors::insert_replica(&mut *tx, Uuid::now_v7(), &conn.id, &req.version).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(RegisterResponse {
        connector_id: conn.id.clone(),
        replica_id: replica.id,
        status: wire,
        poll: PollConfig {
            config_secs: CONFIG_SECS,
            ..PollConfig::default()
        },
    })
}

// ---------------------------------------------------------------------- config

fn norm_etag(v: &str) -> &str {
    v.trim().trim_start_matches("W/").trim_matches('"')
}

fn etag_of(parts: &serde_json::Value) -> String {
    let bytes = aurora_registry::snapshot::canonical_bytes(parts);
    manifest::hex(&Sha256::digest(bytes))
}

fn respond(etag: &str, body: Option<ConfigResponse>) -> Response {
    let mut r = match body {
        Some(b) => Json(b).into_response(),
        None => StatusCode::NOT_MODIFIED.into_response(),
    };
    if let Ok(v) = HeaderValue::from_str(&format!("\"{etag}\"")) {
        r.headers_mut().insert(header::ETAG, v);
    }
    r
}

async fn open_json(
    s: &AppState,
    ctx: &aurora_secrets::SealContext,
    sealed: &str,
) -> Option<serde_json::Value> {
    let r: SecretRef = sealed.parse().ok()?;
    let plain: Secret<Vec<u8>> = s.vault.open(ctx, &r).await.ok()?;
    serde_json::from_slice(plain.expose()).ok()
}

async fn do_config(
    s: &AppState,
    c: &ConnectorCtx,
    if_none_match: Option<&str>,
) -> Result<Response, ApiError> {
    // `touch_replica` (auth) already bumped `last_seen_at`.
    let serving = {
        let row = connectors::get(&s.pool, &c.connector.id).await?;
        row.is_some_and(|r| r.status == "active")
    };
    let (owned, presences, bindings) = if serving {
        let mut tx = s.pool.begin().await.map_err(DbError::from)?;
        let owned = integrations::claim_owned(
            &mut tx,
            &c.connector.id,
            c.replica,
            f64::from(LEASE_SECS),
            LIVE_SECS,
        )
        .await?;
        let ids: Vec<Uuid> = owned.iter().map(|i| i.id).collect();
        let presences = integrations::active_presences(&mut *tx, &ids).await?;
        let bindings = integrations::active_bindings(&mut *tx, &ids).await?;
        tx.commit().await.map_err(DbError::from)?;
        (owned, presences, bindings)
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };

    // The ETag covers every served field except the plaintext credentials: sealed references
    // change whenever a credential is rewritten, so no decryption is needed for a 304.
    let etag = etag_of(&serde_json::json!({
        "i": owned.iter().map(|i| serde_json::json!([
            i.id, i.org_id, i.tenant_ref, i.tenant_label, i.config, i.credentials_ref,
            i.credentials_version, i.shared_bot, i.status])).collect::<Vec<_>>(),
        "p": presences.iter().map(|p| serde_json::json!([
            p.id, p.integration_id, p.mode, p.platform_user_ref, p.handle, p.credentials_ref,
            p.credentials_version, p.status, p.agent_id, p.agent_handle, p.agent_name,
            p.agent_avatar_public_key]))
            .collect::<Vec<_>>(),
        "b": bindings.iter().map(|b| serde_json::json!([
            b.id, b.integration_id, b.space_ref, b.agent_id, b.listen, b.followups_need_mention]))
            .collect::<Vec<_>>(),
    }));
    if if_none_match.is_some_and(|v| norm_etag(v) == etag) {
        return Ok(respond(&etag, None));
    }

    let mut out = ConfigResponse {
        etag: etag.clone(),
        lease_secs: LEASE_SECS,
        integrations: Vec::new(),
        presences: Vec::new(),
        space_bindings: Vec::new(),
    };
    let mut served = std::collections::HashSet::new();
    for i in &owned {
        let ctx = integration_seal_ctx(i.id, i.org_id);
        let Some(creds) = open_json(s, &ctx, &i.credentials_ref).await else {
            // Never log the reference or any value: the id is enough to investigate.
            tracing::error!(integration_id = %i.id, "integration credentials unreadable; not served");
            continue;
        };
        served.insert(i.id);
        out.integrations.push(Integration {
            id: i.id,
            org_id: i.org_id,
            tenant_ref: i.tenant_ref.clone(),
            tenant_label: i.tenant_label.clone(),
            config: i.config.clone(),
            credentials: creds,
            credentials_version: u64::try_from(i.credentials_version).unwrap_or(0),
            shared_bot: i
                .shared_bot
                .clone()
                .and_then(|v| serde_json::from_value::<SharedBot>(v).ok()),
            status: i.status.clone(),
        });
    }
    for p in presences
        .iter()
        .filter(|p| served.contains(&p.integration_id))
    {
        let Ok(mode) = PresenceMode::from_wire(&p.mode) else {
            continue;
        };
        let (credentials, version) = match &p.credentials_ref {
            Some(r) => {
                let ctx = presence_seal_ctx(p.id, p.org_id);
                let Some(v) = open_json(s, &ctx, r).await else {
                    tracing::error!(presence_id = %p.id, "presence credentials unreadable; not served");
                    continue;
                };
                (
                    Some(v),
                    Some(u64::try_from(p.credentials_version).unwrap_or(0)),
                )
            }
            None => (None, None),
        };
        out.presences.push(Presence {
            id: p.id,
            integration_id: p.integration_id,
            mode,
            agent: AgentRef {
                id: p.agent_id,
                handle: p.agent_handle.clone(),
                name: p.agent_name.clone(),
                avatar_url: p
                    .agent_avatar_public_key
                    .as_deref()
                    .map(|k| crate::domains::agents::avatar::public_url(&s.config.external_url, k)),
            },
            platform_user_ref: p.platform_user_ref.clone(),
            handle: p.handle.clone(),
            credentials,
            credentials_version: version,
            status: p.status.clone(),
        });
    }
    for b in bindings
        .iter()
        .filter(|b| served.contains(&b.integration_id))
    {
        let Ok(listen) = ListenMode::from_wire(&b.listen) else {
            continue;
        };
        out.space_bindings.push(Binding {
            integration_id: b.integration_id,
            space_ref: b.space_ref.clone(),
            agent_id: b.agent_id,
            listen,
            followups_need_mention: b.followups_need_mention,
        });
    }
    Ok(respond(&etag, Some(out)))
}

// ----------------------------------------------------------------- credentials

fn stale() -> ApiError {
    ApiError::conflict(
        "stale_version",
        "The credential version changed; re-read the config.",
    )
}

async fn do_credentials(
    s: &AppState,
    c: &ConnectorCtx,
    req: CredentialsRequest,
) -> Result<CredentialsResponse, ApiError> {
    req.validate().map_err(invalid)?;
    let plain = serde_json::to_vec(&req.credentials).map_err(invalid)?;
    if plain.len() > CREDENTIALS_MAX_BYTES {
        return Err(ApiError::too_large("Credentials document too large."));
    }
    let expected = i64::try_from(req.expected_version).map_err(|_| stale())?;
    let not_owned = || ApiError::not_found("Target not found.");
    let discard = |ctx: aurora_secrets::SealContext, r: SecretRef| async move {
        if let Err(e) = s.vault.discard(&ctx, &r, ValueSource::Managed).await {
            tracing::warn!(error = %e, "discarding replaced credentials failed");
        }
    };

    if let Some(id) = req.target.integration_id {
        let row = integrations::get_owned(&s.pool, &c.connector.id, c.replica, id)
            .await?
            .ok_or_else(not_owned)?;
        if row.credentials_version != expected {
            return Err(stale());
        }
        let ctx = integration_seal_ctx(row.id, row.org_id);
        let new_ref = s
            .vault
            .seal(&ctx, Secret::from(plain))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let swapped = integrations::cas_credentials(
            &s.pool,
            &c.connector.id,
            c.replica,
            id,
            expected,
            &new_ref.to_column(),
        )
        .await?;
        return match swapped {
            Some((version, old)) => {
                if let Ok(old) = old.parse::<SecretRef>() {
                    discard(ctx, old).await;
                }
                Ok(CredentialsResponse {
                    version: u64::try_from(version).unwrap_or(0),
                })
            }
            None => {
                discard(ctx, new_ref).await;
                Err(stale())
            }
        };
    }
    let id = req.target.presence_id.ok_or_else(not_owned)?;
    let row = integrations::get_owned_presence(&s.pool, &c.connector.id, c.replica, id)
        .await?
        .ok_or_else(not_owned)?;
    if row.credentials_version != expected {
        return Err(stale());
    }
    let ctx = presence_seal_ctx(row.id, row.org_id);
    let new_ref = s
        .vault
        .seal(&ctx, Secret::from(plain))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let swapped = integrations::cas_presence_credentials(
        &s.pool,
        &c.connector.id,
        c.replica,
        id,
        expected,
        &new_ref.to_column(),
    )
    .await?;
    match swapped {
        Some((version, old)) => {
            if let Some(old) = old.and_then(|o| o.parse::<SecretRef>().ok()) {
                discard(ctx, old).await;
            }
            Ok(CredentialsResponse {
                version: u64::try_from(version).unwrap_or(0),
            })
        }
        None => {
            discard(ctx, new_ref).await;
            Err(stale())
        }
    }
}
