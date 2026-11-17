use aurora_db::repo::user_tokens::{GrantRow, UserTokenRow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct UserTokenIntegration {
    pub id: Uuid,
    /// The connector id of the integration.
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserTokenAgent {
    pub id: Uuid,
    pub name: String,
}

/// A person's chat-platform token (C-855). The value is never returned.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserToken {
    pub id: Uuid,
    pub integration: UserTokenIntegration,
    pub label: Option<String>,
    /// The platform account the token belongs to (its handle), never the platform user id.
    pub platform_user_label: String,
    /// `active` | `invalid` | `revoked`
    pub status: String,
    /// The agents the owner granted this token to.
    pub agents: Vec<UserTokenAgent>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl UserToken {
    pub fn build(r: UserTokenRow, grants: &[GrantRow]) -> Self {
        let agents = grants
            .iter()
            .filter(|g| g.user_token_id == r.id)
            .map(|g| UserTokenAgent {
                id: g.agent_id,
                name: g.agent_name.clone(),
            })
            .collect();
        Self {
            id: r.id,
            integration: UserTokenIntegration {
                id: r.integration_id,
                kind: r.connector_id,
            },
            label: r.label,
            platform_user_label: r.platform_user_label,
            status: r.status,
            agents,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserTokenList {
    pub tokens: Vec<UserToken>,
}

/// `POST /v1/me/user-tokens` (C-853). The `token` is a personal access token of the person's own
/// chat-platform account; it is sealed at once and never returned or logged.
#[derive(Deserialize, ToSchema)]
pub struct CreateUserTokenRequest {
    pub integration_id: Uuid,
    /// At most 256 characters.
    pub token: String,
    /// At most 80 characters.
    pub label: Option<String>,
}

impl std::fmt::Debug for CreateUserTokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateUserTokenRequest")
            .field("integration_id", &self.integration_id)
            .field("token", &"[redacted]")
            .field("label", &self.label)
            .finish()
    }
}

/// A post an agent proposed to make as the caller (C-856), with the exact text the caller is
/// asked to approve. Only the token's owner ever receives this.
#[derive(Debug, Serialize, ToSchema)]
pub struct OwnSendApproval {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub token_id: Uuid,
    /// Platform-composed: "Post as <you> in <conversation> (<n> characters)".
    pub summary: String,
    /// The full text, exactly as it will be posted (the footer naming the agent is added).
    pub text: String,
    /// `pending` | `approved` | `executed` | `failed` | `rejected` | `expired`
    pub status: String,
    pub error: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl OwnSendApproval {
    pub fn build(r: aurora_db::repo::action_approvals::ActionApprovalRow) -> Option<Self> {
        let text = r.payload.get("text")?.as_str()?.to_owned();
        let token_id = r.payload.get("token_id")?.as_str()?.parse().ok()?;
        Some(Self {
            id: r.id,
            workspace_id: r.workspace_id,
            agent_id: r.agent_id,
            token_id,
            summary: r.summary,
            text,
            status: r.status,
            error: r.error,
            expires_at: r.expires_at,
            created_at: r.created_at,
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OwnSendApprovalList {
    pub items: Vec<OwnSendApproval>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OwnSendQuery {
    /// `pending` | `approved` | `executed` | `failed` | `rejected` | `expired`
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchUserTokenRequest {
    /// At most 80 characters; empty or `null` clears it.
    pub label: Option<String>,
}
