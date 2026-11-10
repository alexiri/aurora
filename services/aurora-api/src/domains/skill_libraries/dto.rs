//! Wire types of the git skill library (docs/05 §Skill libraries; C-570..C-579). Everything that
//! comes from a repository (names, descriptions, bodies, paths, reasons) is untrusted text: the
//! dashboard renders it as data.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema, Clone)]
pub struct ConnectionRef {
    pub id: Uuid,
    pub label: String,
    /// `active`, `reauth_required`, `disabled`, …
    pub status: String,
}

#[derive(Serialize, ToSchema, Clone, Copy, Default)]
pub struct SkillCounts {
    pub agent: i64,
    pub workspace: i64,
    pub org: i64,
}

/// A skill library. Org owners/admins see every field; other members who can use a workspace of
/// the org get `id`, `label` and `status` of the active libraries (the browse picker).
#[derive(Serialize, ToSchema)]
pub struct SkillLibrary {
    pub id: Uuid,
    pub label: String,
    /// `active` | `paused` | `error` | `over_budget`.
    pub status: String,
    pub app_id: Option<String>,
    /// Null when the connection was deleted.
    pub connection: Option<ConnectionRef>,
    pub repo: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: Option<String>,
    pub last_head_sha: Option<String>,
    pub last_refresh_at: Option<DateTime<Utc>>,
    /// Sanitized, at most 200 characters; never a token or a URL with credentials.
    pub last_error: Option<String>,
    pub skill_counts: Option<SkillCounts>,
    pub pending_updates: Option<i64>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateSkillLibraryRequest {
    /// 1 to 80 characters, unique in the organization.
    pub label: String,
    /// An **org-scoped**, active connection of an app that declares a `library` block.
    pub connection_id: Uuid,
    /// `owner/name` (the package's `repo_pattern`).
    pub repo: String,
    /// Branch, tag or commit; default the repository's default branch.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// Directory that holds the skill directories (at most 4 segments); default the root.
    pub path: Option<String>,
}

#[derive(Deserialize, ToSchema, Default)]
pub struct PatchSkillLibraryRequest {
    pub label: Option<String>,
    /// `active` | `paused`.
    pub status: Option<String>,
    pub connection_id: Option<Uuid>,
    /// `null` is not distinguishable from absent; send an empty string to clear the ref.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: Option<String>,
}

/// Where a library skill is already added (the same `{ id, tier }` shape as the skills' `TierRef`).
#[derive(Serialize, ToSchema, Clone)]
#[schema(as = LibrarySkillPlacement)]
pub struct TierRef {
    pub tier: &'static str,
    pub id: Uuid,
}

#[derive(Serialize, ToSchema)]
pub struct LibrarySkillItem {
    /// The skill's directory name under the library path.
    pub path: String,
    pub name: String,
    pub description: Option<String>,
    pub valid: bool,
    /// Why an invalid skill cannot be added (sanitized fixed texts).
    pub reason: Option<String>,
    /// Characters of body and files (an estimate before the files are read).
    pub size: u64,
    pub files: usize,
    /// Where this library's version of the skill is already added.
    pub added: Vec<TierRef>,
}

#[derive(Serialize, ToSchema)]
pub struct LibraryCatalogue {
    /// The commit the listing was read at: quote it when adding.
    pub commit_sha: String,
    pub items: Vec<LibrarySkillItem>,
    /// More than `LIBRARY_BROWSE_MAX` skills: the listing is cut; narrow the folder.
    pub truncated: bool,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct LibraryFile {
    pub path: String,
    pub size: usize,
    pub executable: bool,
    pub binary: bool,
    /// Text content (UTF-8 files); null for binary files.
    pub content: Option<String>,
    /// Base64 content of a binary file.
    pub content_b64: Option<String>,
}

/// One skill at one commit: exactly what an add would store.
#[derive(Serialize, ToSchema)]
pub struct LibrarySkillContent {
    pub commit_sha: String,
    pub path: String,
    pub name: String,
    pub valid: bool,
    pub reason: Option<String>,
    pub description: Option<String>,
    pub body: Option<String>,
    pub files: Vec<LibraryFile>,
    /// Characters of body and files (binary as base64 length): the budget unit.
    pub size: usize,
    /// Lowercase hex SHA-256 of the canonical content.
    pub content_sha256: Option<String>,
}
