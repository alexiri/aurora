//! The VAPID key service (C-833): first-start generation under the advisory lock, sealing in the
//! **inline** store (D-266(b)), loading, rotation, status and the daily destruction of retired
//! keys. An operator-managed key (`push.vapid.private_key_env`) is never stored.
use super::crypto::VapidKey;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::ApiError;
use aurora_core::limits::PUSH_VAPID_RETIRE;
use aurora_core::secret::Secret;
use aurora_db::repo::push as repo;
use aurora_db::repo::push::VapidKeyRow;
use aurora_secrets::{Purpose, SealContext, SecretRef};
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn ctx(id: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::WebpushVapid,
        owner_id: id,
        org_id: Uuid::nil(),
        workspace_id: None,
    }
}

fn unavailable() -> ApiError {
    ApiError::unavailable("push_unavailable", "The Web Push key is not available.")
}

/// Generate a key and seal its private half in the inline store; returns the sealed reference.
async fn generate_sealed(state: &AppState) -> Result<(Uuid, VapidKey, String), ApiError> {
    let key = VapidKey::generate().map_err(|_| unavailable())?;
    let der = key.to_pkcs8_der().map_err(|_| unavailable())?;
    let id = Uuid::now_v7();
    let r = state
        .vault
        .seal_inline(&ctx(id), Secret::new(der))
        .await
        .map_err(|_| unavailable())?;
    Ok((id, key, r.to_column()))
}

async fn load(state: &AppState, row: &VapidKeyRow) -> Result<Arc<VapidKey>, ApiError> {
    if let Some(k) = state.push.loaded_key(row.id) {
        return Ok(k);
    }
    let r: SecretRef = row.private_ref.parse().map_err(|_| unavailable())?;
    let der = state
        .vault
        .open(&ctx(row.id), &r)
        .await
        .map_err(|_| unavailable())?;
    let key = Arc::new(VapidKey::from_pkcs8_der(der.expose()).map_err(|_| unavailable())?);
    state.push.remember_key(row.id, key.clone());
    Ok(key)
}

/// The key new subscriptions are bound to: the operator key (no row, id `None`) or the generated
/// active key, generating it when no replica has yet (the first one wins under the advisory lock).
pub async fn active(state: &AppState) -> Result<(Option<Uuid>, Arc<VapidKey>), ApiError> {
    if let Some(k) = state.push.operator_key() {
        return Ok((None, k));
    }
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    if let Some(row) = repo::active_key(&mut conn).await? {
        return Ok((Some(row.id), load(state, &row).await?));
    }
    drop(conn);
    let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
    repo::lock_vapid(&mut tx).await?;
    if let Some(row) = repo::active_key(&mut tx).await? {
        tx.commit().await.map_err(aurora_db::DbError::from)?;
        return Ok((Some(row.id), load(state, &row).await?));
    }
    let (id, key, private_ref) = generate_sealed(state).await?;
    repo::insert_active_key(&mut tx, id, &key.public_b64u(), &private_ref).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    let key = Arc::new(key);
    state.push.remember_key(id, key.clone());
    Ok((Some(id), key))
}

/// The key a stored subscription was registered with: its generated key, or the operator key when
/// the row names none and the operator key is in force. `None` = not servable (stale).
pub async fn key_for(
    state: &AppState,
    key_id: Option<Uuid>,
) -> Result<Option<Arc<VapidKey>>, ApiError> {
    match key_id {
        None => Ok(state.push.operator_key()),
        Some(id) => match repo::key_by_id(&state.pool, id).await? {
            Some(row) => Ok(Some(load(state, &row).await?)),
            None => Ok(None),
        },
    }
}

#[derive(Debug, Serialize)]
pub struct KeyStatus {
    pub id: Uuid,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub retired_at: Option<chrono::DateTime<chrono::Utc>>,
    pub subscriptions: i64,
}

/// `aurora-api push status` (C-833): never key material.
#[derive(Debug, Serialize)]
pub struct Status {
    pub mode: &'static str,
    pub keys: Vec<KeyStatus>,
    pub subscriptions_by_state: std::collections::BTreeMap<String, i64>,
    pub subscriptions_by_host: std::collections::BTreeMap<String, i64>,
}

pub async fn status(state: &AppState) -> Result<Status, ApiError> {
    let keys = repo::keys_with_counts(&state.pool)
        .await?
        .into_iter()
        .map(|(k, n)| KeyStatus {
            id: k.id,
            status: k.status,
            created_at: k.created_at,
            retired_at: k.retired_at,
            subscriptions: n,
        })
        .collect();
    Ok(Status {
        mode: state.push.mode(),
        keys,
        subscriptions_by_state: repo::counts_by_state(&state.pool)
            .await?
            .into_iter()
            .collect(),
        subscriptions_by_host: repo::counts_by_host(&state.pool)
            .await?
            .into_iter()
            .collect(),
    })
}

pub enum Rotation {
    /// `aurora-api push rotate-vapid` with an operator key: nothing to rotate here (exit 3).
    OperatorManaged,
    /// Push is off: no key exists to rotate.
    Disabled,
    Rotated {
        old: Option<Uuid>,
        new: Uuid,
    },
    DryRun {
        old: Option<Uuid>,
    },
}

/// Rotate the generated key (C-833): a new `active` key, the old one `retired` (still served to
/// the subscriptions that reference it until they migrate or `PUSH_VAPID_RETIRE` passes). Critical
/// audit `push.vapid_rotate` in the same transaction.
pub async fn rotate(state: &AppState, dry_run: bool) -> Result<Rotation, ApiError> {
    if !state.push.enabled {
        return Ok(Rotation::Disabled);
    }
    if state.push.operator_key().is_some() {
        return Ok(Rotation::OperatorManaged);
    }
    let mut tx = audit::begin(state).await?;
    repo::lock_vapid(&mut tx).await?;
    let old = repo::active_key(&mut tx).await?.map(|k| k.id);
    if dry_run {
        return Ok(Rotation::DryRun { old });
    }
    let (id, key, private_ref) = generate_sealed(state).await?;
    repo::retire_active_key(&mut tx).await?;
    repo::insert_active_key(&mut tx, id, &key.public_b64u(), &private_ref).await?;
    let ev = AuditEvent::new(
        AuditActor::system("aurora-api push rotate-vapid").with_source("cli"),
        "push.vapid_rotate",
        Outcome::Success,
    )
    .target("instance", "push")
    .metadata(json!({ "old_key_id": old, "new_key_id": id }));
    audit::commit_critical(tx, &ev).await?;
    state.push.remember_key(id, Arc::new(key));
    Ok(Rotation::Rotated { old, new: id })
}

/// The daily job (C-833): destroy retired keys that nothing references or that were retired for
/// longer than `PUSH_VAPID_RETIRE` (their subscriptions go with them, and the sealed secret is
/// discarded with the row). Returns the keys destroyed.
pub async fn destroy_expired(state: &AppState) -> Result<u64, ApiError> {
    let rows = repo::destroyable_keys(&state.pool, PUSH_VAPID_RETIRE.as_secs() as i64).await?;
    let mut n = 0;
    for k in rows {
        let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
        repo::destroy_key(&mut tx, k.id).await?;
        tx.commit().await.map_err(aurora_db::DbError::from)?;
        state.push.forget_key(k.id);
        n += 1;
    }
    Ok(n)
}
