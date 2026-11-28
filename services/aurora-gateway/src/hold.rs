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
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

/// Request facts for [`build_summary`].
pub struct SummaryRequest<'a> {
    pub method: &'a str,
    pub host: &'a str,
    /// (D-467) The matched self-hosted registration's base path; `None` for any other request.
    pub base_path: Option<&'a str>,
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
        && let Some(body) = hook_body(r)
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
                body_prefix: &body,
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

/// `body` after any leading white space and byte order marks.
fn past_space_and_bom(mut body: &[u8]) -> &[u8] {
    loop {
        body = body.trim_ascii_start();
        match body.strip_prefix(b"\xef\xbb\xbf") {
            Some(rest) => body = rest,
            None => return body,
        }
    }
}

/// A hook reads the body with its own parser, which may read what the core's refuses, and the
/// core cannot tell which names it reads. A receiver may read JSON whatever the content type
/// says. So:
///
/// - a body the core reads as JSON goes to a hook only if it repeats no name anywhere, also in
///   another case;
/// - one the core cannot read is a guess when its content type says JSON, or it starts like an
///   object or an array (behind white space or a byte order mark too); a cut-off body does not
///   parse. An empty body has nothing to read;
/// - of a multipart body (a `multipart/` type, or any mention of a boundary) a hook gets the
///   first part only, which must be JSON with one reading: nothing it could read is unchecked;
/// - the core checks bytes as they are, so an encoded body (any `content-encoding` but
///   `identity`) or first part (a transfer encoding that changes bytes) is a guess too.
///
/// The body to give the hook, `None` when it would have to guess.
fn hook_body<'a>(r: &SummaryRequest<'a>) -> Option<Cow<'a, [u8]>> {
    let content_type = r.content_type.unwrap_or_default();
    let lower = content_type.to_ascii_lowercase();
    // Which of two content types a receiver follows is its own choice.
    let content_types = r
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .count();
    if content_types > 1 {
        return None;
    }
    let encoded = r.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-encoding")
            && value
                .split(',')
                .map(str::trim)
                .any(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case("identity"))
    });
    if encoded {
        return None;
    }
    let media = lower.split(';').next().unwrap_or_default().trim();
    let guess = match aurora_policy::json::parse(r.body_prefix) {
        Some(doc) => doc.has_colliding_names(),
        None => {
            let body = past_space_and_bom(r.body_prefix);
            !body.is_empty()
                && (media.contains("json") || matches!(body.first(), Some(b'{' | b'[')))
        }
    };
    if guess {
        return None;
    }
    if media.starts_with("multipart/") || lower.contains("boundary") {
        return first_part_only(content_type, r.body_prefix, r.truncated).map(Cow::Owned);
    }
    Some(Cow::Borrowed(r.body_prefix))
}

/// The multipart boundary, when every reader reads the same one: the one mention of
/// `boundary`, a plain `;boundary=` parameter (no `boundary*=`, no space around the `=`),
/// quoted and closed or not, of RFC 2046's characters (so no backslash escape), not ending in
/// a space.
fn boundary_of(content_type: &str) -> Option<&str> {
    const NAME: &str = "boundary";
    let lower = content_type.to_ascii_lowercase();
    let mut mentions = lower.match_indices(NAME);
    let (at, _) = mentions.next()?;
    if mentions.next().is_some() || !lower.get(..at)?.trim_end().ends_with(';') {
        return None;
    }
    let raw = content_type.get(at + NAME.len()..)?.strip_prefix('=')?;
    let (boundary, quoted) = match raw.strip_prefix('"') {
        Some(rest) => {
            let (value, after) = rest.split_once('"')?;
            let after = after.trim_start();
            if !after.is_empty() && !after.starts_with(';') {
                return None;
            }
            (value, true)
        }
        None => (raw.split(';').next()?.trim_end(), false),
    };
    let allowed =
        |c: char| c.is_ascii_alphanumeric() || "'()+_,-./:=?".contains(c) || (quoted && c == ' ');
    ((1..=70).contains(&boundary.len())
        && boundary.chars().all(allowed)
        && !boundary.ends_with(' '))
    .then_some(boundary)
}

/// Whether a part's (lowercased) headers leave its bytes as sent: every mention of
/// `content-transfer-encoding` is a plain header line of its own, not continued on the next,
/// naming `7bit`, `8bit` or `binary`. Readers differ on any other spelling.
fn bytes_as_sent(headers: &str) -> bool {
    const NAME: &str = "content-transfer-encoding";
    headers.match_indices(NAME).all(|(at, _)| {
        let (before, rest) = headers.split_at(at);
        let Some(value) = rest.strip_prefix(NAME).and_then(|v| v.strip_prefix(':')) else {
            return false;
        };
        let (value, next) = value.split_once('\n').unwrap_or((value, ""));
        before.ends_with('\n')
            && !next.starts_with([' ', '\t'])
            && matches!(value.trim(), "7bit" | "8bit" | "binary")
    })
}

/// A multipart body cut after its first part and closed again, when that part is the first for
/// every reader (the body starts at the delimiter: a lenient reader takes the first place it
/// occurs, a strict one only a line of its own), is declared JSON and has one reading. Of a
/// cut-off body the part must be closed by a delimiter in what was read: else it may go on.
fn first_part_only(content_type: &str, body: &[u8], truncated: bool) -> Option<Vec<u8>> {
    let find = |needle: &[u8], from: usize| {
        let rest = body.get(from..)?;
        let at = rest.windows(needle.len()).position(|w| w == needle)?;
        Some(from + at)
    };
    let delimiter = format!("--{}", boundary_of(content_type)?);
    let after = body.strip_prefix(delimiter.as_bytes())?;
    if !after.starts_with(b"\r\n") && !after.starts_with(b"\n") {
        return None;
    }
    let start = delimiter.len();
    let (headers_end, blank) = match (find(b"\r\n\r\n", start), find(b"\n\n", start)) {
        (Some(a), Some(b)) if b < a => (b, 2),
        (Some(a), _) => (a, 4),
        (None, Some(b)) => (b, 2),
        (None, None) => return None,
    };
    let headers = String::from_utf8_lossy(body.get(start..headers_end)?).to_ascii_lowercase();
    if !headers.contains("json") || !bytes_as_sent(&headers) {
        return None;
    }
    let part = headers_end + blank;
    let end = [
        find(format!("\r\n{delimiter}").as_bytes(), part),
        find(format!("\n{delimiter}").as_bytes(), part),
    ]
    .into_iter()
    .flatten()
    .min()
    .or_else(|| (!truncated).then_some(body.len()))?;
    let one_reading =
        aurora_policy::json::parse(body.get(part..end)?).is_some_and(|d| !d.has_colliding_names());
    one_reading.then(|| {
        let mut only = body.get(..end).unwrap_or_default().to_vec();
        only.extend_from_slice(format!("\r\n{delimiter}--\r\n").as_bytes());
        only
    })
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
                base_path: r.base_path,
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
    /// (P5-TOOLASK, C-1043) Held because the tool's package default is `ask` (no rule decided):
    /// the row's reason names the app.
    pub package_ask: bool,
    /// (FIX-084) The app connection whose credential the request would use; recorded on the row
    /// so that revoking that grant or connection abandons the pending approval.
    pub connection_id: Option<Uuid>,
    /// (P5-UX-16, C-982) The app and the tool the request matched, recorded on the row so the
    /// approval can say what it is about and offer Always allow for that tool.
    pub app_id: Option<&'a str>,
    pub tool_id: Option<&'a str>,
    pub batch: &'a AuroraHeaders,
    pub summary: &'a ApprovalSummary,
    /// (RFE-011a) The decision's rule-level timeout (minimum of the set values of the matching
    /// requiring rules of both scopes); `None` = no preference, the default timeout applies.
    pub rule_timeout_secs: Option<u32>,
}

use aurora_policy::eval::PACKAGE_ASK_REASON;

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

pub struct Holds {
    pub(crate) pool: PgPool,
    hub: Arc<Hub>,
    instance_id: Uuid,
    /// (P4-011B) the live `approvals.park.*` values (`park.rs`).
    pub(crate) park: crate::park::ParkSettings,
    /// `approvals.timeout_secs`: the timeout of holds whose rules set none.
    /// Milliseconds; replaced live when the `approvals` settings change (SET-5).
    timeout: AtomicU64,
    /// (RFE-011a) bounds of a rule-level timeout: `APPROVAL_TIMEOUT_MIN ..= approvals.max_timeout_secs`.
    pub(crate) min_timeout: Duration,
    /// Milliseconds; replaced live with `timeout` (SET-5).
    max_timeout: AtomicU64,
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
            park: crate::park::ParkSettings::new(crate::park::ParkConfig::default()),
            timeout: AtomicU64::new(millis(timeout)),
            min_timeout: aurora_core::limits::APPROVAL_TIMEOUT_MIN,
            max_timeout: AtomicU64::new(millis(aurora_core::limits::APPROVAL_TIMEOUT_MAX)),
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

    pub(crate) fn db_down(&self) -> bool {
        self.health.as_ref().is_some_and(|h| h.db_unreachable())
    }

    /// (P4-011B) Starts from these `approvals.park.*` values (the loaded configuration).
    #[must_use]
    pub fn with_park(self, c: crate::park::ParkConfig) -> Self {
        self.park.store(c);
        self
    }

    /// The default hold timeout (`approvals.timeout_secs`), live.
    pub(crate) fn default_timeout(&self) -> Duration {
        Duration::from_millis(self.timeout.load(Ordering::Relaxed))
    }

    pub(crate) fn report(&self, e: &aurora_db::DbError) {
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
    pub fn with_max_timeout(self, max: Duration) -> Self {
        self.max_timeout.store(millis(max), Ordering::Relaxed);
        self
    }

    /// (SET-5) Replaces the default timeout and the instance maximum after an `approvals`
    /// settings swap; holds already waiting keep the deadline they were given.
    pub fn set_timeouts(&self, timeout: Duration, max: Duration) {
        self.timeout.store(millis(timeout), Ordering::Relaxed);
        self.max_timeout.store(millis(max), Ordering::Relaxed);
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
        let timeout = Duration::from_millis(self.timeout.load(Ordering::Relaxed));
        let max = Duration::from_millis(self.max_timeout.load(Ordering::Relaxed));
        match rule_timeout_secs {
            None => timeout,
            Some(_) => Duration::from_secs(aurora_core::limits::effective_approval_timeout_secs(
                rule_timeout_secs,
                timeout.as_secs(),
                self.min_timeout.as_secs(),
                max.as_secs(),
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

    pub(crate) async fn emit(&self, id: Uuid, workspace_id: Uuid, event: &str) {
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

    /// Records the hold and waits for its resolution. (FIX-146, D-232 (8)) The wait is one
    /// `approval hold` span, a child of the request's `gateway` span: its duration is the time the
    /// request was parked, and the resumed request continues under the same parent.
    pub async fn hold(&self, r: HoldRequest<'_>) -> HoldOutcome {
        self.hold_with_id(r).await.1
    }

    /// [`Holds::hold`], also returning the id of the approval row once one was recorded (the
    /// request-log row of every hold carries it, C-827); `None` when nothing was stored.
    pub async fn hold_with_id(&self, r: HoldRequest<'_>) -> (Option<Uuid>, HoldOutcome) {
        let span = tracing::info_span!("approval_hold", otel.name = "approval hold");
        let mut recorded = None;
        let out = tracing::Instrument::instrument(self.hold_inner(r, &mut recorded), span).await;
        (recorded, out)
    }

    async fn hold_inner(&self, r: HoldRequest<'_>, recorded: &mut Option<Uuid>) -> HoldOutcome {
        // (RFE-014) A new hold cannot be persisted while the database is unreachable.
        if self.db_down() {
            return HoldOutcome::StoreUnavailable;
        }
        let id = Uuid::now_v7();
        let ws = r.identity.workspace_id;
        let timeout = self.effective_timeout(r.rule_timeout_secs);
        // Register before inserting so a fast decision cannot be missed.
        let waiter = self.hub.approval_waiter(id);
        let Some(row) = self.approval_row(id, &r, timeout) else {
            return HoldOutcome::StoreUnavailable;
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
        *recorded = Some(id);
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

    /// The `pending` row of a hold or a park (`None` when the summary cannot be serialized).
    pub(crate) fn approval_row(
        &self,
        id: Uuid,
        r: &HoldRequest<'_>,
        timeout: Duration,
    ) -> Option<NewApproval> {
        let summary = serde_json::to_value(r.summary).ok()?;
        Some(NewApproval {
            id,
            org_id: r.identity.org_id,
            workspace_id: r.identity.workspace_id,
            agent_id: r.identity.agent_id,
            conversation_id: r.identity.conversation_id,
            turn_id: r.identity.turn_id,
            gateway_instance_id: self.instance_id,
            method: r.method.to_owned(),
            host: r.host.to_owned(),
            path: r.path.to_owned(),
            connection_id: r.connection_id,
            app_id: r.app_id.map(clip),
            tool_id: r.tool_id.map(clip),
            rule_logical_id: r.rule.map(|x| x.0),
            rule_name: r
                .rule
                .map(|x| x.1.to_owned())
                .or_else(|| r.package_ask.then(|| PACKAGE_ASK_REASON.to_owned())),
            summary,
            batch_id: r.batch.batch_id.as_deref().map(clip),
            batch_label: r.batch.batch_label.as_deref().map(clip),
            context_label: r.identity.context_label.as_deref().map(clip),
            batch_total: r.batch.batch_total.and_then(|n| i32::try_from(n).ok()),
            timeout_secs: timeout.as_secs_f64(),
        })
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
