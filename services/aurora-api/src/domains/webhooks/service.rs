//! Webhook CRUD shared by the routes and the agent tools, plus failure bookkeeping.
//! Authorization is the caller's job.

use super::dto::VerificationInput;
use crate::app::AppState;
use crate::domains::conversations::service as conv;
use crate::domains::secrets::service::secret_err;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, Secret};
use aurora_db::DbError;
use aurora_db::repo::conversations as convs;
use aurora_db::repo::turns::TurnRow;
use aurora_db::repo::webhooks::{self as repo, NewWebhook, WebhookRow};
use aurora_db::repo::{agents::AgentRow, webhooks};
use aurora_secrets::{Purpose, SealContext, SecretRef};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

const NAME_MAX: usize = 120;
const INSTRUCTIONS_MAX: usize = 20_000;
const SECRET_MIN: usize = 8;
const SECRET_MAX: usize = 512;
/// Webhooks per agent.
const PER_AGENT_MAX: i64 = 20;
/// Consecutive failed turns before auto-disable (docs/10).
const FAILURE_DISABLE: i32 = 5;

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_field("Invalid webhook.", path, msg)
}

pub fn not_found() -> ApiError {
    ApiError::not_found("Webhook not found.")
}

pub fn seal_ctx(agent: &AgentRow, webhook: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::WebhookVerification,
        owner_id: webhook,
        org_id: agent.org_id,
        workspace_id: Some(agent.workspace_id),
    }
}

pub struct Create {
    pub name: String,
    pub instructions: String,
    pub verification: Option<VerificationInput>,
    /// `creator` or `workspace` (RFE-021).
    pub anchor: &'static str,
}

#[derive(Default)]
pub struct Patch {
    pub name: Option<String>,
    pub instructions: Option<String>,
    pub verification: Option<VerificationInput>,
    pub enabled: Option<bool>,
}

fn check_name(v: &str) -> Result<(), ApiError> {
    aurora_core::names::clean(v, NAME_MAX)
        .map(|_| ())
        .map_err(|e| bad("/name", &e.to_string()))
}

fn check_instructions(v: &str) -> Result<(), ApiError> {
    if v.trim().is_empty() || v.chars().count() > INSTRUCTIONS_MAX {
        return Err(bad("/instructions", "1 to 20000 characters"));
    }
    Ok(())
}

/// Validated verification settings, ready to persist.
struct Checked {
    kind: &'static str,
    header: Option<String>,
    secret: Option<String>,
    /// (C-1078) The secret was made here and is returned once.
    generated: bool,
}

fn invalid_verification(path: &str, msg: &str) -> ApiError {
    ApiError::unprocessable("invalid_verification", "Invalid webhook verification.")
        .with_details(json!({"fields": [{"path": path, "message": msg}]}))
}

/// (P5-REFINE-7, C-1078) A secret Aurora makes: 32 random bytes from the system RNG as 64 hex
/// characters.
pub fn generate_secret() -> Result<String, ApiError> {
    let mut b = [0u8; 32];
    aws_lc_rs::rand::fill(&mut b)
        .map_err(|_| ApiError::internal("Random generator unavailable."))?;
    Ok(::hex::encode(b))
}

/// The verification a create without one gets (C-1078): signed, with a generated secret.
fn default_verification() -> VerificationInput {
    VerificationInput {
        kind: "hmac_sha256".into(),
        header: None,
        secret: None,
        generate_secret: true,
    }
}

fn check_verification(mut v: VerificationInput) -> Result<Checked, ApiError> {
    if v.generate_secret {
        if v.secret.is_some() {
            return Err(invalid_verification(
                "/verification/generate_secret",
                "give a secret or generate one, not both",
            ));
        }
        if v.kind == "none" {
            return Err(invalid_verification(
                "/verification/generate_secret",
                "kind none takes no secret",
            ));
        }
    }
    let generated = v.generate_secret && matches!(v.kind.as_str(), "hmac_sha256" | "bearer");
    if generated {
        v.secret = Some(generate_secret()?);
    }
    let mut ch = check_verification_fields(v)?;
    ch.generated = generated;
    Ok(ch)
}

fn check_verification_fields(v: VerificationInput) -> Result<Checked, ApiError> {
    let (kind, default_header) = match v.kind.as_str() {
        "none" => ("none", None),
        "hmac_sha256" => ("hmac_sha256", Some("X-Signature")),
        "bearer" => ("bearer", Some("Authorization")),
        _ => {
            return Err(bad(
                "/verification/kind",
                "one of none, hmac_sha256, bearer",
            ));
        }
    };
    if kind == "none" {
        if v.header.is_some() || v.secret.is_some() {
            return Err(bad("/verification", "kind none takes no header or secret"));
        }
        return Ok(Checked {
            kind,
            header: None,
            secret: None,
            generated: false,
        });
    }
    let header = match v.header {
        Some(h) => {
            if h.is_empty() || h.len() > 64 || http::HeaderName::from_bytes(h.as_bytes()).is_err() {
                return Err(bad("/verification/header", "not a valid header name"));
            }
            h
        }
        None => default_header.unwrap_or_default().to_owned(),
    };
    if let Some(s) = &v.secret
        && !(SECRET_MIN..=SECRET_MAX).contains(&s.len())
    {
        return Err(bad("/verification/secret", "8 to 512 bytes"));
    }
    Ok(Checked {
        kind,
        header: Some(header),
        secret: v.secret,
        generated: false,
    })
}

pub fn url_for(state: &AppState, token: &str) -> String {
    format!(
        "{}/v1/hooks/{token}",
        state.config.external_url.trim_end_matches('/')
    )
}

pub async fn list(state: &AppState, agent: Uuid) -> Result<Vec<WebhookRow>, ApiError> {
    Ok(repo::list_for_agent(&state.pool, agent).await?)
}

pub async fn get(state: &AppState, id: Uuid) -> Result<WebhookRow, ApiError> {
    repo::get(&state.pool, id).await?.ok_or_else(not_found)
}

async fn seal(
    state: &AppState,
    agent: &AgentRow,
    webhook: Uuid,
    secret: String,
) -> Result<SecretRef, ApiError> {
    state
        .vault
        .seal(&seal_ctx(agent, webhook), Secret::new(secret.into_bytes()))
        .await
        .map_err(secret_err)
}

async fn discard(state: &AppState, agent: &AgentRow, webhook: Uuid, r: &str) {
    if let Ok(sr) = r.parse::<SecretRef>()
        && let Err(e) = state
            .vault
            .discard(
                &seal_ctx(agent, webhook),
                &sr,
                aurora_secrets::ValueSource::Managed,
            )
            .await
    {
        tracing::warn!(error = %e, "webhook secret discard failed");
    }
}

fn stored_ref(v: &Option<Value>) -> Option<String> {
    v.as_ref()
        .and_then(|v| v.get("secret_ref"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// What [`create`] returns: the row, the one-time capability URL and (C-1078) the secret Aurora
/// generated on this call, if any (shown once).
pub struct Created {
    pub row: WebhookRow,
    pub url: String,
    pub secret: Option<String>,
}

/// Create a webhook; returns the row and the one-time capability URL. (P5-REFINE-7, C-1078) No
/// `verification` means signed (`hmac_sha256`) with a secret Aurora generates.
pub async fn create(
    state: &AppState,
    agent: &AgentRow,
    c: Create,
    creator: Option<Uuid>,
    origin: Option<Uuid>,
) -> Result<Created, ApiError> {
    check_name(&c.name)?;
    check_instructions(&c.instructions)?;
    let ch = check_verification(c.verification.unwrap_or_else(default_verification))?;
    let id = Uuid::now_v7();
    let mut shown: Option<String> = None;
    let verification = if ch.kind == "none" {
        None
    } else {
        let secret = ch
            .secret
            .ok_or_else(|| bad("/verification/secret", "required for this kind"))?;
        if ch.generated {
            shown = Some(secret.clone());
        }
        let r = seal(state, agent, id, secret).await?;
        Some(json!({"kind": ch.kind, "header": ch.header, "secret_ref": r.to_column()}))
    };
    let tok = token::generate(TokenPrefix::Webhook);
    let hash = token::hash(tok.expose());
    let prefix = token::display_prefix(tok.expose());
    let res = insert(
        state,
        agent,
        id,
        &c.name,
        &c.instructions,
        &hash,
        &prefix,
        &verification,
        creator,
        c.anchor,
        origin,
    )
    .await;
    match res {
        Ok(row) => {
            let url = url_for(state, tok.expose());
            Ok(Created {
                row,
                url,
                secret: shown,
            })
        }
        Err(e) => {
            if let Some(r) = stored_ref(&verification) {
                discard(state, agent, id, &r).await;
            }
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert(
    state: &AppState,
    agent: &AgentRow,
    id: Uuid,
    name: &str,
    instructions: &str,
    hash: &[u8],
    prefix: &str,
    verification: &Option<Value>,
    creator: Option<Uuid>,
    anchor: &str,
    origin: Option<Uuid>,
) -> Result<WebhookRow, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    // Serialize the per-agent cap.
    sqlx::query("SELECT 1 FROM agents WHERE id = $1 FOR UPDATE")
        .bind(agent.id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    if repo::count_for_agent(&mut *tx, agent.id).await? >= PER_AGENT_MAX {
        return Err(ApiError::conflict(
            "webhook_limit_reached",
            "This agent has reached its webhook limit.",
        ));
    }
    let row = repo::insert(
        &mut tx,
        &NewWebhook {
            id,
            agent_id: agent.id,
            name,
            instructions,
            token_hash: hash,
            display_prefix: prefix,
            verification: verification.as_ref(),
            creator_user_id: creator,
            anchor,
            origin_conversation_id: origin,
        },
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(row)
}

/// Apply an `enabled` change in memory.
pub fn apply_enabled(r: &mut WebhookRow, enabled: Option<bool>, actor: Option<Uuid>) {
    match enabled {
        Some(true) if !r.enabled => {
            if r.disabled_reason.as_deref() == Some("authorization")
                && r.anchor == "creator"
                && actor.is_some()
            {
                r.creator_user_id = actor;
            }
            r.enabled = true;
            r.disabled_reason = None;
            r.disabled_by = None;
            r.consecutive_failures = 0;
        }
        Some(false) if r.enabled => {
            r.enabled = false;
            r.disabled_reason = Some("user".into());
            r.disabled_by = actor;
        }
        _ => {}
    }
}

/// Apply a patch. `actor` is the user re-enabling (becomes the fire-time authority anchor when
/// re-enabling after an authorization disable).
pub async fn patch(
    state: &AppState,
    agent: &AgentRow,
    seen: WebhookRow,
    p: Patch,
    actor: Option<Uuid>,
) -> Result<WebhookRow, ApiError> {
    patch_with(state, agent, seen, p, actor, async {}).await
}

/// [`patch`] with a hook between the caller's read and the row lock (tests interleave there).
pub async fn patch_with(
    state: &AppState,
    agent: &AgentRow,
    seen: WebhookRow,
    p: Patch,
    actor: Option<Uuid>,
    between: impl std::future::Future<Output = ()>,
) -> Result<WebhookRow, ApiError> {
    between.await;
    // (FIX-110) Edit the locked current row: a stale `verification.secret_ref` must not undo a
    // rotation that happened after the caller read the webhook.
    if p.verification.as_ref().is_some_and(|v| v.generate_secret) {
        return Err(invalid_verification(
            "/verification/generate_secret",
            "use POST /v1/webhooks/{id}/rotate-secret",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    // (P5-REFINE-7, C-1074) The delivery threads follow the name; written before the webhook row
    // is locked (lock order: conversation, then webhook, as a finishing delivery turn).
    if let Some(v) = &p.name {
        check_name(v)?;
        convs::rename_automation_threads(&mut tx, "webhook", seen.id, v).await?;
    }
    let mut r = repo::get_for_update(&mut tx, seen.id)
        .await?
        .ok_or_else(not_found)?;
    if let Some(v) = p.name {
        check_name(&v)?;
        r.name = v;
    }
    if let Some(v) = p.instructions {
        check_instructions(&v)?;
        r.instructions = v;
    }
    let old_ref = stored_ref(&r.verification);
    let mut new_ref: Option<String> = None;
    if let Some(v) = p.verification {
        let ch = check_verification(v)?;
        if ch.kind == "none" {
            r.verification = None;
        } else {
            let cur_kind = r
                .verification
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(|k| k.as_str());
            let secret_ref = match (ch.secret, old_ref.clone()) {
                (Some(s), _) => {
                    let sr = seal(state, agent, r.id, s).await?.to_column();
                    new_ref = Some(sr.clone());
                    sr
                }
                (None, Some(old)) if cur_kind == Some(ch.kind) => old,
                (None, _) => return Err(bad("/verification/secret", "required for this kind")),
            };
            r.verification =
                Some(json!({"kind": ch.kind, "header": ch.header, "secret_ref": secret_ref}));
        }
    }
    apply_enabled(&mut r, p.enabled, actor);
    let res = repo::update(&mut tx, &r).await;
    let row = match res {
        Ok(row) => row,
        Err(e) => {
            if let Some(n) = &new_ref {
                discard(state, agent, r.id, n).await;
            }
            return Err(e.into());
        }
    };
    if let Err(e) = tx.commit().await {
        if let Some(n) = &new_ref {
            discard(state, agent, r.id, n).await;
        }
        return Err(DbError::from(e).into());
    }
    // The old sealed secret is garbage once replaced or cleared.
    if let Some(old) = old_ref
        && stored_ref(&row.verification).as_deref() != Some(old.as_str())
    {
        discard(state, agent, r.id, &old).await;
    }
    Ok(row)
}

/// Replace the capability token; returns the new URL (shown once).
pub async fn rotate(state: &AppState, id: Uuid) -> Result<String, ApiError> {
    let tok = token::generate(TokenPrefix::Webhook);
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    repo::lock(&mut tx, id).await?.ok_or_else(not_found)?;
    repo::set_token(
        &mut tx,
        id,
        &token::hash(tok.expose()),
        &token::display_prefix(tok.expose()),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(url_for(state, tok.expose()))
}

/// (P5-REFINE-7, C-1078) A new generated secret for a signed webhook; the old one stops working
/// at once (sealed like any secret, the old sealed value discarded). Returns the row and the
/// secret, shown once.
pub async fn rotate_secret(
    state: &AppState,
    agent: &AgentRow,
    id: Uuid,
) -> Result<(WebhookRow, String), ApiError> {
    let secret = generate_secret()?;
    let new_ref = seal(state, agent, id, secret.clone()).await?.to_column();
    let res = async {
        let mut tx = state.pool.begin().await.map_err(DbError::from)?;
        let mut r = repo::get_for_update(&mut tx, id)
            .await?
            .ok_or_else(not_found)?;
        let v = r.verification.clone().unwrap_or(Value::Null);
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("none");
        if !matches!(kind, "hmac_sha256" | "bearer") {
            return Err(ApiError::unprocessable(
                "verification_not_signed",
                "This webhook is not signed; choose a signing method first.",
            ));
        }
        let old = stored_ref(&r.verification);
        r.verification = Some(json!({"kind": kind, "header": v.get("header"),
            "secret_ref": new_ref}));
        let row = repo::update(&mut tx, &r).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok((row, old))
    }
    .await;
    match res {
        Ok((row, old)) => {
            if let Some(old) = old {
                discard(state, agent, id, &old).await;
            }
            Ok((row, secret))
        }
        Err(e) => {
            discard(state, agent, id, &new_ref).await;
            Err(e)
        }
    }
}

/// Delete a webhook and its delivery log; (P5-REFINE-7, C-1074) its delivery threads stay,
/// marked deleted in the same transaction.
pub async fn delete(state: &AppState, agent: &AgentRow, r: &WebhookRow) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    convs::mark_automation_deleted(&mut tx, "webhook", r.id).await?;
    if !repo::delete(&mut tx, r.id).await? {
        return Err(not_found());
    }
    tx.commit().await.map_err(DbError::from)?;
    if let Some(old) = stored_ref(&r.verification) {
        discard(state, agent, r.id, &old).await;
    }
    Ok(())
}

/// Append a notice to the webhook's origin conversation (if any).
pub async fn notify_origin(
    conn: &mut PgConnection,
    w: &WebhookRow,
    code: &str,
    text: &str,
) -> Result<(), ApiError> {
    let Some(origin) = w.origin_conversation_id else {
        return Ok(());
    };
    if !convs::lock(&mut *conn, origin).await? {
        return Ok(());
    }
    conv::append_event(
        conn,
        origin,
        None,
        "notice",
        &json!({"code": code, "text": text, "webhook_id": w.id}),
    )
    .await?;
    Ok(())
}

/// Hook from `lifecycle::finish`: success resets the failure count; `SCHEDULE_FAILURE_DISABLE`
/// consecutive failed turns disable the webhook with `failures` (docs/10).
pub async fn on_turn_finished(conn: &mut PgConnection, t: &TurnRow) -> Result<(), ApiError> {
    if t.door.get("kind").and_then(|k| k.as_str()) != Some("webhook") {
        return Ok(());
    }
    let Some(id) = t
        .door
        .get("ref")
        .and_then(|r| r.as_str())
        .and_then(|r| r.parse::<Uuid>().ok())
    else {
        return Ok(());
    };
    let Some(w) = webhooks::get(&mut *conn, id).await? else {
        return Ok(());
    };
    match t.status.as_str() {
        "done" => repo::reset_failures(conn, id).await?,
        "failed" => {
            let n = repo::add_failure(conn, id).await?;
            let limit = FAILURE_DISABLE;
            if n >= limit && w.enabled {
                repo::disable(conn, id, "failures").await?;
                notify_origin(
                    conn,
                    &w,
                    "webhook_disabled",
                    &format!(
                        "Webhook \"{}\" was disabled after {n} consecutive failed turns.",
                        w.name
                    ),
                )
                .await?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_secret_is_64_hex_characters_and_never_repeats() {
        let a = generate_secret().unwrap();
        let b = generate_secret().unwrap();
        assert_eq!(a.len(), 64);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(a, b);
    }

    fn input(kind: &str, secret: Option<&str>, generate: bool) -> VerificationInput {
        VerificationInput {
            kind: kind.into(),
            header: None,
            secret: secret.map(str::to_owned),
            generate_secret: generate,
        }
    }

    #[test]
    fn generation_is_exclusive_with_a_secret_and_needs_a_signed_kind() {
        let ch = check_verification(input("hmac_sha256", None, true)).unwrap();
        assert!(ch.generated);
        assert_eq!(ch.header.as_deref(), Some("X-Signature"));
        assert_eq!(ch.secret.as_deref().map(str::len), Some(64));
        let ch = check_verification(input("bearer", None, true)).unwrap();
        assert_eq!(ch.header.as_deref(), Some("Authorization"));
        assert_eq!(
            check_verification(input("hmac_sha256", Some("chosen-123"), true))
                .err()
                .map(|e| e.code().to_owned()),
            Some("invalid_verification".to_owned())
        );
        assert!(check_verification(input("none", None, true)).is_err());
        let ch = check_verification(input("hmac_sha256", Some("chosen-123"), false)).unwrap();
        assert!(!ch.generated);
        // The create default is signed with a generated secret.
        let d = default_verification();
        assert_eq!((d.kind.as_str(), d.generate_secret), ("hmac_sha256", true));
    }
}
