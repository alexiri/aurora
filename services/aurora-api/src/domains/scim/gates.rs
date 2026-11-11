//! SCIM gates on sign-in admission (SEC-73, docs/04 §SCIM "exclusive" mode): JIT and
//! invitation acceptance are refused for people the directory removed (or suspended), and —
//! in `exclusive` mode — for people without an `active` link. The IdP is then the only way
//! in. Every refusal writes a `member.add` **denied** audit row in the refusing transaction.

use super::auth::OrgScim;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
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

/// The denied `member.add` audit row of a refusal (best-effort write inside the refusing
/// transaction: a failed row must not mask the gate, but the row is required by `04 §Audit`
/// for denied membership actions, so a failure fails the sign-in path).
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
