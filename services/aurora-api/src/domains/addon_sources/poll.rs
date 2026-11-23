//! The addon source poll job (C-598, D-250): claims due sources under their lease and
//! runs the shared check path with `trigger = "scheduled"`. Like the library refresh, it
//! is safe on every replica at once. The loop skeleton is the shared `polling::spawn_loop`.

use crate::app::AppState;
use crate::audit::AuditActor;

/// Starts the poll loop of this replica.
pub fn spawn(state: AppState) {
    crate::domains::polling::spawn_loop("addon source poll", state, |s| async move {
        super::poll_once(&s).await
    });
}

/// The `AuditActor` the scheduled path audits with.
#[allow(dead_code)]
pub(crate) fn actor() -> AuditActor {
    AuditActor::system("addon-source-poll")
}
