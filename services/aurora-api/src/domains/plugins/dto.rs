use aurora_db::repo::plugins::{PackageRow, TrustedKeyRow};
use aurora_registry::Manifest;
use aurora_registry::claims;
use aurora_registry::diff::Widening;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct PluginSigner {
    /// `builtin` | `trusted_key` | `unsigned`.
    pub kind: String,
    pub key_id: Option<String>,
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct PluginViolation {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct PluginAppRef {
    pub app_id: String,
    pub name: String,
    /// (phase 4, P4-GSL, D-149) "Can serve a skill library: reads repository files from
    /// `<library_host>`": the app declares a `library` block (shown in the permissions review).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_host: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema, Default)]
pub struct PluginPermissions {
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default)]
    pub egress: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub context: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PluginHostClaim {
    pub claim_key: String,
    pub host: String,
    pub path_prefix: Option<String>,
    pub app_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct PluginWidening {
    /// `hook` | `egress` | `context` | `host_claim` | `zone`.
    pub kind: String,
    pub detail: String,
}

#[derive(Serialize, ToSchema)]
pub struct PluginUserRef {
    pub id: Uuid,
    pub display_name: String,
}

/// Per-app share of a [`PluginImpact`]: counts only (C-340).
#[derive(Serialize, ToSchema)]
pub struct PluginImpactApp {
    pub app_id: String,
    /// Live connections (`active`, `reauth_required`) of this app in every organization.
    pub connections: i64,
    /// Of those, the ones failing the required-field check (`0` unless a major upgrade).
    pub needing_reauth: i64,
    /// (phase 4, P4-GRANTS, C-412) `full` agent grants of this app's connections that activation
    /// converts to custom grants; absent unless the version introduces `grants.custom_only`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub grants_to_convert: i64,
    /// (C-412) In how many workspaces those grants live (a count, never an id); absent with
    /// `grants_to_convert`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub grant_workspaces: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// What activating a pending version does to live connections (C-340, `09 §10`). Advisory:
/// the activation recomputes. Counts only: no organization, workspace, connection id or label.
#[derive(Serialize, ToSchema)]
pub struct PluginImpact {
    pub major_upgrade: bool,
    /// The currently active version of the package (`null` for a first install).
    pub from_version: Option<String>,
    pub connections_total: i64,
    pub connections_needing_reauth: i64,
    /// (phase 4, P4-GRANTS, C-412) Agent grants activation converts from full to custom access
    /// (`09 §10`): the sum of the per-app counts. A version with a non-zero count is held for an
    /// admin (`status_reason = converts_grants`). Absent when `0`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub grants_to_convert: i64,
    /// The apps of the new version, ordered by `app_id`.
    pub by_app: Vec<PluginImpactApp>,
}

/// `POST /v1/admin/plugins/{id}/approve` response (C-341): the package plus the number of
/// connections the activation flipped to `reauth_required`.
#[derive(Serialize, ToSchema)]
pub struct PluginApproval {
    #[serde(flatten)]
    pub package: PluginPackage,
    pub connections_reauth: i64,
}

#[derive(Serialize, ToSchema)]
pub struct PluginPackage {
    pub id: Uuid,
    pub package_id: String,
    pub version: String,
    pub contract: i32,
    pub publisher: String,
    pub digest: String,
    pub signer: PluginSigner,
    /// `builtin` | `directory` | `upload`.
    pub source: String,
    /// `pending_approval` | `active` | `superseded` | `rejected` | `disabled` | `invalid`.
    pub status: String,
    pub status_reason: Option<String>,
    pub violations: Vec<PluginViolation>,
    pub apps: Vec<PluginAppRef>,
    pub permissions: PluginPermissions,
    pub host_claims: Vec<PluginHostClaim>,
    pub widenings: Vec<PluginWidening>,
    /// Always present (C-340): set on `GET /v1/admin/plugins/{id}` for a `pending_approval`
    /// version, `null` otherwise and in lists.
    pub impact: Option<PluginImpact>,
    pub approved_by: Option<PluginUserRef>,
    pub approved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

fn widening(w: &Widening) -> PluginWidening {
    let hook = |h: &aurora_registry::manifest::Hook| {
        serde_json::to_value(h)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let (kind, detail) = match w {
        Widening::Hook(h) => ("hook", hook(h)),
        Widening::Egress { hook: h, host } => ("egress", format!("{} -> {host}", hook(h))),
        Widening::Context(c) => ("context", c.clone()),
        Widening::HostClaim(k) => ("host_claim", k.clone()),
        Widening::Zone(z) => ("zone", z.clone()),
        Widening::Library { app, host } => (
            "library",
            format!("{app} can serve a skill library: reads repository files from {host}"),
        ),
    };
    PluginWidening {
        kind: kind.to_owned(),
        detail,
    }
}

impl PluginPackage {
    pub fn from_row(r: PackageRow, widenings: &[Widening]) -> Self {
        let manifest = serde_json::from_value::<Manifest>(r.manifest.clone()).ok();
        let (kind, key_id) = match r.signer_key_id.as_str() {
            "builtin" => ("builtin", None),
            "unsigned" => ("unsigned", None),
            k => ("trusted_key", Some(k.to_owned())),
        };
        Self {
            apps: manifest
                .iter()
                .flat_map(|m| &m.apps)
                .map(|a| PluginAppRef {
                    app_id: a.about.id.clone(),
                    name: a.about.name.clone(),
                    library_host: a.library.as_ref().map(|l| l.request_host.clone()),
                })
                .collect(),
            host_claims: manifest
                .iter()
                .flat_map(claims::claims)
                .map(|c| PluginHostClaim {
                    path_prefix: (c.path_prefix != "/").then_some(c.path_prefix),
                    claim_key: c.claim_key,
                    host: c.host_pattern,
                    app_id: c.app_id,
                })
                .collect(),
            widenings: widenings.iter().map(widening).collect(),
            impact: None,
            permissions: serde_json::from_value(r.permissions).unwrap_or_default(),
            violations: serde_json::from_value(r.violations).unwrap_or_default(),
            signer: PluginSigner {
                kind: kind.to_owned(),
                key_id,
                name: r.signer_name,
            },
            approved_by: r.approved_by.map(|id| PluginUserRef {
                id,
                display_name: r.approved_by_name.unwrap_or_default(),
            }),
            id: r.id,
            package_id: r.package_id,
            version: r.version,
            contract: r.contract,
            publisher: r.publisher,
            digest: r.digest,
            source: r.source,
            status: r.status,
            status_reason: r.status_reason,
            approved_at: r.approved_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListPluginsQuery {
    /// Filter by package status.
    pub status: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// `multipart/form-data` body of the upload route.
#[derive(ToSchema)]
pub struct UploadPluginRequest {
    /// The `.aurora-plugin` archive (tar.gz), at most 20 MiB.
    #[schema(value_type = String, format = Binary)]
    pub package: Vec<u8>,
}

#[derive(Serialize, ToSchema)]
pub struct TrustedKey {
    pub id: Uuid,
    pub publisher: String,
    pub name: String,
    pub public_key: String,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<TrustedKeyRow> for TrustedKey {
    fn from(r: TrustedKeyRow) -> Self {
        Self {
            id: r.id,
            publisher: r.publisher,
            name: r.name,
            public_key: r.public_key,
            revoked_at: r.revoked_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTrustedKeyRequest {
    pub publisher: String,
    pub name: String,
    /// Base64 ed25519 public key.
    pub public_key: String,
}

#[derive(Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct PluginActionRequest {
    /// Free-text reason (at most 64 characters), stored as `status_reason`.
    pub reason: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeletePluginQuery {
    /// Orphan live connections instead of refusing.
    pub force: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct HostClaimMember {
    pub package_id: String,
    pub app_id: String,
    pub claim_key: String,
}

#[derive(Serialize, ToSchema)]
pub struct HostClaimDecision {
    pub winner_package_id: String,
    pub winner_app_id: String,
    pub decided_by: Option<PluginUserRef>,
    pub stale: bool,
}

#[derive(Serialize, ToSchema)]
pub struct HostClaimConflict {
    pub group_key: String,
    pub fingerprint: String,
    pub claims: Vec<HostClaimMember>,
    pub decision: Option<HostClaimDecision>,
}

#[derive(Serialize, ToSchema)]
pub struct HostClaimConflictList {
    pub items: Vec<HostClaimConflict>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HostClaimDecisionRequest {
    pub winner_package_id: String,
    pub winner_app_id: String,
    pub fingerprint: String,
}
