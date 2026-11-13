//! SCIM gates on sign-in admission (SEC-73, docs/04 §SCIM "exclusive" mode): JIT and
//! invitation acceptance are refused for people the directory removed (or suspended), and —
//! in `exclusive` mode — for people without an `active` link. The IdP is then the only way
//! in. Every refusal writes a `member.add` **denied** audit row in the refusing transaction.

use super::auth::OrgScim;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::tenancy::dto::OrgSettings;
use aurora_db::repo::scim;
use sqlx::PgConnection;
use uuid::Uuid;

/// The gate's verdict for admitting `user` into `org`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// No gate applies (no link, not exclusive) or an `active` link.
    Allowed,
    /// The directory removed this person from the org (tombstone).
    Removed,
    /// The link is suspended (a suspended member derives nothing; a fresh admission is
    /// refused rather than silently suspended again).
    Suspended,
    /// `exclusive` mode and no SCIM link.
    Exclusive,
}

impl Admission {
    /// The audit `reason` of a refusal; `None` when allowed.
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Allowed => None,
            Self::Removed => Some("scim_removed"),
            Self::Suspended => Some("scim_suspended"),
            Self::Exclusive => Some("scim_exclusive"),
        }
    }
}

/// Evaluate the admission gate of `org` for `user` (reads `organizations.settings` fresh).
pub async fn admission_gate(
    tx: &mut PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<Admission, aurora_db::DbError> {
    if let Some(link) = scim::user_link(&mut *tx, org, user).await? {
        return Ok(match link.state.as_str() {
            "removed" => Admission::Removed,
            "suspended" => Admission::Suspended,
            _ => Admission::Allowed,
        });
    }
    let settings = scim::settings_of(&mut *tx, org).await?;
    let exclusive = OrgScim::from_settings(
        org,
        settings.get("scim").unwrap_or(&serde_json::Value::Null),
    )
    .exclusive;
    Ok(if exclusive {
        Admission::Exclusive
    } else {
        Admission::Allowed
    })
}

/// Is the gate passed?
pub async fn allows(
    tx: &mut PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<bool, aurora_db::DbError> {
    Ok(admission_gate(tx, org, user).await? == Admission::Allowed)
}

/// The audit `reason` of a refusal by the seat cap (`organizations.settings.max_members`).
pub const MEMBER_LIMIT_REASON: &str = "member_limit_reached";

/// The org-level admission decision for a **new** membership (FIX-080): the SCIM gate, then
/// the seat cap. `Some(reason)` is a refusal, `None` admits. Every path that creates an
/// `org_members` row (invitation acceptance, JIT, new-user placement, IdP role-claim sync)
/// calls this.
///
/// **The caller holds the org lock** ([`aurora_db::repo::groups::lock_org`], `organizations`
/// row `FOR UPDATE`, the lock every SCIM writer takes first) and has re-read its own
/// membership under it: a link written by a concurrent SCIM `DELETE` or `POST` is then either
/// visible here or waits for the caller's transaction, and the seat count cannot move.
pub async fn admit_new_member(
    tx: &mut PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<Option<&'static str>, aurora_db::DbError> {
    if let Some(reason) = admission_gate(&mut *tx, org, user).await?.reason() {
        return Ok(Some(reason));
    }
    let settings = scim::settings_of(&mut *tx, org).await?;
    if let Some(cap) = OrgSettings::from_json(&settings).max_members
        && aurora_db::repo::invitations::count_active_members(&mut *tx, org).await? + 1 > cap
    {
        return Ok(Some(MEMBER_LIMIT_REASON));
    }
    Ok(None)
}

/// The denied `member.add` audit row of a refusal, written **on its own pool connection**
/// (FIX-080): the refusing transaction is rolled back (the route answers 403, the sign-in
/// fails, or a savepoint is abandoned) or goes on without the membership, and `04 §Audit`
/// requires the row of a denied membership action either way. A failed write is an error.
pub async fn audit_denied_detached(
    pool: &sqlx::PgPool,
    org: Uuid,
    user: Uuid,
    path: &str,
    reason: &str,
) -> Result<(), aurora_db::DbError> {
    let mut conn = pool.acquire().await?;
    audit_denied(&mut conn, org, user, path, reason).await
}

/// The denied `member.add` audit row of a refusal on `tx`. Callers whose transaction does not
/// survive the refusal use [`audit_denied_detached`].
pub async fn audit_denied(
    tx: &mut PgConnection,
    org: Uuid,
    user: Uuid,
    path: &str,
    reason: &str,
) -> Result<(), aurora_db::DbError> {
    let actor = AuditActor {
        kind: "service",
        id: None,
        label: "scim".to_owned(),
        source: "api",
        ip: None,
    };
    let ev = AuditEvent::new(actor, "member.add", Outcome::Denied)
        .org(org)
        .target("user", user)
        .metadata(serde_json::json!({"via": "scim", "path": path, "reason": reason}));
    audit::insert_tx(tx, &ev).await.map(|_| ())
}
