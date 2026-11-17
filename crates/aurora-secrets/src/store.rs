use async_trait::async_trait;
use aurora_core::Secret;
use uuid::Uuid;

use crate::{Result, SecretRef};

/// The six credential purposes of 08 §1 (AAD purpose strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    SecretValue,
    ConnectionCredentials,
    OauthClientSecret,
    IntegrationCredentials,
    PresenceCredentials,
    WebhookVerification,
    /// (RFE-024) The credential copy of a self-contained `presence.deprovision` op; owner = op id.
    ChannelOpDeprovision,
    /// (P4-MCON, D-128) Secret and generated settings of a managed connector; owner =
    /// `connectors.secret_owner`, org = nil UUID.
    ConnectorSettings,
    /// (phase 5, SET-1, D-236(g)) A secret field of an instance setting (OIDC client secret,
    /// SMTP password, …): always the inline store; owner = the field's own uuid, org = nil UUID.
    InstanceSetting,
    /// (phase 5, P4-011B-3, D-266(b)) The generated Web Push VAPID private key; owner = the key
    /// id, org = nil UUID; always the inline store.
    WebpushVapid,
    /// (phase 5, P4-011B-3, D-266(b)) A Web Push subscription's `{ endpoint, p256dh, auth }`;
    /// owner = the subscription id, org = nil UUID; always the inline store.
    WebpushSubscription,
}

impl Purpose {
    /// The purposes the migration job and CLI know (one source table each). The Web Push
    /// purposes are not here: they are sealed in the `inline` store whatever the default store is
    /// (D-266(b)) and are never moved, rekeyed or counted by those tools.
    pub const ALL: [Purpose; 9] = [
        Purpose::SecretValue,
        Purpose::ConnectionCredentials,
        Purpose::OauthClientSecret,
        Purpose::IntegrationCredentials,
        Purpose::PresenceCredentials,
        Purpose::WebhookVerification,
        Purpose::ChannelOpDeprovision,
        Purpose::ConnectorSettings,
        Purpose::InstanceSetting,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SecretValue => "secret.value",
            Self::ConnectionCredentials => "connection.credentials",
            Self::OauthClientSecret => "oauth_client.secret",
            Self::IntegrationCredentials => "integration.credentials",
            Self::PresenceCredentials => "presence.credentials",
            Self::WebhookVerification => "webhook.verification",
            Self::ChannelOpDeprovision => "channel_op.deprovision",
            Self::ConnectorSettings => "connector.settings",
            Self::InstanceSetting => "instance_setting",
            Self::WebpushVapid => "webpush.vapid",
            Self::WebpushSubscription => "webpush.subscription",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealContext {
    pub purpose: Purpose,
    pub owner_id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreCapabilities {
    pub managed: bool,
    pub references: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreHealth {
    Healthy,
    Unavailable,
}

#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Config id, e.g. "inline", "vault-prod".
    fn id(&self) -> &str;
    /// "inline", "hashicorp-vault", ...
    fn kind(&self) -> &'static str;
    fn capabilities(&self) -> StoreCapabilities;
    /// Always writes a NEW immutable version.
    async fn put(&self, ctx: &SealContext, plaintext: &Secret<Vec<u8>>) -> Result<SecretRef>;
    async fn get(&self, ctx: &SealContext, r: &SecretRef) -> Result<Secret<Vec<u8>>>;
    async fn delete(&self, ctx: &SealContext, r: &SecretRef) -> Result<()>;
    async fn resolve_reference(
        &self,
        ctx: &SealContext,
        path: &str,
        field: Option<&str>,
    ) -> Result<SecretRef>;
    async fn health(&self) -> StoreHealth;
}
