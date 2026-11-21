//! Who may do what with a skill library (P5-SKLIB-2, D-449, C-1128, SEC-149): the one place that
//! answers it. Every library route goes through [`manage`] or [`read`]; nothing else compares a
//! library's organization with the caller's.
//!
//! - **Organization library** (`org_id` set): its organization's owner/admin manage it; anyone who
//!   can use a workspace of the organization reads it (members only while it is `active`).
//! - **Instance library** (`org_id` null): instance admins manage it, **session only** (an API key
//!   never does); a person reads it *as* one organization (`?org=`) they can use a workspace of.
//!   A caller who may do neither gets 404, never 403, so ids do not leak.

use aurora_core::ApiError;
use axum::http::StatusCode;
use uuid::Uuid;

use aurora_db::repo::skill_libraries::LibraryRow;

use super::service::View;
use crate::app::AppState;
use crate::http::auth::Caller;

fn not_found() -> ApiError {
    ApiError::not_found("Skill library not found.")
}

/// A refusal (404 or 403) becomes the library's own 404; any other error (a database or other
/// internal failure) is passed through unchanged.
fn hide_denial(e: ApiError) -> ApiError {
    if matches!(e.status(), StatusCode::NOT_FOUND | StatusCode::FORBIDDEN) {
        not_found()
    } else {
        e
    }
}

/// How `GET /v1/skill-libraries/{id}` looks at a library: its manager's view, or one
/// organization's numbers.
///
/// - Organization library: its owner/admin (`Manage`; `?org=` is not consulted).
/// - Instance library with `?org=`: an instance admin's session may look at **any existing**
///   organization's per-tier counts (no names; D-449(c)); an organization's owner/admin sees
///   that organization's view; anyone else, and an organization that does not exist, is 404.
/// - Instance library without `?org=`: an instance admin's totals, else 404.
pub async fn view_as(
    c: &Caller,
    s: &AppState,
    row: &LibraryRow,
    org: Option<Uuid>,
) -> Result<View, ApiError> {
    match (row.org_id, org) {
        (Some(_), _) | (None, None) => {
            manage(c, s, row).await?;
            Ok(View::Manage)
        }
        (None, Some(o)) => {
            if c.is_instance_admin && c.is_session() {
                return match aurora_db::repo::orgs::name_of(&s.pool, o).await? {
                    Some(_) => Ok(View::Org(o)),
                    None => Err(not_found()),
                };
            }
            match c.org(s, o).await.and_then(|a| a.require_admin()) {
                Ok(()) => Ok(View::Org(o)),
                Err(e) => Err(hide_denial(e)),
            }
        }
    }
}

/// May `c` manage (change, pause, delete, refresh, token) this library? Org library: the
/// organization's owner/admin (403 for a member, 404 for a stranger, as before); instance library:
/// an instance admin's session, else 404.
pub async fn manage(c: &Caller, s: &AppState, row: &LibraryRow) -> Result<(), ApiError> {
    match row.org_id {
        Some(org) => {
            c.org(s, org).await?.require_admin()?;
            Ok(())
        }
        None if c.is_instance_admin && c.is_session() => Ok(()),
        None => Err(not_found()),
    }
}

/// The organization a read happens as, and whether the caller is an owner/admin of it.
#[derive(Clone, Copy, Debug)]
pub struct ReadAs {
    pub org: Uuid,
    pub admin: bool,
}

/// Browse audience of one organization: owners/admins (`true`) and anyone who can use a workspace
/// of it (`false`); a stranger is 404, a member without a workspace 403.
pub async fn audience(c: &Caller, s: &AppState, org: Uuid) -> Result<bool, ApiError> {
    let a = c.org(s, org).await?;
    if a.role.is_admin() {
        return Ok(true);
    }
    if aurora_db::authz::uses_any_workspace(&s.pool, c.user_id, org).await? {
        Ok(false)
    } else {
        Err(ApiError::forbidden("Workspace access required."))
    }
}

/// May `c` read (browse, read content, add from) this library, and as which organization?
///
/// `org` is the `?org=` of the request: optional for an organization library (it must be the
/// library's organization when given), **required** for an instance library (400 without it; 404
/// when the caller cannot use a workspace of it). Members get 404 for a library that is not
/// `active` (the list hides it); owners/admins keep reading every status.
pub async fn read(
    c: &Caller,
    s: &AppState,
    row: &LibraryRow,
    org: Option<Uuid>,
) -> Result<ReadAs, ApiError> {
    let (org, admin) = match row.org_id {
        Some(own) => {
            if org.is_some_and(|o| o != own) {
                return Err(not_found());
            }
            (own, audience(c, s, own).await?)
        }
        None => {
            let Some(o) = org else {
                return Err(ApiError::invalid_field(
                    "Invalid request.",
                    "/org",
                    "the organization to read the library as",
                ));
            };
            // Another organization's id, or no right to use a workspace there: the same 404.
            // Only "not found" and "forbidden" are hidden that way; a database or other
            // internal failure stays a 5xx.
            match audience(c, s, o).await {
                Ok(admin) => (o, admin),
                Err(e) => return Err(hide_denial(e)),
            }
        }
    };
    if !admin && row.status != "active" {
        return Err(not_found());
    }
    Ok(ReadAs { org, admin })
}
