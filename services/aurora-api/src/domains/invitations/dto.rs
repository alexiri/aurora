use crate::domains::tenancy::dto::{Org, OrgSummary};
use aurora_db::repo::invitations::InvitationRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InvitedRole {
    Admin,
    Member,
}

impl InvitedRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InvitationStatus {
    Pending,
    Accepted,
    Cancelled,
    Expired,
}

impl InvitationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "accepted" => Self::Accepted,
            "cancelled" => Self::Cancelled,
            "expired" => Self::Expired,
            _ => Self::Pending,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct InviterRef {
    pub id: Uuid,
    pub display_name: String,
}

/// An invitation. The token is never part of any response.
#[derive(Serialize, ToSchema)]
pub struct Invitation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub role: InvitedRole,
    pub workspace_ids: Vec<Uuid>,
    pub status: InvitationStatus,
    pub invited_by: Option<InviterRef>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl From<InvitationRow> for Invitation {
    fn from(r: InvitationRow) -> Self {
        Self {
            id: r.id,
            org_id: r.org_id,
            email: r.email,
            role: if r.role == "admin" {
                InvitedRole::Admin
            } else {
                InvitedRole::Member
            },
            workspace_ids: r.workspace_ids,
            status: InvitationStatus::parse(&r.status),
            invited_by: r.invited_by.map(|id| InviterRef {
                id,
                display_name: r.invited_by_name.unwrap_or_default(),
            }),
            expires_at: r.expires_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct CreateInvitationRequest {
    pub email: String,
    pub role: InvitedRole,
    #[serde(default)]
    pub workspace_ids: Vec<Uuid>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListInvitationsQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub status: Option<InvitationStatus>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PreviewQuery {
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct InvitationPreview {
    pub org: OrgSummary,
    pub email: String,
    pub role: InvitedRole,
    pub inviter_name: Option<String>,
    pub status: InvitationStatus,
    pub expires_at: DateTime<Utc>,
}

#[derive(Deserialize, ToSchema)]
pub struct AcceptInvitationRequest {
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct AcceptInvitationResponse {
    pub org: Org,
    pub workspace_ids: Vec<Uuid>,
}
