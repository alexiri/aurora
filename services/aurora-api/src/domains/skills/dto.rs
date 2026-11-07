use super::service::SkillView;
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

#[derive(Serialize, ToSchema)]
pub struct Skill {
    pub id: Uuid,
    /// `agent` | `organization`.
    pub scope: &'static str,
    pub agent_id: Option<Uuid>,
    pub org_id: Uuid,
    pub name: String,
    pub description: String,
    pub body: String,
    pub enabled: bool,
    /// Metadata only; content is fetched per file.
    pub files: Vec<SkillFile>,
    /// Body plus files, in characters (binary counted as base64 length).
    pub total_size: usize,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl From<SkillView> for Skill {
    fn from(v: SkillView) -> Self {
        let r = v.row;
        Self {
            id: r.id,
            scope: if r.agent_id.is_some() {
                "agent"
            } else {
                "organization"
            },
            agent_id: r.agent_id,
            org_id: r.org_id,
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
