//! (P5-REFINE-9, C-1083, SEC-143) One agent's timeline: the sources read newest first, folded
//! into rows, filtered by who may see them, and merged by `(at, id)`.
//!
//! **Privacy.** A row that comes from a conversation is built only when `can_see_conversation`
//! holds for the viewer (another person's `direct` conversation: only that person and the
//! workspace's owners). The check runs before a row exists, so a hidden conversation's turn, its
//! commands, files, hosts and calls are never counted into anything. Rows of a conversation that
//! is gone, or of another workspace, are omitted; `eval` conversations are not on the timeline.
//!
//! **Paging.** Each source returns its rows at or before the cursor's time, newest first, with
//! `limit + 1` rows; a source that returned that many has a *horizon* (its oldest row): above
//! it every one of its rows is known. Rows are emitted newest first down to the highest horizon
//! only, so a row is never skipped and never returned twice (the cursor is the last emitted
//! `(at, id)`, or the horizon when the page ended there).

use super::computer::{self, StepKind};
use super::dto::{
    TimelineItem, TimelineKind, TimelineLink, TimelineLinkType, TimelinePerson, TimelineRef,
    TimelineRule, TimelineState,
};
use crate::app::AppState;
use crate::domains::schedules::results;
use aurora_core::ApiError;
use aurora_db::authz::{WorkspaceAuth, can_see_conversation};
use aurora_db::repo::timeline::{
    self as repo, AccessItemRow, ActionApprovalItemRow, ApprovalItemRow, AutomationItemRow,
    CallRow, PeerItemRow, StepRow, TurnRow,
};
use aurora_registry::snapshot::Snapshot;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Default and largest page (C-1083).
pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 100;
/// At most this many `computer` rows per turn are separate; the rest fold into the last (C-1083).
pub const COMPUTER_ROWS_PER_TURN: usize = 3;
/// Turns created before a page's horizon that may still have steps above it.
const OVERLAP_TURNS: i64 = 200;

/// Who reads the timeline: the caller and their access to the agent's workspace.
#[derive(Debug, Clone)]
pub struct Viewer {
    pub user: Uuid,
    pub auth: WorkspaceAuth,
}

/// The agent whose timeline it is.
#[derive(Debug, Clone)]
pub struct AgentCtx {
    pub id: Uuid,
    pub name: String,
    pub workspace_id: Uuid,
    /// The harness its turns ran under (the computer classification's table).
    pub harness: String,
}

/// A row's conversation, as far as visibility is concerned.
#[derive(Debug, Clone, Copy)]
pub struct ConvFacts<'a> {
    pub id: Option<Uuid>,
    pub kind: Option<&'a str>,
    pub direct_user: Option<Uuid>,
    pub workspace: Option<Uuid>,
}

impl Viewer {
    /// SEC-143: may this viewer see a row from `c`? No conversation: yes (`can_use` was checked
    /// by the route). A conversation that is gone, in another workspace, or an evaluation: no.
    /// Otherwise `can_see_conversation` (docs/04).
    pub fn sees(&self, agent_ws: Uuid, c: ConvFacts<'_>) -> bool {
        if c.id.is_none() {
            return true;
        }
        let (Some(kind), Some(ws)) = (c.kind, c.workspace) else {
            return false;
        };
        ws == agent_ws
            && kind != "eval"
            && can_see_conversation(&self.auth, self.user, kind, c.direct_user)
    }
}

/// `kind` and `outcome` filters (comma-separated in the query).
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub kinds: Option<HashSet<TimelineKind>>,
    pub outcomes: Option<HashSet<String>>,
}

impl Filter {
    pub fn parse(kind: Option<&str>, outcome: Option<&str>) -> Result<Self, ApiError> {
        let split = |s: &str| -> Vec<String> {
            s.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let kinds = match kind {
            None => None,
            Some(s) => Some(
                split(s)
                    .iter()
                    .map(|k| {
                        TimelineKind::parse(k).ok_or_else(|| {
                            ApiError::invalid_field("Invalid kind.", "/kind", "unknown kind")
                        })
                    })
                    .collect::<Result<HashSet<_>, _>>()?,
            ),
        };
        let outcomes = match outcome {
            None => None,
            Some(s) => {
                let v = split(s);
                if v.iter().any(|o| {
                    o.len() > 40 || !o.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                }) {
                    return Err(ApiError::invalid_field(
                        "Invalid outcome.",
                        "/outcome",
                        "unknown outcome",
                    ));
                }
                Some(v.into_iter().collect())
            }
        };
        Ok(Self { kinds, outcomes })
    }

    fn wants(&self, kinds: &[TimelineKind]) -> bool {
        self.kinds
            .as_ref()
            .is_none_or(|k| kinds.iter().any(|x| k.contains(x)))
    }

    pub fn keep(&self, it: &TimelineItem) -> bool {
        self.kinds.as_ref().is_none_or(|k| k.contains(&it.kind))
            && self
                .outcomes
                .as_ref()
                .is_none_or(|o| it.outcome.as_ref().is_some_and(|x| o.contains(x)))
    }
}

/// The opaque cursor: the last emitted `(at, id)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub t: DateTime<Utc>,
    pub i: String,
}

/// Sorts after every row id (`id < MAX_ID` for all of them).
const MAX_ID: &str = "~";

impl Cursor {
    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).unwrap_or_default())
    }

    pub fn decode(s: &str) -> Result<Self, ApiError> {
        let bad = || ApiError::invalid_field("Invalid cursor.", "/cursor", "malformed cursor");
        let bytes = URL_SAFE_NO_PAD.decode(s).map_err(|_| bad())?;
        let c: Self = serde_json::from_slice(&bytes).map_err(|_| bad())?;
        if c.i.len() > 200 {
            return Err(bad());
        }
        Ok(c)
    }

    fn before(&self, it: &TimelineItem) -> bool {
        (it.at, it.id.as_str()) < (self.t, self.i.as_str())
    }
}

/// What a turn's events and calls add to its rows.
#[derive(Debug, Default)]
struct TurnData {
    steps: HashMap<Uuid, Vec<StepRow>>,
    calls: HashMap<Uuid, Vec<CallRow>>,
    last_seq: HashMap<Uuid, i64>,
}

impl TurnData {
    /// The turn's tool calls (every `tool.started`).
    fn tool_count(&self, turn: Uuid) -> i64 {
        self.steps.get(&turn).map_or(0, |s| {
            s.iter().filter(|e| e.ty == "tool.started").count() as i64
        })
    }

    /// The model id the turn's LLM requests used most (C-1083); a value that is not a model id
    /// is never returned.
    fn model(&self, turn: Uuid) -> Option<String> {
        let mut n: Vec<(&str, usize)> = Vec::new();
        for c in self.calls.get(&turn).into_iter().flatten() {
            let Some(m) = c.llm_model.as_deref() else {
                continue;
            };
            if !aurora_core::model_id::validate_model_id(m) {
                continue;
            }
            match n.iter_mut().find(|x| x.0 == m) {
                Some(x) => x.1 += 1,
                None => n.push((m, 1)),
            }
        }
        // The most frequent; on a tie the first used.
        let mut best: Option<(&str, usize)> = None;
        for x in n {
            if best.is_none_or(|b| x.1 > b.1) {
                best = Some(x);
            }
        }
        best.map(|b| b.0.to_owned())
    }
}

pub struct Feed<'a> {
    s: &'a AppState,
    pub viewer: Viewer,
    pub agent: AgentCtx,
    snap: Option<Snapshot>,
}

impl<'a> Feed<'a> {
    pub async fn new(s: &'a AppState, viewer: Viewer, agent: AgentCtx) -> Result<Self, ApiError> {
        let snap = crate::domains::apps::service::snapshot(s).await?;
        Ok(Self {
            s,
            viewer,
            agent,
            snap,
        })
    }

    fn pool(&self) -> &aurora_db::PgPool {
        &self.s.pool
    }

    fn sees(&self, c: ConvFacts<'_>) -> bool {
        self.viewer.sees(self.agent.workspace_id, c)
    }

    fn agent_ref(&self) -> TimelineRef {
        TimelineRef {
            id: self.agent.id.to_string(),
            name: self.agent.name.clone(),
        }
    }

    fn app_ref(&self, app_id: &str) -> TimelineRef {
        let name = self
            .snap
            .as_ref()
            .and_then(|s| s.app(app_id))
            .map_or_else(|| app_id.to_owned(), |a| a.about.name.clone());
        TimelineRef {
            id: app_id.to_owned(),
            name,
        }
    }

    fn item(&self, id: String, at: DateTime<Utc>, kind: TimelineKind) -> TimelineItem {
        TimelineItem {
            id,
            at,
            ended_at: None,
            state: TimelineState::Done,
            kind,
            outcome: None,
            agent: self.agent_ref(),
            person: None,
            app: None,
            tool: None,
            host: None,
            rule: None,
            model: None,
            schedule: None,
            webhook: None,
            peer: None,
            count: None,
            command: None,
            files: None,
            duration_ms: None,
            link: None,
        }
    }

    // ------------------------------------------------------------------ the page

    /// One page, newest first, and the cursor of the next (older) one.
    pub async fn page(
        &self,
        cursor: Option<&Cursor>,
        limit: usize,
        f: &Filter,
    ) -> Result<(Vec<TimelineItem>, Option<Cursor>), ApiError> {
        let t = cursor.map(|c| c.t);
        let fetch = i64::try_from(limit).unwrap_or(100) + 1;
        let full = |n: usize| i64::try_from(n).unwrap_or(i64::MAX) >= fetch;
        let pool = self.pool();
        let agent = self.agent.id;
        let mut horizons: Vec<DateTime<Utc>> = Vec::new();
        let mut items: Vec<TimelineItem> = Vec::new();

        let mut turns: Vec<TurnRow> = Vec::new();
        let mut derived_from: HashSet<Uuid> = HashSet::new();
        if f.wants(&[
            TimelineKind::Answered,
            TimelineKind::Computer,
            TimelineKind::Called,
        ]) {
            turns = repo::turns_before(pool, agent, t, fetch).await?;
            if full(turns.len())
                && let Some(h) = turns.last().map(|r| r.created_at)
            {
                horizons.push(h);
                let ids: Vec<Uuid> = turns.iter().map(|r| r.id).collect();
                let not_before = h - chrono::Duration::from_std(aurora_core::limits::TURN_CEILING)
                    .unwrap_or_else(|_| chrono::Duration::hours(6));
                let over = repo::turns_overlapping(pool, agent, h, not_before, &ids, OVERLAP_TURNS)
                    .await?;
                turns.extend(over);
            }
            derived_from = turns.iter().map(|r| r.id).collect();
        }
        let (runs, deliveries) = (
            if f.wants(&[TimelineKind::RanSchedule]) {
                let r = repo::runs_before(pool, agent, t, fetch).await?;
                if full(r.len()) {
                    horizons.extend(r.last().map(|x| x.at));
                }
                r
            } else {
                Vec::new()
            },
            if f.wants(&[TimelineKind::ReceivedWebhook]) {
                let d = repo::deliveries_before(pool, agent, t, fetch).await?;
                if full(d.len()) {
                    horizons.extend(d.last().map(|x| x.at));
                }
                d
            } else {
                Vec::new()
            },
        );
        let mut turn_ids: Vec<Uuid> = turns.iter().map(|r| r.id).collect();
        turn_ids.extend(runs.iter().chain(&deliveries).filter_map(|r| r.turn_id));
        turn_ids.sort_unstable();
        turn_ids.dedup();
        let data = self.turn_data(&turn_ids).await?;
        for r in &turns {
            if derived_from.contains(&r.id) {
                items.extend(self.turn_items(r, &data));
            }
        }
        items.extend(runs.iter().filter_map(|r| self.run_item(r, &data)));
        items.extend(
            deliveries
                .iter()
                .filter_map(|r| self.delivery_item(r, &data)),
        );

        if f.wants(&[TimelineKind::Called]) {
            let rows = repo::calls_without_turn(pool, agent, t, fetch).await?;
            if full(rows.len()) {
                horizons.extend(rows.last().map(|x| x.ts));
            }
            items.extend(rows.iter().filter_map(|r| self.single_call_item(r)));
        }
        if f.wants(&[TimelineKind::AskedBefore]) {
            let rows = repo::approvals_before(pool, agent, t, fetch).await?;
            if full(rows.len()) {
                horizons.extend(rows.last().map(|x| x.created_at));
            }
            items.extend(rows.iter().filter_map(|r| self.approval_item(r)));
            let rows = repo::action_approvals_before(pool, agent, t, fetch).await?;
            if full(rows.len()) {
                horizons.extend(rows.last().map(|x| x.created_at));
            }
            items.extend(rows.iter().filter_map(|r| self.action_approval_item(r)));
        }
        if f.wants(&[TimelineKind::HandedTask, TimelineKind::GotTask]) {
            let rows = repo::peers_before(pool, agent, t, fetch).await?;
            if full(rows.len()) {
                horizons.extend(rows.last().map(|x| x.created_at));
            }
            items.extend(rows.iter().map(|r| self.peer_item(r)));
        }
        if f.wants(&[TimelineKind::AskedForAccess]) {
            let rows = repo::access_before(pool, agent, t, fetch).await?;
            if full(rows.len()) {
                horizons.extend(rows.last().map(|x| x.created_at));
            }
            items.extend(rows.iter().filter_map(|r| self.access_item(r)));
        }

        Ok(merge(items, cursor, horizons.into_iter().max(), limit, f))
    }

    // ------------------------------------------------------------------ turns

    async fn turn_data(&self, turns: &[Uuid]) -> Result<TurnData, ApiError> {
        let mut d = TurnData::default();
        if turns.is_empty() {
            return Ok(d);
        }
        for s in repo::steps_of_turns(self.pool(), turns).await? {
            d.steps.entry(s.turn_id).or_default().push(s);
        }
        for c in repo::calls_of_turns(self.pool(), self.agent.id, turns).await? {
            if let Some(t) = c.turn_id {
                d.calls.entry(t).or_default().push(c);
            }
        }
        d.last_seq = repo::last_seqs(self.pool(), turns)
            .await?
            .into_iter()
            .collect();
        Ok(d)
    }

    /// The rows of the given turns of the agent (the stream reloads a turn), with the runs and
    /// deliveries those turns belong to.
    pub async fn rows_of_turns(&self, ids: &[Uuid]) -> Result<Vec<TimelineItem>, ApiError> {
        let turns = repo::turns_by_ids(self.pool(), self.agent.id, ids).await?;
        let runs = repo::runs_of_turns(self.pool(), self.agent.id, ids).await?;
        let deliveries = repo::deliveries_of_turns(self.pool(), self.agent.id, ids).await?;
        let data = self.turn_data(ids).await?;
        let mut out: Vec<TimelineItem> = Vec::new();
        for r in &turns {
            out.extend(self.turn_items(r, &data));
        }
        out.extend(runs.iter().filter_map(|r| self.run_item(r, &data)));
        out.extend(
            deliveries
                .iter()
                .filter_map(|r| self.delivery_item(r, &data)),
        );
        Ok(out)
    }

    fn turn_items(&self, r: &TurnRow, d: &TurnData) -> Vec<TimelineItem> {
        let conv = ConvFacts {
            id: Some(r.conversation_id),
            kind: r.conv_kind.as_deref(),
            direct_user: r.conv_direct_user,
            workspace: r.conv_workspace,
        };
        if !self.sees(conv) {
            return Vec::new();
        }
        let mut out = Vec::new();
        // A run's or delivery's turn is shown by its `ran_schedule` / `received_webhook` row.
        if r.conv_automation.is_none() {
            out.push(self.answered_item(r, d));
        }
        let steps = d.steps.get(&r.id).map_or(&[][..], Vec::as_slice);
        out.extend(self.computer_items(r, steps));
        let calls = d.calls.get(&r.id).map_or(&[][..], Vec::as_slice);
        out.extend(self.called_items(r, calls));
        out
    }

    fn answered_item(&self, r: &TurnRow, d: &TurnData) -> TimelineItem {
        let mut it = self.item(
            format!("turn:{}", r.id),
            r.created_at,
            TimelineKind::Answered,
        );
        let (state, outcome) = match r.status.as_str() {
            "done" => (TimelineState::Done, Some("answered")),
            "failed" => (TimelineState::Failed, Some("failed")),
            "aborted" => (TimelineState::Done, Some("stopped")),
            _ => (TimelineState::Running, None),
        };
        it.state = state;
        it.outcome = outcome.map(str::to_owned);
        it.ended_at = r.finished_at;
        it.person = match r.speaker_kind.as_str() {
            "person" => r.speaker_name.clone(),
            "bot" | "agent" => Some(r.speaker_label.clone()).filter(|l| !l.is_empty()),
            _ => None,
        }
        .map(|display_name| TimelinePerson { display_name });
        it.model = d.model(r.id);
        it.count = Some(d.tool_count(r.id));
        it.duration_ms = duration(r.started_at.or(Some(r.created_at)), r.finished_at);
        it.link = Some(TimelineLink {
            ty: TimelineLinkType::Conversation,
            id: r.conversation_id.to_string(),
            seq: d.last_seq.get(&r.id).copied(),
            path: None,
        });
        it
    }

    fn computer_items(&self, r: &TurnRow, events: &[StepRow]) -> Vec<TimelineItem> {
        let active = matches!(r.status.as_str(), "queued" | "dispatched" | "running");
        let steps = steps_of(&self.agent.harness, events);
        group_steps(steps)
            .into_iter()
            .map(|g| {
                let mut it = self.item(
                    format!("computer:{}:{}", r.id, g.first_seq),
                    g.at,
                    TimelineKind::Computer,
                );
                let (state, outcome) = if g.failed {
                    (TimelineState::Failed, Some("failed"))
                } else if g.open && active {
                    (TimelineState::Running, None)
                } else {
                    (TimelineState::Done, Some("ok"))
                };
                it.state = state;
                it.outcome = outcome.map(str::to_owned);
                it.ended_at = (!g.open).then_some(g.ended_at).flatten();
                it.command = g.command.clone();
                if !g.paths.is_empty() {
                    it.files = Some(
                        g.paths
                            .iter()
                            .take(computer::FILES_MAX)
                            .map(|p| computer::display_path(p))
                            .collect(),
                    );
                    it.count = Some(g.paths.len() as i64);
                }
                let file = (g.paths.len() == 1)
                    .then(|| computer::files_page_path(&g.paths[0]))
                    .flatten();
                it.link = Some(match file {
                    Some(path) => TimelineLink {
                        ty: TimelineLinkType::File,
                        id: self.agent.id.to_string(),
                        seq: None,
                        path: Some(path),
                    },
                    None => TimelineLink {
                        ty: TimelineLinkType::Conversation,
                        id: r.conversation_id.to_string(),
                        seq: Some(g.first_seq),
                        path: None,
                    },
                });
                it
            })
            .collect()
    }

    fn called_items(&self, r: &TurnRow, calls: &[CallRow]) -> Vec<TimelineItem> {
        fold_calls(calls)
            .into_iter()
            .map(|g| {
                let mut it = self.item(
                    format!("call:{}:{}:{}", r.id, g.key, g.outcome),
                    g.first,
                    TimelineKind::Called,
                );
                self.fill_call(&mut it, &g);
                it
            })
            .collect()
    }

    fn fill_call(&self, it: &mut TimelineItem, g: &CallGroup) {
        it.state = g.state;
        it.outcome = Some(g.outcome.to_owned());
        it.ended_at = (g.count > 1).then_some(g.last);
        it.count = Some(g.count);
        it.app = g.app_id.as_deref().map(|a| self.app_ref(a));
        it.tool = g.tool_id.clone();
        it.host = Some(g.host.clone());
        it.rule = g.rule.clone();
        it.link = match (&g.rule, &g.app_id) {
            (Some(rule), _) => Some(TimelineLink {
                ty: TimelineLinkType::Rule,
                id: rule.logical_id.to_string(),
                seq: None,
                path: None,
            }),
            (None, Some(app)) => Some(TimelineLink {
                ty: TimelineLinkType::App,
                id: app.clone(),
                seq: None,
                path: None,
            }),
            _ => None,
        };
    }

    /// A request without a turn: one row of its own (`call:<request id>`).
    pub fn single_call_item(&self, r: &CallRow) -> Option<TimelineItem> {
        if r.llm_model.is_some() || r.turn_id.is_some() {
            return None;
        }
        let conv = ConvFacts {
            id: r.conversation_id,
            kind: r.conv_kind.as_deref(),
            direct_user: r.conv_direct_user,
            workspace: r.conv_workspace,
        };
        if !self.sees(conv) {
            return None;
        }
        let g = fold_calls(std::slice::from_ref(r)).into_iter().next()?;
        let mut it = self.item(format!("call:{}", r.id), r.ts, TimelineKind::Called);
        self.fill_call(&mut it, &g);
        Some(it)
    }

    // ------------------------------------------------------------------ the other sources

    pub fn approval_item(&self, r: &ApprovalItemRow) -> Option<TimelineItem> {
        if r.agent_id != self.agent.id {
            return None;
        }
        let conv = ConvFacts {
            id: r.conversation_id,
            kind: r.conv_kind.as_deref(),
            direct_user: r.conv_direct_user,
            workspace: r.conv_workspace,
        };
        if !self.sees(conv) {
            return None;
        }
        let mut it = self.item(
            format!("approval:{}", r.id),
            r.created_at,
            TimelineKind::AskedBefore,
        );
        let (state, outcome) = match r.status.as_str() {
            "pending" => (TimelineState::Waiting, None),
            "approved" if r.always => (TimelineState::Done, Some("always_allowed")),
            "approved" => (TimelineState::Done, Some("allowed_once")),
            "denied" => (TimelineState::Done, Some("blocked")),
            "expired" => (TimelineState::Done, Some("expired")),
            _ => (TimelineState::Done, Some("abandoned")),
        };
        it.state = state;
        it.outcome = outcome.map(str::to_owned);
        it.ended_at = r.decided_at.or(r.resolved_at);
        it.person = r
            .decided_by_name
            .clone()
            .map(|display_name| TimelinePerson { display_name });
        it.app = r.app_id.as_deref().map(|a| self.app_ref(a));
        it.tool = r.tool_id.clone();
        it.host = Some(r.host.clone());
        it.rule = rule(r.rule_logical_id, r.rule_name.as_deref());
        it.link = Some(TimelineLink {
            ty: TimelineLinkType::Approval,
            id: r.id.to_string(),
            seq: None,
            path: None,
        });
        Some(it)
    }

    pub fn action_approval_item(&self, r: &ActionApprovalItemRow) -> Option<TimelineItem> {
        if r.agent_id != self.agent.id {
            return None;
        }
        let conv = ConvFacts {
            id: r.conversation_id,
            kind: r.conv_kind.as_deref(),
            direct_user: r.conv_direct_user,
            workspace: r.conv_workspace,
        };
        if !self.sees(conv) {
            return None;
        }
        let mut it = self.item(
            format!("action_approval:{}", r.id),
            r.created_at,
            TimelineKind::AskedBefore,
        );
        let (state, outcome) = match r.status.as_str() {
            "pending" => (TimelineState::Waiting, None),
            "approved" => (TimelineState::Running, Some("allowed_once")),
            "executed" => (TimelineState::Done, Some("allowed_once")),
            "failed" => (TimelineState::Failed, Some("allowed_once")),
            "rejected" => (TimelineState::Done, Some("blocked")),
            _ => (TimelineState::Done, Some("expired")),
        };
        it.state = state;
        it.outcome = outcome.map(str::to_owned);
        it.ended_at = r.executed_at.or(r.decided_at);
        it.person = r
            .decided_by_name
            .clone()
            .map(|display_name| TimelinePerson { display_name });
        it.tool = Some(r.action_key.clone());
        it.link = Some(TimelineLink {
            ty: TimelineLinkType::ActionApproval,
            id: r.id.to_string(),
            seq: None,
            path: None,
        });
        Some(it)
    }

    /// The thread link of a run or delivery, when the viewer may open it (D-409's run threads
    /// are `schedule`/`webhook` conversations, visible to `can_use`; checked all the same).
    fn automation_link(&self, r: &AutomationItemRow, fallback: TimelineLinkType) -> TimelineLink {
        let conv = ConvFacts {
            id: r.conversation_id,
            kind: r.conv_kind.as_deref(),
            direct_user: r.conv_direct_user,
            workspace: r.conv_workspace,
        };
        match r.conversation_id {
            Some(c) if self.sees(conv) => TimelineLink {
                ty: TimelineLinkType::Conversation,
                id: c.to_string(),
                seq: None,
                path: None,
            },
            _ => TimelineLink {
                ty: fallback,
                id: r.id.to_string(),
                seq: None,
                path: None,
            },
        }
    }

    fn run_item(&self, r: &AutomationItemRow, d: &TurnData) -> Option<TimelineItem> {
        let mut it = self.item(format!("run:{}", r.id), r.at, TimelineKind::RanSchedule);
        let (result, _) = results::run_result(
            &r.outcome,
            r.turn_status.as_deref(),
            r.turn_error_code.as_deref(),
        );
        it.state = match result {
            "running" => TimelineState::Running,
            "answered" => TimelineState::Done,
            "failed" => TimelineState::Failed,
            _ => TimelineState::Skipped,
        };
        it.outcome = (result != "running").then(|| result.to_owned());
        it.schedule = Some(TimelineRef {
            id: r.owner_id.to_string(),
            name: r.owner_name.clone(),
        });
        self.fill_automation(&mut it, r, d);
        it.link = Some(self.automation_link(r, TimelineLinkType::Run));
        Some(it)
    }

    fn delivery_item(&self, r: &AutomationItemRow, d: &TurnData) -> Option<TimelineItem> {
        let mut it = self.item(
            format!("delivery:{}", r.id),
            r.at,
            TimelineKind::ReceivedWebhook,
        );
        let result = results::delivery_result(
            &r.outcome,
            r.reason.as_deref(),
            r.turn_status.as_deref(),
            r.turn_error_code.as_deref(),
        );
        it.state = match result {
            "running" => TimelineState::Running,
            "answered" => TimelineState::Done,
            "failed" => TimelineState::Failed,
            "duplicate" => TimelineState::Skipped,
            _ => TimelineState::Blocked,
        };
        it.outcome = (result != "running").then(|| result.to_owned());
        it.webhook = Some(TimelineRef {
            id: r.owner_id.to_string(),
            name: r.owner_name.clone(),
        });
        if r.outcome == "accepted" {
            self.fill_automation(&mut it, r, d);
        }
        it.link = Some(self.automation_link(r, TimelineLinkType::Delivery));
        Some(it)
    }

    fn fill_automation(&self, it: &mut TimelineItem, r: &AutomationItemRow, d: &TurnData) {
        let Some(turn) = r.turn_id else {
            return;
        };
        it.ended_at = r.turn_finished_at;
        it.duration_ms = duration(r.turn_started_at, r.turn_finished_at);
        let conv = ConvFacts {
            id: r.conversation_id,
            kind: r.conv_kind.as_deref(),
            direct_user: r.conv_direct_user,
            workspace: r.conv_workspace,
        };
        // Counts and the model come from the thread: only when the viewer may read it.
        if self.sees(conv) {
            it.model = d.model(turn);
            it.count = Some(d.tool_count(turn));
        }
    }

    pub fn peer_item(&self, r: &PeerItemRow) -> TimelineItem {
        let handed = r.from_agent_id == self.agent.id;
        let mut it = self.item(
            format!("peer:{}", r.id),
            r.created_at,
            if handed {
                TimelineKind::HandedTask
            } else {
                TimelineKind::GotTask
            },
        );
        let other = if handed {
            r.to_agent_id
        } else {
            r.from_agent_id
        };
        let (state, outcome) = match r.status.as_str() {
            "completed" => (TimelineState::Done, Some("completed")),
            "cancelled" => (TimelineState::Skipped, Some("cancelled")),
            "expired" => (TimelineState::Failed, Some("expired")),
            _ => (TimelineState::Running, None),
        };
        it.state = state;
        it.outcome = outcome.map(str::to_owned);
        it.ended_at = outcome.is_some().then_some(r.updated_at);
        it.peer = Some(TimelineRef {
            id: other.to_string(),
            name: r.peer_name.clone(),
        });
        it.link = Some(TimelineLink {
            ty: TimelineLinkType::PeerTask,
            id: r.id.to_string(),
            seq: None,
            path: None,
        });
        it
    }

    pub fn access_item(&self, r: &AccessItemRow) -> Option<TimelineItem> {
        if r.agent_id != self.agent.id {
            return None;
        }
        let conv = ConvFacts {
            id: Some(r.conversation_id),
            kind: r.conv_kind.as_deref(),
            direct_user: r.conv_direct_user,
            workspace: r.conv_workspace,
        };
        if !self.sees(conv) {
            return None;
        }
        let mut it = self.item(
            format!("access:{}", r.id),
            r.created_at,
            TimelineKind::AskedForAccess,
        );
        it.state = if r.status == "open" {
            TimelineState::Waiting
        } else {
            TimelineState::Done
        };
        it.outcome = (r.status != "open").then(|| r.status.clone());
        it.ended_at = r.resolved_at;
        match (r.kind.as_str(), r.app_id.as_deref()) {
            ("app", Some(app)) => {
                it.app = Some(self.app_ref(app));
                it.link = Some(TimelineLink {
                    ty: TimelineLinkType::App,
                    id: app.to_owned(),
                    seq: None,
                    path: None,
                });
            }
            _ => {
                it.host = r.host.clone();
                it.link = Some(TimelineLink {
                    ty: TimelineLinkType::Conversation,
                    id: r.conversation_id.to_string(),
                    seq: None,
                    path: None,
                });
            }
        }
        Some(it)
    }

    // ------------------------------------------------------------------ single rows (stream)

    pub async fn load(&self, src: repo::Source, id: Uuid) -> Result<Vec<TimelineItem>, ApiError> {
        let pool = self.pool();
        let agent = self.agent.id;
        let empty = TurnData::default;
        Ok(match src {
            repo::Source::Turn => self.rows_of_turns(&[id]).await?,
            repo::Source::Approval => repo::approval_by_id(pool, id)
                .await?
                .and_then(|r| self.approval_item(&r))
                .into_iter()
                .collect(),
            repo::Source::ActionApproval => repo::action_approval_by_id(pool, id)
                .await?
                .and_then(|r| self.action_approval_item(&r))
                .into_iter()
                .collect(),
            repo::Source::Run => match repo::run_by_id(pool, agent, id).await? {
                Some(r) => {
                    let d = match r.turn_id {
                        Some(t) => self.turn_data(&[t]).await?,
                        None => empty(),
                    };
                    self.run_item(&r, &d).into_iter().collect()
                }
                None => Vec::new(),
            },
            repo::Source::Delivery => match repo::delivery_by_id(pool, agent, id).await? {
                Some(r) => {
                    let d = match r.turn_id {
                        Some(t) => self.turn_data(&[t]).await?,
                        None => empty(),
                    };
                    self.delivery_item(&r, &d).into_iter().collect()
                }
                None => Vec::new(),
            },
            repo::Source::Peer => repo::peer_by_id(pool, agent, id)
                .await?
                .map(|r| self.peer_item(&r))
                .into_iter()
                .collect(),
            repo::Source::Access => repo::access_by_id(pool, id)
                .await?
                .and_then(|r| self.access_item(&r))
                .into_iter()
                .collect(),
            repo::Source::Request => Vec::new(),
        })
    }
}

fn rule(id: Option<Uuid>, name: Option<&str>) -> Option<TimelineRule> {
    Some(TimelineRule {
        logical_id: id?,
        name: name.unwrap_or_default().to_owned(),
    })
}

fn duration(from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> Option<i64> {
    Some((to? - from?).num_milliseconds().max(0))
}

/// Merge the sources' rows into one page (see the module comment).
pub fn merge(
    mut items: Vec<TimelineItem>,
    cursor: Option<&Cursor>,
    horizon: Option<DateTime<Utc>>,
    limit: usize,
    f: &Filter,
) -> (Vec<TimelineItem>, Option<Cursor>) {
    items.retain(|it| {
        cursor.is_none_or(|c| c.before(it)) && horizon.is_none_or(|h| it.at > h) && f.keep(it)
    });
    items.sort_by(|a, b| (b.at, &b.id).cmp(&(a.at, &a.id)));
    items.dedup_by(|a, b| a.id == b.id);
    if items.len() > limit {
        items.truncate(limit);
        let last = items.last().map(|it| Cursor {
            t: it.at,
            i: it.id.clone(),
        });
        return (items, last);
    }
    let next = horizon.map(|h| {
        // Every row above `h` is out; the next page starts with the rows at `h`. Should a source
        // have more than a page of rows at exactly one instant, step past it (never loop).
        let stuck = cursor.is_some_and(|c| c.t == h && c.i == MAX_ID);
        Cursor {
            t: if stuck {
                h - chrono::Duration::microseconds(1)
            } else {
                h
            },
            i: MAX_ID.to_owned(),
        }
    });
    (items, next)
}

// ---------------------------------------------------------------------------------------------
// Folding (pure)
// ---------------------------------------------------------------------------------------------

/// One computer step of a turn.
#[derive(Debug, Clone)]
pub struct StepFact {
    pub seq: i64,
    pub at: DateTime<Utc>,
    pub kind: StepKind,
    pub command: Option<String>,
    pub paths: Vec<String>,
    /// `None` until its `tool.finished`.
    pub ok: Option<bool>,
    pub ended_at: Option<DateTime<Utc>>,
}

/// The computer steps among a turn's tool events, in order.
pub fn steps_of(harness: &str, events: &[StepRow]) -> Vec<StepFact> {
    let mut out: Vec<StepFact> = Vec::new();
    let mut by_call: HashMap<String, usize> = HashMap::new();
    for e in events {
        let call = e
            .payload
            .get("call_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match e.ty.as_str() {
            "tool.started" => {
                let name = e.payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let summary = e.payload.get("summary").and_then(|v| v.as_str());
                if let Some(s) = computer::parse(harness, name, summary) {
                    if !call.is_empty() {
                        by_call.insert(call.to_owned(), out.len());
                    }
                    out.push(StepFact {
                        seq: e.seq,
                        at: e.created_at,
                        kind: s.kind,
                        command: s.command,
                        paths: s.paths,
                        ok: None,
                        ended_at: None,
                    });
                }
            }
            "tool.finished" => {
                if let Some(&i) = by_call.get(call) {
                    out[i].ok = Some(
                        e.payload
                            .get("ok")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true),
                    );
                    out[i].ended_at = Some(e.created_at);
                }
            }
            _ => {}
        }
    }
    out
}

/// A `computer` row: a command and the files written after it.
#[derive(Debug, Clone, PartialEq)]
pub struct StepGroup {
    pub first_seq: i64,
    pub at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub command: Option<String>,
    /// Distinct, in order of writing.
    pub paths: Vec<String>,
    pub failed: bool,
    /// A step has not finished yet.
    pub open: bool,
}

/// Group a turn's steps into rows: each command starts a row, the writes that follow it join
/// it (writes before any command form a row of their own); beyond
/// [`COMPUTER_ROWS_PER_TURN`] rows the rest fold into the last one.
pub fn group_steps(steps: Vec<StepFact>) -> Vec<StepGroup> {
    let mut groups: Vec<StepGroup> = Vec::new();
    for s in steps {
        let start_new = s.kind == StepKind::Shell || groups.is_empty();
        if start_new {
            if groups.len() == COMPUTER_ROWS_PER_TURN {
                // Fold into the last row.
            } else {
                groups.push(StepGroup {
                    first_seq: s.seq,
                    at: s.at,
                    ended_at: None,
                    command: s.command.clone(),
                    paths: Vec::new(),
                    failed: false,
                    open: false,
                });
            }
        }
        let Some(g) = groups.last_mut() else {
            continue;
        };
        if g.command.is_none() && s.kind == StepKind::Shell {
            g.command = s.command.clone();
        }
        for p in s.paths {
            if !g.paths.contains(&p) {
                g.paths.push(p);
            }
        }
        g.failed |= s.ok == Some(false);
        g.open |= s.ok.is_none();
        if s.ended_at > g.ended_at {
            g.ended_at = s.ended_at;
        }
    }
    groups
}

/// A `called` row: a turn's requests to one app or host with one outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct CallGroup {
    pub key: String,
    pub outcome: &'static str,
    pub state: TimelineState,
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
    pub count: i64,
    pub app_id: Option<String>,
    pub tool_id: Option<String>,
    pub host: String,
    pub rule: Option<TimelineRule>,
}

/// The outcome word and state of one request-log decision (C-1083).
pub fn call_outcome(decision: &str, has_rule: bool) -> (&'static str, TimelineState) {
    match decision {
        "allow" | "approved" => ("allowed", TimelineState::Done),
        "blocked" if has_rule => ("blocked_by_rule", TimelineState::Blocked),
        "rate_limited" => ("rate_limited", TimelineState::Blocked),
        "approval_denied" | "approval_expired" => ("not_approved", TimelineState::Blocked),
        "error" => ("failed", TimelineState::Failed),
        _ => ("blocked_default", TimelineState::Blocked),
    }
}

/// Fold a turn's requests (C-1083): model requests (`llm_model` set) are never rows; the rest
/// group by app (else host) and outcome, oldest first.
pub fn fold_calls(calls: &[CallRow]) -> Vec<CallGroup> {
    let mut out: Vec<CallGroup> = Vec::new();
    for c in calls {
        if c.llm_model.is_some() {
            continue;
        }
        let (outcome, state) = call_outcome(&c.decision, c.rule_logical_id.is_some());
        let key = c.app_id.clone().unwrap_or_else(|| c.host.clone());
        match out
            .iter_mut()
            .find(|g| g.key == key && g.outcome == outcome)
        {
            Some(g) => {
                g.count += 1;
                g.first = g.first.min(c.ts);
                g.last = g.last.max(c.ts);
                if c.tool_id.is_some() {
                    g.tool_id.clone_from(&c.tool_id);
                }
                if let Some(r) = rule(c.rule_logical_id, c.rule_name.as_deref()) {
                    g.rule = Some(r);
                }
            }
            None => out.push(CallGroup {
                key,
                outcome,
                state,
                first: c.ts,
                last: c.ts,
                count: 1,
                app_id: c.app_id.clone(),
                tool_id: c.tool_id.clone(),
                host: c.host.clone(),
                rule: rule(c.rule_logical_id, c.rule_name.as_deref()),
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(
        ts: i64,
        host: &str,
        app: Option<&str>,
        decision: &str,
        model: Option<&str>,
    ) -> CallRow {
        CallRow {
            id: Uuid::now_v7(),
            ts: DateTime::from_timestamp(ts, 0).unwrap(),
            turn_id: None,
            conversation_id: None,
            host: host.into(),
            app_id: app.map(Into::into),
            tool_id: None,
            decision: decision.into(),
            rule_logical_id: None,
            rule_name: None,
            llm_model: model.map(Into::into),
            conv_kind: None,
            conv_direct_user: None,
            conv_workspace: None,
            conv_automation: None,
        }
    }

    #[test]
    fn model_calls_are_never_rows_and_one_app_folds() {
        let mut calls: Vec<CallRow> = (0..5)
            .map(|i| {
                call(
                    10 + i,
                    "api.tracker.example",
                    Some("tracker"),
                    "allow",
                    None,
                )
            })
            .collect();
        calls.push(call(9, "llm.example", None, "allow", Some("claude-x")));
        calls.push(call(20, "evil.example.org", None, "blocked", None));
        let g = fold_calls(&calls);
        assert_eq!(g.len(), 2);
        assert_eq!(
            (g[0].key.as_str(), g[0].count, g[0].outcome),
            ("tracker", 5, "allowed")
        );
        assert_eq!(g[0].first.timestamp(), 10);
        assert_eq!(g[0].last.timestamp(), 14);
        assert_eq!(
            (g[1].key.as_str(), g[1].outcome),
            ("evil.example.org", "blocked_default")
        );
    }

    #[test]
    fn a_rule_decides_the_blocked_word() {
        let mut c = call(1, "h", None, "blocked", None);
        c.rule_logical_id = Some(Uuid::now_v7());
        c.rule_name = Some("Block evil.example.org".into());
        let g = fold_calls(&[c]);
        assert_eq!(g[0].outcome, "blocked_by_rule");
        assert_eq!(g[0].rule.as_ref().unwrap().name, "Block evil.example.org");
        assert_eq!(call_outcome("approval_denied", false).0, "not_approved");
        assert_eq!(call_outcome("error", false).0, "failed");
        assert_eq!(call_outcome("egress_denied", false).0, "blocked_default");
    }

    fn step(
        seq: i64,
        kind: StepKind,
        cmd: Option<&str>,
        paths: &[&str],
        ok: Option<bool>,
    ) -> StepFact {
        StepFact {
            seq,
            at: DateTime::from_timestamp(seq, 0).unwrap(),
            kind,
            command: cmd.map(Into::into),
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
            ok,
            ended_at: ok.map(|_| DateTime::from_timestamp(seq + 1, 0).unwrap()),
        }
    }

    #[test]
    fn a_command_and_the_writes_after_it_are_one_row() {
        let g = group_steps(vec![
            step(1, StepKind::Shell, Some("ls"), &[], Some(true)),
            step(3, StepKind::Write, None, &["notes.txt"], Some(true)),
        ]);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].command.as_deref(), Some("ls"));
        assert_eq!(g[0].paths, ["notes.txt"]);
        assert!(!g[0].open && !g[0].failed);
    }

    #[test]
    fn at_most_three_rows_per_turn() {
        let steps: Vec<StepFact> = (0..6)
            .map(|i| {
                step(
                    i * 2,
                    StepKind::Shell,
                    Some(&format!("cmd{i}")),
                    &[&format!("f{i}")],
                    Some(i != 4),
                )
            })
            .collect();
        let g = group_steps(steps);
        assert_eq!(g.len(), COMPUTER_ROWS_PER_TURN);
        assert_eq!(g[2].command.as_deref(), Some("cmd2"));
        assert_eq!(g[2].paths, ["f2", "f3", "f4", "f5"]);
        assert!(g[2].failed, "a failed step folded in fails the row");
        let open = group_steps(vec![step(1, StepKind::Write, None, &["a"], None)]);
        assert!(open[0].open && open[0].command.is_none());
    }

    #[test]
    fn steps_pair_started_and_finished_and_skip_other_tools() {
        let ev = |seq: i64, ty: &str, p: serde_json::Value| StepRow {
            turn_id: Uuid::nil(),
            seq,
            ty: ty.into(),
            payload: p,
            created_at: DateTime::from_timestamp(seq, 0).unwrap(),
        };
        let events = vec![
            ev(
                1,
                "tool.started",
                serde_json::json!({"call_id": "a", "name": "Bash", "summary": "Bash: ls"}),
            ),
            ev(
                2,
                "tool.started",
                serde_json::json!({"call_id": "b", "name": "Read", "summary": "Read: x"}),
            ),
            ev(
                3,
                "tool.finished",
                serde_json::json!({"call_id": "a", "ok": false, "summary": "secret output"}),
            ),
            ev(
                4,
                "tool.started",
                serde_json::json!({"call_id": "c", "name": "Write", "summary": "Write: notes.txt"}),
            ),
        ];
        let s = steps_of("claude-code", &events);
        assert_eq!(s.len(), 2);
        assert_eq!(
            (s[0].ok, s[0].command.as_deref()),
            (Some(false), Some("ls"))
        );
        assert_eq!(
            (s[1].ok, s[1].paths.clone()),
            (None, vec!["notes.txt".to_owned()])
        );
    }

    fn it(at: i64, id: &str) -> TimelineItem {
        TimelineItem {
            id: id.into(),
            at: DateTime::from_timestamp(at, 0).unwrap(),
            ended_at: None,
            state: TimelineState::Done,
            kind: TimelineKind::Called,
            outcome: Some("allowed".into()),
            agent: TimelineRef {
                id: "a".into(),
                name: "A".into(),
            },
            person: None,
            app: None,
            tool: None,
            host: None,
            rule: None,
            model: None,
            schedule: None,
            webhook: None,
            peer: None,
            count: None,
            command: None,
            files: None,
            duration_ms: None,
            link: None,
        }
    }

    #[test]
    fn merge_stops_at_the_horizon_and_never_repeats() {
        let f = Filter::default();
        let items = vec![it(10, "a"), it(9, "b"), it(8, "c"), it(7, "d")];
        // A source is complete only above 8: 8 and 7 wait for the next page.
        let h = DateTime::from_timestamp(8, 0);
        let (page, next) = merge(items.clone(), None, h, 10, &f);
        assert_eq!(
            page.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        let next = next.unwrap();
        let (page2, n2) = merge(items.clone(), Some(&next), None, 10, &f);
        assert_eq!(
            page2.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["c", "d"]
        );
        assert!(n2.is_none());
        // Over the limit: the cursor is the last row.
        let (p, n) = merge(items, None, None, 2, &f);
        assert_eq!(p.len(), 2);
        assert_eq!(n.unwrap().i, "b");
    }

    #[test]
    fn a_cursor_round_trips_and_rejects_garbage() {
        let c = Cursor {
            t: Utc::now(),
            i: "turn:x".into(),
        };
        assert_eq!(Cursor::decode(&c.encode()).unwrap(), c);
        assert!(Cursor::decode("not-base64!").is_err());
    }

    #[test]
    fn filters_parse_and_refuse_unknown_kinds() {
        let f = Filter::parse(Some("answered,computer"), Some("ok")).unwrap();
        assert!(f.wants(&[TimelineKind::Computer]));
        assert!(!f.wants(&[TimelineKind::Called]));
        assert!(Filter::parse(Some("nope"), None).is_err());
        assert!(Filter::parse(None, Some("Bad Word")).is_err());
    }
}
