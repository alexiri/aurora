//! Report queue: batches (<= 200 reports, <= 1 MiB) and posts with retry.

use std::sync::Arc;

use aurora_protocol::common::Validate;
use aurora_protocol::runner::{
    MAX_REPORTS_BODY, MAX_REPORTS_PER_BATCH, Report, ReportBody, ReportStatus, ReportsRequest,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::api::ApiClient;

/// Fixed JSON overhead of `{"reports":[...]}` plus separators.
const BATCH_OVERHEAD: usize = 64;

enum Msg {
    Report(Box<Report>),
    Flush,
}

/// Cheap, cloneable producer handle.
#[derive(Clone)]
pub struct ReportSink {
    tx: mpsc::UnboundedSender<Msg>,
}

impl ReportSink {
    /// Queue a report. Reports violating sender-side bounds are dropped (logged).
    pub fn send(&self, report: Report) {
        if let Err(e) = report.validate() {
            tracing::error!(kind = report.body.kind(), error = %e, "dropping invalid report");
            return;
        }
        let _ = self.tx.send(Msg::Report(Box::new(report)));
    }

    /// Ask the reporter to deliver everything queued so far and stop.
    pub fn finish(&self) {
        let _ = self.tx.send(Msg::Flush);
    }
}

pub fn spawn(api: Arc<ApiClient>, shutdown: watch::Receiver<bool>) -> (ReportSink, JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (ReportSink { tx }, tokio::spawn(run(api, rx, shutdown)))
}

fn size_of(r: &Report) -> usize {
    serde_json::to_vec(r).map_or(usize::MAX, |v| v.len() + 1)
}

async fn run(
    api: Arc<ApiClient>,
    mut rx: mpsc::UnboundedReceiver<Msg>,
    shutdown: watch::Receiver<bool>,
) {
    let mut carry: Option<(Report, usize)> = None;
    let mut stop = false;
    loop {
        let (first, first_size) = match carry.take() {
            Some(c) => c,
            None => {
                if stop {
                    break;
                }
                match rx.recv().await {
                    Some(Msg::Report(r)) => {
                        let s = size_of(&r);
                        (*r, s)
                    }
                    Some(Msg::Flush) | None => break,
                }
            }
        };
        if first_size + BATCH_OVERHEAD > MAX_REPORTS_BODY {
            tracing::error!(kind = first.body.kind(), "dropping oversized report");
            continue;
        }
        let mut size = BATCH_OVERHEAD + first_size;
        let mut batch = vec![first];
        while batch.len() < MAX_REPORTS_PER_BATCH {
            match rx.try_recv() {
                Ok(Msg::Report(r)) => {
                    let s = size_of(&r);
                    if size + s > MAX_REPORTS_BODY {
                        carry = Some((*r, s));
                        break;
                    }
                    size += s;
                    batch.push(*r);
                }
                Ok(Msg::Flush) => {
                    stop = true;
                    break;
                }
                Err(_) => break,
            }
        }
        deliver(&api, batch, &shutdown).await;
    }
}

async fn deliver(api: &ApiClient, reports: Vec<Report>, shutdown: &watch::Receiver<bool>) {
    let req = ReportsRequest { reports };
    let mut round = 0u32;
    loop {
        match api.post_reports(&req).await {
            Ok(results) => {
                log_results(&results);
                return;
            }
            Err(e) if e.retryable() && !*shutdown.borrow() => {
                tracing::warn!(error = %e, count = req.reports.len(), "report post failed; retrying");
                tokio::time::sleep(std::time::Duration::from_secs(1 << round.min(4))).await;
                round += 1;
            }
            Err(e) => {
                tracing::error!(error = %e, count = req.reports.len(), "dropping report batch");
                return;
            }
        }
    }
}

fn log_results(results: &[aurora_protocol::runner::ReportResult]) {
    for r in results {
        let id: Uuid = r.report_id;
        match r.status {
            ReportStatus::Applied | ReportStatus::Duplicate => {}
            ReportStatus::Fenced => tracing::info!(report_id = %id, "report fenced; dropped"),
            _ => {
                tracing::warn!(report_id = %id, status = r.status.as_str(), detail = ?r.detail, "report not applied")
            }
        }
    }
}

/// Convenience constructor shared by executor and translator.
pub fn report(report_id: Uuid, sandbox_id: Uuid, boot_id: Uuid, body: ReportBody) -> Report {
    Report {
        report_id,
        sandbox_id,
        boot_id,
        body,
    }
}
