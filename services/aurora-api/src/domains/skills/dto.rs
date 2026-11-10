use super::service::{self, SkillView};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct SkillFile {
    pub path: String,
    /// Characters for text, base64 length for binary content.
    pub size: usize,
    pub executable: bool,
}

/// An agent that overrides a workspace skill with an enabled agent-tier skill of the same name.
#[derive(Serialize, ToSchema, Clone)]
pub struct OverriddenAgent {
    pub agent: AgentRef,
    pub skill_id: Uuid,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct AgentRef {
    pub id: Uuid,
    pub handle: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct Skill {
    pub id: Uuid,
    /// `agent` | `workspace` | `org`.
    pub tier: &'static str,
    /// `agent` | `workspace` | `organization` (kept for compatibility, D-107).
    pub scope: &'static str,
    pub agent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub org_id: Uuid,
    pub name: String,
    pub description: String,
    pub body: String,
    pub enabled: bool,
    /// Metadata only; content is fetched per file.
    pub files: Vec<SkillFile>,
    /// Body plus files, in characters (binary counted as base64 length).
    pub total_size: usize,
    /// Workspace tier only: the agents of the workspace whose own enabled skill of this name
    /// overrides it; null for the other tiers.
    pub overridden_in: Option<Vec<OverriddenAgent>>,
    /// (phase 4, P4-GSL) where a library skill came from; null for an ordinary skill.
    pub source: Option<SkillSource>,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Provenance of a skill added from a git skill library (`10 §Git skill library`). Repository
/// text: untrusted, shown as data.
#[derive(Serialize, ToSchema, Clone)]
pub struct SkillSource {
    /// Always `library`.
    pub kind: &'static str,
    /// Null once the skill was detached or its library deleted.
    pub library_id: Option<Uuid>,
    pub label: Option<String>,
    pub repo: Option<String>,
    pub path: Option<String>,
    pub commit_sha: Option<String>,
    pub pinned_at: Option<DateTime<Utc>>,
    /// The library link is gone (detach or library deletion): an ordinary editable skill.
    pub detached: bool,
    pub update_pending: bool,
}

pub fn source_of(
    r: &aurora_db::repo::skills::SkillRow,
    update_pending: bool,
) -> Option<SkillSource> {
    r.library_commit_sha.as_ref()?;
    Some(SkillSource {
        kind: "library",
        library_id: r.library_id,
        label: r.library_label.clone(),
        repo: r.library_repo.clone(),
        path: r.library_path.clone(),
        commit_sha: r.library_commit_sha.clone(),
        pinned_at: r.library_pinned_at,
        detached: r.library_id.is_none(),
        update_pending: r.library_id.is_some() && update_pending,
    })
}

impl From<SkillView> for Skill {
    fn from(v: SkillView) -> Self {
        let source = source_of(&v.row, v.update_pending);
        let r = v.row;
        let tier = service::tier_of(&r);
        Self {
            source,
            id: r.id,
            tier,
            scope: if tier == "org" { "organization" } else { tier },
            agent_id: r.agent_id,
            workspace_id: r.workspace_id,
            org_id: r.org_id,
            overridden_in: None,
            name: r.name,
            description: r.description,
            body: r.body,
            enabled: r.enabled,
            files: v
                .files
                .into_iter()
                .map(|f| SkillFile {
                    path: f.path,
                    size: f.size,
                    executable: f.executable,
                })
                .collect(),
            total_size: v.total_size,
            updated_at: r.updated_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Serialize, ToSchema, Clone)]
pub struct TierRef {
    pub id: Uuid,
    pub tier: &'static str,
}

#[derive(Serialize, ToSchema)]
pub struct EffectiveSkill {
    pub skill: Skill,
    pub tier: &'static str,
    /// Always true: a disabled skill never wins.
    pub enabled: bool,
    /// Every lower-tier skill of this name (enabled or not).
    pub shadows: Vec<TierRef>,
}

#[derive(Serialize, ToSchema)]
pub struct ShadowedSkill {
    pub skill: Skill,
    pub tier: &'static str,
    pub enabled: bool,
    /// The winner of this name; null for a disabled skill whose name has no enabled skill.
    pub shadowed_by: Option<TierRef>,
}

#[derive(Serialize, ToSchema)]
pub struct EffectiveSkills {
    pub items: Vec<EffectiveSkill>,
    pub shadowed: Vec<ShadowedSkill>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateSkillRequest {
    /// Immutable kebab-case directory name, at most 64 characters; `aurora-*` is reserved.
    /// Required unless `from_library` is given.
    pub name: Option<String>,
    /// Single line, at most 500 characters. Required unless `from_library` is given.
    pub description: Option<String>,
    /// Required unless `from_library` is given.
    pub body: Option<String>,
    pub enabled: Option<bool>,
    /// (phase 4, P4-GSL, D-148) Add a skill from a git skill library instead of a body: the
    /// server reads exactly that commit and stores the skill pinned and read-only. Excludes
    /// `name`, `description` and `body`.
    pub from_library: Option<FromLibrary>,
}

/// Which library skill to add (`10 §Git skill library`).
#[derive(Deserialize, ToSchema, Clone)]
pub struct FromLibrary {
    pub library_id: Uuid,
    /// The skill's directory name under the library path, as browse lists it.
    pub path: String,
    /// The commit the person saw in browse. Required: a later push is never fetched silently.
    pub commit_sha: Option<String>,
}

impl CreateSkillRequest {
    /// The manual-creation fields, or the field errors when one is missing.
    pub fn manual(&self) -> Result<(&str, &str, &str), aurora_core::ApiError> {
        match (&self.name, &self.description, &self.body) {
            (Some(n), Some(d), Some(b)) => Ok((n, d, b)),
            _ => Err(aurora_core::ApiError::invalid_request(
                "Invalid skill.",
                vec![aurora_core::FieldError::new(
                    "/name",
                    "name, description and body are required unless from_library is given",
                )],
            )),
        }
    }

    /// `from_library` excludes the manual fields.
    pub fn check_exclusive(&self) -> Result<(), aurora_core::ApiError> {
        if self.from_library.is_some()
            && (self.name.is_some() || self.description.is_some() || self.body.is_some())
        {
            return Err(aurora_core::ApiError::invalid_request(
                "Invalid skill.",
                vec![aurora_core::FieldError::new(
                    "/from_library",
                    "cannot be combined with name, description or body",
                )],
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateSkillRequest {
    pub description: Option<String>,
    pub body: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FileParams {
    /// Executable bit (projected 0555, else 0444). Omitted keeps the current bit.
    pub executable: Option<bool>,
}
