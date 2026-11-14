//! Wire types of the addon-source routes (C-590..C-602). Typed so the OpenAPI document — and
//! `web/src/lib/api/schema.d.ts` generated from it — describe the real shapes.

use crate::domains::plugins::dto::PluginPackage;
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// One version of one addon as the manifest lists it, with this instance's install state.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AddonVersion {
    pub version: String,
    pub min_core: Option<String>,
    /// False while `min_core` is above the running core (listed, not ingestable).
    pub ingestable: bool,
    /// `core_too_new` when not ingestable.
    pub not_ingestable_reason: Option<String>,
    /// The active version of this package on the instance, else the newest ingested one.
    pub installed_version: Option<String>,
    /// This version is semver-newer than the newest ingested one.
    pub update_available: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AddonEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Newest first.
    pub versions: Vec<AddonVersion>,
}

/// A version this instance ingested from the source that the manifest no longer lists.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RemovedUpstream {
    pub package_id: String,
    pub version: String,
}

/// `GET /v1/admin/addon-sources[/{id}]`, C-590.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AddonSource {
    pub id: Uuid,
    pub name: String,
    /// The resolved manifest URL.
    pub url: String,
    pub homepage: Option<String>,
    pub contact: Option<String>,
    pub publisher: String,
    /// `sha256:<12 hex>` of the manifest-published signing key.
    pub key_fingerprint: String,
    pub key_description: Option<String>,
    /// `6h` | `12h` | `1d` | `1w` | `2w` | `1mo` | `never`.
    pub update_interval: String,
    /// `ok` | `check_failed` | `signing_key_changed`.
    pub status: String,
    pub check_failed_at: Option<DateTime<Utc>>,
    /// Sanitized, at most 200 characters.
    pub check_failed_reason: Option<String>,
    /// Only while `status = signing_key_changed`.
    pub new_key_fingerprint: Option<String>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub manifest_digest: String,
    pub addons: Vec<AddonEntry>,
    pub addon_count: usize,
    pub removed_upstream: Vec<RemovedUpstream>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AddonSourceList {
    pub items: Vec<AddonSource>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AddonRepoInfo {
    pub name: String,
    pub description: String,
    pub homepage: Option<String>,
    pub contact: Option<String>,
}

/// `POST /v1/admin/addon-sources/preview`, C-592: what registering would trust and ingest.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AddonSourcePreview {
    /// The normalized manifest URL that would be stored.
    pub url: String,
    pub repo: AddonRepoInfo,
    pub publisher: String,
    pub key_fingerprint: String,
    pub key_description: Option<String>,
    pub addons: Vec<AddonEntry>,
    /// Packages registering would ingest now (not yet on this instance).
    pub packages_new: usize,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FailedVersion {
    pub package_id: String,
    pub version: String,
    pub reason: String,
}

/// One source's check, C-596.
#[derive(Serialize, ToSchema)]
pub struct AddonSourceCheckResult {
    pub source_id: Uuid,
    /// `updated` | `up_to_date` | `failed`.
    pub status: String,
    pub ingested: Vec<PluginPackage>,
    pub failed_versions: Vec<FailedVersion>,
    pub check_failed_reason: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AddonSourceCheckAll {
    pub items: Vec<AddonSourceCheckResult>,
}
