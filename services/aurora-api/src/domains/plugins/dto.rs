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
    /// (P5-REFINE-6, C-1071) The violation in words; absent on a package stored before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentence: Option<String>,
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
    /// `builtin` | `directory` | `upload` | `addon` (P5-ASRC).
    pub source: String,
    /// (P5-ASRC, C-600) the addon source row for `source = "addon"`.
    pub source_id: Option<Uuid>,
    /// (P5-POLISH-3, C-1032) Where the package came from, in the words the list shows: `aurora` (shipped
    /// with Aurora), `uploaded`, `directory` or `addon`.
    pub origin: String,
    /// (C-1032) The hosts the package's apps reach (host claims, deduplicated, sorted), at most five.
    pub reaches: Vec<String>,
    /// (C-1032) How many more hosts there are than `reaches` lists.
    pub reaches_more: u32,
    /// (C-1032, C-1031) The first app's `about.summary`, for the list's one-line description.
    pub summary: Option<String>,
    /// (P5-ASRC, C-600) the addon source's name at read time (never stored).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    /// (P5-ASRC, C-600) a manifest version of the same package is newer than this row.
    #[serde(default)]
    pub update_available: bool,
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
    /// (P5-APPSKILL-2, C-1136) What agents that use each app get from the package: one entry per
    /// app, in manifest order. Filled on `GET /v1/admin/plugins/{id}` (it reads the stored archive
    /// for the digests); empty in lists and in the responses of the lifecycle routes.
    #[serde(default)]
    pub agent: Vec<PluginAgent>,
}

/// (C-1136) What agents of one app get from a package (`docs/24 §9`).
#[derive(Serialize, ToSchema)]
pub struct PluginAgent {
    pub app_id: String,
    pub summary: Option<String>,
    /// The essentials (`agent.instructions`); third-party text, shown as plain text only.
    pub instructions: String,
    /// The operations file; `null` without one.
    pub operations: Option<PluginAgentOperations>,
    /// The app's skills, by name. Scripts are marked by `executable` on their files.
    pub skills: Vec<PluginAgentSkill>,
    /// Against the active version of the same package; `null` when there is none or this is it.
    pub changed: Option<PluginAgentChanged>,
}

#[derive(Serialize, ToSchema)]
pub struct PluginAgentOperations {
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub count: u32,
}

/// An app skill: `size` is `SKILL.md` and every file together in bytes; `files` lists `SKILL.md`
/// first, then the other files by path.
#[derive(Serialize, ToSchema)]
pub struct PluginAgentSkill {
    pub name: String,
    pub description: String,
    pub size: u64,
    pub files: Vec<PluginAgentSkillFile>,
    /// (FIX-221, D-465(d)) Present only when no agent gets this skill because the name belongs to
    /// an app of another package (it was installed after this one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<PluginAgentSkillClaim>,
}

/// The app that owns a skill's name.
#[derive(Serialize, ToSchema)]
pub struct PluginAgentSkillClaim {
    pub app_id: String,
    pub app_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct PluginAgentSkillFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
}

#[derive(Serialize, ToSchema)]
pub struct PluginAgentChanged {
    pub summary: bool,
    pub instructions: bool,
    pub operations: bool,
    pub skills: Vec<PluginAgentSkillChange>,
}

#[derive(Serialize, ToSchema)]
pub struct PluginAgentSkillChange {
    pub name: String,
    /// `added` | `changed` | `removed`.
    pub change: String,
    pub files: Vec<PluginAgentFileChange>,
}

#[derive(Serialize, ToSchema)]
pub struct PluginAgentFileChange {
    pub path: String,
    /// `added` | `changed` | `removed` | `mode` (the executable bit differs; listed next to `changed` when the content changed too).
    pub change: String,
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
        Widening::TokenHost(h) => ("token_host", h.clone()),
        Widening::Directory {
            app,
            server,
            base_dn,
        } => (
            "directory",
            format!("{app} can read directory entries from {server} under {base_dn}"),
        ),
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

/// Hosts listed in `reaches` before the rest are only counted.
const REACHES_MAX: usize = 5;

impl PluginPackage {
    pub fn from_row(r: PackageRow, widenings: &[Widening]) -> Self {
        let manifest = serde_json::from_value::<Manifest>(r.manifest.clone()).ok();
        let (kind, key_id) = match r.signer_key_id.as_str() {
            "builtin" => ("builtin", None),
            "unsigned" => ("unsigned", None),
            k => ("trusted_key", Some(k.to_owned())),
        };
        let mut hosts: Vec<String> = manifest
            .iter()
            .flat_map(claims::claims)
            .map(|c| c.host_pattern)
            .collect();
        hosts.sort();
        hosts.dedup();
        let reaches_more =
            u32::try_from(hosts.len().saturating_sub(REACHES_MAX)).unwrap_or(u32::MAX);
        hosts.truncate(REACHES_MAX);
        let origin = match r.source.as_str() {
            "builtin" => "aurora",
            "upload" => "uploaded",
            other => other,
        }
        .to_owned();
        let summary = manifest
            .iter()
            .flat_map(|m| &m.apps)
            .find_map(|a| a.about.summary.clone());
        Self {
            origin,
            summary,
            reaches: hosts,
            reaches_more,
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
            source_id: r.source_id,
            source_name: None,
            update_available: false,
            status: r.status,
            status_reason: r.status_reason,
            approved_at: r.approved_at,
            created_at: r.created_at,
            agent: Vec::new(),
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListPluginsQuery {
    /// Filter by package status.
    pub status: Option<String>,
    /// Exact match on the package id (C-1120); no match is an empty page.
    pub package_id: Option<String>,
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
