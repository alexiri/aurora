//! Approval holds (docs/06 §Approval holds, docs/07 §Approvals (gateway holds)).
//!
//! A request whose verdict is `needs_approval` is stored as a `pending` row and
//! parked here until the API writes a decision (first decision wins), the
//! deadline passes, the client goes away or the gateway stops. The row is the
//! source of truth; NOTIFY events only shorten the wait (2 s poll fallback).

use crate::apps::AppMatch;
use crate::auth::RequestIdentity;
use crate::health::Health;
use crate::hygiene::AuroraHeaders;
use crate::invalidation::{Hub, stopped};
use crate::snapshot::Loaded;
use aurora_db::PgPool;
use aurora_db::bus::{Channel, notify};
use aurora_db::repo::gateway::{self as repo, NewApproval};
use aurora_summary::ApprovalSummary;
use aurora_summary::template;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

/// Request facts for [`build_summary`].
pub struct SummaryRequest<'a> {
    pub method: &'a str,
    pub host: &'a str,
    /// Redacted, without query.
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub content_type: Option<&'a str>,
    /// Case-insensitive request header lookup.
    pub header: &'a (dyn Fn(&str) -> Option<String> + Sync),
    pub body_prefix: &'a [u8],
    pub truncated: bool,
    /// All request headers (the hook client filters sensitive ones). Empty skips none.
    pub headers: &'a [(String, String)],
}

/// Hook-first variant of [`build_summary`] (live holds): when the tool's approval says
/// `summarize: hook` and a summarizer is configured, ask the hook host (1 s budget, no
/// egress); any failure falls through to the template, then the generic summary.
pub async fn build_summary_hooked(
    summarizer: Option<&dyn crate::summarize_client::Summarizer>,
    l: Option<&Loaded>,
    app: Option<&AppMatch<'_>>,
    tool_id: Option<&str>,
    r: &SummaryRequest<'_>,
) -> ApprovalSummary {
    if let (Some(sz), Some(l), Some(m)) = (summarizer, l, app)
        && let Some(a) = tool_id.and_then(|t| m.app.approvals.get(t))
        && template::wants_hook(a)
    {
        let digest = l
            .snapshot
            .packages
            .iter()
            .find(|p| p.package_id == m.app.package_id)
            .map(|p| p.digest.as_str());
        if let Some(digest) = digest {
            let injected = template::injected_header_names(&m.entry.inject);
            let call = crate::summarize_client::SummarizeCall {
                package_id: &m.app.package_id,
                package_digest: digest,
                app_id: &m.app.app_id,
                tool_id,
                method: r.method,
                host: r.host,
                path: r.path,
                query: r.query,
                headers: r.headers,
                injected_headers: &injected,
                body_prefix: r.body_prefix,
                truncated: r.truncated,
            };
            if let Some(s) = sz.summarize(call).await {
                return s;
            }
            tracing::warn!(app_id = %m.app.app_id, "approval.summarize unavailable; using template");
        }
    }
    build_summary(l, app, tool_id, r)
}

/// The one approval-summary entry point (live holds and `check-snapshot`): the tool's
/// template when the app declares one, else the generic redacting summary.
/// Hook seam (HOOK-004): when `template::wants_hook(approval)`, call the hook first and
/// only then fall through to this function.
pub fn build_summary(
    l: Option<&Loaded>,
    app: Option<&AppMatch<'_>>,
    tool_id: Option<&str>,
    r: &SummaryRequest<'_>,
) -> ApprovalSummary {
    if let (Some(l), Some(m)) = (l, app)
        && let Some(a) = tool_id.and_then(|t| m.app.approvals.get(t))
    {
        let declared: Vec<&str> = l
            .snapshot
            .hosts
            .iter()
            .filter(|h| h.app_id == m.app.app_id)
            .map(|h| h.host.as_str())
            .collect();
        let injected = template::injected_header_names(&m.entry.inject);
        return template::summarize(
            a,
            &template::Input {
                host: r.host,
                path: r.path,
                query: r.query,
                header: r.header,
                body_prefix: r.body_prefix,
                truncated: r.truncated,
                declared_hosts: &declared,
                web_hosts: &m.app.about.web_hosts,
                injected_headers: &injected,
            },
        );
    }
    aurora_summary::generic(
        r.method,
        r.host,
        r.path,
        r.query,
        r.content_type,
        r.body_prefix,
        r.truncated,
    )
}

pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldOutcome {
    Approved,
    Denied(Uuid),
    Expired(Uuid),
    /// The gateway is stopping (or the hold was abandoned by a sweeper).
    Restarting,
    /// The approval store could not record or read the hold (fail closed).
    StoreUnavailable,
}

pub struct HoldRequest<'a> {
    pub identity: &'a RequestIdentity,
    pub method: &'a str,
    pub host: &'a str,
    /// Redacted, without query.
    pub path: &'a str,
    pub rule: Option<(Uuid, &'a str)>,
    pub batch: &'a AuroraHeaders,
    pub summary: &'a ApprovalSummary,
    /// (RFE-011a) The decision's rule-level timeout (minimum of the set values of the matching
    /// requiring rules of both scopes); `None` = no preference, the default timeout applies.
    pub rule_timeout_secs: Option<u32>,
}

pub struct Holds {
    pool: PgPool,
    hub: Arc<Hub>,
    instance_id: Uuid,
    /// `approvals.timeout_secs`: the timeout of holds whose rules set none.
    timeout: Duration,
    /// (RFE-011a) bounds of a rule-level timeout: `APPROVAL_TIMEOUT_MIN ..= approvals.max_timeout_secs`.
    min_timeout: Duration,
    max_timeout: Duration,
    poll: Duration,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    /// (RFE-014) Outage state: no hold is recorded while the database is unreachable.
    health: Option<Arc<Health>>,
    /// (RFE-014) Holds answered `manual_approval_expired` while the database was down: their
    /// rows are expired on recovery (`UPDATE ... WHERE status = 'pending'`, retried).
    unexpired: Mutex<Vec<(Uuid, Uuid)>>,
}

/// Marks the hold `abandoned` if the request future is dropped (client gone).
///
/// The guard is armed *before* the row is inserted (the row is visible to others the moment it
/// commits, before the inserting future sees the reply), and the insert runs on its own task, so
/// a drop at any point leaves no `pending` row behind: whichever of "row committed" and "guard
/// dropped" happens second abandons it (`abandon_approval` is idempotent).
struct AbandonOnDrop {
    pool: PgPool,
    id: Uuid,
    armed: bool,
    sync: Arc<AbandonSync>,
}

#[derive(Default)]
struct AbandonSync {
    inserted: AtomicBool,
    dropped: AtomicBool,
}

async fn abandon_logged(pool: &PgPool, id: Uuid) {
    if let Err(e) = repo::abandon_approval(pool, id).await {
        tracing::warn!(error = %e, approval_id = %id, "abandoning approval failed");
    }
}

impl Drop for AbandonOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.sync.dropped.store(true, Ordering::SeqCst);
        // Not committed yet: the insert task abandons the row once it has (it sees `dropped`).
        if !self.sync.inserted.load(Ordering::SeqCst) {
            return;
        }
        let (pool, id) = (self.pool.clone(), self.id);
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move { abandon_logged(&pool, id).await });
        }
    }
}

fn clip(s: &str) -> String {
    s.chars()
        .take(aurora_core::limits::BATCH_HEADER_MAX)
        .collect()
}

impl Holds {
    pub fn new(
        pool: PgPool,
        hub: Arc<Hub>,
        instance_id: Uuid,
        timeout: Duration,
        shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Self {
        Self {
            pool,
            hub,
            instance_id,
            timeout,
            min_timeout: aurora_core::limits::APPROVAL_TIMEOUT_MIN,
            max_timeout: aurora_core::limits::APPROVAL_TIMEOUT_MAX,
            poll: POLL_INTERVAL,
            shutdown,
            health: None,
            unexpired: Mutex::new(Vec::new()),
        }
    }

    /// (RFE-014) Refuses new holds while the database is unreachable and reconciles the rows of
    /// holds that expired without it.
    #[must_use]
    pub fn with_health(mut self, h: Arc<Health>) -> Self {
        self.health = Some(h);
        self
    }

    fn db_down(&self) -> bool {
        self.health.as_ref().is_some_and(|h| h.db_unreachable())
    }

    fn report(&self, e: &aurora_db::DbError) {
        if let Some(h) = &self.health
            && crate::health::is_db_connection_error(e)
        {
            h.report_db_error();
        }
    }

    /// Rows still to be expired after an outage (hold id, workspace id).
    pub fn pending_expiry(&self) -> usize {
        self.unexpired.lock().map_or(0, |u| u.len())
    }

    /// Expires the rows of holds that timed out while the database was down; what fails stays
    /// queued for the next attempt. Idempotent: a row decided meanwhile is left alone.
    pub async fn reconcile(&self) {
        let todo: Vec<(Uuid, Uuid)> = match self.unexpired.lock() {
            Ok(mut u) => std::mem::take(&mut *u),
            Err(_) => return,
        };
        let mut failed = Vec::new();
        for (id, ws) in todo {
            match repo::expire_approval(&self.pool, id).await {
                Ok(true) => self.emit(id, ws, "expired").await,
                Ok(false) => {}
                Err(e) => {
                    self.report(&e);
                    failed.push((id, ws));
                }
            }
        }
        if !failed.is_empty()
            && let Ok(mut u) = self.unexpired.lock()
        {
            u.extend(failed);
        }
    }

    /// Retries [`Holds::reconcile`] every `interval` while rows are queued and at once when the
    /// database recovers, until `shutdown`.
    pub async fn run_reconcile(
        self: Arc<Self>,
        interval: Duration,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                () = stopped(&mut shutdown) => return,
                () = tokio::time::sleep(interval) => {}
                () = async {
                    match &self.health {
                        Some(h) => h.wait_recovered().await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
            if self.pending_expiry() > 0 && !self.db_down() {
                self.reconcile().await;
            }
        }
    }

    /// (RFE-011a) Sets the instance maximum of rule-level timeouts (`approvals.max_timeout_secs`).
    #[must_use]
    pub fn with_max_timeout(mut self, max: Duration) -> Self {
        self.max_timeout = max;
        self
    }

    /// Overrides the lower bound of rule-level timeouts (tests).
    #[must_use]
    pub fn with_min_timeout(mut self, min: Duration) -> Self {
        self.min_timeout = min;
        self
    }

    /// (RFE-011a) The effective timeout of a hold: the rule-level minimum clamped to
    /// `[APPROVAL_TIMEOUT_MIN, approvals.max_timeout_secs]`, else the default timeout.
    pub fn effective_timeout(&self, rule_timeout_secs: Option<u32>) -> Duration {
        // Sub-second default timeouts exist only in tests; keep them exact.
        match rule_timeout_secs {
            None => self.timeout,
            Some(_) => Duration::from_secs(aurora_core::limits::effective_approval_timeout_secs(
                rule_timeout_secs,
                self.timeout.as_secs(),
                self.min_timeout.as_secs(),
                self.max_timeout.as_secs(),
            )),
        }
    }

    /// Overrides the poll interval (tests).
    #[must_use]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    async fn emit(&self, id: Uuid, workspace_id: Uuid, event: &str) {
        let payload = json!({ "id": id, "workspace_id": workspace_id, "event": event });
        if let Err(e) = notify(&self.pool, Channel::Approval, &payload).await {
            tracing::warn!(error = %e, approval_id = %id, event, "approval notify failed");
        }
    }

    /// Map a non-pending status to an outcome.
    fn outcome(id: Uuid, status: &str) -> Option<HoldOutcome> {
        match status {
            "approved" => Some(HoldOutcome::Approved),
            "denied" => Some(HoldOutcome::Denied(id)),
            "expired" => Some(HoldOutcome::Expired(id)),
            "abandoned" => Some(HoldOutcome::Restarting),
            _ => None,
        }
    }

    /// Records the hold and waits for its resolution.
    pub async fn hold(&self, r: HoldRequest<'_>) -> HoldOutcome {
        // (RFE-014) A new hold cannot be persisted while the database is unreachable.
        if self.db_down() {
            return HoldOutcome::StoreUnavailable;
        }
        let id = Uuid::now_v7();
        let ws = r.identity.workspace_id;
        let timeout = self.effective_timeout(r.rule_timeout_secs);
        // Register before inserting so a fast decision cannot be missed.
        let waiter = self.hub.approval_waiter(id);
        let summary = match serde_json::to_value(r.summary) {
            Ok(v) => v,
            Err(_) => return HoldOutcome::StoreUnavailable,
        };
        let row = NewApproval {
            id,
            org_id: r.identity.org_id,
            workspace_id: ws,
            agent_id: r.identity.agent_id,
            conversation_id: r.identity.conversation_id,
            turn_id: r.identity.turn_id,
            gateway_instance_id: self.instance_id,
            method: r.method.to_owned(),
            host: r.host.to_owned(),
            path: r.path.to_owned(),
            connection_id: None,
            rule_logical_id: r.rule.map(|x| x.0),
            rule_name: r.rule.map(|x| x.1.to_owned()),
            summary,
            batch_id: r.batch.batch_id.as_deref().map(clip),
            batch_label: r.batch.batch_label.as_deref().map(clip),
            context_label: r.identity.context_label.as_deref().map(clip),
            batch_total: r.batch.batch_total.and_then(|n| i32::try_from(n).ok()),
            timeout_secs: timeout.as_secs_f64(),
        };
        // The row is visible to everyone (the approvals UI, a freeze reacting to it) as soon as
        // it commits, before this future has seen the reply. If the request is dropped in that
        // window (a freeze or BYO tightening refuses it, the client goes) the row would stay
        // `pending` with nobody to abandon it. So the guard is armed first and the insert runs
        // to completion on its own task (see `AbandonOnDrop`).
        let sync = Arc::new(AbandonSync::default());
        let mut guard = AbandonOnDrop {
            pool: self.pool.clone(),
            id,
            armed: true,
            sync: sync.clone(),
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let pool = self.pool.clone();
            tokio::spawn(async move {
                let res = repo::insert_approval(&pool, &row).await;
                if res.is_ok() {
                    sync.inserted.store(true, Ordering::SeqCst);
                    if sync.dropped.load(Ordering::SeqCst) {
                        abandon_logged(&pool, id).await;
                    }
                }
                let _ = tx.send(res);
            });
        }
        match rx.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                guard.armed = false;
                tracing::error!(error = %e, "approval insert failed");
                self.report(&e);
                return HoldOutcome::StoreUnavailable;
            }
            Err(_) => {
                guard.armed = false;
                return HoldOutcome::StoreUnavailable;
            }
        }
        self.emit(id, ws, "created").await;

        let deadline = Instant::now() + timeout;
        let mut sd = self.shutdown.clone();
        let stop = async {
            match sd.as_mut() {
                Some(rx) => stopped(rx).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(stop);
        let out = loop {
            tokio::select! {
                () = &mut stop => {
                    if let Err(e) = repo::abandon_approval(&self.pool, id).await {
                        tracing::warn!(error = %e, approval_id = %id, "abandoning approval failed");
                    }
                    break HoldOutcome::Restarting;
                }
                () = tokio::time::sleep_until(deadline) => {
                    // (RFE-014) The database is down: the hold expires in memory, its row on
                    // recovery. A failed poll is never read as a decision.
                    if self.db_down() {
                        self.queue_expiry(id, ws);
                        break HoldOutcome::Expired(id);
                    }
                    match repo::expire_approval(&self.pool, id).await {
                        Ok(true) => {
                            self.emit(id, ws, "expired").await;
                            break HoldOutcome::Expired(id);
                        }
                        // Lost the race to a decision (or another expirer): honour it.
                        Ok(false) => {
                            let st = repo::get_approval_status(&self.pool, id).await;
                            break match st {
                                Ok(Some(s)) => Self::outcome(id, &s).unwrap_or(HoldOutcome::Expired(id)),
                                _ => HoldOutcome::Expired(id),
                            };
                        }
                        // Fail closed; the row is expired on recovery (and by the API sweeper).
                        Err(e) => {
                            tracing::warn!(error = %e, approval_id = %id, "expiring approval failed");
                            self.report(&e);
                            self.queue_expiry(id, ws);
                            break HoldOutcome::Expired(id);
                        }
                    }
                }
                () = waiter.wait() => {}
                () = tokio::time::sleep(self.poll) => {}
            }
            // While the database is down the poll is not attempted (it cannot answer); the
            // hold keeps waiting until its deadline.
            if self.db_down() {
                continue;
            }
            match repo::get_approval_status(&self.pool, id).await {
                Ok(Some(s)) => {
                    if let Some(o) = Self::outcome(id, &s) {
                        break o;
                    }
                }
                Ok(None) => break HoldOutcome::StoreUnavailable,
                Err(e) => {
                    tracing::warn!(error = %e, approval_id = %id, "approval poll failed");
                    self.report(&e);
                }
            }
        };
        guard.armed = false;
        out
    }

    fn queue_expiry(&self, id: Uuid, ws: Uuid) {
        if let Ok(mut u) = self.unexpired.lock() {
            u.push((id, ws));
        }
    }

    /// Safety net at shutdown: abandon anything still pending for this instance.
    pub async fn abandon_all(&self) {
        if let Err(e) = repo::abandon_approvals(&self.pool, self.instance_id).await {
            tracing::warn!(error = %e, "abandoning approvals at shutdown failed");
        }
    }
}
