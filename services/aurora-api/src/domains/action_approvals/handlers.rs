//! Handler registry keyed by `action_key`. Phase 0: `message_agent` (BE-065); `send_message`
//! is registered by the M6 messaging task by adding one entry to [`REGISTRY`].

use aurora_db::repo::action_approvals::ActionApprovalRow;
use sqlx::PgConnection;
use uuid::Uuid;

/// What a handler sees: the row (with its frozen `payload`) and who approved.
pub struct ActionContext<'a> {
    pub approval: &'a ActionApprovalRow,
    pub decided_by: Uuid,
}

/// Result of a successful execution.
pub struct ActionOutcome {
    /// Platform-composed line appended to the origin conversation as a `notice`.
    pub line: String,
    /// Queue a system follow-up turn (when the origin turn finished) so the agent learns it.
    pub notify_agent: bool,
}

/// Execution failure: the row becomes `failed` with this (platform-composed) message.
#[derive(Debug)]
pub struct ActionError(pub String);

#[async_trait::async_trait]
pub trait ActionHandler: Send + Sync {
    fn key(&self) -> &'static str;
    /// Run the frozen payload inside `tx` (a savepoint: dropped on `Err`).
    async fn execute(
        &self,
        tx: &mut PgConnection,
        ctx: &ActionContext<'_>,
    ) -> Result<ActionOutcome, ActionError>;
}

static REGISTRY: &[&dyn ActionHandler] = &[&crate::domains::agent_links::handler::MessageAgent];

/// The handler for `key`, if registered.
pub fn handler(key: &str) -> Option<&'static dyn ActionHandler> {
    REGISTRY.iter().copied().find(|h| h.key() == key)
}

/// Registered keys (tests, diagnostics).
pub fn keys() -> Vec<&'static str> {
    REGISTRY.iter().map(|h| h.key()).collect()
}
