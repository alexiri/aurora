//! Seam between inbox processing (BE-072) and cards (BE-073): typed requests for interactive
//! cards that ask a person to choose. BE-072 calls these inside its transaction; BE-073's
//! implementation ([`super::cards::requests`]) renders and queues the card. The default is a
//! no-op so the inbox can fall back to plain text.

use crate::app::AppState;
use aurora_core::ApiError;
use sqlx::PgConnection;
use uuid::Uuid;

/// An agent a person may pick.
#[derive(Debug, Clone)]
pub struct AgentChoice {
    pub agent_id: Uuid,
    /// Display label (untrusted; sanitized by the card builder).
    pub label: String,
}

/// `binding.choose`: which agent a shared bot binds to a space.
#[derive(Debug, Clone)]
pub struct BindingChoose {
    pub integration_id: Uuid,
    pub space_ref: String,
    /// Where to post the card (thread of the message that triggered it).
    pub thread_ref: Option<String>,
    /// Agents the clicker may choose from (≤ 25).
    pub agents: Vec<AgentChoice>,
}

/// `dm_default.choose`: which agent answers a person's DMs with the shared bot.
#[derive(Debug, Clone)]
pub struct DmDefaultChoose {
    pub integration_id: Uuid,
    /// The platform user whose default is chosen; the card is DMed to them.
    pub user_ref: String,
    pub agents: Vec<AgentChoice>,
}

/// `reach.decide`: a pending reach grant, DMed to the workspace owners linked on the integration.
#[derive(Debug, Clone)]
pub struct ReachDecide {
    pub integration_id: Uuid,
    pub grant_id: Uuid,
    pub agent_id: Uuid,
    pub agent_label: String,
    /// What asks for reach (person or space label; untrusted).
    pub subject_label: String,
    /// Platform refs of the owners to DM.
    pub owner_refs: Vec<String>,
}

#[async_trait::async_trait]
pub trait CardRequests: Send + Sync {
    async fn binding_choose(
        &self,
        _tx: &mut PgConnection,
        _req: BindingChoose,
    ) -> Result<(), ApiError> {
        Ok(())
    }
    async fn dm_default_choose(
        &self,
        _tx: &mut PgConnection,
        _req: DmDefaultChoose,
    ) -> Result<(), ApiError> {
        Ok(())
    }
    async fn reach_decide(
        &self,
        _tx: &mut PgConnection,
        _req: ReachDecide,
    ) -> Result<(), ApiError> {
        Ok(())
    }
    /// Whether this implementation renders cards (the inbox then skips its plain-text fallback).
    fn renders_cards(&self) -> bool {
        false
    }
}

/// Default: does nothing.
pub struct NoopCardRequests;

impl CardRequests for NoopCardRequests {}

pub fn card_requests(state: &AppState) -> &dyn CardRequests {
    &*state.card_requests
}
