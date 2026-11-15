//! Interactive cards (docs/13 §5.6, BE-073): builders per subject, posting and settling through
//! BE-071 ops, the `channel_cards` lifecycle and the fan-out job.
//!
//! Decisions:
//! * a card is `posting` from the moment its `message.post` op is queued (one transaction, op
//!   `dedupe_key` = `approval:<id>` etc.); it becomes `posted` when the sync step sees the op
//!   `done` (the platform `message_ref` comes from the op result, so no hook into the result
//!   handler is needed) or `failed` when the op failed/was dropped;
//! * settling claims `posted -> settled` exactly once, then queues `message.update` (platform has
//!   `edits` and the card's space is known) or a new short `message.post` (`edits` absent, or the
//!   card went to a DM, whose space ref is only learned from a click);
//! * cards DMed to a person store `user:<ref>` as `space_ref` until a click event reveals the
//!   real space;
//! * action tokens live for `ACTION_TOKEN_TTL`; the single-use ledger is written only when a click
//!   is accepted, so refused clicks (unlinked, not a member) do not burn the token.
pub mod build;
pub mod fanout;
pub mod requests;

use crate::app::AppState;
use aurora_channels::{
    ActionKind, ActionToken, CapabilitySet, Card, Claims, Content, FallbackContext, render_card,
};
use aurora_core::ApiError;
use aurora_core::limits::ACTION_TOKEN_TTL;
use aurora_crypto::DerivedKeys;
use aurora_db::DbError;
use aurora_db::bus::Channel;
use aurora_db::repo::channel_cards::{self as repo, CardRow, NewCard};
use aurora_db::repo::channel_ops::{self as ops, InsertOp};
use aurora_db::repo::{channel_integrations as integrations, connectors};
use aurora_protocol::connector::{MessagePostOp, MessageUpdateOp, Sender, Target};
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

/// Prefix of `space_ref` for cards sent to a person's DM.
pub const DM_PREFIX: &str = "user:";

/// Signs action tokens for one integration.
pub struct Minter<'a> {
    pub keys: &'a DerivedKeys,
    pub integration_id: Uuid,
}

impl Minter<'_> {
    pub fn mint(&self, subject: Uuid, action: ActionKind) -> Result<String, ApiError> {
        let ttl = chrono::Duration::from_std(ACTION_TOKEN_TTL).unwrap_or(chrono::Duration::days(1));
        let exp = (Utc::now() + ttl).timestamp();
        ActionToken::sign(
            self.keys,
            &Claims::new(self.integration_id, subject, exp, action),
        )
        .map_err(|e| ApiError::internal(format!("action token: {e}")))
    }
}

pub fn minter<'a>(state: &'a AppState, integration_id: Uuid) -> Minter<'a> {
    Minter {
        keys: &state.derived,
        integration_id,
    }
}

/// Capabilities of an integration: manifest narrowed by `config._capabilities`.
pub async fn capabilities(
    conn: &mut PgConnection,
    integration_id: Uuid,
) -> Result<CapabilitySet, ApiError> {
    let integ = integrations::get(&mut *conn, integration_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Integration not found."))?;
    let manifest = connectors::get(&mut *conn, &integ.connector_id)
        .await?
        .and_then(|c| c.manifest);
    Ok(caps_from(manifest.as_ref(), &integ.config))
}

pub fn caps_from(manifest: Option<&Value>, config: &Value) -> CapabilitySet {
    let names: Vec<String> = manifest
        .and_then(|m| m.get("capabilities"))
        .and_then(Value::as_object)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    CapabilitySet::new(
        names
            .iter()
            .filter(|n| super::capability_enabled(manifest, config, n))
            .filter_map(|n| aurora_channels::Capability::from_wire(n).ok()),
    )
}

/// The 8-character code people type in `approve <code>` / `deny <code>` (13 §4.5 fallback):
/// RFC 4648 base32 (upper case) of the first 40 bits of `sha256(id)`. Decision S-12: the spec's
/// "first 8 base32 chars of the id" collides for UUIDv7 ids created close together (their leading
/// bits are a timestamp). Matching scans the clicker's pending approvals, so collisions only make
/// a code ambiguous (sent to the dashboard), never decide the wrong approval.
pub fn approval_code(id: Uuid) -> String {
    use sha2::{Digest, Sha256};
    const A: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let d = Sha256::digest(id.as_bytes());
    let bits = d[..5]
        .iter()
        .fold(0u64, |acc, x| (acc << 8) | u64::from(*x));
    (0..8)
        .map(|i| A[((bits >> (35 - 5 * i)) & 31) as usize] as char)
        .collect()
}

/// Dashboard URL under the instance's external URL.
pub fn dashboard_url_for(external_url: &str, path: &str) -> String {
    format!("{}{}", external_url.trim_end_matches('/'), path)
}

pub fn dashboard_url(state: &AppState, path: &str) -> String {
    dashboard_url_for(&state.config.external_url, path)
}

/// Dashboard URL of a workspace page (`/o/<org slug>/w/<workspace slug>/<page>`, the web app's
/// routes); falls back to the dashboard home when the workspace is gone.
pub async fn workspace_page_url(
    conn: &mut PgConnection,
    state: &AppState,
    workspace_id: Uuid,
    page: &str,
) -> Result<String, ApiError> {
    workspace_page_url_for(conn, &state.config.external_url, workspace_id, page).await
}

/// [`workspace_page_url`] for callers without `AppState` (lifecycle hooks).
pub async fn workspace_page_url_for(
    conn: &mut PgConnection,
    external_url: &str,
    workspace_id: Uuid,
    page: &str,
) -> Result<String, ApiError> {
    let slugs: Option<(String, String)> = sqlx::query_as(
        "SELECT o.slug, w.slug FROM workspaces w JOIN organizations o ON o.id = w.org_id WHERE w.id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    Ok(match slugs {
        Some((org, ws)) => dashboard_url_for(external_url, &format!("/o/{org}/w/{ws}/{page}")),
        None => dashboard_url_for(external_url, "/"),
    })
}

async fn enqueue(
    conn: &mut PgConnection,
    integration_id: Uuid,
    kind: &str,
    id: Uuid,
    payload: &Value,
    dedupe: &str,
) -> Result<bool, ApiError> {
    let ins = ops::insert(
        conn,
        &InsertOp {
            id,
            connector_id: None,
            integration_id: Some(integration_id),
            presence_id: None,
            kind,
            payload,
            dedupe_key: Some(dedupe),
            correlation_id: None,
        },
    )
    .await?;
    let Some((_, connector, inserted)) = ins else {
        return Err(ApiError::not_found("Integration not found."));
    };
    if inserted {
        aurora_db::bus::notify(conn, Channel::Ops, &json!({"connector": connector}))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok(inserted)
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| ApiError::internal(e.to_string()))
}

/// A card to post.
pub struct NewPost<'a> {
    pub integration_id: Uuid,
    pub subject_kind: &'a str,
    pub subject_id: Uuid,
    /// Id of the `channel_cards` row (tokens of `binding`/`dm_default` cards are bound to it).
    pub card_id: Uuid,
    pub target: Target,
    pub sender: Sender,
    pub card: Card,
    pub dashboard_url: Option<String>,
    /// Set for approval cards: enables `approve`/`deny` text fallbacks.
    pub approval_code: Option<String>,
    pub dedupe_key: String,
}

/// Queue the card's `message.post` and its `channel_cards` row. `false` = already posted
/// (same `dedupe_key`).
pub async fn post_card(
    conn: &mut PgConnection,
    caps: &CapabilitySet,
    p: NewPost<'_>,
) -> Result<bool, ApiError> {
    let rendered = render_card(
        &p.card,
        caps,
        &FallbackContext {
            dashboard_url: p.dashboard_url.as_deref(),
            approval_code: p.approval_code.as_deref(),
        },
    );
    let content = Content {
        text: rendered.text,
        card: rendered.card,
    };
    let (target, space_ref, thread_ref) = match p.target {
        Target::Space {
            space_ref,
            thread_ref,
        } => {
            let t = caps
                .thread_ref(thread_ref.as_deref().unwrap_or(""))
                .to_owned();
            (
                Target::Space {
                    space_ref: space_ref.clone(),
                    thread_ref: (!t.is_empty()).then(|| t.clone()),
                },
                space_ref,
                t,
            )
        }
        Target::User { user_ref } => {
            let s = format!("{DM_PREFIX}{user_ref}");
            (Target::User { user_ref }, s, String::new())
        }
    };
    let op_id = Uuid::now_v7();
    let payload = to_json(&MessagePostOp {
        target,
        sender: p.sender,
        content: to_json(&content)?,
        files: vec![],
        ephemeral_to: None,
        op_marker: op_id.to_string(),
    })?;
    if !enqueue(
        conn,
        p.integration_id,
        "message.post",
        op_id,
        &payload,
        &p.dedupe_key,
    )
    .await?
    {
        return Ok(false);
    }
    repo::insert(
        &mut *conn,
        &NewCard {
            id: p.card_id,
            subject_kind: p.subject_kind,
            subject_id: p.subject_id,
            integration_id: p.integration_id,
            post_op_id: op_id,
            space_ref: &space_ref,
            thread_ref: &thread_ref,
            addressee_ref: space_ref.strip_prefix(DM_PREFIX),
        },
    )
    .await?;
    Ok(true)
}

/// A short private notice (refusal, link prompt): ephemeral when supported and the space is
/// known, else a DM to the person.
pub async fn notify_person(
    conn: &mut PgConnection,
    caps: &CapabilitySet,
    integration_id: Uuid,
    user_ref: &str,
    space_ref: Option<&str>,
    content: Content,
    dedupe: &str,
) -> Result<(), ApiError> {
    use aurora_channels::capability::PrivateNotice;
    let (target, ephemeral_to) = match (caps.private_notice(), space_ref) {
        (PrivateNotice::Ephemeral, Some(s)) if !s.is_empty() && !s.starts_with(DM_PREFIX) => (
            Target::Space {
                space_ref: s.to_owned(),
                thread_ref: None,
            },
            Some(user_ref.to_owned()),
        ),
        _ => (
            Target::User {
                user_ref: user_ref.to_owned(),
            },
            None,
        ),
    };
    let op_id = Uuid::now_v7();
    let payload = to_json(&MessagePostOp {
        target,
        sender: Sender::shared(),
        content: to_json(&content)?,
        files: vec![],
        ephemeral_to,
        op_marker: op_id.to_string(),
    })?;
    enqueue(
        conn,
        integration_id,
        "message.post",
        op_id,
        &payload,
        dedupe,
    )
    .await?;
    Ok(())
}

/// Settle a posted card with `summary` (who decided and how). Exactly once: the first caller
/// claims `posted -> settled`. Cards still `posting` are left for the next sync.
pub async fn settle_card(
    conn: &mut PgConnection,
    card: &CardRow,
    caps: &CapabilitySet,
    summary: &str,
) -> Result<bool, ApiError> {
    use aurora_channels::capability::SettleMode;
    if card.state != "posted" || !repo::claim_settle(&mut *conn, card.id).await? {
        return Ok(false);
    }
    let sender = match ops::get(&mut *conn, card.post_op_id).await? {
        Some(op) => op
            .payload
            .get("sender")
            .cloned()
            .and_then(|s| serde_json::from_value(s).ok())
            .unwrap_or_else(Sender::shared),
        None => Sender::shared(),
    };
    let edit = caps.settle_mode() == SettleMode::EditInPlace
        && !card.space_ref.starts_with(DM_PREFIX)
        && card.message_ref.is_some();
    let dedupe = format!("settle:{}", card.id);
    let op_id = Uuid::now_v7();
    if let (true, Some(message_ref)) = (edit, card.message_ref.as_deref()) {
        let content = Content::card(Card {
            blocks: vec![aurora_channels::Block::text(summary)],
        });
        let payload = to_json(&MessageUpdateOp {
            space_ref: card.space_ref.clone(),
            message_ref: message_ref.to_owned(),
            sender,
            content: to_json(&content)?,
        })?;
        enqueue(
            conn,
            card.integration_id,
            "message.update",
            op_id,
            &payload,
            &dedupe,
        )
        .await?;
    } else {
        let target = match card.space_ref.strip_prefix(DM_PREFIX) {
            Some(u) => Target::User {
                user_ref: u.to_owned(),
            },
            None => Target::Space {
                space_ref: card.space_ref.clone(),
                thread_ref: (!card.thread_ref.is_empty()).then(|| card.thread_ref.clone()),
            },
        };
        let payload = to_json(&MessagePostOp {
            target,
            sender,
            content: to_json(&Content::platform(summary))?,
            files: vec![],
            ephemeral_to: None,
            op_marker: op_id.to_string(),
        })?;
        enqueue(
            conn,
            card.integration_id,
            "message.post",
            op_id,
            &payload,
            &dedupe,
        )
        .await?;
    }
    Ok(true)
}
