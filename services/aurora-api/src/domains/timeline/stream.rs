//! (P5-REFINE-9, C-1084, SEC-143) The live timeline: one shared LISTEN connection per API
//! process (`aurora_timeline`, `aurora_conversation`, `aurora_approval`) fanned out to the open
//! streams, each of which turns hints into rows for its own viewer.
//!
//! A hint names what changed, never text. The stream loads the row by id and builds it with the
//! feed's own functions, so the privacy rule of C-1083 applies to every event: a row the viewer
//! may not see is never built, so it is never sent. The viewer's access is re-read before every
//! batch and on a timer; losing `can_use` ends the stream.

use super::dto::{TimelineItem, TimelineRequestEvent, TimelineRowEvent};
use super::feed::{AgentCtx, Feed, Filter, Viewer};
use crate::app::AppState;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel, Listener};
use aurora_db::repo::timeline::{self as repo, Hint, Source};
use axum::response::sse::Event;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OnceCell, broadcast};
use tokio::time::Instant;
use uuid::Uuid;

/// Per-row coalescing: one batch at most every 500 ms (C-1084).
pub const COALESCE: Duration = Duration::from_millis(500);
/// Events one stream may have waiting; beyond it the queue is dropped for a `resync` (C-1084).
pub const QUEUE_MAX: usize = 200;
/// The drift check (C-1084).
pub const HEARTBEAT: Duration = Duration::from_secs(30);
/// Rows the drift check compares.
const HEAD: usize = 20;
/// Rows a stream remembers having sent (beyond it it forgets and re-reads the head).
const KNOWN_MAX: usize = 5_000;
/// Request-log rows read per `request` hint.
const REQUESTS_PER_HINT: i64 = 500;
/// Conversation → agent entries the bus keeps.
const CONV_CACHE_MAX: usize = 50_000;

#[derive(Debug, Clone)]
pub enum Msg {
    Hint(Hint),
    /// A durable transcript event was committed (`aurora_conversation {c, s}`).
    Seq {
        conversation: Uuid,
        seq: i64,
    },
    /// A gateway approval changed (`aurora_approval {id, workspace_id}`).
    Approval {
        id: Uuid,
        workspace: Uuid,
    },
    /// Notifications may have been missed (the listener reconnected).
    Resync,
}

#[derive(Default)]
pub struct TimelineBus {
    tx: OnceCell<broadcast::Sender<Msg>>,
    agents: Mutex<HashMap<Uuid, Uuid>>,
    slots: Mutex<HashMap<Uuid, u32>>,
}

fn parse(channel: Channel, p: &serde_json::Value) -> Option<Msg> {
    let uuid = |k: &str| p.get(k)?.as_str()?.parse::<Uuid>().ok();
    match channel {
        Channel::Timeline => Hint::from_json(p).map(Msg::Hint),
        Channel::Conversation => Some(Msg::Seq {
            conversation: uuid("c")?,
            seq: p.get("s")?.as_i64()?,
        }),
        Channel::Approval => Some(Msg::Approval {
            id: uuid("id")?,
            workspace: uuid("workspace_id")?,
        }),
        _ => None,
    }
}

impl TimelineBus {
    /// Subscribe; the LISTEN is active when this returns.
    pub async fn subscribe(&self, pool: &PgPool) -> Result<broadcast::Receiver<Msg>, ApiError> {
        let tx = self
            .tx
            .get_or_try_init(|| async {
                let mut listener = Listener::connect(
                    pool,
                    &[Channel::Timeline, Channel::Conversation, Channel::Approval],
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "timeline bus connect failed");
                    ApiError::unavailable("bus_unavailable", "Event bus unavailable.")
                })?;
                let (tx, _) = broadcast::channel(1024);
                let out = tx.clone();
                tokio::spawn(async move {
                    loop {
                        let msg = match listener.next().await {
                            BusEvent::Resync => Msg::Resync,
                            BusEvent::Notification { channel, payload } => {
                                match parse(channel, &payload) {
                                    Some(m) => m,
                                    None => continue,
                                }
                            }
                        };
                        let _ = out.send(msg);
                    }
                });
                Ok::<_, ApiError>(tx)
            })
            .await?;
        Ok(tx.subscribe())
    }

    /// The agent of a conversation (cached: it never changes).
    pub async fn agent_of(&self, pool: &PgPool, conversation: Uuid) -> Option<Uuid> {
        if let Some(a) = self
            .agents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&conversation)
        {
            return Some(*a);
        }
        let a = repo::conversation_agent(pool, conversation).await.ok()??;
        let mut m = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() >= CONV_CACHE_MAX {
            m.clear();
        }
        m.insert(conversation, a);
        Some(a)
    }

    /// One of the person's `TIMELINE_STREAMS_PER_USER` streams on this replica (C-1084).
    pub fn acquire(self: &Arc<Self>, user: Uuid) -> Result<Slot, ApiError> {
        let mut g = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let n = g.entry(user).or_insert(0);
        if *n >= aurora_core::limits::TIMELINE_STREAMS_PER_USER {
            return Err(ApiError::rate_limited(
                5,
                "Too many timelines are open. Close one and try again.",
            ));
        }
        *n += 1;
        metrics::gauge!("aurora_timeline_streams").increment(1.0);
        Ok(Slot {
            bus: self.clone(),
            user,
        })
    }
}

/// An open stream's place; released on every exit path.
pub struct Slot {
    bus: Arc<TimelineBus>,
    user: Uuid,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut g = self.bus.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = g.get_mut(&self.user) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.remove(&self.user);
            }
        }
        metrics::gauge!("aurora_timeline_streams").decrement(1.0);
    }
}

/// What changed since the last batch.
#[derive(Debug, Default)]
struct Dirty {
    convs: HashMap<Uuid, Vec<i64>>,
    rows: HashSet<(SourceKey, Uuid)>,
    /// The oldest `ts` (µs) of new request-log rows.
    requests_since: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SourceKey(&'static str);

impl Dirty {
    fn is_empty(&self) -> bool {
        self.convs.is_empty() && self.rows.is_empty() && self.requests_since.is_none()
    }
}

fn hash_of(it: &TimelineItem) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(it).unwrap_or_default().hash(&mut h);
    h.finish()
}

pub struct Stream {
    s: AppState,
    c: Caller,
    viewer: Viewer,
    agent: AgentCtx,
    technical: bool,
    rx: broadcast::Receiver<Msg>,
    pending: VecDeque<Event>,
    dirty: Dirty,
    flush_at: Option<Instant>,
    last_flush: Instant,
    heartbeat: tokio::time::Interval,
    access: tokio::time::Interval,
    known: HashMap<String, u64>,
    seen_requests: HashSet<Uuid>,
    seen_order: VecDeque<Uuid>,
    opened_at: DateTime<Utc>,
    ended: bool,
    _slot: Slot,
}

impl Stream {
    pub async fn open(
        s: AppState,
        c: Caller,
        viewer: Viewer,
        agent: AgentCtx,
        technical: bool,
        slot: Slot,
    ) -> Result<Self, ApiError> {
        let rx = s.timeline_bus.subscribe(&s.pool).await?;
        let mut heartbeat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let every = s.stream_recheck.min(HEARTBEAT);
        let mut access = tokio::time::interval_at(Instant::now() + every, every);
        access.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut st = Self {
            s,
            c,
            viewer,
            agent,
            technical,
            rx,
            pending: VecDeque::new(),
            dirty: Dirty::default(),
            flush_at: None,
            last_flush: Instant::now(),
            heartbeat,
            access,
            known: HashMap::new(),
            seen_requests: HashSet::new(),
            seen_order: VecDeque::new(),
            opened_at: Utc::now(),
            ended: false,
            _slot: slot,
        };
        // The baseline the drift check compares with: what the client's first page shows.
        st.check_head(false).await;
        Ok(st)
    }

    /// The SSE body.
    pub fn into_events(self) -> impl futures::Stream<Item = Result<Event, Infallible>> {
        futures::stream::unfold(self, |mut st| async move {
            loop {
                if let Some(ev) = st.pending.pop_front() {
                    return Some((Ok(ev), st));
                }
                if st.ended {
                    return None;
                }
                st.step().await;
            }
        })
    }

    fn push_resync(&mut self) {
        self.pending.clear();
        self.pending
            .push_back(Event::default().event("resync").data("{}"));
        metrics::counter!("aurora_timeline_events_total", "kind" => "resync").increment(1);
    }

    fn mark(&mut self) {
        if self.flush_at.is_none() {
            let soon = Instant::now() + Duration::from_millis(50);
            self.flush_at = Some(soon.max(self.last_flush + COALESCE));
        }
    }

    async fn step(&mut self) {
        let far = Instant::now() + Duration::from_secs(3600);
        tokio::select! {
            m = self.rx.recv() => match m {
                Ok(m) => self.on_msg(m).await,
                Err(broadcast::error::RecvError::Lagged(_)) => self.push_resync(),
                Err(broadcast::error::RecvError::Closed) => self.ended = true,
            },
            () = tokio::time::sleep_until(self.flush_at.unwrap_or(far)), if self.flush_at.is_some() => {
                self.flush_at = None;
                self.last_flush = Instant::now();
                self.flush().await;
            }
            _ = self.heartbeat.tick() => {
                if self.refresh_access().await {
                    self.check_head(true).await;
                }
            }
            _ = self.access.tick() => {
                self.refresh_access().await;
            }
        }
    }

    async fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Resync => self.push_resync(),
            Msg::Hint(h) if h.agent == self.agent.id => {
                if h.source == Source::Request {
                    let q = h.q.unwrap_or_else(|| Utc::now().timestamp_micros());
                    self.dirty.requests_since =
                        Some(self.dirty.requests_since.map_or(q, |p| p.min(q)));
                } else {
                    self.dirty.rows.insert((SourceKey(h.source.as_str()), h.id));
                }
                self.mark();
            }
            Msg::Hint(_) => {}
            Msg::Seq { conversation, seq } => {
                if self
                    .s
                    .timeline_bus
                    .agent_of(&self.s.pool, conversation)
                    .await
                    == Some(self.agent.id)
                {
                    let v = self.dirty.convs.entry(conversation).or_default();
                    if v.len() < 1000 {
                        v.push(seq);
                    }
                    self.mark();
                }
            }
            Msg::Approval { id, workspace } if workspace == self.agent.workspace_id => {
                self.dirty
                    .rows
                    .insert((SourceKey(Source::Approval.as_str()), id));
                self.mark();
            }
            Msg::Approval { .. } => {}
        }
    }

    /// Re-read the viewer's access to the agent's workspace (role changes apply at once). False
    /// (and the stream ends) when the viewer lost it.
    async fn refresh_access(&mut self) -> bool {
        match self.c.workspace(&self.s, self.agent.workspace_id).await {
            Ok(auth) => {
                self.viewer.auth = auth;
                true
            }
            // Fail closed (SEC-143): whatever the reason, a stream that cannot confirm the viewer's
            // access ends (the web reconnects and is checked again by `resolve`); keeping the
            // last role would keep sending rows to someone who may have lost it.
            Err(e) => {
                if !e.status().is_client_error() {
                    tracing::warn!(error = %e.code(), "timeline stream access check failed; ending the stream");
                }
                self.ended = true;
                false
            }
        }
    }

    async fn feed(&self) -> Option<Feed<'_>> {
        Feed::new(&self.s, self.viewer.clone(), self.agent.clone())
            .await
            .map_err(|e| tracing::warn!(error = %e.code(), "timeline feed unavailable"))
            .ok()
    }

    fn remember_request(&mut self, id: Uuid) -> bool {
        if !self.seen_requests.insert(id) {
            return false;
        }
        self.seen_order.push_back(id);
        if self.seen_order.len() > 4 * REQUESTS_PER_HINT as usize
            && let Some(old) = self.seen_order.pop_front()
        {
            self.seen_requests.remove(&old);
        }
        true
    }

    async fn flush(&mut self) {
        if self.dirty.is_empty() || !self.refresh_access().await {
            return;
        }
        let dirty = std::mem::take(&mut self.dirty);
        match self.build(dirty).await {
            Ok((items, requests)) => {
                for it in items {
                    let h = hash_of(&it);
                    if self.known.get(&it.id) == Some(&h) {
                        continue;
                    }
                    if self.known.len() >= KNOWN_MAX {
                        self.known.clear();
                    }
                    self.known.insert(it.id.clone(), h);
                    metrics::counter!("aurora_timeline_events_total", "kind" => it.kind.as_str())
                        .increment(1);
                    let ev = TimelineRowEvent {
                        op: "upsert".to_owned(),
                        item: it,
                    };
                    self.pending.push_back(
                        Event::default()
                            .event("row")
                            .json_data(&ev)
                            .unwrap_or_else(|_| Event::default().comment("encode error")),
                    );
                }
                for r in requests {
                    metrics::counter!("aurora_timeline_events_total", "kind" => "request")
                        .increment(1);
                    self.pending.push_back(
                        Event::default()
                            .event("request")
                            .json_data(&r)
                            .unwrap_or_else(|_| Event::default().comment("encode error")),
                    );
                }
                if self.pending.len() > QUEUE_MAX {
                    self.push_resync();
                }
            }
            Err(e) => {
                tracing::warn!(error = %e.code(), "timeline stream reload failed");
                self.push_resync();
            }
        }
    }

    /// The rows (and Technical-view entries) the dirty set names, newest first.
    async fn build(
        &mut self,
        dirty: Dirty,
    ) -> Result<(Vec<TimelineItem>, Vec<TimelineRequestEvent>), ApiError> {
        let pool = self.s.pool.clone();
        let mut turns: HashSet<Uuid> = HashSet::new();
        for (conv, seqs) in &dirty.convs {
            turns.extend(repo::turns_of_events(&pool, *conv, seqs).await?);
        }
        let mut singles: Vec<repo::CallRow> = Vec::new();
        let mut new_requests: Vec<repo::CallRow> = Vec::new();
        if let Some(q) = dirty.requests_since {
            let since = DateTime::from_timestamp_micros(q).unwrap_or_else(Utc::now);
            let rows = repo::calls_since(
                &pool,
                self.agent.workspace_id,
                self.agent.id,
                since,
                REQUESTS_PER_HINT,
            )
            .await?;
            for r in rows {
                if !self.remember_request(r.id) {
                    continue;
                }
                if self.technical {
                    new_requests.push(r.clone());
                }
                match r.turn_id {
                    Some(t) => {
                        turns.insert(t);
                    }
                    None => singles.push(r),
                }
            }
        }
        let Some(feed) = self.feed().await else {
            return Err(ApiError::unavailable("timeline_unavailable", "Try again."));
        };
        let mut items: Vec<TimelineItem> = Vec::new();
        let turns: Vec<Uuid> = turns.into_iter().collect();
        items.extend(feed.rows_of_turns(&turns).await?);
        items.extend(singles.iter().filter_map(|r| feed.single_call_item(r)));
        for (SourceKey(src), id) in &dirty.rows {
            if let Some(src) = Source::parse(src) {
                items.extend(feed.load(src, *id).await?);
            }
        }
        items.sort_by(|a, b| (b.at, &b.id).cmp(&(a.at, &a.id)));
        // (SEC-143) The same visibility check as the rows, and a slim event: no path, context label
        // or secret id; the host only with a visible conversation.
        let requests = new_requests
            .iter()
            .filter_map(|r| feed.request_event(r))
            .collect();
        Ok((items, requests))
    }

    /// Compare the newest rows with what this stream knows: a row it never saw that is newer than
    /// the stream, or a known row that changed, means a hint was lost (`resync`).
    async fn check_head(&mut self, report: bool) {
        let Some(feed) = self.feed().await else {
            return;
        };
        let Ok((head, _)) = feed.page(None, HEAD, &Filter::default()).await else {
            return;
        };
        drop(feed);
        let mut drift = false;
        for it in head {
            let h = hash_of(&it);
            match self.known.get(&it.id) {
                Some(k) if *k == h => {}
                Some(_) => drift = true,
                None => drift |= it.at > self.opened_at,
            }
            if self.known.len() >= KNOWN_MAX {
                self.known.clear();
            }
            self.known.insert(it.id, h);
        }
        if drift && report {
            self.push_resync();
        }
    }
}
