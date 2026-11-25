//! Web Push delivery (phase 5, P4-011B-3; C-832, C-834; SEC-75, SEC-76). A sweep finds approvals
//! that wait with nobody looking, works out who may decide them *now*, and pushes one end-to-end
//! encrypted, fixed-field message per opted-in device through the destination guard.
//!
//! * Trigger: the approvals bus `created` event plus [`APPROVAL_PUSH_DELAY`], and a periodic
//!   catch-up sweep for approvals up to [`CATCH_UP`] old (a replica that was down, a lost notify).
//!   Every sweep is idempotent: the per-(approval, user, stage) claim is an insert-on-conflict.
//! * Audience at send time: deciders of the approval (the e-mail decider set, minus its e-mail
//!   switch) with no live watcher in the workspace, owning a subscription that is neither paused,
//!   disabled nor expired, whose key can still be served and whose registering session is live.
//! * Never in the payload, a log line, a metric label or an error: the endpoint, the keys, a JWT,
//!   or any approval title, summary, rule, host, path or body (SEC-75).
//!
//! A push never affects the approval, its decision or the e-mail path.

use super::crypto::{self, VapidKey};
use super::dto::PushState;
use super::service::{SealedSub, open_sub};
use super::{PushRuntime, check_endpoint, state_of, vapid};
use crate::app::AppState;
use crate::domains::approvals::bus::BusMsg;
use aurora_config::ApiConfig;
use aurora_core::limits::{
    APPROVAL_PUSH_DELAY, APPROVAL_WATCHER_TTL, PUSH_DISABLE_AFTER_FAILURES, PUSH_PAYLOAD_MAX,
    PUSH_PER_USER_HOUR, PUSH_RETRY_MAX, PUSH_TTL_MAX,
};
use aurora_core::{ApiError, sanitize};
use aurora_db::repo::push::{self as repo, SubRow, Waiting};
use aurora_db::repo::{approval_notify, approvals, sessions};
use chrono::{SecondsFormat, Utc};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, broadcast};
use uuid::Uuid;

/// Periodic catch-up sweep (C-834: "a 10 s catch-up").
pub const SWEEP_EVERY: Duration = Duration::from_secs(10);
/// Approvals older than this are not pushed by the initial stage any more (C-834: ≤ 10 min old).
pub const CATCH_UP: Duration = Duration::from_secs(600);
/// Approvals handled per sweep.
const SWEEP_LIMIT: i64 = 100;
/// `agent` / `workspace` names in a `show_details` payload (C-832).
const NAME_MAX: usize = 40;
/// One push is retried within this window (C-834).
const RETRY_WINDOW: Duration = Duration::from_secs(60);
/// Floor of the `TTL` header (C-832).
const TTL_MIN_SECS: u64 = 60;
/// `TTL` of a digest and of a test push (C-832).
const TTL_SHORT_SECS: u64 = 3600;
/// The gauge `aurora_push_subscriptions{state}` is refreshed at most this often.
const GAUGE_EVERY: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------------------------
// Payload (C-832)
// ---------------------------------------------------------------------------------------------

/// The message types of Payload v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pending,
    Reminder,
    Digest,
    Test,
    /// (FIX-167 Part B, C-1023) A new decision item waits in Administration.
    AdminAttention,
    /// (P5-POLISH-4, C-1030) An organization owner or admin: a person asks for an app to be set up.
    AppSetupRequest,
    /// (P5-POLISH-4) The asker: the app is ready.
    AppSetupResolved,
    /// (P5-POLISH-4) The asker: an owner or admin is not setting the app up now.
    AppSetupDeclined,
    /// (P5-SCHED-ONCE, C-1095) A one-time schedule's answer is ready in its creator's chat.
    ScheduleAnswer,
    /// (FIX-234, C-1161) An administrator gave themselves workspace access; the owners are told.
    SelfGrant,
    /// (FIX-236, D-478(c), C-1165) An org owner opened emergency read access to a workspace.
    EmergencyAccess,
}

impl Kind {
    fn type_str(self) -> &'static str {
        match self {
            Self::AppSetupRequest => "app_setup_request",
            Self::AppSetupResolved => "app_setup_resolved",
            Self::AppSetupDeclined => "app_setup_declined",
            Self::Pending => "approval.pending",
            Self::Reminder => "approval.reminder",
            Self::Digest => "approval.digest",
            Self::Test => "test",
            Self::AdminAttention => "admin_attention",
            Self::ScheduleAnswer => "schedule_answer",
            Self::SelfGrant => "self_grant",
            Self::EmergencyAccess => "emergency_access",
        }
    }
}

/// One encrypted-to-be message: plaintext plus the headers that differ per message.
#[derive(Debug)]
pub struct Message {
    pub plaintext: Vec<u8>,
    pub ttl_secs: u64,
    pub urgency: &'static str,
    pub topic: String,
}

fn path_of(w: &Waiting, focus: bool) -> String {
    let base = format!("/o/{}/w/{}/approvals", w.org_slug, w.workspace_slug);
    if focus {
        format!("{base}?focus={}", w.id)
    } else {
        base
    }
}

/// `Topic` (C-832, D-266(e)): the first 32 base64url characters of SHA-256 over the subscription
/// id and the subject (approval id bytes, or a fixed word for a digest or a test): a push service
/// can neither learn an approval id nor tie one approval to a person's other devices.
pub fn topic(subscription: Uuid, subject: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(subscription.as_bytes());
    h.update(subject);
    let mut t = crypto::b64u(&h.finalize());
    t.truncate(32);
    t
}

/// The payload of one approval push for one device. Only the fields of C-832 ever appear;
/// `agent`/`workspace` only when the device opted into details.
pub fn approval_message(kind: Kind, sub: &SubRow, w: &Waiting) -> Result<Message, String> {
    let mut v = json!({
        "v": 1,
        "type": kind.type_str(),
        "approval_id": w.id,
        "workspace_id": w.workspace_id,
        "expires_at": w.expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        "parked": w.parked,
        "url": path_of(w, true),
    });
    if sub.show_details {
        v["agent"] = json!(sanitize::one_line(&w.agent_name, NAME_MAX));
        v["workspace"] = json!(sanitize::one_line(&w.workspace_name, NAME_MAX));
    }
    let remaining = (w.expires_at - Utc::now()).num_seconds().max(0) as u64;
    finish_message(
        v,
        remaining.min(PUSH_TTL_MAX.as_secs()).max(TTL_MIN_SECS),
        "high",
        topic(sub.id, w.id.as_bytes()),
    )
}

/// The digest that stands for approvals beyond the hourly cap (`count` waiting; the link is the
/// approvals page of the newest one's workspace).
pub fn digest_message(sub: &SubRow, w: &Waiting, count: usize) -> Result<Message, String> {
    let v = json!({
        "v": 1,
        "type": Kind::Digest.type_str(),
        "count": count,
        "url": path_of(w, false),
    });
    finish_message(v, TTL_SHORT_SECS, "normal", topic(sub.id, b"digest"))
}

/// (FIX-167 Part B, C-1023) The push of a new administration decision item: its type, the fixed
/// url `/admin` and, only on a device that turned names on, the item's title (names and versions,
/// never a detail, a setting or a secret). The notification text itself is fixed (C-835).
pub fn attention_message(sub: &SubRow, title: &str, subject: &[u8]) -> Result<Message, String> {
    let mut v = json!({
        "v": 1,
        "type": Kind::AdminAttention.type_str(),
        "url": "/admin",
    });
    if sub.show_details {
        v["title"] = json!(sanitize::one_line(title, 2 * NAME_MAX));
    }
    finish_message(v, PUSH_TTL_MAX.as_secs(), "normal", topic(sub.id, subject))
}

/// (P5-POLISH-4, C-1030, SEC-135) The push of an app setup notice: its type, the dashboard path it opens
/// and, only on a device that turned names on, the notice's title (an app name and display names, never
/// anything else). The notification text itself is fixed per type in the service worker.
pub fn app_setup_message(
    sub: &SubRow,
    kind: Kind,
    href: &str,
    title: &str,
    subject: &[u8],
) -> Result<Message, String> {
    let mut v = json!({ "v": 1, "type": kind.type_str(), "url": href });
    if sub.show_details {
        v["title"] = json!(sanitize::one_line(title, 2 * NAME_MAX));
    }
    finish_message(v, PUSH_TTL_MAX.as_secs(), "normal", topic(sub.id, subject))
}

/// (P5-SCHED-ONCE, C-1095, SEC-144) The push of a one-time schedule's answer: its type, the
/// conversation's dashboard path and, only on a device that turned names on, the schedule's name.
/// Never the answer or the prompt: a lock screen shows the fixed text of the service worker.
pub fn schedule_answer_message(
    sub: &SubRow,
    href: &str,
    name: &str,
    run: Uuid,
) -> Result<Message, String> {
    let mut v = json!({ "v": 1, "type": Kind::ScheduleAnswer.type_str(), "url": href });
    if sub.show_details {
        v["title"] = json!(sanitize::one_line(name, 2 * NAME_MAX));
    }
    finish_message(
        v,
        PUSH_TTL_MAX.as_secs(),
        "normal",
        topic(sub.id, format!("schedule_answer\n{run}").as_bytes()),
    )
}

/// (FIX-234, C-1161) The push of a self-grant notice to a workspace's owners: the workspace's
/// People page and, only on a device that turned names on, the fixed sentence (names and the
/// role, never content).
pub fn self_grant_message(sub: &SubRow, title: &str, href: &str) -> Result<Message, String> {
    let mut v = json!({ "v": 1, "type": Kind::SelfGrant.type_str(), "url": href });
    if sub.show_details {
        v["title"] = json!(sanitize::one_line(title, 2 * NAME_MAX));
    }
    finish_message(
        v,
        PUSH_TTL_MAX.as_secs(),
        "normal",
        topic(sub.id, href.as_bytes()),
    )
}

/// (FIX-234, C-1161) One push per device of `user` for a self-grant notice. Returns the pushes
/// attempted; does nothing with push off.
pub async fn notify_self_grant(state: &AppState, user: Uuid, title: &str, href: &str) -> usize {
    if !state.push.enabled {
        return 0;
    }
    let Ok(devices) = deliverable(state, user).await else {
        return 0;
    };
    send_all(state, &devices, |s| self_grant_message(s, title, href)).await
}

/// (FIX-236, D-478(c), C-1165) One push per device of `user` for an emergency-access notice.
pub async fn notify_emergency_access(
    state: &AppState,
    user: Uuid,
    title: &str,
    href: &str,
) -> usize {
    if !state.push.enabled {
        return 0;
    }
    let Ok(devices) = deliverable(state, user).await else {
        return 0;
    };
    send_all(state, &devices, |s| {
        emergency_access_message(s, title, href)
    })
    .await
}

/// (FIX-236) The payload of one emergency-access notice push.
pub fn emergency_access_message(sub: &SubRow, title: &str, href: &str) -> Result<Message, String> {
    let mut v = json!({ "v": 1, "type": Kind::EmergencyAccess.type_str(), "url": href });
    if sub.show_details {
        v["title"] = json!(sanitize::one_line(title, 2 * NAME_MAX));
    }
    finish_message(
        v,
        PUSH_TTL_MAX.as_secs(),
        "normal",
        topic(sub.id, href.as_bytes()),
    )
}

/// The dashboard path of a conversation: the agent page with `?c=` (D-102).
pub fn conversation_path(org: &str, workspace: &str, agent: &str, conversation: Uuid) -> String {
    format!("/o/{org}/w/{workspace}/agents/{agent}?c={conversation}")
}

/// The "test" push: nothing but its type.
pub fn test_message(sub: &SubRow) -> Result<Message, String> {
    finish_message(
        json!({ "v": 1, "type": Kind::Test.type_str() }),
        TTL_SHORT_SECS,
        "normal",
        topic(sub.id, b"test"),
    )
}

fn finish_message(
    v: Value,
    ttl_secs: u64,
    urgency: &'static str,
    topic: String,
) -> Result<Message, String> {
    let plaintext = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
    if plaintext.len() > PUSH_PAYLOAD_MAX {
        return Err("payload over the limit".into());
    }
    Ok(Message {
        plaintext,
        ttl_secs,
        urgency,
        topic,
    })
}

// ---------------------------------------------------------------------------------------------
// The destination-guarded sender (C-834, SEC-76)
// ---------------------------------------------------------------------------------------------

/// What a send came to (metric `aurora_push_sent_total{result}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    /// 404 / 410: the subscription is gone.
    Gone,
    /// 400 / 413 and other refusals of the message.
    Rejected,
    Forbidden,
    Throttled,
    Error,
    Timeout,
    /// The endpoint's host is not (any more) on the allow-list.
    RejectedHost,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Gone => "gone",
            Self::Rejected => "rejected",
            Self::Forbidden => "forbidden",
            Self::Throttled => "throttled",
            Self::Error => "error",
            Self::Timeout => "timeout",
            Self::RejectedHost => "rejected_host",
        }
    }

    fn of_status(status: u16) -> Self {
        match status {
            200..=299 => Self::Sent,
            404 | 410 => Self::Gone,
            401 | 403 => Self::Forbidden,
            429 => Self::Throttled,
            400..=499 => Self::Rejected,
            _ => Self::Error,
        }
    }
}

/// 429 and 5xx answers are retried; connection, DNS, TLS and time-out failures are not.
fn status_retryable(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

/// A destination that resolved to nothing a push service may be.
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("destination refused")
    }
}

impl std::error::Error for Refused {}

/// Keep only publicly routable unicast addresses (FIX-106's table, `aurora_core::ranges`, with the
/// translated forms); nothing left refuses the destination. The sender dials exactly these
/// addresses, so the answer that was judged is the answer that is used (pinned resolution).
fn public_only(all: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, Refused> {
    let kept: Vec<SocketAddr> = all
        .into_iter()
        .filter(|a| !aurora_core::ranges::is_special(a.ip()))
        .collect();
    if kept.is_empty() {
        Err(Refused)
    } else {
        Ok(kept)
    }
}

/// Judged at connect time, so a name that changed its answer after registration is still refused.
/// No `gateway.allowed_private` exception: push services are public hosts.
struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let all: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            let kept = public_only(all)?;
            Ok(Box::new(kept.into_iter()) as Addrs)
        })
    }
}

/// The outbound side of Web Push.
pub struct Sender {
    /// Public addresses only, no redirects, `push.timeout_secs`.
    http: reqwest::Client,
    /// Test seam (`push.dev_endpoint_overrides`, loopback targets only): no resolver guard, no
    /// redirects, same timeout. `None` unless overrides are configured.
    dev: Option<reqwest::Client>,
    overrides: BTreeMap<String, reqwest::Url>,
    permits: Arc<Semaphore>,
}

impl Sender {
    pub fn from_config(cfg: &ApiConfig) -> Result<Self, String> {
        let timeout = Duration::from_secs(cfg.push.timeout_secs);
        let build = |guarded: bool| {
            let mut b = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout)
                .connect_timeout(timeout)
                .user_agent("aurora-push");
            if guarded {
                // (FIX-217, SEC-04) A system proxy would bypass the guarded resolver.
                b = b.no_proxy().dns_resolver(Arc::new(GuardedResolver));
            }
            b.build().map_err(|e| format!("push http client: {e}"))
        };
        let mut overrides = BTreeMap::new();
        for (host, target) in &cfg.push.dev_endpoint_overrides {
            let url = reqwest::Url::parse(target)
                .map_err(|_| format!("push.dev_endpoint_overrides: {host}: not a URL"))?;
            overrides.insert(host.clone(), url);
        }
        Ok(Self {
            http: build(true)?,
            dev: if overrides.is_empty() {
                None
            } else {
                Some(build(false)?)
            },
            overrides,
            permits: Arc::new(Semaphore::new(cfg.push.concurrency.max(1) as usize)),
        })
    }

    /// Where a request for `endpoint` really goes and with which client.
    fn target(&self, endpoint: &reqwest::Url, host: &str) -> (reqwest::Url, &reqwest::Client) {
        if let (Some(base), Some(dev)) = (self.overrides.get(host), self.dev.as_ref()) {
            let mut u = base.clone();
            u.set_path(endpoint.path());
            u.set_query(endpoint.query());
            return (u, dev);
        }
        (endpoint.clone(), &self.http)
    }

    /// One POST; the answer's body is never read (a slow or endless body cannot hold the pool).
    async fn post_once(
        &self,
        url: reqwest::Url,
        client: &reqwest::Client,
        authorization: &str,
        msg: &Message,
        body: Vec<u8>,
    ) -> (Outcome, Option<u64>, bool) {
        let req = client
            .post(url)
            .header("Authorization", authorization)
            .header("TTL", msg.ttl_secs.to_string())
            .header("Urgency", msg.urgency)
            .header("Topic", &msg.topic)
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .body(body);
        match req.send().await {
            Ok(resp) => {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok());
                let status = resp.status().as_u16();
                (
                    Outcome::of_status(status),
                    retry_after,
                    status_retryable(status),
                )
            }
            Err(e) if e.is_timeout() => (Outcome::Timeout, None, false),
            Err(_) => (Outcome::Error, None, false),
        }
    }

    /// Send one encrypted message: allow-list re-check, destination guard, bounded pool, up to
    /// [`PUSH_RETRY_MAX`] tries within [`RETRY_WINDOW`] for 429/5xx, honouring `Retry-After` ≤ 60 s.
    pub async fn deliver(
        &self,
        rt: &PushRuntime,
        allowed: &[String],
        key: &VapidKey,
        endpoint: &str,
        msg: &Message,
        body: &[u8],
    ) -> Outcome {
        let Ok((url, host)) = check_endpoint(allowed, endpoint) else {
            return Outcome::RejectedHost;
        };
        let aud = format!("https://{host}");
        let Ok(_permit) = self.permits.acquire().await else {
            return Outcome::Error;
        };
        let started = Instant::now();
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let (target, client) = self.target(&url, &host);
            let (outcome, retry_after, retryable) = self
                .post_once(
                    target,
                    client,
                    &rt.authorization(key, &aud),
                    msg,
                    body.to_vec(),
                )
                .await;
            if !retryable || attempt >= PUSH_RETRY_MAX {
                return outcome;
            }
            let wait = Duration::from_secs(retry_after.unwrap_or(u64::from(attempt)));
            if wait > RETRY_WINDOW || started.elapsed() + wait >= RETRY_WINDOW {
                return outcome;
            }
            tokio::time::sleep(wait).await;
        }
    }
}

/// Encrypt and send `msg` to one device, then record the outcome. `None` = nothing was sent (the
/// device's key can no longer be served, or its material is unreadable).
async fn push_to(state: &AppState, row: &SubRow, msg: &Message) -> Option<Outcome> {
    if !state.push.enabled {
        return None;
    }
    let sealed: SealedSub = open_sub(state, row).await.ok()?;
    let key = vapid::key_for(state, row.vapid_key_id).await.ok()??;
    let keys = crypto::parse_subscription_keys(&sealed.p256dh, &sealed.auth).ok()?;
    let body = crypto::encrypt(&msg.plaintext, &keys.public, &keys.auth).ok()?;
    let started = Instant::now();
    let outcome = state
        .push
        .sender
        .deliver(
            &state.push,
            &state.config.push.allowed_endpoint_hosts,
            &key,
            &sealed.endpoint,
            msg,
            &body,
        )
        .await;
    metrics::histogram!("aurora_push_duration_seconds").record(started.elapsed().as_secs_f64());
    metrics::counter!("aurora_push_sent_total", "result" => outcome.label()).increment(1);
    let recorded = match outcome {
        Outcome::Sent => {
            repo::record_outcome(&state.pool, row.id, true, PUSH_DISABLE_AFTER_FAILURES).await
        }
        Outcome::Gone => repo::delete_by_id(&state.pool, row.id).await,
        Outcome::RejectedHost => repo::disable(&state.pool, row.id).await,
        _ => repo::record_outcome(&state.pool, row.id, false, PUSH_DISABLE_AFTER_FAILURES).await,
    };
    if let Err(e) = recorded {
        tracing::warn!(error = %e, "Web Push outcome could not be recorded");
    }
    Some(outcome)
}

/// `POST /{id}/test`: push the "test" payload to one device in the background (the outcome is
/// visible in the row's timestamps).
pub fn spawn_test(state: AppState, sub: Uuid) {
    tokio::spawn(async move {
        let Ok(Some(row)) = repo::get(&state.pool, sub).await else {
            return;
        };
        if let Ok(msg) = test_message(&row) {
            push_to(&state, &row, &msg).await;
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Audience, claims, batching (C-834)
// ---------------------------------------------------------------------------------------------

/// The devices of `user` that may be pushed to right now.
async fn deliverable(state: &AppState, user: Uuid) -> Result<Vec<SubRow>, ApiError> {
    let lifetimes = state.auth();
    let mut out = Vec::new();
    for r in repo::list_for_user(&state.pool, user).await? {
        // A device on a rotated-out key is still served until it migrates (retired keys live
        // `PUSH_VAPID_RETIRE`); paused, disabled and expired ones never are.
        if !matches!(state_of(&r), PushState::Active | PushState::StaleKey) {
            continue;
        }
        if !sessions::is_live(
            &state.pool,
            r.session_id,
            lifetimes.auth.session_idle_secs,
            lifetimes.auth.session_absolute_secs,
        )
        .await?
        {
            continue;
        }
        if vapid::key_for(state, r.vapid_key_id).await?.is_none() {
            continue;
        }
        out.push(r);
    }
    Ok(out)
}

async fn send_all(
    state: &AppState,
    devices: &[SubRow],
    build: impl Fn(&SubRow) -> Result<Message, String>,
) -> usize {
    let mut sent = 0;
    for d in devices {
        match build(d) {
            Ok(msg) => {
                if push_to(state, d, &msg).await.is_some() {
                    sent += 1;
                }
            }
            Err(_) => tracing::warn!("Web Push payload could not be built"),
        }
    }
    sent
}

/// Push one approval to its deciders at one stage; returns the number of device pushes attempted.
async fn serve(state: &AppState, w: &Waiting, reminder: bool) -> Result<usize, ApiError> {
    let stage = if reminder { "reminder" } else { "initial" };
    let kind = if reminder {
        Kind::Reminder
    } else {
        Kind::Pending
    };
    let mut sent = 0;
    let deciders = approval_notify::deciders_with(
        &state.pool,
        w.org_id,
        w.workspace_id,
        w.conversation_id,
        false,
    )
    .await?;
    for d in &deciders {
        if repo::claimed(&state.pool, w.id, d.user_id, stage).await? {
            continue;
        }
        let devices = deliverable(state, d.user_id).await?;
        if devices.is_empty() {
            continue;
        }
        if !reminder
            && let Some(b) = w.batch_id.as_deref()
            && repo::batch_served(&state.pool, d.user_id, w.agent_id, b).await?
        {
            repo::claim(&state.pool, w.id, d.user_id, &w.kind, stage, "batch").await?;
            continue;
        }
        // Decided, expired or frozen since the sweep read it: nothing is sent (C-834).
        if !repo::still_pending(&state.pool, w.id, &w.kind).await? {
            return Ok(sent);
        }
        if reminder {
            if repo::claim(&state.pool, w.id, d.user_id, &w.kind, stage, "push").await? {
                sent += send_all(state, &devices, |s| approval_message(kind, s, w)).await;
            }
            continue;
        }
        let (pushes, digest_sent) = repo::hourly(&state.pool, d.user_id).await?;
        if pushes < i64::from(PUSH_PER_USER_HOUR) {
            if !repo::claim(&state.pool, w.id, d.user_id, &w.kind, stage, "push").await? {
                continue; // another replica won
            }
            if let Some(b) = w.batch_id.as_deref() {
                for sib in repo::batch_siblings(&state.pool, w.agent_id, b, w.id).await? {
                    repo::claim(&state.pool, sib, d.user_id, &w.kind, stage, "batch").await?;
                }
            }
            sent += send_all(state, &devices, |s| approval_message(kind, s, w)).await;
        } else if digest_sent {
            // Over the cap and a digest already went out this hour: folded silently.
            repo::claim(&state.pool, w.id, d.user_id, &w.kind, stage, "digest").await?;
        } else if repo::claim(&state.pool, w.id, d.user_id, &w.kind, stage, "digest").await? {
            let waiting = approvals::list_pending_for(&state.pool, d.user_id, None, None, 200)
                .await?
                .len();
            sent += send_all(state, &devices, |s| digest_message(s, w, waiting)).await;
        }
    }
    Ok(sent)
}

/// One delivery sweep; returns the number of device pushes attempted. Does nothing with push off.
pub async fn sweep_once(state: &AppState) -> Result<usize, ApiError> {
    if !state.push.enabled {
        return Ok(0);
    }
    let ttl = APPROVAL_WATCHER_TTL.as_secs_f64();
    let mut sent = 0;
    for w in &repo::waiting(
        &state.pool,
        APPROVAL_PUSH_DELAY.as_secs_f64(),
        CATCH_UP.as_secs_f64(),
        ttl,
        SWEEP_LIMIT,
    )
    .await?
    {
        sent += serve(state, w, false).await?;
    }
    // The reminder stage of a parked approval (C-828 timing), its own claim per (approval, user).
    for w in &repo::reminders(&state.pool, ttl, SWEEP_LIMIT).await? {
        sent += serve(state, w, true).await?;
    }
    Ok(sent)
}

/// (FIX-167 Part B, D-370, C-1023) One push per device of each administrator for every decision
/// item that first appeared since the last sweep: claim the notice (`pushed_at`), then send. The
/// device's own opt-in decides (a device that never turned push on has no subscription); the
/// e-mail switch does not apply. Does nothing with push off. Returns the pushes attempted.
pub async fn sweep_attention(state: &AppState) -> Result<usize, ApiError> {
    if !state.push.enabled {
        return Ok(0);
    }
    let mut sent = 0;
    for n in aurora_db::repo::admin_attention::claim_push(
        &state.pool,
        crate::domains::admin::attention::PUSH_MAX_AGE.as_secs_f64(),
        SWEEP_LIMIT,
    )
    .await?
    {
        if !n.is_admin {
            continue;
        }
        let devices = deliverable(state, n.user_id).await?;
        let subject = format!("{}\n{}\n{}", n.kind, n.subject_id, n.subject_version);
        sent += send_all(state, &devices, |s| {
            attention_message(s, &n.title, subject.as_bytes())
        })
        .await;
    }
    Ok(sent)
}

/// (P5-POLISH-4, D-386) One push per device for every app setup notice since the last sweep: claim the
/// notice (`pushed_at`), then send. The device's own opt-in decides. Does nothing with push off.
pub async fn sweep_app_setup(state: &AppState) -> Result<usize, ApiError> {
    if !state.push.enabled {
        return Ok(0);
    }
    let mut sent = 0;
    for n in aurora_db::repo::app_setup_requests::claim_push(
        &state.pool,
        crate::domains::app_setup_requests::service::PUSH_MAX_AGE.as_secs_f64(),
        SWEEP_LIMIT,
    )
    .await?
    {
        if !n.active {
            continue;
        }
        let kind = match n.kind.as_str() {
            "resolved" => Kind::AppSetupResolved,
            "declined" => Kind::AppSetupDeclined,
            _ => Kind::AppSetupRequest,
        };
        let devices = deliverable(state, n.user_id).await?;
        let subject = format!("{}\n{}", n.request_id, n.kind);
        sent += send_all(state, &devices, |s| {
            app_setup_message(s, kind, &n.href, &n.title, subject.as_bytes())
        })
        .await;
    }
    Ok(sent)
}

/// (P5-SCHED-ONCE, D-422, C-1095) Runs older than this are never pushed.
pub const SCHEDULE_ANSWER_MAX_AGE: Duration = Duration::from_secs(3600);

/// (P5-SCHED-ONCE, C-1095, SEC-144) One push per opted-in device of a schedule's creator for
/// every one-time run that happened in their own chat and ended `done`: claim the run
/// (`answer_pushed_at`), then send. The claim requires the conversation to still be the
/// creator's direct chat, so no other person's devices are ever reached. Does nothing with push
/// off. Returns the pushes attempted.
pub async fn sweep_schedule_answers(state: &AppState) -> Result<usize, ApiError> {
    if !state.push.enabled {
        return Ok(0);
    }
    let claimed = {
        let mut conn = state
            .pool
            .acquire()
            .await
            .map_err(aurora_db::DbError::from)?;
        aurora_db::repo::schedules::claim_answer_push(
            &mut conn,
            SCHEDULE_ANSWER_MAX_AGE.as_secs_f64(),
            SWEEP_LIMIT,
        )
        .await?
    };
    let mut sent = 0;
    for n in claimed {
        let devices = deliverable(state, n.user_id).await?;
        let href = conversation_path(
            &n.org_slug,
            &n.workspace_slug,
            &n.agent_handle,
            n.conversation_id,
        );
        sent += send_all(state, &devices, |s| {
            schedule_answer_message(s, &href, &n.schedule_name, n.run_id)
        })
        .await;
    }
    Ok(sent)
}

async fn refresh_gauges(state: &AppState) {
    match repo::counts_by_state(&state.pool).await {
        Ok(rows) => {
            for s in ["active", "paused", "disabled", "expired", "stale_key"] {
                let n = rows.iter().find(|(k, _)| k == s).map_or(0, |(_, n)| *n);
                metrics::gauge!("aurora_push_subscriptions", "state" => s).set(n as f64);
            }
        }
        Err(e) => tracing::warn!(error = %e, "Web Push gauge refresh failed"),
    }
}

/// What woke the loop.
enum Wake {
    Created,
    Other,
}

async fn next_event(rx: &mut Option<broadcast::Receiver<BusMsg>>) -> Wake {
    let Some(r) = rx.as_mut() else {
        return std::future::pending().await;
    };
    loop {
        match r.recv().await {
            Ok(BusMsg::Event { event, .. }) if event == "created" => return Wake::Created,
            Ok(BusMsg::Event { .. }) => {}
            Ok(BusMsg::Resync) | Err(broadcast::error::RecvError::Lagged(_)) => return Wake::Other,
            Err(broadcast::error::RecvError::Closed) => {
                *rx = None;
                return std::future::pending().await;
            }
        }
    }
}

/// The delivery loop: a sweep on every `created` event after the delay, and one every
/// [`SWEEP_EVERY`] as the catch-up. Started only with push on.
pub fn spawn(state: AppState) {
    if !state.push.enabled {
        return;
    }
    tokio::spawn(async move {
        let mut rx = match state.approval_bus.subscribe(&state.pool).await {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(error = %e.message(), "Web Push falls back to the catch-up sweep only");
                None
            }
        };
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut gauges: Option<Instant> = None;
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                wake = next_event(&mut rx) => {
                    if matches!(wake, Wake::Created) {
                        tokio::time::sleep(APPROVAL_PUSH_DELAY + Duration::from_millis(250)).await;
                    }
                }
            }
            if let Err(e) = sweep_once(&state).await {
                tracing::warn!(error = %e.message(), "Web Push sweep failed");
            }
            if let Err(e) = sweep_attention(&state).await {
                tracing::warn!(error = %e.message(), "Web Push administration sweep failed");
            }
            if let Err(e) = sweep_app_setup(&state).await {
                tracing::warn!(error = %e.message(), "Web Push app setup sweep failed");
            }
            if let Err(e) = sweep_schedule_answers(&state).await {
                tracing::warn!(error = %e.message(), "Web Push schedule answer sweep failed");
            }
            if gauges.is_none_or(|t| t.elapsed() >= GAUGE_EVERY) {
                refresh_gauges(&state).await;
                gauges = Some(Instant::now());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as Dur;

    fn sub(show_details: bool) -> SubRow {
        SubRow {
            id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            session_id: Uuid::now_v7(),
            vapid_key_id: None,
            endpoint_host: "fcm.googleapis.com".into(),
            secret_ref: String::new(),
            label: String::new(),
            show_details,
            paused_at: None,
            expires_at: None,
            failure_count: 0,
            disabled_at: None,
            last_success_at: None,
            last_failure_at: None,
            created_at: Utc::now(),
            key_status: None,
        }
    }

    fn waiting(agent: &str, workspace: &str) -> Waiting {
        Waiting {
            id: Uuid::now_v7(),
            kind: "gateway".into(),
            org_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            agent_id: Uuid::now_v7(),
            conversation_id: None,
            batch_id: None,
            expires_at: Utc::now() + Dur::minutes(10),
            parked: true,
            agent_name: agent.into(),
            workspace_name: workspace.into(),
            workspace_slug: "w".repeat(60),
            org_slug: "o".repeat(60),
        }
    }

    fn fields(m: &Message) -> Vec<String> {
        let v: Value = serde_json::from_slice(&m.plaintext).unwrap();
        v.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn payload_has_only_the_fixed_fields_and_details_only_on_request() {
        let w = waiting("Deployer", "Prod");
        let plain = approval_message(Kind::Pending, &sub(false), &w).unwrap();
        let mut f = fields(&plain);
        f.sort();
        assert_eq!(
            f,
            [
                "approval_id",
                "expires_at",
                "parked",
                "type",
                "url",
                "v",
                "workspace_id"
            ]
        );
        let text = String::from_utf8(plain.plaintext.clone()).unwrap();
        assert!(!text.contains("Deployer") && !text.contains("Prod"));
        let detailed = approval_message(Kind::Reminder, &sub(true), &w).unwrap();
        let mut f = fields(&detailed);
        f.sort();
        assert_eq!(
            f,
            [
                "agent",
                "approval_id",
                "expires_at",
                "parked",
                "type",
                "url",
                "v",
                "workspace",
                "workspace_id"
            ]
        );
        assert_eq!(detailed.urgency, "high");
        let digest = digest_message(&sub(true), &w, 7).unwrap();
        let mut f = fields(&digest);
        f.sort();
        assert_eq!(f, ["count", "type", "url", "v"]);
        assert_eq!(digest.urgency, "normal");
        assert_eq!(digest.ttl_secs, 3600);
        let test = test_message(&sub(true)).unwrap();
        let mut f = fields(&test);
        f.sort();
        assert_eq!(f, ["type", "v"]);
    }

    #[test]
    fn payload_stays_within_one_kib_with_maximal_names() {
        // 4-byte characters, 40 of each, and the longest slugs the dashboard allows.
        let big = "\u{1F600}".repeat(60);
        let w = waiting(&big, &big);
        let m = approval_message(Kind::Pending, &sub(true), &w).unwrap();
        assert!(
            m.plaintext.len() <= PUSH_PAYLOAD_MAX,
            "{}",
            m.plaintext.len()
        );
        let v: Value = serde_json::from_slice(&m.plaintext).unwrap();
        assert_eq!(v["agent"].as_str().unwrap().chars().count(), NAME_MAX);
    }

    #[test]
    fn ttl_follows_the_remaining_time_within_bounds() {
        let mut w = waiting("a", "w");
        w.expires_at = Utc::now() + Dur::seconds(5);
        assert_eq!(
            approval_message(Kind::Pending, &sub(false), &w)
                .unwrap()
                .ttl_secs,
            60
        );
        w.expires_at = Utc::now() + Dur::days(30);
        assert_eq!(
            approval_message(Kind::Pending, &sub(false), &w)
                .unwrap()
                .ttl_secs,
            PUSH_TTL_MAX.as_secs()
        );
    }

    #[test]
    fn topic_differs_per_device_and_never_holds_the_approval_id() {
        let w = waiting("a", "w");
        let (a, b) = (sub(false), sub(false));
        let ta = approval_message(Kind::Pending, &a, &w).unwrap().topic;
        let tb = approval_message(Kind::Pending, &b, &w).unwrap().topic;
        assert_ne!(ta, tb);
        assert_eq!(ta.len(), 32);
        assert!(!ta.contains(&w.id.simple().to_string()) && !ta.contains(&w.id.to_string()));
        assert_ne!(topic(a.id, b"digest"), topic(a.id, w.id.as_bytes()));
        assert!(
            ta.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
    }

    /// (P5-POLISH-4, SEC-135) An app setup push carries its type and the dashboard path; the title only on a
    /// device that opted into names. Nothing else is ever in it.
    #[test]
    fn an_app_setup_push_holds_only_its_type_a_path_and_an_opted_in_title() {
        let path = "/o/acme/apps?tab=apps&app=google-calendar";
        for (kind, ty) in [
            (Kind::AppSetupRequest, "app_setup_request"),
            (Kind::AppSetupResolved, "app_setup_resolved"),
            (Kind::AppSetupDeclined, "app_setup_declined"),
        ] {
            let plain =
                app_setup_message(&sub(false), kind, path, "Bob wants Calendar", b"x").unwrap();
            let v: serde_json::Value = serde_json::from_slice(&plain.plaintext).unwrap();
            assert_eq!(v, json!({"v": 1, "type": ty, "url": path}));
            let named =
                app_setup_message(&sub(true), kind, path, "Bob wants Calendar", b"x").unwrap();
            let v: serde_json::Value = serde_json::from_slice(&named.plaintext).unwrap();
            assert_eq!(v["title"], "Bob wants Calendar");
            assert_eq!(v.as_object().unwrap().len(), 4);
        }
    }

    #[test]
    fn statuses_map_to_the_documented_results() {
        for (s, o) in [
            (201, Outcome::Sent),
            (404, Outcome::Gone),
            (410, Outcome::Gone),
            (400, Outcome::Rejected),
            (413, Outcome::Rejected),
            (403, Outcome::Forbidden),
            (429, Outcome::Throttled),
            (500, Outcome::Error),
            (503, Outcome::Error),
            (302, Outcome::Error),
        ] {
            assert_eq!(Outcome::of_status(s), o, "{s}");
        }
    }

    #[test]
    fn the_resolver_judges_what_a_name_resolves_to_now() {
        let sa = |s: &str| -> SocketAddr { format!("{s}:0").parse().unwrap() };
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.9",
            "169.254.169.254",
            "[::1]",
            "[fd00::1]",
        ] {
            assert!(public_only(vec![sa(private)]).is_err(), "{private}");
        }
        // A mixed answer keeps only the public address: the private one is never dialled.
        let kept = public_only(vec![sa("10.0.0.1"), sa("8.8.8.8")]).unwrap();
        assert_eq!(kept, vec![sa("8.8.8.8")]);
        assert!(public_only(vec![]).is_err());
    }

    /// (P5-SCHED-ONCE, C-1095, SEC-144) The reminder push: type and path only, the schedule's
    /// name only on a device that asked for names, one line and capped; a topic per device.
    #[test]
    fn schedule_answer_payload_is_fixed_and_names_only_on_request() {
        let conv = Uuid::now_v7();
        let href = conversation_path("org", "ws", "bot", conv);
        assert_eq!(href, format!("/o/org/w/ws/agents/bot?c={conv}"));
        let run = Uuid::now_v7();
        let plain = schedule_answer_message(&sub(false), &href, "Call Ada", run).unwrap();
        let mut f = fields(&plain);
        f.sort();
        assert_eq!(f, ["type", "url", "v"]);
        assert_eq!(
            (plain.urgency, plain.ttl_secs),
            ("normal", PUSH_TTL_MAX.as_secs())
        );
        let named = schedule_answer_message(&sub(true), &href, "Call\nAda", run).unwrap();
        let v: Value = serde_json::from_slice(&named.plaintext).unwrap();
        assert_eq!(v["type"], "schedule_answer");
        assert_eq!(v["title"], "Call Ada");
        let long = schedule_answer_message(&sub(true), &href, &"x".repeat(500), run).unwrap();
        let v: Value = serde_json::from_slice(&long.plaintext).unwrap();
        assert!(v["title"].as_str().unwrap().chars().count() <= 2 * NAME_MAX);
        assert_ne!(plain.topic, named.topic, "a topic per device");
    }
}
