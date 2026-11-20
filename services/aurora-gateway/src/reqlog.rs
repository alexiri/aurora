//! Request-log batching (docs/06 step 15, §Telemetry): flushed every 5 s or 500 rows.

use crate::health::{Health, REQUEST_LOG_OUTAGE_BUFFER};
use async_trait::async_trait;
use aurora_db::repo::gateway::{self as repo, RequestLogRow};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;

pub use aurora_db::repo::gateway::RequestLogRow as Row;

pub const FLUSH_INTERVAL: Duration = aurora_core::limits::REQUEST_LOG_FLUSH;
pub const FLUSH_ROWS: usize = aurora_core::limits::REQUEST_LOG_FLUSH_ROWS as usize;
/// Beyond this, the oldest rows are dropped and counted (`REQUEST_LOG_OUTAGE_BUFFER`, 06
/// §Operation during a Postgres outage).
const MAX_BUFFERED: usize = REQUEST_LOG_OUTAGE_BUFFER;

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
            .map_err(|e| e.to_string())?;
        // (P5-REFINE-9, C-1084) One `aurora_timeline` hint per agent of the committed batch.
        aurora_db::repo::timeline::notify_requests(&self.0, rows).await;
        Ok(())
    }
}

pub struct RequestLog {
    writer: Arc<dyn LogWriter>,
    buf: Mutex<VecDeque<RequestLogRow>>,
    wake: Notify,
    cap: usize,
    /// (RFE-014) While the database is unreachable nothing is written; rows wait here.
    health: OnceLock<Arc<Health>>,
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
        Self::with_cap(writer, MAX_BUFFERED)
    }

    /// A log that buffers at most `cap` rows (tests).
    pub fn with_cap(writer: Arc<dyn LogWriter>, cap: usize) -> Arc<Self> {
        Arc::new(Self {
            writer,
            buf: Mutex::new(VecDeque::new()),
            wake: Notify::new(),
            cap,
            health: OnceLock::new(),
        })
    }

    /// (RFE-014) Holds rows back while the database is unreachable and flushes on recovery
    /// (set once, before `run`).
    pub fn set_health(&self, h: Arc<Health>) {
        let _ = self.health.set(h);
    }

    /// Rows waiting for the next flush.
    pub fn buffered(&self) -> usize {
        self.buf.lock().map_or(0, |b| b.len())
    }

    fn publish_buffered(&self, n: usize) {
        metrics::gauge!("aurora_gateway_request_log_buffered").set(n as f64);
    }

    pub fn record(&self, mut row: RequestLogRow) {
        row.path = redact_path(&row.path);
        // (FIX-146, D-232 (8)) The row is the one place the outcome of a proxied request is known:
        // put the allowlisted facts on the request's `gateway` span (a no-op outside one).
        let span = tracing::Span::current();
        span.record("aurora.decision", row.decision);
        span.record("http.response.status_code", i64::from(row.status));
        if let Some(a) = &row.app_id {
            span.record("aurora.app_id", a.as_str());
        }
        if let Some(t) = &row.tool_id {
            span.record("aurora.tool", t.as_str());
        }
        if let Some(r) = row.rule_logical_id {
            span.record("aurora.rule_id", tracing::field::display(r));
        }
        if let Some(c) = row.connection_id {
            span.record("aurora.connection_id", tracing::field::display(c));
        }
        let Ok(mut b) = self.buf.lock() else { return };
        // The oldest rows go first, one per new row, and are counted.
        if b.len() >= self.cap {
            b.pop_front();
            metrics::counter!("aurora_gateway_request_log_dropped_total").increment(1);
        }
        b.push_back(row);
        let n = b.len();
        drop(b);
        self.publish_buffered(n);
        if n >= FLUSH_ROWS {
            self.wake.notify_one();
        }
    }

    pub async fn flush(&self) {
        // Nothing is attempted while the database is known to be down: the rows wait.
        if self.health.get().is_some_and(|h| h.db_unreachable()) {
            return;
        }
        loop {
            let batch: Vec<RequestLogRow> = {
                let Ok(mut b) = self.buf.lock() else { return };
                let n = b.len().min(1000);
                b.drain(..n).collect()
            };
            if batch.is_empty() {
                self.publish_buffered(self.buffered());
                return;
            }
            if let Err(e) = self.writer.write(&batch).await {
                tracing::warn!(error = %e, rows = batch.len(), "request-log flush failed; retrying later");
                if let Ok(mut b) = self.buf.lock() {
                    // Rows recorded meanwhile may have filled the buffer: the oldest (this
                    // batch) yield first, counted.
                    for row in batch.into_iter().rev() {
                        if b.len() >= self.cap {
                            metrics::counter!("aurora_gateway_request_log_dropped_total")
                                .increment(1);
                            continue;
                        }
                        b.push_front(row);
                    }
                }
                self.publish_buffered(self.buffered());
                return;
            }
            self.publish_buffered(self.buffered());
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
                () = async {
                    match self.health.get() {
                        Some(h) => h.wait_recovered().await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
            self.flush().await;
        }
        self.flush().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_db::repo::timeline::{HINT_MAX_BYTES, Hint, Source, request_hints};
    use uuid::Uuid;

    fn row(agent: Uuid, age_ms: i64) -> RequestLogRow {
        RequestLogRow {
            id: Uuid::now_v7(),
            ts: chrono::Utc::now() - chrono::Duration::milliseconds(age_ms),
            org_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            agent_id: agent,
            conversation_id: None,
            turn_id: None,
            credential_kind: "agent",
            lane: "mitm",
            method: "GET".into(),
            host: "h.example".into(),
            path: "/secret/sk-abcdef".into(),
            app_id: None,
            tool_id: None,
            connection_id: None,
            secret_id: None,
            decision: "allow",
            rule_logical_id: None,
            rule_name: None,
            upstream_status: Some(200),
            status: 200,
            error_code: None,
            injected: 0,
            latency_ms: 1,
            bytes_up: 0,
            bytes_down: 0,
            context_label: None,
            speaker_user_id: None,
            approval_id: None,
            resumed: false,
            llm_model: None,
        }
    }

    /// (P5-REFINE-9, C-1084) A flushed batch sends one hint per agent: its newest row and the
    /// oldest `ts` of its rows, ids and numbers only.
    #[test]
    fn a_flush_hints_once_per_agent() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let rows = vec![row(a, 30), row(b, 20), row(a, 10), row(a, 50)];
        let hints = request_hints(&rows);
        assert_eq!(hints.len(), 2);
        let ha = hints
            .iter()
            .map(|h| Hint::from_json(h).unwrap())
            .find(|h| h.agent == a)
            .unwrap();
        assert_eq!(ha.source, Source::Request);
        assert_eq!(ha.id, rows[2].id, "the agent's newest row");
        assert_eq!(ha.q, Some(rows[3].ts.timestamp_micros()), "the oldest ts");
        for h in &hints {
            let text = h.to_string();
            assert!(text.len() <= HINT_MAX_BYTES);
            assert!(!text.contains("secret") && !text.contains("h.example"));
        }
        assert!(request_hints(&[]).is_empty());
    }
}
