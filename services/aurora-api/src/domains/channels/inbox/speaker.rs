//! Speaker resolution (docs/13 §5.1 step 4, §5.9) and the shared bits several handlers need.

use super::Retry;
use super::send::{self, Ctx};
use crate::app::AppState;
use crate::domains::connectors::inbox::InboxError;
use aurora_core::sanitize;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::authz;
use aurora_db::repo::{link_requests, user_links, users};
use aurora_protocol::connector::{Sender, Space};
use sqlx::PgConnection;
use uuid::Uuid;

const LINK_TTL_SECS: f64 = 30.0 * 60.0;

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// A person with an Aurora user (linked now or before).
    User(Uuid),
    /// A bot author.
    Bot,
    /// A person with no link yet.
    NeedsLink,
}

/// Whether an org admin enabled verified-email auto-linking for the integration
/// (`config.verified_email_autolink`, default off; docs/13 §5.9, RFE-001).
pub fn autolink_enabled(config: &serde_json::Value) -> bool {
    config
        .get("verified_email_autolink")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Only when the integration opted in (`verified_email_autolink`), a person's
/// platform-verified email equal to a verified Aurora user's email links them
/// (`verified_email`), provided the platform reports it verified, the connector has
/// `email_verified`, and that user is an active member of the integration's org. Otherwise
/// (the default) the person gets the sign-in link.
pub async fn resolve_person(
    state: &AppState,
    ctx: &Ctx,
    user_ref: &str,
    email: Option<&str>,
    email_verified: bool,
) -> Result<Speaker, InboxError> {
    let pool = &state.pool;
    if let Some(l) = user_links::user_link_for(pool, ctx.id(), user_ref)
        .await
        .retry()?
    {
        return Ok(Speaker::User(l.user_id));
    }
    let method_ok = autolink_enabled(&ctx.integ.config)
        && matches!(
            ctx.caps.link_method(),
            aurora_channels::capability::LinkMethod::VerifiedEmail
        );
    let Some(email) = email.filter(|e| method_ok && email_verified && !e.trim().is_empty()) else {
        return Ok(Speaker::NeedsLink);
    };
    let Some(user) = users::find_by_email(pool, email).await.retry()? else {
        return Ok(Speaker::NeedsLink);
    };
    if user.email_verified_at.is_none() || user.disabled_at.is_some() {
        return Ok(Speaker::NeedsLink);
    }
    let member = authz::org_membership(pool, user.id, ctx.integ.org_id)
        .await
        .retry()?
        .is_some_and(|m| m.is_active());
    if !member {
        return Ok(Speaker::NeedsLink);
    }
    match user_links::insert(pool, ctx.id(), user_ref, user.id, "verified_email").await {
        Ok(_) => Ok(Speaker::User(user.id)),
        Err(aurora_db::DbError::UniqueViolation { .. }) => {
            // Linked concurrently (to this or another user): read what is there now.
            match user_links::user_link_for(pool, ctx.id(), user_ref)
                .await
                .retry()?
            {
                Some(l) => Ok(Speaker::User(l.user_id)),
                None => Ok(Speaker::NeedsLink),
            }
        }
        Err(e) => Err(InboxError::Retry(e.to_string())),
    }
}

/// A person is prompted to link at most once per this long (FIX-112; docs/13 §5.9).
pub const LINK_PROMPT_INTERVAL_SECS: f64 = 10.0 * 60.0;

type RefusalKey = (Uuid, Uuid, String);
static REFUSALS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<RefusalKey, std::time::Instant>>,
> = std::sync::OnceLock::new();

/// Whether the "you can't talk to this agent yet" notice should go out now: at most one per
/// (integration, agent, person) per [`LINK_PROMPT_INTERVAL_SECS`] (FIX-112); records it when it
/// returns `true`. Per replica and not persisted: a restart only repeats a notice once.
pub fn refusal_due(integration: Uuid, agent: Uuid, person: &str) -> bool {
    let map = REFUSALS.get_or_init(Default::default);
    let Ok(mut m) = map.lock() else { return true };
    let now = std::time::Instant::now();
    let interval = std::time::Duration::from_secs_f64(LINK_PROMPT_INTERVAL_SECS);
    if m.len() > 10_000 {
        m.retain(|_, t| now.duration_since(*t) < interval);
    }
    let key = (integration, agent, person.to_owned());
    match m.get(&key) {
        Some(t) if now.duration_since(*t) < interval => false,
        _ => {
            m.insert(key, now);
            true
        }
    }
}

/// Creates a `link_requests` row and tells the person (privately) where to sign in. A person who
/// already has an unexpired, unused request made within [`LINK_PROMPT_INTERVAL_SECS`] gets neither
/// a new row nor another message (a `listen=all` channel or a hostile member cannot flood DMs).
#[allow(clippy::too_many_arguments)]
pub async fn send_link_prompt(
    state: &AppState,
    ctx: &Ctx,
    conn: &mut PgConnection,
    sender: &Sender,
    space: &Space,
    thread_ref: &str,
    user_ref: &str,
    platform_email: Option<&str>,
    platform_display_name: Option<&str>,
    dedupe: String,
) -> Result<(), InboxError> {
    let hint = platform_email.map(str::trim).filter(|e| !e.is_empty());
    let shown = platform_display_name
        .map(sanitize::label)
        .filter(|n| !n.is_empty());
    let tok = token::generate(TokenPrefix::Invitation);
    let created = link_requests::insert_throttled(
        &mut *conn,
        ctx.id(),
        user_ref,
        &token::hash(tok.expose()),
        LINK_TTL_SECS,
        LINK_PROMPT_INTERVAL_SECS,
        hint,
        shown.as_deref(),
    )
    .await
    .retry()?;
    if created.is_none() {
        return Ok(());
    }
    let url = format!(
        "{}/link/channel?token={}",
        state.config.external_url.trim_end_matches('/'),
        tok.expose()
    );
    let text = format!(
        "To work with agents here, link your Aurora account: {url} (the link works once, for 30 minutes)."
    );
    send::private(
        conn, ctx, sender, space, thread_ref, user_ref, &text, dedupe,
    )
    .await
    .retry()
    .map(|_| ())
}

/// The approval code (one implementation, decision S-12): see [`crate::domains::channels::cards::approval_code`].
pub fn approval_code(id: Uuid) -> String {
    crate::domains::channels::cards::approval_code(id)
}

/// Names for "ask X or Y to set this up" replies.
pub async fn owner_names(state: &AppState, ctx: &Ctx) -> Result<String, InboxError> {
    let owners = aurora_db::repo::channel_presences::setup_owners(&state.pool, ctx.id(), 5)
        .await
        .retry()?;
    let names: Vec<String> = owners.iter().map(|(_, n)| sanitize::label(n)).collect();
    Ok(if names.is_empty() {
        "a workspace owner".to_owned()
    } else {
        names.join(", ")
    })
}

/// The five bytes (40 bits) a typed approval code encodes (the inverse of the base32 of
/// `cards::approval_code`), so the database can find the approval by `sha256(id)[..5]` instead of
/// the caller scanning a page of pending approvals. `None` for anything that is not a code.
pub fn approval_code_prefix(typed: &str) -> Option<[u8; 5]> {
    let mut bits: u64 = 0;
    let mut n = 0;
    for ch in typed.chars().filter(char::is_ascii_alphanumeric) {
        let v = match ch.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u64 - 'A' as u64,
            c @ '2'..='7' => c as u64 - '2' as u64 + 26,
            _ => return None,
        };
        bits = (bits << 5) | v;
        n += 1;
    }
    if n != 8 {
        return None;
    }
    let b = bits.to_be_bytes();
    Some([b[3], b[4], b[5], b[6], b[7]])
}

/// Whether a typed code names this approval (case-insensitive, spaces and dashes ignored).
pub fn approval_code_matches(id: Uuid, typed: &str) -> bool {
    let t: String = typed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    t.len() == 8 && t == approval_code(id)
}
