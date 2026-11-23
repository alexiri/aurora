//! The shapes of the template-source routes (C-1107). Built from stored rows, never a live
//! read; the token is never in any of them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// One C-1141 problem, as data (`code` is the fixed code, `path` where it applies).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TemplateProblemView {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl From<&aurora_template::TemplateProblem> for TemplateProblemView {
    fn from(p: &aurora_template::TemplateProblem) -> Self {
        Self {
            code: p.code.as_str().to_owned(),
            path: p.path.clone(),
            message: p.message.clone(),
        }
    }
}

/// What a ref resolved to (C-1107, as C-1124's `resolved`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ResolvedRefView {
    /// `branch` | `tag` | `commit`.
    pub kind: String,
    pub name: String,
    /// Whether this is the repository's default branch.
    pub default: bool,
}

/// The state of one template of a source (C-1107).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SourceTemplateView {
    pub template_id: String,
    pub dir: String,
    pub name: String,
    pub summary: String,
    pub head_version: Option<String>,
    /// `ok` | `needs_newer_aurora` | `invalid` (the head's state).
    pub head_state: String,
    /// `new` | `ingested` | `id_taken` | `invalid`.
    pub state: String,
}

/// A template source (C-1107): the stored rows, never a live read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AgentTemplateSource {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    /// The repository's display form (`host[:port]/path`, as C-1122 shows it).
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: String,
    /// Whether a sealed access token is set (the value is never returned).
    pub token_set: bool,
    pub token_updated_at: Option<DateTime<Utc>>,
    pub homepage: Option<String>,
    pub contact: Option<String>,
    /// The manifest's `packages.url` template.
    pub packages_url: String,
    pub update_interval: String,
    /// `ok` | `check_failed`.
    pub status: String,
    pub check_failed_at: Option<DateTime<Utc>>,
    pub check_failed_reason: Option<String>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_head_sha: Option<String>,
    pub templates: Vec<SourceTemplateView>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AgentTemplateSourceList {
    pub items: Vec<AgentTemplateSource>,
}

/// The repository part of a manifest (C-1103).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ManifestRepoView {
    pub name: String,
    pub description: String,
}

/// One template the preview found in the folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PreviewTemplateView {
    pub template_id: String,
    pub dir: String,
    pub name: String,
    pub summary: String,
    pub version: Option<String>,
    /// `new` | `id_taken` (a stored row's states cannot apply to a source that is not saved).
    pub state: String,
    pub problems: Vec<TemplateProblemView>,
}

/// What a preview read found (C-1107): check without saving, nothing stored, no audit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AgentTemplateSourcePreview {
    /// The normalized address.
    pub url: String,
    /// The repository's display form.
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub resolved: ResolvedRefView,
    pub commit_sha: String,
    pub path: String,
    pub manifest: ManifestRepoView,
    pub templates: Vec<PreviewTemplateView>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewTemplateSourceRequest {
    pub url: String,
    /// A branch, tag or commit; default the repository's default branch.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// The folder in the repository; default the root.
    pub path: Option<String>,
    /// An access token used for this read only (never stored).
    pub token: Option<String>,
    pub token_username: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTemplateSourceRequest {
    pub url: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: Option<String>,
    /// A read-only access token, sealed (C-1126); `token_username` defaults to `x-access-token`.
    pub token: Option<String>,
    pub token_username: Option<String>,
    /// `6h|12h|1d|1w|2w|1mo|never`; default `agents.templates.update_interval`.
    pub update_interval: Option<String>,
}

/// Absent = unchanged, `null` = removed, a string = replaced (the agents' patch convention).
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    // A field present with any value (including `null`) lands here: the inner option is the
    // value's own optionality.
    Ok(Some(serde::de::Deserialize::deserialize(d)?))
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTemplateSourceRequest {
    /// `6h|12h|1d|1w|2w|1mo|never`.
    pub update_interval: Option<String>,
    /// A branch, tag or commit; `""` clears (the default branch again).
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// The folder in the repository.
    pub path: Option<String>,
    /// A new token (replaces), or `null` (removes); absent = unchanged.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub token: Option<Option<String>>,
    pub token_username: Option<String>,
}
