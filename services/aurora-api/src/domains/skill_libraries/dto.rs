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

/// Why a skill of a repository cannot be added, as a stable code (the free-text `reason` beside
/// it is for logs and older clients).
#[derive(Serialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillInvalidReason {
    /// Body plus files are over the skill's total size.
    TooLarge,
    BodyTooLarge,
    /// `SKILL.md` or another file is over the per-file limit.
    FileTooLarge,
    TooManyFiles,
    /// A file path or its characters.
    UnsupportedPath,
    /// A symlink, submodule, unusual file mode or entry type.
    UnsupportedEntry,
    /// `SKILL.md` is missing or misnamed, or its front matter, description or body is wrong.
    SkillMd,
    /// The description in `SKILL.md` is over the format's maximum.
    DescriptionTooLong,
    /// The folder name is not a valid skill name.
    BadName,
    /// It could not be judged within the limits of one read of the repository.
    UnknownSize,
}

impl SkillInvalidReason {
    pub const ALL: [Self; 10] = [
        Self::TooLarge,
        Self::BodyTooLarge,
        Self::FileTooLarge,
        Self::TooManyFiles,
        Self::UnsupportedPath,
        Self::UnsupportedEntry,
        Self::SkillMd,
        Self::DescriptionTooLong,
        Self::BadName,
        Self::UnknownSize,
    ];

    /// The stored and wire form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::BodyTooLarge => "body_too_large",
            Self::FileTooLarge => "file_too_large",
            Self::TooManyFiles => "too_many_files",
            Self::UnsupportedPath => "unsupported_path",
            Self::UnsupportedEntry => "unsupported_entry",
            Self::SkillMd => "skill_md",
            Self::DescriptionTooLong => "description_too_long",
            Self::BadName => "bad_name",
            Self::UnknownSize => "unknown_size",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

impl From<aurora_registry::skill_format::Kind> for SkillInvalidReason {
    fn from(k: aurora_registry::skill_format::Kind) -> Self {
        use aurora_registry::skill_format::Kind;
        match k {
            Kind::Frontmatter => Self::SkillMd,
            Kind::DescriptionTooLong => Self::DescriptionTooLong,
            Kind::BodyTooLarge => Self::BodyTooLarge,
            Kind::Path => Self::UnsupportedPath,
            Kind::TooManyFiles => Self::TooManyFiles,
            Kind::FileTooLarge => Self::FileTooLarge,
            Kind::TooLarge => Self::TooLarge,
        }
    }
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
    /// The name of the repository's default branch, for a library that sets no branch (null with
    /// a `ref`, before the first read that learned it, and for members).
    pub default_branch: Option<String>,
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
    /// An instance library, in its manager's view only: how many organizations have a skill
    /// added from it (never which skills).
    pub orgs_using: Option<i64>,
    /// (C-1176) Skill folders that can be added, at the last read of the repository (create,
    /// address change, browse or refresh); null before the first. Members see it too.
    pub skills_offered: Option<i32>,
    /// (C-1176) Skill folders that cannot be added (broken), at the same read.
    pub skills_unavailable: Option<i32>,
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
    /// The same as a code; non-null exactly when the skill is not valid.
    pub reason_code: Option<SkillInvalidReason>,
    /// Characters of body and files (binary as base64 length), what an add counts; null only
    /// for a skill that could not be judged (`unknown_size`).
    #[schema(required)]
    pub size: Option<u64>,
    pub files: usize,
    /// Where this library's version of the skill is already added.
    pub added: Vec<TierRef>,
    /// (C-1176) Why it cannot be added to the tier the browse is for (`?tier=`); null when it
    /// can, and without `?tier=`.
    pub blocked: Option<BlockedSkill>,
}

#[derive(Serialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockedReason {
    /// The tier already has a skill of this folder name.
    NameTaken,
    /// Adding it would pass the tier's library budget (size or number of skills).
    Budget,
    /// The skill is broken in the repository (the item's `reason`).
    Invalid,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct BlockedSkill {
    pub reason: BlockedReason,
    /// A plain sentence saying why.
    pub message: String,
}

/// (C-1176) The tier's budget for library skills, in characters of body and files.
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct LibraryBudget {
    pub bytes: usize,
    pub used: usize,
}

#[derive(Serialize, ToSchema)]
pub struct LibraryCatalogue {
    /// The commit the listing was read at: quote it when adding.
    pub commit_sha: String,
    pub items: Vec<LibrarySkillItem>,
    /// More than `LIBRARY_BROWSE_MAX` skills: the listing is cut; narrow the folder.
    pub truncated: bool,
    pub next_cursor: Option<String>,
    /// The tier's budget; null without `?tier=`.
    pub budget: Option<LibraryBudget>,
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
    /// Non-null exactly when the skill is not valid.
    pub reason_code: Option<SkillInvalidReason>,
    pub description: Option<String>,
    pub body: Option<String>,
    pub files: Vec<LibraryFile>,
    /// Characters of body and files (binary as base64 length): the budget unit.
    pub size: usize,
    /// Lowercase hex SHA-256 of the canonical content.
    pub content_sha256: Option<String>,
}
