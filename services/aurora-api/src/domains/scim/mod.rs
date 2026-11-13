//! SCIM 2.0 provisioning (phase 4, P4-GRP-2, D-132, D-135, SEC-72, SEC-73; docs/04 §SCIM,
//! docs/16 §W15, docs/05 §SCIM management): the `/scim/v2` plane and its management routes.
//!
//! Authority: a per-org `aud_` bearer created by an org owner. The surface answers 404 while
//! the instance switch is off, and every authentication failure is one identical 401. The
//! domain never mutates owners, never sets roles and never touches manual or IdP-linked
//! groups. Every write is one transaction with its security-critical audit row.

pub mod auth;
pub mod deprovision;
pub mod dto;
pub mod error;
pub mod gates;
pub mod management;
pub mod pending;
pub mod protocol;
pub mod rate_limit;
pub mod resource_groups;
pub mod resource_users;
pub mod service;
pub mod settings;

/// The `/v1` management routes (settings, tokens, status), merged into the v1 router.
pub use management::router as management_router;
/// The router of the whole `/scim/v2` plane (rate limiting, media types, discovery, Users,
/// Groups) — nested at `/scim/v2` in `app.rs`, never in the OpenAPI document.
pub use protocol::router;
