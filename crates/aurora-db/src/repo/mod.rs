//! Repositories: all SQL lives here (docs/02 §Coding rules). One module per domain.

pub mod agents;
pub mod api_keys;
pub mod audit;
pub mod members;
pub mod orgs;
pub mod policy;
pub mod sessions;
pub mod users;
pub mod workspaces;

use crate::DbError;
use crate::bus::{self, BusError, Channel};
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// Keyset position `(created_at, id)` of the last row of the previous page.
pub type After = Option<(DateTime<Utc>, Uuid)>;

fn split(after: After) -> (Option<DateTime<Utc>>, Option<Uuid>) {
    (after.map(|a| a.0), after.map(|a| a.1))
}

/// Emit `aurora_auth_revoked {kind, id}` (`kind`: `session`, `key`, `user`).
/// Inside a transaction it is delivered on commit.
pub async fn notify_auth_revoked<'e, E: PgExecutor<'e>>(
    exec: E,
    kind: &str,
    id: Uuid,
) -> Result<(), DbError> {
    bus::notify(
        exec,
        Channel::AuthRevoked,
        &serde_json::json!({"kind": kind, "id": id}),
    )
    .await
    .map_err(|e| match e {
        BusError::Db(e) => e.into(),
        other => DbError::Other(sqlx::Error::Protocol(other.to_string())),
    })
}
