//! The Web Push routes' logic (C-830, C-831, C-838). Session only; own rows only (another
//! person's id is 404). Audit rows are best-effort and carry the endpoint's host, never the
//! endpoint, keys or label contents beyond the label the person typed.
use super::dto::*;
use super::{EndpointRefusal, check_endpoint, push_unavailable, vapid};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Principal, SessionCaller};
use aurora_core::limits::PUSH_SUBSCRIPTIONS_PER_USER;
use aurora_core::secret::Secret;
use aurora_core::{ApiError, FieldError, sanitize};
use aurora_db::repo::push as repo;
use aurora_secrets::{Purpose, SealContext, SecretRef};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn not_found() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "push_subscription_not_found",
        "Push subscription not found.",
    )
}

fn session_of(c: &SessionCaller) -> Result<Uuid, ApiError> {
    match c.principal {
        Principal::Session { id } => Ok(id),
        _ => Err(ApiError::forbidden("Session required.")),
    }
}

fn ctx(id: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::WebpushSubscription,
        owner_id: id,
        org_id: Uuid::nil(),
        workspace_id: None,
    }
}

/// The plaintext sealed per subscription.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SealedSub {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

pub async fn open_sub(state: &AppState, row: &repo::SubRow) -> Result<SealedSub, ApiError> {
    let r: SecretRef = row
        .secret_ref
        .parse()
        .map_err(|_| ApiError::internal("Subscription material unreadable."))?;
    let v = state
        .vault
        .open(&ctx(row.id), &r)
        .await
        .map_err(|_| ApiError::internal("Subscription material unreadable."))?;
    serde_json::from_slice(v.expose())
        .map_err(|_| ApiError::internal("Subscription material unreadable."))
}

async fn seal_sub(state: &AppState, id: Uuid, s: &SealedSub) -> Result<String, ApiError> {
    let bytes = serde_json::to_vec(s).map_err(|_| ApiError::internal("Sealing failed."))?;
    let r = state
        .vault
        .seal_inline(&ctx(id), Secret::new(bytes))
        .await
        .map_err(|_| {
            ApiError::unavailable("push_unavailable", "Subscription could not be stored.")
        })?;
    Ok(r.to_column())
}

fn endpoint_hash(endpoint: &str) -> Vec<u8> {
    Sha256::digest(endpoint.as_bytes()).to_vec()
}

fn require_on(state: &AppState) -> Result<(), ApiError> {
    if state.push.enabled {
        Ok(())
    } else {
        Err(push_unavailable())
    }
}

pub async fn config(state: &AppState) -> Result<PushConfig, ApiError> {
    require_on(state)?;
    let key = vapid::active(state).await.ok();
    Ok(PushConfig {
        available: key.is_some(),
        vapid_public_key: key.map(|(_, k)| k.public_b64u()),
        allowed_hosts: state.config.push.allowed_endpoint_hosts.clone(),
    })
}

pub async fn list(state: &AppState, c: &SessionCaller) -> Result<PushSubscriptionList, ApiError> {
    require_on(state)?;
    let current = session_of(c)?;
    let rows = repo::list_for_user(&state.pool, c.user_id).await?;
    Ok(PushSubscriptionList {
        subscriptions: rows
            .iter()
            .map(|r| PushSubscription::from_row(r, Some(current)))
            .collect(),
    })
}

fn clean_label(label: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(l) = label else { return Ok(None) };
    let l = sanitize::one_line(l, 80);
    if l.chars().count() > 80 {
        return Err(ApiError::invalid_request(
            "Invalid label.",
            vec![FieldError::new("/label", "must be at most 80 characters")],
        ));
    }
    Ok(Some(l))
}

fn refusal(r: EndpointRefusal) -> ApiError {
    ApiError::unprocessable(
        "push_endpoint_not_allowed",
        "This push endpoint is not allowed on this instance.",
    )
    .with_details(json!({ "reason": r.reason() }))
}

fn keys_invalid() -> ApiError {
    ApiError::unprocessable(
        "push_keys_invalid",
        "The subscription keys are not valid (p256dh must be a P-256 point, auth 16 bytes).",
    )
}

fn audit_best_effort(
    state: &AppState,
    c: &SessionCaller,
    action: &str,
    id: Uuid,
    label: &str,
    host: &str,
) {
    audit::record(
        state,
        AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
            .target("push_subscription", id)
            .metadata(json!({ "subscription_id": id, "label": label, "endpoint_host": host })),
    );
}

/// `POST /v1/me/push-subscriptions`. Returns the row and whether it was created (201) or an
/// existing registration of the caller re-bound (200).
pub async fn create(
    state: &AppState,
    c: &SessionCaller,
    req: CreatePushSubscription,
) -> Result<(PushSubscription, bool), ApiError> {
    require_on(state)?;
    let session = session_of(c)?;
    let rebind = req.rebind == Some(true);
    let label = clean_label(req.label.as_deref())?;
    let (url, host) = check_endpoint(&state.config.push.allowed_endpoint_hosts, &req.endpoint)
        .map_err(refusal)?;
    let keys = super::crypto::parse_subscription_keys(&req.keys.p256dh, &req.keys.auth)
        .map_err(|_| keys_invalid())?;
    let _ = keys;
    // The browser reports the key it subscribed with; only the key in force is accepted (D-266(d)).
    let (key_id, key) = vapid::active(state).await?;
    let reported = super::crypto::b64_decode(&req.vapid_public_key)
        .map(|b| super::crypto::b64u(&b))
        .unwrap_or_default();
    if reported != key.public_b64u() {
        return Err(ApiError::conflict(
            "push_key_stale",
            "The push key changed. Fetch /v1/push/config and subscribe again.",
        ));
    }
    let endpoint = url.as_str().to_owned();
    let hash = endpoint_hash(&endpoint);
    let expires_at: Option<DateTime<Utc>> = req
        .expiration_time
        .and_then(DateTime::<Utc>::from_timestamp_millis);

    let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
    let existing = repo::find_by_hash(&mut tx, &hash).await?;
    let sealed = SealedSub {
        endpoint: endpoint.clone(),
        p256dh: req.keys.p256dh.clone(),
        auth: req.keys.auth.clone(),
    };

    let (id, created, moved) = match existing {
        Some(row) if row.user_id == c.user_id => {
            let secret_ref = seal_sub(state, row.id, &sealed).await?;
            repo::rebind(
                &mut tx,
                row.id,
                &repo::Binding {
                    session,
                    vapid_key_id: key_id,
                    secret_ref: &secret_ref,
                    expires_at,
                },
                label.as_deref(),
                req.show_details,
            )
            .await?;
            (row.id, false, false)
        }
        // A silent re-bind never inherits another person's device (D-266(c)).
        Some(_) if rebind => return Err(not_found()),
        None if rebind => return Err(not_found()),
        Some(row) => {
            let secret_ref = seal_sub(state, row.id, &sealed).await?;
            if repo::count_for_user(&mut tx, c.user_id).await?
                >= i64::from(PUSH_SUBSCRIPTIONS_PER_USER)
            {
                return Err(limit_reached());
            }
            repo::move_to(
                &mut tx,
                row.id,
                c.user_id,
                &repo::Binding {
                    session,
                    vapid_key_id: key_id,
                    secret_ref: &secret_ref,
                    expires_at,
                },
                label.as_deref().unwrap_or(&row.label),
                req.show_details.unwrap_or(false),
            )
            .await?;
            (row.id, true, true)
        }
        None => {
            // One critical section per person for the cap: the user row.
            sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
                .bind(c.user_id)
                .execute(&mut *tx)
                .await
                .map_err(aurora_db::DbError::from)?;
            if repo::count_for_user(&mut tx, c.user_id).await?
                >= i64::from(PUSH_SUBSCRIPTIONS_PER_USER)
            {
                return Err(limit_reached());
            }
            let id = Uuid::now_v7();
            let secret_ref = seal_sub(state, id, &sealed).await?;
            repo::insert(
                &mut tx,
                &repo::NewSub {
                    id,
                    user_id: c.user_id,
                    session_id: session,
                    vapid_key_id: key_id,
                    endpoint_hash: &hash,
                    endpoint_host: &host,
                    secret_ref: &secret_ref,
                    label: label.as_deref().unwrap_or(""),
                    show_details: req.show_details.unwrap_or(false),
                    expires_at,
                },
            )
            .await?;
            (id, true, false)
        }
    };
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    let row = repo::get_owned(&state.pool, id, c.user_id)
        .await?
        .ok_or_else(not_found)?;
    let action = if moved {
        "push_subscription.move"
    } else if created {
        "push_subscription.create"
    } else {
        "push_subscription.update"
    };
    audit_best_effort(state, c, action, id, &row.label, &row.endpoint_host);
    Ok((PushSubscription::from_row(&row, Some(session)), created))
}

fn limit_reached() -> ApiError {
    ApiError::conflict(
        "push_subscription_limit_reached",
        format!(
            "You can register at most {PUSH_SUBSCRIPTIONS_PER_USER} devices. Remove one first."
        ),
    )
}

pub async fn patch(
    state: &AppState,
    c: &SessionCaller,
    id: Uuid,
    req: UpdatePushSubscription,
) -> Result<PushSubscription, ApiError> {
    require_on(state)?;
    let session = session_of(c)?;
    let label = clean_label(req.label.as_deref())?;
    if !repo::update(
        &state.pool,
        id,
        c.user_id,
        label.as_deref(),
        req.show_details,
        req.paused,
    )
    .await?
    {
        return Err(not_found());
    }
    let row = repo::get_owned(&state.pool, id, c.user_id)
        .await?
        .ok_or_else(not_found)?;
    audit_best_effort(
        state,
        c,
        "push_subscription.update",
        id,
        &row.label,
        &row.endpoint_host,
    );
    Ok(PushSubscription::from_row(&row, Some(session)))
}

/// `POST /{id}/test` (202): one test push to the caller's own device, `PUSH_TEST_PER_HOUR` per
/// person per hour (per replica: the window lives in the process). The outcome shows in the row.
pub async fn test(state: &AppState, c: &SessionCaller, id: Uuid) -> Result<(), ApiError> {
    require_on(state)?;
    session_of(c)?;
    repo::get_owned(&state.pool, id, c.user_id)
        .await?
        .ok_or_else(not_found)?;
    state.push.take_test_slot(c.user_id).map_err(|wait| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "push_test_rate_limited",
            "Too many test notifications. Try again later.",
        )
        .with_retry_after(wait)
    })?;
    super::deliver::spawn_test(state.clone(), id);
    Ok(())
}

/// `DELETE`: idempotent (204 whether or not the row existed).
pub async fn delete(state: &AppState, c: &SessionCaller, id: Uuid) -> Result<(), ApiError> {
    require_on(state)?;
    if let Some((host, label)) = repo::delete_owned(&state.pool, id, c.user_id).await? {
        audit_best_effort(state, c, "push_subscription.delete", id, &label, &host);
    }
    Ok(())
}
