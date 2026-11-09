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
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl From<SkillView> for Skill {
    fn from(v: SkillView) -> Self {
        let r = v.row;
        let tier = service::tier_of(&r);
        Self {
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
    pub name: String,
    /// Single line, at most 500 characters.
    pub description: String,
    pub body: String,
    pub enabled: Option<bool>,
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
