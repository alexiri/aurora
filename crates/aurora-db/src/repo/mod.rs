//! Repositories: all SQL lives here (docs/02 §Coding rules). One module per domain.

pub mod account;
pub mod action_approvals;
pub mod agent_links;
pub mod agents;
pub mod api_keys;
pub mod app_instances;
pub mod approval_notify;
pub mod approvals;
pub mod attachments;
pub mod audit;
pub mod automations;
pub mod availability;
pub mod blobs;
pub mod channel_cards;
pub mod channel_inbox;
pub mod channel_integrations;
pub mod channel_ops;
pub mod channel_presences;
pub mod cli_logins;
pub mod connections;
pub mod connector_hosts;
pub mod connectors;
pub mod conversations;
pub mod credentials;
pub mod derived_rules;
pub mod dm_defaults;
pub mod freeze;
pub mod gateway;
pub mod governance;
pub mod grants;
pub mod groups;
pub mod hosted;
pub mod identities;
pub mod idp_roles;
pub mod invitations;
pub mod lifecycle;
pub mod link_requests;
pub mod members;
pub mod memory;
pub mod mention_anchors;
pub mod oauth_clients;
pub mod orgs;
pub mod peer_tasks;
pub mod placement;
pub mod plugins;
pub mod policy;
pub mod reach_grants;
pub mod request_log;
pub mod runners;
pub mod sandboxes;
pub mod schedules;
pub mod secrets;
pub mod sessions;
pub mod skill_libraries;
pub mod skills;
pub mod space_bindings;
pub mod thread_links;
pub mod tool_identity;
pub mod transcript;
pub mod turns;
pub mod user_links;
pub mod users;
pub mod webhooks;
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
