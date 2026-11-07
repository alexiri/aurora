//! Speaker resolution (docs/13 §5.1 step 4, §5.9) and the shared bits several handlers need.

use super::Retry;
use super::send::{self, Ctx};
use crate::app::AppState;
use crate::domains::connectors::inbox::InboxError;
use aurora_core::sanitize;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::authz;
use aurora_db::repo::{link_requests, user_links, users};
use aurora_protocol::connector::Sender;
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

/// A person's platform-verified email equal to a verified Aurora user's email links them
/// (`verified_email`), provided the platform reports it verified, the connector has
/// `email_verified`, and that user is an active member of the integration's org.
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
    let method_ok = matches!(
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

/// Creates a `link_requests` row and tells the person (privately) where to sign in.
#[allow(clippy::too_many_arguments)]
pub async fn send_link_prompt(
    state: &AppState,
    ctx: &Ctx,
    conn: &mut PgConnection,
    sender: &Sender,
    space_ref: &str,
    thread_ref: &str,
    user_ref: &str,
    dedupe: String,
) -> Result<(), InboxError> {
    let tok = token::generate(TokenPrefix::Invitation);
    link_requests::insert(
        &mut *conn,
        ctx.id(),
        user_ref,
        &token::hash(tok.expose()),
        LINK_TTL_SECS,
    )
    .await
    .retry()?;
    let url = format!(
        "{}/link/channel?token={}",
        state.config.external_url.trim_end_matches('/'),
        tok.expose()
    );
    let text = format!(
        "To work with agents here, link your Aurora account: {url} (the link works once, for 30 minutes)."
    );
    send::private(
        conn, ctx, sender, space_ref, thread_ref, user_ref, &text, dedupe,
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

/// Whether a typed code names this approval (case-insensitive, spaces and dashes ignored).
pub fn approval_code_matches(id: Uuid, typed: &str) -> bool {
    let t: String = typed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    t.len() == 8 && t == approval_code(id)
}
