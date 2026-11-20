//! Connectors (W8, docs/13): registration, trust, ownership and the config feed (BE-070).
//!
//! * management plane — `GET /v1/connectors[/{id}/icon]` and `/v1/admin/connectors…`
//!   (`routes.rs`, `admin.rs`; in the dashboard OpenAPI document);
//! * connector plane — `/v1/connector/*` authenticated with the `aun_…` anchor token
//!   (`plane.rs`, `auth.rs`; machine plane, excluded from the dashboard OpenAPI document);
//! * (phase 4, P4-MCON) managed connectors: type catalogue, create/PATCH with sealed settings
//!   (`managed.rs`, `setup.rs`) and the connector-host plane `/v1/connector-host/*` with the
//!   `auh_…` token (`host.rs`).
mod admin;
pub use admin::bootstrap_tokens;
mod auth;
pub mod bus;
mod dto;
mod files;
mod host;
pub use host::bootstrap_host_token;
mod host_tokens;
pub mod inbox;
mod ingest;
mod link;
mod managed;
pub(crate) mod manifest;
pub mod ops;
mod plane;
pub(crate) mod readiness;
mod routes;
mod setup;

use aurora_secrets::{Purpose, SealContext};
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

use crate::app::AppState;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(routes::router())
        .merge(plane::router())
        .merge(host::router())
        .merge(ingest::router())
        .merge(ops::router())
        .merge(files::router())
        .merge(link::router())
}

/// (D-369, SEC-134) The manifest a connector row is served under: the approved baseline while an
/// update waits for approval, the announced one otherwise. Every reader that grants, routes or
/// assigns something goes through this.
pub fn effective_manifest(
    row: &aurora_db::repo::connectors::ConnectorRow,
) -> Option<serde_json::Value> {
    effective_manifest_ref(row).cloned()
}

/// [`effective_manifest`], borrowed.
pub fn effective_manifest_ref(
    row: &aurora_db::repo::connectors::ConnectorRow,
) -> Option<&serde_json::Value> {
    row.manifest.as_ref().map(manifest::effective)
}

/// Seal context of an integration's platform credentials.
pub fn integration_seal_ctx(integration_id: Uuid, org_id: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::IntegrationCredentials,
        owner_id: integration_id,
        org_id,
        workspace_id: None,
    }
}

/// Seal context of a dedicated presence's platform credentials.
pub fn presence_seal_ctx(presence_id: Uuid, org_id: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::PresenceCredentials,
        owner_id: presence_id,
        org_id,
        workspace_id: None,
    }
}
