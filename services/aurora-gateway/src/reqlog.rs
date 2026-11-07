//! Request-log batching (docs/06 step 15, §Telemetry): flushed every 5 s or 500 rows.

use async_trait::async_trait;
use aurora_db::repo::gateway::{self as repo, RequestLogRow};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

pub use aurora_db::repo::gateway::RequestLogRow as Row;

pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
pub const FLUSH_ROWS: usize = 500;
/// Beyond this, the oldest rows are dropped (database down for a long time).
const MAX_BUFFERED: usize = 20_000;

#[async_trait]
pub trait LogWriter: Send + Sync + 'static {
    async fn write(&self, rows: &[RequestLogRow]) -> Result<(), String>;
}

pub struct DbWriter(pub aurora_db::PgPool);

#[async_trait]
impl LogWriter for DbWriter {
    async fn write(&self, rows: &[RequestLogRow]) -> Result<(), String> {
        repo::insert_request_log(&self.0, rows)
            .await
            .map_err(|e| e.to_string())
    }
}

pub struct RequestLog {
    writer: Arc<dyn LogWriter>,
    buf: Mutex<Vec<RequestLogRow>>,
    wake: Notify,
}

/// Redact path segments that look like embedded credentials.
pub fn redact_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len().min(2048));
    for (i, seg) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        let secretish = seg.len() >= 24
            && seg.chars().any(|c| c.is_ascii_digit())
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_:.=".contains(c))
            || ["aug_", "aub_", "auc_", "sk-", "sk_"]
                .iter()
                .any(|p| seg.starts_with(p));
        if secretish {
            out.push_str("[redacted]");
        } else {
            out.push_str(seg);
        }
        if out.len() >= 2048 {
            out.truncate(2048);
            break;
        }
    }
    out
}

impl RequestLog {
    pub fn new(writer: Arc<dyn LogWriter>) -> Arc<Self> {
        Arc::new(Self {
            writer,
            buf: Mutex::new(Vec::new()),
            wake: Notify::new(),
        })
    }

    pub fn record(&self, mut row: RequestLogRow) {
        row.path = redact_path(&row.path);
        let Ok(mut b) = self.buf.lock() else { return };
        if b.len() >= MAX_BUFFERED {
            b.drain(..FLUSH_ROWS);
        }
        b.push(row);
        if b.len() >= FLUSH_ROWS {
            self.wake.notify_one();
        }
    }

    pub async fn flush(&self) {
        loop {
            let batch: Vec<RequestLogRow> = {
                let Ok(mut b) = self.buf.lock() else { return };
                let n = b.len().min(1000);
                b.drain(..n).collect()
            };
            if batch.is_empty() {
                return;
            }
            if let Err(e) = self.writer.write(&batch).await {
                tracing::warn!(error = %e, rows = batch.len(), "request-log flush failed; retrying later");
                if let Ok(mut b) = self.buf.lock() {
                    b.splice(0..0, batch);
                }
                return;
            }
        }
    }

    /// Flush periodically until `shutdown` flips, then flush once more.
    pub async fn run(
        self: Arc<Self>,
        interval: Duration,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                () = crate::invalidation::stopped(&mut shutdown) => break,
                () = tokio::time::sleep(interval) => {}
                () = self.wake.notified() => {}
            }
            self.flush().await;
        }
        self.flush().await;
    }
}
