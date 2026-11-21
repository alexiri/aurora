//! Wire types of the git skill library (docs/05 §Skill libraries; C-570..C-579). Everything that
//! comes from a repository (names, descriptions, bodies, paths, reasons) is untrusted text: the
//! dashboard renders it as data.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema, Clone, Copy, Default)]
pub struct SkillCounts {
    pub agent: i64,
    pub workspace: i64,
    pub org: i64,
}

/// A skill library (C-1122). Managers see every field; other members who can use a workspace of
/// the org get `id`, `label`, `status` and `scope` of the active libraries (the browse picker).
/// The access token is never returned: only whether one is set and when.
#[derive(Serialize, ToSchema)]
pub struct SkillLibrary {
    pub id: Uuid,
    /// `org` | `instance`.
    pub scope: &'static str,
    /// Null for an instance library (and for members).
    pub org_id: Option<Uuid>,
    pub label: String,
    /// `active` | `paused` | `error` | `over_budget`.
    pub status: String,
    /// The normalized https clone URL; null for a library migrated without one.
    pub url: Option<String>,
    /// The display form `host/path` (without `.git`).
    pub repo: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: Option<String>,
    /// Whether an access token is saved (never the token).
    pub token_set: Option<bool>,
    pub token_updated_at: Option<DateTime<Utc>>,
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
    /// The repository's https address or its web page (C-1123).
    pub url: String,
    /// Branch, tag or commit; default: from a page URL, else the default branch.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// Directory that holds the skill directories (at most 4 segments); default: from a page
    /// URL, else the root.
    pub path: Option<String>,
    /// A read-only access token for a private repository (1..1024 printable ASCII).
    pub token: Option<String>,
    /// The Basic-auth user name sent with the token; default `x-access-token`.
    pub token_username: Option<String>,
}

/// `POST …/skill-libraries/check` (C-1124): the create body without `label`.
#[derive(Deserialize, ToSchema)]
pub struct CheckSkillLibraryRequest {
    pub url: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: Option<String>,
    /// Used for this one read and dropped.
    pub token: Option<String>,
    pub token_username: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ResolvedRef {
    /// `branch` | `tag` | `commit`.
    pub kind: &'static str,
    pub name: String,
    /// Whether this is the repository's default branch.
    pub default: bool,
}

#[derive(Serialize, ToSchema)]
pub struct CheckSkills {
    pub found: usize,
    pub valid: usize,
    /// The first 20 valid names, sorted (untrusted text).
    pub names: Vec<String>,
}

/// What a check found (C-1124); nothing was stored.
#[derive(Serialize, ToSchema)]
pub struct LibraryCheck {
    pub url: String,
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub resolved: ResolvedRef,
    pub commit_sha: String,
    pub path: String,
    pub skills: CheckSkills,
    /// More than `LIBRARY_BROWSE_MAX` skill directories: only the first were read.
    pub truncated: bool,
    pub suggested_label: String,
}

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize, ToSchema, Default)]
pub struct PatchSkillLibraryRequest {
    pub label: Option<String>,
    /// `active` | `paused`.
    pub status: Option<String>,
    /// Send an empty string to clear the ref.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: Option<String>,
    /// A new token (set or replace; the repository is read with it first) or `null` (remove).
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub token: Option<Option<String>>,
    pub token_username: Option<String>,
    /// Only while the stored address is null (a migrated library); else 409 `library_url_fixed`.
    pub url: Option<String>,
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
