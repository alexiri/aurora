use aurora_config::{ApiConfig, OrgCreation, SignupMode};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct OidcProviderInfo {
    pub id: String,
    pub display_name: String,
    /// (phase 4, P4-GRP-1, D-156) The provider has a `group_claim`: IdP-linked groups may name
    /// it. Absent (false) otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub groups: bool,
}

#[derive(Serialize, ToSchema)]
pub struct Features {
    pub hosted: bool,
    pub channels: bool,
}

#[derive(Serialize, ToSchema)]
pub struct InstanceInfo {
    pub version: String,
    /// `open` | `invite_only` | `sso_jit`.
    pub signup_mode: String,
    /// `instance_admins` | `anyone`.
    pub org_creation: String,
    pub oidc_providers: Vec<OidcProviderInfo>,
    pub features: Features,
    pub default_agent_name: String,
    pub external_url: String,
    /// `smtp` | `log`.
    pub email_delivery: String,
    /// (phase 2, RFE-011a) `smtp` (approval e-mails are sent), `log` (SMTP unset: they are only
    /// logged; the admin overview warns "approval e-mails disabled") or `disabled`
    /// (`approvals.email_notifications = false`).
    pub approval_email: String,
    pub setup_errors: Vec<String>,
}

pub fn instance_info(cfg: &ApiConfig) -> InstanceInfo {
    InstanceInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        signup_mode: match cfg.signup_mode {
            SignupMode::Open => "open",
            SignupMode::InviteOnly => "invite_only",
            SignupMode::SsoJit => "sso_jit",
        }
        .to_owned(),
        org_creation: match cfg.org_creation {
            OrgCreation::InstanceAdmins => "instance_admins",
            OrgCreation::Anyone => "anyone",
        }
        .to_owned(),
        oidc_providers: cfg
            .auth
            .oidc_providers
            .iter()
            .map(|p| OidcProviderInfo {
                id: p.id.clone(),
                display_name: p.display_name.clone().unwrap_or_else(|| p.id.clone()),
                groups: p.group_claim.is_some(),
            })
            .collect(),
        features: Features {
            hosted: cfg.hosted.enabled,
            channels: false,
        },
        default_agent_name: cfg.agents.default_name.clone(),
        external_url: cfg.external_url.clone(),
        email_delivery: if cfg.email.smtp_url.is_some() {
            "smtp"
        } else {
            "log"
        }
        .to_owned(),
        approval_email: if !cfg.approvals.email_notifications {
            "disabled"
        } else if cfg.email.smtp_url.is_some() {
            "smtp"
        } else {
            "log"
        }
        .to_owned(),
        setup_errors: vec![],
    }
}
