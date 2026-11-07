//! The seam between the inbox (BE-072) and cards (BE-073): requests for interactive cards the
//! inbox raises (docs/13 §5.1 steps 3 and 5, §5.3). BE-073 implements [`CardRequests`] and
//! switches [`card_requests`] to its implementation. [`NoopCardRequests`] queues nothing; the
//! inbox then falls back to plain-text instructions (`use <handle>`, `bind <handle>`).

use crate::app::AppState;
use aurora_core::ApiError;
use sqlx::PgConnection;
use uuid::Uuid;

/// `binding.choose`: DM the person who added the shared bot a select of agents they own.
#[derive(Debug, Clone)]
pub struct BindingChoose {
    pub integration_id: Uuid,
    pub actor_user_id: Uuid,
    /// The adder's platform user; the DM goes there.
    pub actor_ref: String,
    pub space_ref: String,
    pub space_label: String,
    /// Agents the adder can own that have a shared presence on the integration.
    pub allowed_agent_ids: Vec<Uuid>,
}

/// `dm_default.choose`: an agent picker for a person's top-level DMs with the shared bot.
#[derive(Debug, Clone)]
pub struct DmDefaultChoose {
    pub integration_id: Uuid,
    pub user_id: Uuid,
    /// The platform user who wrote.
    pub user_ref: String,
    /// Where the picker is posted: the DM and thread the person wrote in.
    pub space_ref: String,
    pub thread_ref: String,
    pub allowed_agent_ids: Vec<Uuid>,
}

/// `reach.decide`: DM a card to owners linked on the integration.
#[derive(Debug, Clone)]
pub struct ReachDecide {
    pub integration_id: Uuid,
    pub grant_id: Uuid,
    pub agent_id: Uuid,
    /// Sanitized label of the person asking.
    pub subject_label: String,
    /// Workspace owners with a user link on the integration: `(user id, platform user ref)`.
    pub owners: Vec<(Uuid, String)>,
}

/// Card requests; `tx` is the caller's connection (ops are queued on it).
#[async_trait::async_trait]
pub trait CardRequests: Send + Sync {
    /// Whether this implementation renders cards. When `false` the inbox adds the plain-text
    /// instructions itself.
    fn renders_cards(&self) -> bool {
        false
    }
    async fn binding_choose(
        &self,
        tx: &mut PgConnection,
        req: BindingChoose,
    ) -> Result<(), ApiError>;
    async fn dm_default_choose(
        &self,
        tx: &mut PgConnection,
        req: DmDefaultChoose,
    ) -> Result<(), ApiError>;
    async fn reach_decide(&self, tx: &mut PgConnection, req: ReachDecide) -> Result<(), ApiError>;
}

/// The default: no cards.
pub struct NoopCardRequests;

#[async_trait::async_trait]
impl CardRequests for NoopCardRequests {
    async fn binding_choose(&self, _: &mut PgConnection, _: BindingChoose) -> Result<(), ApiError> {
        Ok(())
    }
    async fn dm_default_choose(
        &self,
        _: &mut PgConnection,
        _: DmDefaultChoose,
    ) -> Result<(), ApiError> {
        Ok(())
    }
    async fn reach_decide(&self, _: &mut PgConnection, _: ReachDecide) -> Result<(), ApiError> {
        Ok(())
    }
}

static NOOP: NoopCardRequests = NoopCardRequests;

/// The card implementation in use (BE-073 switches this).
pub fn card_requests(_state: &AppState) -> &dyn CardRequests {
    &NOOP
}
