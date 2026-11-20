//! (phase 5, P5-REFINE-9, D-415; C-1083, C-1084, SEC-143) The agent timeline: the reads of its
//! sources and the `aurora_timeline` hints its stream listens to.
//!
//! Every source is read newest first for one agent, bounded by time and `limit` (the indexes are
//! migration `20261201000267`). The rows carry the facts a timeline row is built from and the
//! visibility columns of their conversation (`conv_*`); the API applies `can_see_conversation`
//! and folds them (`aurora-api` `domains/timeline`). Nothing here returns message text, request
//! bodies, headers or paths.

use crate::DbError;
use crate::bus::{self, BusError, Channel};
use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

// ---------------------------------------------------------------------------------------------
// Hints (C-1084)
// ---------------------------------------------------------------------------------------------

/// What changed, as the `s` of a hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Turn,
    Request,
    Approval,
    ActionApproval,
    Run,
    Delivery,
    Peer,
    Access,
}

impl Source {
    pub const ALL: [Source; 8] = [
        Source::Turn,
        Source::Request,
        Source::Approval,
        Source::ActionApproval,
        Source::Run,
        Source::Delivery,
        Source::Peer,
        Source::Access,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Turn => "turn",
            Source::Request => "request",
            Source::Approval => "approval",
            Source::ActionApproval => "action_approval",
            Source::Run => "run",
            Source::Delivery => "delivery",
            Source::Peer => "peer",
            Source::Access => "access",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|x| x.as_str() == s)
    }
}

/// The largest hint payload (C-1084: "≤ 200 bytes"; ids and numbers only, never text).
pub const HINT_MAX_BYTES: usize = 200;

/// `{ a, s, i, q? }`.
pub fn hint_payload(agent: Uuid, source: Source, id: Uuid, q: Option<i64>) -> serde_json::Value {
    let mut v = serde_json::json!({ "a": agent, "s": source.as_str(), "i": id });
    if let Some(q) = q {
        v["q"] = serde_json::json!(q);
    }
    v
}

/// A hint parsed back (the stream side); `None` for anything malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hint {
    pub agent: Uuid,
    pub source: Source,
    pub id: Uuid,
    pub q: Option<i64>,
}

impl Hint {
    pub fn from_json(p: &serde_json::Value) -> Option<Self> {
        let uuid = |k: &str| p.get(k)?.as_str()?.parse::<Uuid>().ok();
        Some(Self {
            agent: uuid("a")?,
            source: Source::parse(p.get("s")?.as_str()?)?,
            id: uuid("i")?,
            q: p.get("q").and_then(serde_json::Value::as_i64),
        })
    }
}

fn bus_err(e: BusError) -> DbError {
    match e {
        BusError::Db(e) => e.into(),
        other => DbError::Other(sqlx::Error::Protocol(other.to_string())),
    }
}

/// Queue a hint on `exec`. Inside a transaction it is delivered on commit (writers call this
/// with their transaction, as `append_durable_event` does for `aurora_conversation`).
pub async fn hint<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    source: Source,
    id: Uuid,
) -> Result<(), DbError> {
    bus::notify(
        exec,
        Channel::Timeline,
        &hint_payload(agent, source, id, None),
    )
    .await
    .map_err(bus_err)
}

/// Queue a hint for a schedule run (`owner` is the schedule) or a webhook delivery (`owner` is
/// the webhook): the agent is read from the owner's row in the same statement.
pub async fn hint_automation<'e, E: PgExecutor<'e>>(
    exec: E,
    source: Source,
    owner: Uuid,
    id: Uuid,
) -> Result<(), DbError> {
    let table = match source {
        Source::Run => "schedules",
        Source::Delivery => "webhooks",
        _ => return Ok(()),
    };
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT pg_notify('aurora_timeline', \
           json_build_object('a', agent_id, 's', '{}', 'i', $2::uuid)::text) \
         FROM {table} WHERE id = $1",
        source.as_str()
    )))
    .bind(owner)
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

/// The SQL that sends one `aurora_timeline` hint per row of a CTE named `hinted` with columns
/// `agent_id` and `id` (bulk writers: expiry sweeps). `source` is a constant of [`Source`].
pub fn hint_rows_sql(source: Source) -> String {
    format!(
        "SELECT pg_notify('aurora_timeline', json_build_object('a', agent_id, 's', '{}', 'i', id)::text) \
         FROM hinted",
        source.as_str()
    )
}

/// One `request` hint per agent present in a request-log batch: `i` is the agent's newest row,
/// `q` the oldest `ts` of the agent's rows in microseconds (the stream reads the agent's rows
/// from there).
pub fn request_hints(rows: &[super::gateway::RequestLogRow]) -> Vec<serde_json::Value> {
    let mut per: Vec<(Uuid, Uuid, DateTime<Utc>, DateTime<Utc>)> = Vec::new();
    for r in rows {
        match per.iter_mut().find(|p| p.0 == r.agent_id) {
            Some(p) => {
                if r.ts >= p.3 {
                    p.1 = r.id;
                    p.3 = r.ts;
                }
                if r.ts < p.2 {
                    p.2 = r.ts;
                }
            }
            None => per.push((r.agent_id, r.id, r.ts, r.ts)),
        }
    }
    per.into_iter()
        .map(|(a, newest, oldest, _)| {
            hint_payload(a, Source::Request, newest, Some(oldest.timestamp_micros()))
        })
        .collect()
}

/// Send [`request_hints`] after a request-log batch was written. Best effort: the stream's
/// heartbeat check repairs a lost hint.
pub async fn notify_requests(pool: &PgPool, rows: &[super::gateway::RequestLogRow]) {
    for h in request_hints(rows) {
        if let Err(e) = bus::notify(pool, Channel::Timeline, &h).await {
            tracing::warn!(error = %e, "timeline request hint failed");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------------------------

/// The visibility facts of a row's conversation (all null when it has none, or it is gone).
const CONV_COLS: &str = "c.kind AS conv_kind, c.direct_user_id AS conv_direct_user, \
    c.workspace_id AS conv_workspace, c.automation_kind AS conv_automation";

/// A turn of the agent with its conversation's visibility facts and its speaker's name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TurnRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub status: String,
    pub error_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// `person` | `bot` | `agent` | `system`
    pub speaker_kind: String,
    /// `users.display_name` of a person speaker.
    pub speaker_name: Option<String>,
    pub speaker_label: String,
    pub conv_kind: Option<String>,
    pub conv_direct_user: Option<Uuid>,
    pub conv_workspace: Option<Uuid>,
    pub conv_automation: Option<String>,
}

const TURN_SELECT: &str = "SELECT t.id, t.conversation_id, t.status, t.error_code, t.created_at, \
    t.started_at, t.finished_at, t.speaker_kind, u.display_name AS speaker_name, t.speaker_label, \
    {CONV} \
    FROM turns t JOIN conversations c ON c.id = t.conversation_id \
    LEFT JOIN users u ON u.id = t.speaker_user_id AND t.speaker_kind = 'person'";

fn turn_select() -> String {
    TURN_SELECT.replace("{CONV}", CONV_COLS)
}

/// The agent's turns created at or before `t` (all when `None`), newest first.
pub async fn turns_before(
    pool: &PgPool,
    agent: Uuid,
    t: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE t.agent_id = $1 AND t.role = 'turn' \
           AND ($2::timestamptz IS NULL OR t.created_at <= $2) \
         ORDER BY t.created_at DESC, t.id DESC LIMIT $3",
        turn_select()
    )))
    .bind(agent)
    .bind(t)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Turns created at or before `h` (and not before `not_before`) that were still running after
/// `h`, other than `exclude`: their later steps and calls belong above `h`.
pub async fn turns_overlapping(
    pool: &PgPool,
    agent: Uuid,
    h: DateTime<Utc>,
    not_before: DateTime<Utc>,
    exclude: &[Uuid],
    limit: i64,
) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE t.agent_id = $1 AND t.role = 'turn' AND t.created_at <= $2 \
           AND t.created_at >= $3 AND (t.finished_at IS NULL OR t.finished_at > $2) \
           AND t.id <> ALL($4) \
         ORDER BY t.created_at DESC, t.id DESC LIMIT $5",
        turn_select()
    )))
    .bind(agent)
    .bind(h)
    .bind(not_before)
    .bind(exclude)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Turns of the agent by id (the stream reloads a turn's rows).
pub async fn turns_by_ids(
    pool: &PgPool,
    agent: Uuid,
    ids: &[Uuid],
) -> Result<Vec<TurnRow>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE t.agent_id = $1 AND t.id = ANY($2) AND t.role = 'turn'",
        turn_select()
    )))
    .bind(agent)
    .bind(ids)
    .fetch_all(pool)
    .await?)
}

/// A durable `tool.started` / `tool.finished` event of a turn (never a redacted one).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StepRow {
    pub turn_id: Uuid,
    pub seq: i64,
    #[sqlx(rename = "type")]
    pub ty: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Most tool events read for one page.
pub const STEPS_MAX: i64 = 20_000;

/// The tool events of `turns`, by turn and seq.
pub async fn steps_of_turns(pool: &PgPool, turns: &[Uuid]) -> Result<Vec<StepRow>, DbError> {
    if turns.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT turn_id, seq, type, payload, created_at FROM transcript_events \
         WHERE turn_id = ANY($1) AND type IN ('tool.started', 'tool.finished') \
           AND redacted_at IS NULL \
         ORDER BY turn_id, seq LIMIT $2",
    )
    .bind(turns)
    .bind(STEPS_MAX)
    .fetch_all(pool)
    .await?)
}

/// The last durable seq of each of `turns` (the `answered` row's link).
pub async fn last_seqs(pool: &PgPool, turns: &[Uuid]) -> Result<Vec<(Uuid, i64)>, DbError> {
    if turns.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT turn_id, max(seq) FROM transcript_events WHERE turn_id = ANY($1) GROUP BY turn_id",
    )
    .bind(turns)
    .fetch_all(pool)
    .await?)
}

/// A request-log row, the facts a `called` row needs.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CallRow {
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    pub turn_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub host: String,
    pub app_id: Option<String>,
    pub tool_id: Option<String>,
    pub decision: String,
    pub rule_logical_id: Option<Uuid>,
    pub rule_name: Option<String>,
    pub llm_model: Option<String>,
    pub conv_kind: Option<String>,
    pub conv_direct_user: Option<Uuid>,
    pub conv_workspace: Option<Uuid>,
    pub conv_automation: Option<String>,
}

fn call_select() -> String {
    format!(
        "SELECT r.id, r.ts, r.turn_id, r.conversation_id, r.host, r.app_id, r.tool_id, r.decision, \
         r.rule_logical_id, r.rule_name, r.llm_model, {CONV_COLS} \
         FROM request_log r LEFT JOIN conversations c ON c.id = r.conversation_id"
    )
}

/// Most request-log rows read for the turns of one page.
pub const CALLS_MAX: i64 = 20_000;

/// The request-log rows of `turns` (model calls included: they name the turn's model), oldest
/// first.
pub async fn calls_of_turns(
    pool: &PgPool,
    agent: Uuid,
    turns: &[Uuid],
) -> Result<Vec<CallRow>, DbError> {
    if turns.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE r.turn_id = ANY($1) AND r.agent_id = $2 ORDER BY r.ts, r.id LIMIT $3",
        call_select()
    )))
    .bind(turns)
    .bind(agent)
    .bind(CALLS_MAX)
    .fetch_all(pool)
    .await?)
}

/// The agent's non-model request-log rows that belong to no turn (BYO agents, boot-time
/// requests), at or before `t`, newest first.
pub async fn calls_without_turn(
    pool: &PgPool,
    agent: Uuid,
    t: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<CallRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE r.agent_id = $1 AND r.turn_id IS NULL AND r.llm_model IS NULL \
           AND ($2::timestamptz IS NULL OR r.ts <= $2) \
         ORDER BY r.ts DESC, r.id DESC LIMIT $3",
        call_select()
    )))
    .bind(agent)
    .bind(t)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// The agent's request-log rows with `ts >= since` (the stream after a `request` hint), with the
/// visibility facts; bounded by the workspace index and `limit`.
pub async fn calls_since(
    pool: &PgPool,
    ws: Uuid,
    agent: Uuid,
    since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<CallRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE r.workspace_id = $1 AND r.agent_id = $2 AND r.ts >= $3 \
         ORDER BY r.ts DESC, r.id DESC LIMIT $4",
        call_select()
    )))
    .bind(ws)
    .bind(agent)
    .bind(since)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Full request-log rows by id for the Technical view's live `request` events, with the same
/// direct-conversation rule as `GET /workspaces/{ws}/activity` (`viewer` cannot see another
/// person's direct conversation unless they own the workspace).
pub async fn log_rows_by_ids(
    pool: &PgPool,
    viewer: Uuid,
    ws: Uuid,
    ids: &[Uuid],
) -> Result<Vec<super::request_log::LogRow>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, ts, agent_id, conversation_id, turn_id, credential_kind, lane, method, host, \
                path, app_id, tool_id, connection_id, secret_id, decision, rule_logical_id, \
                rule_name, upstream_status, status, error_code, injected, latency_ms, bytes_up, \
                bytes_down, context_label, approval_id, resumed, llm_model \
         FROM request_log WHERE workspace_id = $1 AND id = ANY($2) AND NOT {} \
         ORDER BY ts DESC, id DESC",
        crate::authz::hidden_direct_conversation_sql("conversation_id", "$3", "workspace_id")
    )))
    .bind(ws)
    .bind(ids)
    .bind(viewer)
    .fetch_all(pool)
    .await?)
}

/// A gateway approval of the agent.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApprovalItemRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub host: String,
    pub app_id: Option<String>,
    pub tool_id: Option<String>,
    pub rule_logical_id: Option<Uuid>,
    pub rule_name: Option<String>,
    /// `pending` | `approved` | `denied` | `expired` | `abandoned`
    pub status: String,
    pub decided_by_name: Option<String>,
    /// The decision was Always allow (its `approval.decided` event carries `always`).
    pub always: bool,
    pub conv_kind: Option<String>,
    pub conv_direct_user: Option<Uuid>,
    pub conv_workspace: Option<Uuid>,
    pub conv_automation: Option<String>,
}

fn approval_select() -> String {
    format!(
        "SELECT a.id, a.agent_id, a.created_at, a.decided_at, a.resolved_at, a.conversation_id, \
         a.turn_id, a.host, a.app_id, a.tool_id, a.rule_logical_id, a.rule_name, a.status, \
         u.display_name AS decided_by_name, \
         (a.status = 'approved' AND a.turn_id IS NOT NULL AND EXISTS (SELECT 1 FROM transcript_events e \
            WHERE e.turn_id = a.turn_id AND e.type = 'approval.decided' \
              AND e.payload->>'approval_id' = a.id::text AND e.payload ? 'always')) AS always, \
         {CONV_COLS} \
         FROM approvals a LEFT JOIN users u ON u.id = a.decided_by_user_id \
         LEFT JOIN conversations c ON c.id = a.conversation_id"
    )
}

pub async fn approvals_before(
    pool: &PgPool,
    agent: Uuid,
    t: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<ApprovalItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE a.agent_id = $1 AND ($2::timestamptz IS NULL OR a.created_at <= $2) \
         ORDER BY a.created_at DESC, a.id DESC LIMIT $3",
        approval_select()
    )))
    .bind(agent)
    .bind(t)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn approval_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ApprovalItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE a.id = $1",
        approval_select()
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// A platform action approval of the agent (send-as-me, messages to other spaces, ...).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActionApprovalItemRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub executed_at: Option<DateTime<Utc>>,
    pub conversation_id: Option<Uuid>,
    pub action_key: String,
    /// `pending` | `approved` | `executed` | `failed` | `rejected` | `expired`
    pub status: String,
    pub decided_by_name: Option<String>,
    pub conv_kind: Option<String>,
    pub conv_direct_user: Option<Uuid>,
    pub conv_workspace: Option<Uuid>,
    pub conv_automation: Option<String>,
}

fn action_approval_select() -> String {
    format!(
        "SELECT a.id, a.agent_id, a.created_at, a.decided_at, a.executed_at, a.conversation_id, \
         a.action_key, a.status, u.display_name AS decided_by_name, {CONV_COLS} \
         FROM action_approvals a LEFT JOIN users u ON u.id = a.decided_by_user_id \
         LEFT JOIN conversations c ON c.id = a.conversation_id"
    )
}

pub async fn action_approvals_before(
    pool: &PgPool,
    agent: Uuid,
    t: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<ActionApprovalItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE a.agent_id = $1 AND ($2::timestamptz IS NULL OR a.created_at <= $2) \
         ORDER BY a.created_at DESC, a.id DESC LIMIT $3",
        action_approval_select()
    )))
    .bind(agent)
    .bind(t)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn action_approval_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<ActionApprovalItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE a.id = $1",
        action_approval_select()
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// A schedule run or a webhook delivery of the agent, with its turn and thread.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AutomationItemRow {
    pub id: Uuid,
    /// The schedule's or webhook's id and name.
    pub owner_id: Uuid,
    pub owner_name: String,
    pub at: DateTime<Utc>,
    pub turn_id: Option<Uuid>,
    /// Runs: the stored `outcome`. Deliveries: the stored `status`.
    pub outcome: String,
    /// Deliveries: the stored `reason` (a refusal code or an idempotency key; never shown).
    pub reason: Option<String>,
    pub conversation_id: Option<Uuid>,
    pub turn_status: Option<String>,
    pub turn_error_code: Option<String>,
    pub turn_started_at: Option<DateTime<Utc>>,
    pub turn_finished_at: Option<DateTime<Utc>>,
    pub conv_kind: Option<String>,
    pub conv_direct_user: Option<Uuid>,
    pub conv_workspace: Option<Uuid>,
    pub conv_automation: Option<String>,
}

fn run_select(inner_where: &str) -> String {
    format!(
        "SELECT x.id, x.owner_id, x.owner_name, x.at, x.turn_id, x.outcome, NULL::text AS reason, \
            x.conversation_id, t.status AS turn_status, t.error_code AS turn_error_code, \
            t.started_at AS turn_started_at, t.finished_at AS turn_finished_at, {CONV_COLS} \
         FROM (SELECT r.id, s.id AS owner_id, s.name AS owner_name, r.fired_at AS at, r.turn_id, \
                 r.outcome, COALESCE((SELECT tt.conversation_id FROM turns tt WHERE tt.id = r.turn_id), \
                   (SELECT cc.id FROM conversations cc WHERE cc.automation_kind = 'schedule' \
                      AND cc.automation_id = s.id \
                      AND cc.external_ref = 'schedule:' || s.id::text || ':' || r.id::text)) \
                   AS conversation_id \
               FROM schedules s CROSS JOIN LATERAL (SELECT * FROM schedule_runs r \
                   WHERE r.schedule_id = s.id {inner_where} \
                   ORDER BY r.fired_at DESC, r.id DESC LIMIT $3) r \
               WHERE s.agent_id = $1) x \
         LEFT JOIN turns t ON t.id = x.turn_id \
         LEFT JOIN conversations c ON c.id = x.conversation_id"
    )
}

/// The agent's schedule runs fired at or before `t`, newest first (at most `limit` per schedule
/// read from the `(schedule_id, fired_at desc)` index, then `limit` overall).
pub async fn runs_before(
    pool: &PgPool,
    agent: Uuid,
    t: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<AutomationItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} ORDER BY x.at DESC, x.id DESC LIMIT $3",
        run_select("AND ($2::timestamptz IS NULL OR r.fired_at <= $2)")
    )))
    .bind(agent)
    .bind(t)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn run_by_id(
    pool: &PgPool,
    agent: Uuid,
    id: Uuid,
) -> Result<Option<AutomationItemRow>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(run_select("AND r.id = $2")))
            .bind(agent)
            .bind(id)
            .bind(1_i64)
            .fetch_optional(pool)
            .await?,
    )
}

/// Runs whose turn is one of `turns` (the stream: a run's state follows its turn).
pub async fn runs_of_turns(
    pool: &PgPool,
    agent: Uuid,
    turns: &[Uuid],
) -> Result<Vec<AutomationItemRow>, DbError> {
    if turns.is_empty() {
        return Ok(Vec::new());
    }
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(run_select("AND r.turn_id = ANY($2)")))
            .bind(agent)
            .bind(turns)
            .bind(100_i64)
            .fetch_all(pool)
            .await?,
    )
}

fn delivery_select(inner_where: &str) -> String {
    format!(
        "SELECT x.id, x.owner_id, x.owner_name, x.at, x.turn_id, x.outcome, x.reason, \
            x.conversation_id, t.status AS turn_status, t.error_code AS turn_error_code, \
            t.started_at AS turn_started_at, t.finished_at AS turn_finished_at, {CONV_COLS} \
         FROM (SELECT d.id, w.id AS owner_id, w.name AS owner_name, d.created_at AS at, d.turn_id, \
                 d.status AS outcome, d.reason, \
                 CASE WHEN d.status = 'accepted' THEN \
                   COALESCE((SELECT tt.conversation_id FROM turns tt WHERE tt.id = d.turn_id), \
                     (SELECT cc.id FROM conversations cc WHERE cc.automation_kind = 'webhook' \
                        AND cc.automation_id = w.id \
                        AND cc.external_ref = 'webhook:' || w.id::text || ':' || d.id::text)) \
                 END AS conversation_id \
               FROM webhooks w CROSS JOIN LATERAL (SELECT * FROM webhook_deliveries d \
                   WHERE d.webhook_id = w.id {inner_where} \
                   ORDER BY d.created_at DESC, d.id DESC LIMIT $3) d \
               WHERE w.agent_id = $1) x \
         LEFT JOIN turns t ON t.id = x.turn_id \
         LEFT JOIN conversations c ON c.id = x.conversation_id"
    )
}

/// The agent's webhook deliveries received at or before `t`, newest first.
pub async fn deliveries_before(
    pool: &PgPool,
    agent: Uuid,
    t: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<AutomationItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} ORDER BY x.at DESC, x.id DESC LIMIT $3",
        delivery_select("AND ($2::timestamptz IS NULL OR d.created_at <= $2)")
    )))
    .bind(agent)
    .bind(t)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn delivery_by_id(
    pool: &PgPool,
    agent: Uuid,
    id: Uuid,
) -> Result<Option<AutomationItemRow>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(delivery_select("AND d.id = $2")))
            .bind(agent)
            .bind(id)
            .bind(1_i64)
            .fetch_optional(pool)
            .await?,
    )
}

/// Deliveries whose turn is one of `turns` (duplicates name the original's turn too).
pub async fn deliveries_of_turns(
    pool: &PgPool,
    agent: Uuid,
    turns: &[Uuid],
) -> Result<Vec<AutomationItemRow>, DbError> {
    if turns.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(delivery_select(
        "AND d.turn_id = ANY($2)",
    )))
    .bind(agent)
    .bind(turns)
    .bind(100_i64)
    .fetch_all(pool)
    .await?)
}

/// A peer task the agent handed (`from`) or got (`to`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PeerItemRow {
    pub id: Uuid,
    pub from_agent_id: Uuid,
    pub to_agent_id: Uuid,
    /// The other agent's name.
    pub peer_name: String,
    /// `queued` | `open` | `completed` | `cancelled` | `expired`
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const PEER_COLS: &str = "p.id, p.from_agent_id, p.to_agent_id, \
    COALESCE(o.name, '') AS peer_name, p.status, p.created_at, p.updated_at";

pub async fn peers_before(
    pool: &PgPool,
    agent: Uuid,
    t: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<PeerItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM ( \
           (SELECT {PEER_COLS} FROM peer_tasks p LEFT JOIN agents o ON o.id = p.to_agent_id \
             WHERE p.from_agent_id = $1 AND ($2::timestamptz IS NULL OR p.created_at <= $2) \
             ORDER BY p.created_at DESC, p.id DESC LIMIT $3) \
           UNION ALL \
           (SELECT {PEER_COLS} FROM peer_tasks p LEFT JOIN agents o ON o.id = p.from_agent_id \
             WHERE p.to_agent_id = $1 AND p.from_agent_id <> $1 \
               AND ($2::timestamptz IS NULL OR p.created_at <= $2) \
             ORDER BY p.created_at DESC, p.id DESC LIMIT $3)) x \
         ORDER BY created_at DESC, id DESC LIMIT $3"
    )))
    .bind(agent)
    .bind(t)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn peer_by_id(
    pool: &PgPool,
    agent: Uuid,
    id: Uuid,
) -> Result<Option<PeerItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {PEER_COLS} FROM peer_tasks p \
         LEFT JOIN agents o ON o.id = CASE WHEN p.from_agent_id = $1 THEN p.to_agent_id ELSE p.from_agent_id END \
         WHERE p.id = $2 AND (p.from_agent_id = $1 OR p.to_agent_id = $1)"
    )))
    .bind(agent)
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// An access request of the agent.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccessItemRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub conversation_id: Uuid,
    /// `app` | `website`
    pub kind: String,
    pub app_id: Option<String>,
    /// The proposed website's host pattern.
    pub host: Option<String>,
    /// `open` | `given` | `dismissed` | `expired`
    pub status: String,
    pub conv_kind: Option<String>,
    pub conv_direct_user: Option<Uuid>,
    pub conv_workspace: Option<Uuid>,
    pub conv_automation: Option<String>,
}

fn access_select() -> String {
    format!(
        "SELECT q.id, q.agent_id, q.created_at, q.resolved_at, q.conversation_id, q.kind, q.app_id, \
         s.host_pattern AS host, q.status, {CONV_COLS} \
         FROM agent_access_requests q LEFT JOIN secrets s ON s.id = q.secret_id \
         LEFT JOIN conversations c ON c.id = q.conversation_id"
    )
}

pub async fn access_before(
    pool: &PgPool,
    agent: Uuid,
    t: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<AccessItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE q.agent_id = $1 AND ($2::timestamptz IS NULL OR q.created_at <= $2) \
         ORDER BY q.created_at DESC, q.id DESC LIMIT $3",
        access_select()
    )))
    .bind(agent)
    .bind(t)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn access_by_id(pool: &PgPool, id: Uuid) -> Result<Option<AccessItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE q.id = $1",
        access_select()
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

// ---------------------------------------------------------------------------------------------
// The stream's lookups
// ---------------------------------------------------------------------------------------------

/// The agent of a conversation (never changes; the stream caches it).
pub async fn conversation_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT agent_id FROM conversations WHERE id = $1")
            .bind(conversation)
            .fetch_optional(exec)
            .await?,
    )
}

/// The turns the events `(conversation, seqs)` belong to.
pub async fn turns_of_events(
    pool: &PgPool,
    conversation: Uuid,
    seqs: &[i64],
) -> Result<Vec<Uuid>, DbError> {
    if seqs.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT turn_id FROM transcript_events \
         WHERE conversation_id = $1 AND seq = ANY($2) AND turn_id IS NOT NULL",
    )
    .bind(conversation)
    .bind(seqs)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_payload_is_small_and_parses_back() {
        let a = Uuid::now_v7();
        let i = Uuid::now_v7();
        for s in Source::ALL {
            let p = hint_payload(a, s, i, Some(i64::MAX));
            assert!(p.to_string().len() <= HINT_MAX_BYTES, "{s:?}: {p}");
            let h = Hint::from_json(&p).unwrap();
            assert_eq!((h.agent, h.source, h.id, h.q), (a, s, i, Some(i64::MAX)));
        }
        assert!(Hint::from_json(&serde_json::json!({"a": a, "s": "nope", "i": i})).is_none());
    }

    #[test]
    fn hint_rows_sql_names_a_constant_source() {
        assert!(hint_rows_sql(Source::Access).contains("'s', 'access'"));
    }
}
