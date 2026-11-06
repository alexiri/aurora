//! Repositories: all SQL lives here (docs/02 §Coding rules). One module per domain.

pub mod api_keys;
pub mod audit;
pub mod sessions;
pub mod users;

use crate::DbError;
use crate::bus::{self, BusError, Channel};
use sqlx::PgExecutor;
use uuid::Uuid;

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
