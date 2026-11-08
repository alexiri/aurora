//! Trust store and trusted-key administration (09 §2, SEC-15).

use super::registry;
use crate::app::AppState;
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::repo::plugins::{self, TrustedKeyRow};
use aurora_db::{DbError, PgPool};
use aurora_registry::signature::{PROJECT_PUBLISHER, TrustStore, TrustedKey};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use uuid::Uuid;

/// Base64 ed25519 public key of the Aurora project (publisher `aurora`).
///
/// Set by the owner on 2026-11-08. The private half is the CI secret
/// `AURORA_PROJECT_SIGNING_KEY` (environment `release`), used by the
/// built-in package signing step (P2-CAT-0). Only packages of publisher
/// `aurora` signed with this key (key id `builtin` or the key itself) are
/// trusted as project packages; rotating it means a new release.
pub const AURORA_PROJECT_PUBLIC_KEY: &str = "H+zP/3e/AvDIupz+XgxH31uErU7l4ySFFV9lv9qiOpk=";

pub const REVOKED_REASON: &str = "signing_key_revoked";

/// Project key plus every stored trusted key (revoked ones flagged).
pub async fn trust_store(pool: &PgPool) -> Result<TrustStore, ApiError> {
    let rows = plugins::all_trusted_keys(pool).await?;
    let keys = rows
        .into_iter()
        .filter_map(|r| {
            Some(TrustedKey {
                key: aurora_crypto::parse_public_key_b64(&r.public_key).ok()?,
                publisher: r.publisher,
                revoked: r.revoked_at.is_some(),
            })
        })
        .collect();
    let project_key = (!AURORA_PROJECT_PUBLIC_KEY.is_empty())
        .then(|| aurora_crypto::parse_public_key_b64(AURORA_PROJECT_PUBLIC_KEY).ok())
        .flatten();
    Ok(TrustStore { project_key, keys })
}

pub async fn create(
    state: &AppState,
    actor: Uuid,
    publisher: &str,
    name: &str,
    public_key: &str,
) -> Result<TrustedKeyRow, ApiError> {
    let publisher = publisher.trim();
    let name = name.trim();
    let mut errs = Vec::new();
    if !aurora_registry::manifest::valid_id(publisher) {
        errs.push(FieldError::new(
            "/publisher",
            "must match [a-z][a-z0-9_-]{1,47}",
        ));
    } else if publisher == PROJECT_PUBLISHER {
        errs.push(FieldError::new(
            "/publisher",
            "publisher `aurora` is reserved for the project key",
        ));
    }
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        errs.push(FieldError::new("/name", "must be 1-80 characters"));
    }
    let canonical = aurora_crypto::parse_public_key_b64(public_key.trim())
        .map(|k| STANDARD.encode(k.to_bytes()));
    if canonical.is_err() {
        errs.push(FieldError::new(
            "/public_key",
            "must be a base64 ed25519 public key",
        ));
    }
    if let (true, Ok(canonical)) = (errs.is_empty(), &canonical) {
        return plugins::insert_trusted_key(
            &state.pool,
            ids::new_id(),
            publisher,
            name,
            canonical,
            actor,
        )
        .await
        .map_err(|e| aurora_db::conflict_code(e, &[("trusted_keys_public_key_key", "conflict")]));
    }
    Err(ApiError::invalid_request("Invalid trusted key.", errs))
}

/// Revoke `id` and disable every package it signed. Returns the key and the
/// ids of the disabled packages. Idempotent.
pub async fn revoke(state: &AppState, id: Uuid) -> Result<(TrustedKeyRow, Vec<Uuid>), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let key = plugins::revoke_trusted_key(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Trusted key not found."))?;
    let disabled = plugins::disable_signed_by(&mut tx, &key.public_key, REVOKED_REASON).await?;
    tx.commit().await.map_err(DbError::from)?;
    if disabled.iter().any(|(_, was_active)| *was_active) {
        registry::request_compile(state).await?;
    }
    Ok((key, disabled.into_iter().map(|(i, _)| i).collect()))
}
