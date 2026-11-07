//! Hosted-agent due-work queries (docs/05 arms 4-8, docs/10 §Turns / §Failure semantics) and the
//! sandbox/turn facts the report handlers need. Claims never lock turn rows before the
//! conversation row (lock order: conversation, then turn); every transition is fenced.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

/// Desired home generation: agent + workspace counters, offset by 1 so a fresh boot (applied 0)
/// always syncs once.
const DESIRED: &str = "(a.home_sync_generation + w.home_sync_generation + 1)";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SandboxFacts {
    pub sandbox_id: Uuid,
    pub agent_id: Uuid,
    pub runner_id: Option<Uuid>,
    pub state: String,
    pub boot_id: Option<Uuid>,
    pub channel_replica_id: Option<Uuid>,
    pub harness_steer: bool,
}

pub async fn sandbox_facts(
    conn: &mut PgConnection,
    sandbox: Uuid,
) -> Result<Option<SandboxFacts>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id AS sandbox_id, agent_id, runner_id, state, boot_id, channel_replica_id,
            coalesce(resource_profile->'harness'->'capabilities' ? 'steer', false) AS harness_steer
         FROM sandboxes WHERE id = $1",
    )
    .bind(sandbox)
    .fetch_optional(conn)
    .await?)
}

/// Record the harness announced by `supervisor.ready` (fenced on boot + channel holder).
pub async fn record_harness(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
    replica: Uuid,
    harness: &serde_json::Value,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE sandboxes SET resource_profile = coalesce(resource_profile, '{}'::jsonb)
                || jsonb_build_object('harness', $4::jsonb), updated_at = now()
         WHERE id=$1 AND boot_id=$2 AND channel_replica_id=$3",
    )
    .bind(sandbox)
    .bind(boot)
    .bind(replica)
    .bind(harness)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HomeSyncClaim {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub agent_id: Uuid,
    pub generation: i64,
    pub harness_id: Option<String>,
}

/// Arm 5: running sandboxes whose channel this replica holds with desired > applied. The sync is
/// leased (`lifecycle_lease_until`, unused while `running`) and re-sent when the lease lapses.
pub async fn claim_home_syncs(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    lease_secs: f64,
    limit: i64,
) -> Result<Vec<HomeSyncClaim>, DbError> {
    Ok(sqlx::query_as(&format!(
        "WITH cand AS (
           SELECT s.id FROM sandboxes s JOIN agents a ON a.id = s.agent_id
             JOIN workspaces w ON w.id = a.workspace_id
           WHERE s.runner_id = $1 AND s.channel_replica_id = $2 AND s.state = 'running'
             AND s.boot_id IS NOT NULL AND s.home_applied_generation < {DESIRED}
             AND s.resource_profile ? 'harness'
             AND (s.lifecycle_lease_until IS NULL OR s.lifecycle_lease_until < now())
           ORDER BY s.updated_at LIMIT $4 FOR UPDATE OF s SKIP LOCKED)
         UPDATE sandboxes s SET lifecycle_lease_until = now() + make_interval(secs => $3),
             updated_at = now()
         FROM cand, agents a, workspaces w
         WHERE s.id = cand.id AND a.id = s.agent_id AND w.id = a.workspace_id
         RETURNING s.id AS sandbox_id, s.boot_id AS boot_id, s.agent_id, {DESIRED} AS generation,
             s.resource_profile->'harness'->>'id' AS harness_id"
    ))
    .bind(runner)
    .bind(replica)
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// After `home.synced`: release the sync lease (only while running) and report the applied state.
pub async fn clear_sync_lease(conn: &mut PgConnection, sandbox: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE sandboxes SET lifecycle_lease_until = NULL
         WHERE id = $1 AND state = 'running' AND lifecycle_lease_until IS NOT NULL",
    )
    .bind(sandbox)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DispatchCand {
    pub turn_id: Uuid,
    pub conversation_id: Uuid,
    pub agent_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
}

/// Arm 6 candidates: queued turns whose sandbox is running here, channel live, home synced.
pub async fn dispatch_candidates(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    limit: i64,
) -> Result<Vec<DispatchCand>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT t.id AS turn_id, t.conversation_id, t.agent_id, s.id AS sandbox_id, s.boot_id
         FROM turns t JOIN agents a ON a.id = t.agent_id JOIN workspaces w ON w.id = a.workspace_id
           JOIN sandboxes s ON s.agent_id = t.agent_id
         WHERE t.role = 'turn' AND t.status = 'queued'
           AND s.runner_id = $1 AND s.channel_replica_id = $2 AND s.state = 'running'
           AND s.boot_id IS NOT NULL AND s.home_applied_generation >= {DESIRED}
         ORDER BY t.created_at LIMIT $3"
    ))
    .bind(runner)
    .bind(replica)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// `queued` -> `dispatched` (fenced); stamps the dispatch clock and boot.
pub async fn mark_dispatched(
    conn: &mut PgConnection,
    turn: Uuid,
    boot: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE turns SET status = 'dispatched', dispatched_at = now(), boot_id = $2,
             dispatch_lease_until = NULL, updated_at = now()
         WHERE id = $1 AND role = 'turn' AND status = 'queued'",
    )
    .bind(turn)
    .bind(boot)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Mint a turn-bound context credential (hash only is stored); previous ones for the turn are
/// revoked first (re-dispatch). Returns the revoked ids for gateway invalidation.
#[allow(clippy::too_many_arguments)]
pub async fn mint_turn_credential(
    conn: &mut PgConnection,
    agent: Uuid,
    conversation: Uuid,
    turn: Uuid,
    token_hash: &[u8],
    display_prefix: &str,
    ttl_secs: f64,
) -> Result<Vec<Uuid>, DbError> {
    let revoked = sqlx::query_scalar(
        "UPDATE gateway_credentials SET revoked_at = now(), updated_at = now()
         WHERE turn_id = $1 AND kind = 'context' AND revoked_at IS NULL RETURNING id",
    )
    .bind(turn)
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO gateway_credentials
            (id, kind, token_hash, display_prefix, agent_id, conversation_id, turn_id,
             context_label, expires_at)
         VALUES ($1,'context',$2,$3,$4,$5,$6,'turn', now() + make_interval(secs => $7))",
    )
    .bind(Uuid::now_v7())
    .bind(token_hash)
    .bind(display_prefix)
    .bind(agent)
    .bind(conversation)
    .bind(turn)
    .bind(ttl_secs)
    .execute(conn)
    .await?;
    Ok(revoked)
}

/// At turn end: the turn's context credentials expire `grace_secs` from now (08 §7).
pub async fn expire_turn_credentials(
    conn: &mut PgConnection,
    turn: Uuid,
    grace_secs: f64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE gateway_credentials SET expires_at = now() + make_interval(secs => $2),
             updated_at = now()
         WHERE turn_id = $1 AND kind = 'context' AND revoked_at IS NULL
           AND (expires_at IS NULL OR expires_at > now() + make_interval(secs => $2))
         RETURNING id",
    )
    .bind(turn)
    .bind(grace_secs)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AttachmentMeta {
    pub id: Uuid,
    pub filename: String,
    pub size_bytes: i64,
}

pub async fn turn_attachments(
    conn: &mut PgConnection,
    turn: Uuid,
) -> Result<Vec<AttachmentMeta>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, filename, size_bytes FROM attachments
         WHERE turn_id = $1 AND direction = 'in' ORDER BY created_at, id",
    )
    .bind(turn)
    .fetch_all(conn)
    .await?)
}

pub async fn session_of(
    conn: &mut PgConnection,
    conversation: Uuid,
) -> Result<(Option<String>, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT harness_session_ref, harness_session_epoch FROM conversations WHERE id = $1",
    )
    .bind(conversation)
    .fetch_one(conn)
    .await?)
}

/// `session` report: ignored when the epoch is stale or the conversation is another agent's.
pub async fn set_session(
    conn: &mut PgConnection,
    conversation: Uuid,
    agent: Uuid,
    session_ref: &str,
    epoch: i64,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE conversations SET harness_session_ref = $3, harness_session_epoch = $4,
             updated_at = now()
         WHERE id = $1 AND agent_id = $2 AND harness_session_epoch <= $4",
    )
    .bind(conversation)
    .bind(agent)
    .bind(session_ref)
    .bind(epoch)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// `memory/index.md` lines: `(key, title, description, first content line)`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemoryRow {
    pub key: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: String,
}

pub async fn memories(conn: &mut PgConnection, agent: Uuid) -> Result<Vec<MemoryRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT key, title, description, content FROM memories WHERE agent_id = $1 ORDER BY key",
    )
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SkillRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Effective skills of an agent: org tier then agent tier (the agent's wins on a name clash).
pub async fn skills(
    conn: &mut PgConnection,
    org: Uuid,
    agent: Uuid,
) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT ON (name) id, name, description, body FROM skills
         WHERE enabled AND org_id = $1 AND (agent_id IS NULL OR agent_id = $2)
         ORDER BY name, (agent_id IS NULL)",
    )
    .bind(org)
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SkillFileRow {
    pub path: String,
    pub content: Vec<u8>,
    pub executable: bool,
}

pub async fn skill_files(
    conn: &mut PgConnection,
    skill: Uuid,
) -> Result<Vec<SkillFileRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT path, content, executable FROM skill_files WHERE skill_id = $1 ORDER BY path",
    )
    .bind(skill)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HomeInputs {
    pub workspace_name: String,
    pub timezone: String,
    pub memory_language: String,
    pub brief: String,
    pub handle: String,
    pub name: String,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub harness: Option<String>,
}

pub async fn home_inputs(conn: &mut PgConnection, agent: Uuid) -> Result<HomeInputs, DbError> {
    Ok(sqlx::query_as(
        "SELECT w.name AS workspace_name, w.timezone, w.memory_language, a.brief, a.handle,
                a.name, a.org_id, a.workspace_id, a.harness
         FROM agents a JOIN workspaces w ON w.id = a.workspace_id WHERE a.id = $1",
    )
    .bind(agent)
    .fetch_one(conn)
    .await?)
}

// ------------------------------------------------------------------ arms 4, 7, 8

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TurnSignal {
    pub turn_id: Uuid,
    pub conversation_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
}

const SIGNAL_JOIN: &str = "FROM turns t JOIN sandboxes s ON s.agent_id = t.agent_id
    WHERE t.role = 'turn' AND t.status IN ('dispatched','running')
      AND s.runner_id = $1 AND s.channel_replica_id = $2 AND s.state = 'running'
      AND s.boot_id IS NOT NULL AND t.boot_id = s.boot_id";

/// Arm 4: abort-requested turns; re-sent every `lease_secs` until the turn ends.
pub async fn claim_aborts(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    lease_secs: f64,
    limit: i64,
) -> Result<Vec<TurnSignal>, DbError> {
    Ok(sqlx::query_as(&format!(
        "WITH cand AS (SELECT t.id {SIGNAL_JOIN} AND t.abort_requested_at IS NOT NULL
             AND (t.dispatch_lease_until IS NULL OR t.dispatch_lease_until < now())
             ORDER BY t.created_at LIMIT $4 FOR UPDATE OF t SKIP LOCKED)
         UPDATE turns t SET dispatch_lease_until = now() + make_interval(secs => $3)
         FROM cand, sandboxes s WHERE t.id = cand.id AND s.agent_id = t.agent_id
         RETURNING t.id AS turn_id, t.conversation_id, s.id AS sandbox_id, s.boot_id"
    ))
    .bind(runner)
    .bind(replica)
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Arm 4: send-now requests, delivered once (`steer_delivered_at` doubles as the delivered mark
/// on `turn` rows, where it is otherwise unused).
pub async fn claim_send_nows(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    limit: i64,
) -> Result<Vec<TurnSignal>, DbError> {
    Ok(sqlx::query_as(&format!(
        "WITH cand AS (SELECT t.id {SIGNAL_JOIN} AND t.send_now_requested_at IS NOT NULL
             AND t.steer_delivered_at IS NULL AND t.abort_requested_at IS NULL
             ORDER BY t.created_at LIMIT $3 FOR UPDATE OF t SKIP LOCKED)
         UPDATE turns t SET steer_delivered_at = now()
         FROM cand, sandboxes s WHERE t.id = cand.id AND s.agent_id = t.agent_id
         RETURNING t.id AS turn_id, t.conversation_id, s.id AS sandbox_id, s.boot_id"
    ))
    .bind(runner)
    .bind(replica)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SteerCand {
    pub followup_id: Uuid,
    pub turn_id: Uuid,
    pub conversation_id: Uuid,
    pub text: String,
    pub speaker_label: String,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
}

/// Arm 7: `joining` follow-ups of running turns, for harnesses that declared `steer`.
pub async fn steer_candidates(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    limit: i64,
) -> Result<Vec<SteerCand>, DbError> {
    Ok(sqlx::query_as(
        "SELECT f.id AS followup_id, t.id AS turn_id, t.conversation_id, f.text, f.speaker_label,
                s.id AS sandbox_id, s.boot_id
         FROM turns f JOIN turns t ON t.id = f.parent_turn_id
           JOIN sandboxes s ON s.agent_id = t.agent_id
         WHERE f.role = 'followup' AND f.status = 'joining' AND f.steer_delivered_at IS NULL
           AND t.role = 'turn' AND t.status = 'running' AND t.boot_id = s.boot_id
           AND s.runner_id = $1 AND s.channel_replica_id = $2 AND s.state = 'running'
           AND coalesce(s.resource_profile->'harness'->'capabilities' ? 'steer', false)
         ORDER BY f.created_at LIMIT $3",
    )
    .bind(runner)
    .bind(replica)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum Stale {
    /// Dispatched, never acknowledged within `STALE_DISPATCH`.
    NeverStarted,
    /// Running with no progress for `TURN_STALL`.
    Stalled,
    /// Past `TURN_CEILING`.
    Ceiling,
    /// Past `TURN_CEILING - TURN_CEILING_WARNING`, not yet warned.
    CeilingWarn,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StaleTurn {
    pub turn_id: Uuid,
    pub conversation_id: Uuid,
    pub revived: bool,
    /// The agent's sandbox is not running the boot the turn was dispatched to.
    pub sandbox_dead: bool,
    pub sandbox_id: Option<Uuid>,
    pub boot_id: Option<Uuid>,
    pub channel_here: bool,
}

/// Arm 8 candidates of one kind for sandboxes placed on `runner`.
pub async fn stale_turns(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    kind: Stale,
    secs: f64,
    limit: i64,
) -> Result<Vec<StaleTurn>, DbError> {
    let cond = match kind {
        Stale::NeverStarted => {
            "t.status = 'dispatched' AND t.dispatched_at < now() - make_interval(secs => $3)"
        }
        Stale::Stalled => {
            "t.status = 'running' AND coalesce(t.last_progress_at, t.started_at) < now() - make_interval(secs => $3)"
        }
        Stale::Ceiling => {
            "t.status IN ('dispatched','running') AND t.dispatched_at < now() - make_interval(secs => $3)"
        }
        Stale::CeilingWarn => {
            "t.status = 'running' AND t.ceiling_warned_at IS NULL AND t.dispatched_at < now() - make_interval(secs => $3)"
        }
    };
    Ok(sqlx::query_as(&format!(
        "SELECT t.id AS turn_id, t.conversation_id, t.revived_at IS NOT NULL AS revived,
            NOT (s.state = 'running' AND s.boot_id IS NOT DISTINCT FROM t.boot_id) AS sandbox_dead,
            s.id AS sandbox_id, s.boot_id,
            (s.channel_replica_id = $2 AND s.state = 'running') AS channel_here
         FROM turns t JOIN sandboxes s ON s.agent_id = t.agent_id
         WHERE t.role = 'turn' AND s.runner_id = $1 AND {cond}
         ORDER BY t.created_at LIMIT $4"
    ))
    .bind(runner)
    .bind(replica)
    .bind(secs)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// `dispatched` -> `queued` once (the `revived_at` fence).
pub async fn revive(conn: &mut PgConnection, turn: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE turns SET status = 'queued', revived_at = now(), dispatched_at = NULL,
             boot_id = NULL, updated_at = now()
         WHERE id = $1 AND role = 'turn' AND status = 'dispatched' AND revived_at IS NULL",
    )
    .bind(turn)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

pub async fn mark_ceiling_warned(conn: &mut PgConnection, turn: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE turns SET ceiling_warned_at = now(), updated_at = now()
         WHERE id = $1 AND role = 'turn' AND status IN ('dispatched','running')
           AND ceiling_warned_at IS NULL",
    )
    .bind(turn)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

pub async fn touch_progress(conn: &mut PgConnection, turn: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE turns SET last_progress_at = now() WHERE id = $1 AND role = 'turn'
            AND status IN ('dispatched','running')",
    )
    .bind(turn)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Active (`queued`) hosted turns that have not reached a sandbox yet, for the `no_model_key`
/// check before any start.
pub async fn queued_hosted_turns(
    conn: &mut PgConnection,
    limit: i64,
) -> Result<Vec<(Uuid, Uuid, Uuid)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.conversation_id, t.agent_id FROM turns t JOIN agents a ON a.id = t.agent_id
         WHERE t.role = 'turn' AND t.status = 'queued' AND a.kind = 'hosted'
         ORDER BY t.created_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Active turns (any status) of an agent as `(turn id, conversation id)`.
pub async fn active_turns_of_agent(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, conversation_id FROM turns WHERE agent_id = $1 AND role = 'turn'
            AND status IN ('queued','dispatched','running')",
    )
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

/// Request a restart of a starting/running sandbox; false when nothing to restart.
pub async fn request_restart(conn: &mut PgConnection, agent: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE sandboxes SET restart_requested_at = coalesce(restart_requested_at, now()),
             updated_at = now()
         WHERE agent_id = $1 AND state IN ('starting','running')",
    )
    .bind(agent)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SandboxViewRow {
    pub id: Uuid,
    pub state: String,
    pub boot_id: Option<Uuid>,
    pub runner_id: Option<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub listening_ports: Vec<i32>,
    pub active_turn: bool,
    pub busy_process: bool,
    pub armed_watch: bool,
}

/// The env/files hash recorded when the agent's current boot started (`resource_profile`).
pub async fn boot_app_inputs_hash(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT resource_profile->>'app_inputs_hash' FROM sandboxes WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// The capabilities the agent's current boot was started with (`resource_profile`); empty for
/// boots recorded before capabilities existed (RFE-003).
pub async fn boot_capabilities(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Vec<String>, DbError> {
    let v: Option<Option<serde_json::Value>> = sqlx::query_scalar(
        "SELECT resource_profile->'capabilities' FROM sandboxes WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_optional(conn)
    .await?;
    Ok(v.flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

/// The image the agent's current boot used, recorded in `resource_profile` at boot (absent for
/// boots recorded before it was).
pub async fn boot_image(conn: &mut PgConnection, agent: Uuid) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT resource_profile->>'boot_image' FROM sandboxes WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// Boots started before `boot_image` was recorded: when their runner starts advertising a different
/// image, pin them to the image it advertised until now (what they booted with), so the new image
/// is known to differ and `restart_to_update` flags.
pub async fn backfill_boot_image(
    conn: &mut PgConnection,
    runner: Uuid,
    previous_image: &str,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE sandboxes SET resource_profile = COALESCE(resource_profile, '{}'::jsonb)
                                                 || jsonb_build_object('boot_image', $2::text)
         WHERE runner_id = $1 AND state IN ('starting', 'running')
           AND COALESCE(resource_profile, '{}'::jsonb) ? 'boot_image' IS FALSE",
    )
    .bind(runner)
    .bind(previous_image)
    .execute(conn)
    .await?
    .rows_affected())
}

pub async fn sandbox_view(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Option<SandboxViewRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.state, s.boot_id, s.runner_id,
            (SELECT ri.started_at FROM runtime_intervals ri WHERE ri.boot_id = s.boot_id) AS started_at,
            s.failure_code, s.failure_message, s.listening_ports,
            EXISTS (SELECT 1 FROM turns t WHERE t.agent_id = s.agent_id AND (
                (t.role = 'turn' AND t.status IN ('queued','dispatched','running'))
                OR (t.role = 'followup' AND t.status = 'joining'))) AS active_turn,
            EXISTS (SELECT 1 FROM processes p WHERE p.sandbox_id = s.id AND p.boot_id = s.boot_id
                AND p.status = 'running' AND p.busy) AS busy_process,
            EXISTS (SELECT 1 FROM watches w JOIN processes p ON p.id = w.process_id
                WHERE p.sandbox_id = s.id AND p.boot_id = s.boot_id AND w.status = 'armed') AS armed_watch
         FROM sandboxes s WHERE s.agent_id = $1",
    )
    .bind(agent)
    .fetch_optional(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProcessViewRow {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub busy: bool,
    pub started_at: DateTime<Utc>,
}

pub async fn processes_of_boot(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
) -> Result<Vec<ProcessViewRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, name, status, busy, started_at FROM processes
         WHERE sandbox_id = $1 AND boot_id = $2 ORDER BY started_at, id LIMIT 100",
    )
    .bind(sandbox)
    .bind(boot)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone)]
pub struct ProcessUpsert<'a> {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub name: &'a str,
    pub command: &'a str,
    pub status: &'a str,
    pub busy: bool,
    pub exit_code: Option<i32>,
    pub tail: Option<&'a str>,
}

/// `process.state` upsert; an id owned by another sandbox is left untouched (false).
pub async fn upsert_process(
    conn: &mut PgConnection,
    p: &ProcessUpsert<'_>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO processes (id, agent_id, sandbox_id, boot_id, name, command, status, busy,
                exit_code, tail, last_output_at, started_at, ended_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,coalesce($10,''), CASE WHEN $10 IS NULL THEN NULL ELSE now() END,
                 now(), CASE WHEN $7 = 'running' THEN NULL ELSE now() END)
         ON CONFLICT (id) DO UPDATE SET status = EXCLUDED.status, busy = EXCLUDED.busy,
             exit_code = coalesce(EXCLUDED.exit_code, processes.exit_code),
             tail = CASE WHEN $10 IS NULL THEN processes.tail ELSE EXCLUDED.tail END,
             last_output_at = CASE WHEN $10 IS NULL THEN processes.last_output_at ELSE now() END,
             ended_at = CASE WHEN EXCLUDED.status = 'running' THEN NULL
                             ELSE coalesce(processes.ended_at, now()) END,
             updated_at = now()
         WHERE processes.sandbox_id = EXCLUDED.sandbox_id AND processes.boot_id = EXCLUDED.boot_id",
    )
    .bind(p.id)
    .bind(p.agent_id)
    .bind(p.sandbox_id)
    .bind(p.boot_id)
    .bind(p.name)
    .bind(p.command)
    .bind(p.status)
    .bind(p.busy)
    .bind(p.exit_code)
    .bind(p.tail)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// `watch.fired`: arm -> fired for a watch of this agent; returns its conversation.
pub async fn fire_watch(
    conn: &mut PgConnection,
    watch: Uuid,
    agent: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE watches SET status = 'fired', updated_at = now()
         WHERE id = $1 AND agent_id = $2 AND status = 'armed' RETURNING conversation_id",
    )
    .bind(watch)
    .bind(agent)
    .fetch_optional(conn)
    .await?)
}

pub async fn set_watch_turn(
    conn: &mut PgConnection,
    watch: Uuid,
    turn: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE watches SET fired_turn_id = $2 WHERE id = $1")
        .bind(watch)
        .bind(turn)
        .execute(conn)
        .await?;
    Ok(())
}

/// Lock a turn row and return `(agent_id, status, last_event_n)`.
pub async fn lock_turn_events(
    conn: &mut PgConnection,
    turn: Uuid,
) -> Result<Option<(Uuid, Uuid, String, i32)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT agent_id, conversation_id, status, last_event_n FROM turns
         WHERE id = $1 AND role = 'turn' FOR UPDATE",
    )
    .bind(turn)
    .fetch_optional(conn)
    .await?)
}

pub async fn set_last_event_n(conn: &mut PgConnection, turn: Uuid, n: i32) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE turns SET last_event_n = $2, last_progress_at = now(), updated_at = now()
         WHERE id = $1",
    )
    .bind(turn)
    .bind(n)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn set_usage(
    conn: &mut PgConnection,
    turn: Uuid,
    usage: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query("UPDATE turns SET usage = $2 WHERE id = $1 AND role = 'turn'")
        .bind(turn)
        .bind(usage)
        .execute(conn)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmCandidate {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub llm_provider: String,
    pub llm_endpoint_id: String,
    pub origin: Option<serde_json::Value>,
    pub value_version: i32,
}

/// Active LLM secrets granted to the agent (org- or workspace-scoped), oldest first.
pub async fn granted_llm_secrets(
    conn: &mut PgConnection,
    agent: Uuid,
    org: Uuid,
    workspace: Uuid,
) -> Result<Vec<LlmCandidate>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.workspace_id, s.llm_provider, s.llm_endpoint_id, s.origin, s.value_version
         FROM secrets s JOIN grants g ON g.secret_id = s.id AND g.agent_id = $1
         WHERE s.kind = 'llm' AND s.status = 'active' AND s.org_id = $2
           AND (s.workspace_id IS NULL OR s.workspace_id = $3)
           AND s.llm_provider IS NOT NULL AND s.llm_endpoint_id IS NOT NULL
         ORDER BY s.created_at, s.id",
    )
    .bind(agent)
    .bind(org)
    .bind(workspace)
    .fetch_all(conn)
    .await?)
}

/// Agent model override and effort.
pub async fn model_prefs(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<
    (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
    DbError,
> {
    Ok(sqlx::query_as(
        "SELECT model_provider, model_endpoint_id, model_id, effort FROM agents WHERE id = $1",
    )
    .bind(agent)
    .fetch_one(conn)
    .await?)
}

/// Queued turns of an agent as `(turn id, conversation id)`.
pub async fn queued_turns_of_agent(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, conversation_id FROM turns WHERE agent_id = $1 AND role = 'turn'
            AND status = 'queued'",
    )
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

/// Boot a turn was dispatched to.
pub async fn turn_boot(conn: &mut PgConnection, turn: Uuid) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT boot_id FROM turns WHERE id = $1")
            .bind(turn)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

/// The turn's durable answer buffer (`[{id,text,reply_to?}]`), if any (FUP-M4.1).
pub async fn answer_buffer_get(
    conn: &mut PgConnection,
    turn: Uuid,
) -> Result<Option<serde_json::Value>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT msgs FROM turn_answer_buffers WHERE turn_id = $1")
            .bind(turn)
            .fetch_optional(conn)
            .await?,
    )
}

pub async fn answer_buffer_set(
    conn: &mut PgConnection,
    turn: Uuid,
    msgs: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO turn_answer_buffers (turn_id, msgs) VALUES ($1, $2)
         ON CONFLICT (turn_id) DO UPDATE SET msgs = EXCLUDED.msgs, updated_at = now()",
    )
    .bind(turn)
    .bind(msgs)
    .execute(conn)
    .await?;
    Ok(())
}

/// Remove and return the turn's answer buffer.
pub async fn answer_buffer_take(
    conn: &mut PgConnection,
    turn: Uuid,
) -> Result<Option<serde_json::Value>, DbError> {
    Ok(
        sqlx::query_scalar("DELETE FROM turn_answer_buffers WHERE turn_id = $1 RETURNING msgs")
            .bind(turn)
            .fetch_optional(conn)
            .await?,
    )
}

// ------------------------------------------------------------------ render inputs (FUP-M4.5)

/// An agent's usable connection grant (active connection), for the rendered instructions.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HomeAppConn {
    pub connection_id: Uuid,
    pub app_id: String,
    pub label: String,
}

/// Active connections the agent holds a grant on, stable order.
pub async fn home_app_connections(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Vec<HomeAppConn>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id AS connection_id, c.app_id, c.label
         FROM grants g JOIN app_connections c ON c.id = g.connection_id
         WHERE g.agent_id = $1 AND c.status = 'active'
         ORDER BY c.app_id, g.created_at, g.id",
    )
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HomePresence {
    pub platform: String,
    pub tenant_label: String,
    pub mode: String,
    pub handle: Option<String>,
}

/// The agent's active presences on usable integrations.
pub async fn home_presences(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Vec<HomePresence>, DbError> {
    Ok(sqlx::query_as(
        "SELECT i.connector_id AS platform, i.tenant_label, p.mode,
                COALESCE(p.handle, i.shared_bot->>'handle') AS handle
         FROM channel_presences p JOIN channel_integrations i ON i.id = p.integration_id
         WHERE p.agent_id = $1 AND p.status = 'active' AND i.status = 'active'
         ORDER BY i.connector_id, i.tenant_label, p.id",
    )
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HomePeer {
    pub handle: String,
    pub name: String,
    pub own_policy: String,
    pub peer_policy: String,
}

/// Linked agents with the policy of each side.
pub async fn home_peers(conn: &mut PgConnection, agent: Uuid) -> Result<Vec<HomePeer>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.handle, o.name,
                CASE WHEN l.agent_a_id = $1 THEN l.policy_a ELSE l.policy_b END AS own_policy,
                CASE WHEN l.agent_a_id = $1 THEN l.policy_b ELSE l.policy_a END AS peer_policy
         FROM agent_links l
         JOIN agents o ON o.id = CASE WHEN l.agent_a_id = $1 THEN l.agent_b_id ELSE l.agent_a_id END
         WHERE l.agent_a_id = $1 OR l.agent_b_id = $1
         ORDER BY o.handle, o.id",
    )
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HomeSchedule {
    pub name: String,
    pub cron: String,
    pub timezone: String,
    pub enabled: bool,
}

pub async fn home_schedules(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Vec<HomeSchedule>, DbError> {
    Ok(sqlx::query_as(
        "SELECT name, cron, timezone, enabled FROM schedules WHERE agent_id = $1
         ORDER BY name, id",
    )
    .bind(agent)
    .fetch_all(conn)
    .await?)
}
