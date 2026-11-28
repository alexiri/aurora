//! Spawned work that something may have to wait for: a process that is about to exit, or a
//! step that needs the work done first.

use std::sync::Arc;
use tokio::sync::watch;

/// Counts the tasks spawned through it that have not ended. Cheap to clone; clones share the
/// count.
#[derive(Debug, Clone, Default)]
pub struct InFlight(Arc<watch::Sender<usize>>);

/// Counts one task as ended, also when it panics.
struct Ended(Arc<watch::Sender<usize>>);

impl Drop for Ended {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n -= 1);
    }
}

impl InFlight {
    /// Spawn `work`, counted until it ends. The caller does not wait for it, and dropping the
    /// caller does not cancel it.
    pub fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) {
        self.0.send_modify(|n| *n += 1);
        let ended = Ended(self.0.clone());
        tokio::spawn(async move {
            let _ended = ended;
            work.await;
        });
    }

    /// Wait until no task spawned through this is running.
    pub async fn idle(&self) {
        // The sender lives in `self`, so the channel cannot close while this waits.
        let _ = self.0.subscribe().wait_for(|n| *n == 0).await;
    }
}
