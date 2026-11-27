use super::service::{self, SkillView};
use crate::domains::conversations::reference::ConversationRef;
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
    /// The folder name (immutable).
    pub name: String,
    /// (C-1173) The display name, at most 80 characters of plain text; null for a skill
    /// without one (library and app skills have none).
    pub title: Option<String>,
    pub description: String,
    pub body: String,
    pub enabled: bool,
    /// (C-1175) Workspace and organization skills: how many distinct agents used it in the last
    /// 30 days; null for an agent's own skill.
    pub used_by_agents: Option<i64>,
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
            title: r.title,
            description: r.description,
            body: r.body,
            enabled: r.enabled,
            used_by_agents: None,
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

/// A tier (or app) that holds or wins a name. `id` is null for an app skill.
#[derive(Serialize, ToSchema, Clone)]
pub struct TierRef {
    pub id: Option<Uuid>,
    /// `agent` | `workspace` | `org` | `app`.
    pub tier: &'static str,
    /// The app, when `tier` is `app`.
    pub app_id: Option<String>,
}

/// (P5-APPSKILL-2, C-1133) The app a skill comes from.
#[derive(Serialize, ToSchema, Clone)]
pub struct AppRef {
    pub app_id: String,
    pub name: String,
    pub version: String,
}

/// A skill as the effective set shows it: the fields of [`Skill`], with the ones an app skill has
/// no value for nullable (`id`, `org_id`, the timestamps).
#[derive(Serialize, ToSchema)]
pub struct EffectiveBody {
    /// Null for an app skill.
    pub id: Option<Uuid>,
    /// `agent` | `workspace` | `org` | `app`.
    pub tier: &'static str,
    /// `agent` | `workspace` | `organization` | `app`.
    pub scope: &'static str,
    pub agent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub org_id: Option<Uuid>,
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub body: String,
    pub enabled: bool,
    pub used_by_agents: Option<i64>,
    pub files: Vec<SkillFile>,
    pub total_size: usize,
    pub overridden_in: Option<Vec<OverriddenAgent>>,
    pub source: Option<SkillSource>,
    pub updated_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
}

impl From<Skill> for EffectiveBody {
    fn from(s: Skill) -> Self {
        Self {
            id: Some(s.id),
            tier: s.tier,
            scope: s.scope,
            agent_id: s.agent_id,
            workspace_id: s.workspace_id,
            org_id: Some(s.org_id),
            name: s.name,
            title: s.title,
            description: s.description,
            body: s.body,
            enabled: s.enabled,
            used_by_agents: s.used_by_agents,
            files: s.files,
            total_size: s.total_size,
            overridden_in: s.overridden_in,
            source: s.source,
            updated_at: Some(s.updated_at),
            created_at: Some(s.created_at),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct EffectiveSkill {
    pub skill: EffectiveBody,
    pub tier: &'static str,
    /// Always true: a disabled skill never wins.
    pub enabled: bool,
    /// Every lower-tier skill of this name (enabled or not), app skills included.
    pub shadows: Vec<TierRef>,
    /// The app of an app skill; null for the other tiers.
    pub app: Option<AppRef>,
    /// (C-1174) Always false: a skill turned off for the agent is never in effect (it is in
    /// `shadowed`).
    pub off_for_agent: bool,
    /// (C-1175) This agent's use of the skill; null for an app skill.
    pub usage: Option<SkillUsage>,
}

/// Why a skill of the agent's tiers is not in effect.
#[derive(Serialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotAppliedReason {
    /// Another skill of the same name wins (`shadowed_by`).
    Replaced,
    /// The skill's owners switched it off.
    Disabled,
    /// Turned off for this agent only (C-1174).
    OffForAgent,
}

#[derive(Serialize, ToSchema)]
pub struct ShadowedSkill {
    pub skill: EffectiveBody,
    pub tier: &'static str,
    pub enabled: bool,
    /// The skill that is in effect under this name; null when none is (every skill of the name
    /// is switched off or turned off for the agent).
    pub shadowed_by: Option<TierRef>,
    /// The app of an app skill; null for the other tiers.
    pub app: Option<AppRef>,
    /// (C-1174) Turned off for this agent (it may also be switched off by its owners: `enabled`).
    pub off_for_agent: bool,
    /// `disabled` when its owners switched it off, else `off_for_agent`, else `replaced`.
    pub reason: NotAppliedReason,
    /// (C-1175) This agent's use of the skill; null for an app skill.
    pub usage: Option<SkillUsage>,
}

/// (C-1175) An agent's use of one skill.
#[derive(Serialize, ToSchema, Clone, Default)]
pub struct SkillUsage {
    /// How many turns used it in the last 7 days.
    pub week: i64,
    /// The latest use within the 90 days uses are kept; null: not used.
    pub last: Option<SkillLastUse>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct SkillLastUse {
    pub at: DateTime<Utc>,
    /// Null when the caller may not read that conversation, or it no longer exists.
    pub conversation: Option<ConversationRef>,
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
    /// The display name: one line of plain text, at most 80 characters; blank or absent means
    /// none.
    pub title: Option<String>,
    /// Single line, at most 500 characters. Required unless `from_library` is given.
    pub description: Option<String>,
    /// Required unless `from_library` is given.
    pub body: Option<String>,
    pub enabled: Option<bool>,
    /// (phase 4, P4-GSL, D-148) Add a skill from a git skill library instead of a body: the
    /// server reads exactly that commit and stores the skill pinned and read-only. Excludes
    /// `name`, `title`, `description` and `body`.
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
            && (self.name.is_some()
                || self.title.is_some()
                || self.description.is_some()
                || self.body.is_some())
        {
            return Err(aurora_core::ApiError::invalid_request(
                "Invalid skill.",
                vec![aurora_core::FieldError::new(
                    "/from_library",
                    "cannot be combined with name, title, description or body",
                )],
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateSkillRequest {
    /// The display name; `""` clears it, absent keeps it.
    pub title: Option<String>,
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
