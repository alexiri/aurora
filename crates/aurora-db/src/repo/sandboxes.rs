//! Sandbox lifecycle persistence and the due-work claims for arms 1-3
//! (docs/05 §The due-work engine, docs/11 §Lifecycle).
//!
//! Claims follow the `aurora_db::claim` pattern (`UPDATE … WHERE id IN (… FOR UPDATE SKIP
//! LOCKED)` with a lease in `lifecycle_lease_until`) but also set several columns, so they are
//! spelled out here. No network call happens inside a claim.

use crate::DbError;
use aurora_core::limits;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// An agent has a turn that needs its sandbox awake (active turn or `joining` follow-up).
const ACTIVE_TURN: &str = "EXISTS (SELECT 1 FROM turns t WHERE t.agent_id = s.agent_id AND (
        (t.role = 'turn' AND t.status IN ('queued','dispatched','running'))
        OR (t.role = 'followup' AND t.status = 'joining')))";
/// Busy process or armed watch on the current boot (background keep-awake reasons).
const BACKGROUND: &str = "(EXISTS (SELECT 1 FROM processes p WHERE p.sandbox_id = s.id
        AND p.boot_id = s.boot_id AND p.status = 'running' AND p.busy)
    OR EXISTS (SELECT 1 FROM watches w JOIN processes p ON p.id = w.process_id
        WHERE p.sandbox_id = s.id AND p.boot_id = s.boot_id AND w.status = 'armed'))";
const ENGAGED_AGE: &str =
    "extract(epoch from now() - coalesce(s.last_engaged_at, s.last_active_at, s.created_at))";
const DUE_SCHEDULE: &str = "EXISTS (SELECT 1 FROM schedules sc WHERE sc.agent_id = s.agent_id
        AND sc.enabled AND sc.next_fire_at <= now()
        AND (sc.fire_lease_until IS NULL OR sc.fire_lease_until < now()))";

/// Sandbox rows that must exist and may be (re)started by `$1` runner / `$2` replica;
/// `$3` is the default harness id.
fn start_eligible() -> String {
    format!(
        "a.kind = 'hosted' AND (s.runner_id IS NULL OR s.runner_id = $1)
         AND (s.owner_replica_id IS NULL OR s.owner_replica_id = $2)
         AND (s.lifecycle_lease_until IS NULL OR s.lifecycle_lease_until < now())
         AND (s.state IN ('unprovisioned','stopped','starting')
              OR (s.state = 'failed' AND (
                    s.start_failures < 3
                    -- (FIX-025, D-158) after three failures automatic retries stop; a message
                    -- sent after the last failure retries once the backoff has passed.
                    OR EXISTS (SELECT 1 FROM turns t
                        WHERE t.agent_id = s.agent_id AND t.role = 'turn' AND t.status = 'queued'
                          AND coalesce(t.revived_at, t.created_at) > s.updated_at))))
         AND ({ACTIVE_TURN} OR {DUE_SCHEDULE} OR {PREVIEW_DEMAND} OR {TERMINAL_DEMAND})
         AND NOT {frozen}
         AND coalesce((SELECT capabilities->'harnesses' FROM runners WHERE id = $1) ? coalesce(a.harness, $3), false)",
        // (phase 2, RFE-006) Due-work arm 3 skips effectively frozen agents.
        frozen = super::freeze::frozen_agent_sql("a.id"),
        // (P4-PRV-1, D-145) a preview request found the sandbox stopped: the demand
        // (`preview_wake_at`, set by the relay resolve) is due work for the wake window. The org
        // must have Port previews on and an active share must exist; a frozen agent never wakes
        // (the `NOT {frozen}` above already covers it) and the hosted check is `a.kind`.
        PREVIEW_DEMAND = "EXISTS (SELECT 1 FROM preview_shares ps
               JOIN organizations po ON po.id = a.org_id
               WHERE ps.agent_id = s.agent_id AND ps.state = 'active'
                 AND s.preview_wake_at > now() - make_interval(secs => $6)
                 AND coalesce((po.settings->'previews_enabled')::bool, false))",
        // (P4-TERM, C-621) a terminal open found the sandbox parked: the demand
        // (`terminal_wake_at`, set by the API route) is due work for the wake window. Hosted
        // agents only (the `a.kind` arm above); a frozen agent never wakes.
        TERMINAL_DEMAND = "s.terminal_wake_at > now() - make_interval(secs => $6)",
    )
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SandboxRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub runner_id: Option<Uuid>,
    pub state: String,
    pub owner_replica_id: Option<Uuid>,
    pub channel_replica_id: Option<Uuid>,
    pub boot_id: Option<Uuid>,
    pub home_ref: Option<String>,
    pub stop_reason: Option<String>,
    pub home_applied_generation: i64,
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "id, agent_id, runner_id, state, owner_replica_id, channel_replica_id, boot_id,
    home_ref, stop_reason, home_applied_generation, created_at";

pub async fn get(conn: &mut PgConnection, id: Uuid) -> Result<Option<SandboxRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM sandboxes WHERE id = $1"))
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Arm 1: sandboxes of this runner whose owner replica is not live move to `replica`.
pub async fn take_over(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    online_secs: f64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE sandboxes s SET owner_replica_id = $2, updated_at = now()
         WHERE s.runner_id = $1 AND s.owner_replica_id IS NOT NULL AND s.owner_replica_id <> $2
           AND NOT EXISTS (SELECT 1 FROM runner_replicas r WHERE r.id = s.owner_replica_id
               AND r.retired_at IS NULL AND r.last_seen_at > now() - make_interval(secs => $3))",
    )
    .bind(runner)
    .bind(replica)
    .bind(online_secs)
    .execute(conn)
    .await?
    .rows_affected())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StopClaim {
    pub sandbox_id: Uuid,
    pub boot_id: Option<Uuid>,
    pub reason: String,
}

/// Arm 2: requested stop/restart, idle park, background cap, and stalled `stopping` re-drives.
/// `awake_cap_secs` is `PREVIEW_AWAKE_CAP`/`TERMINAL_AWAKE_CAP` (the keep-awake reasons
/// `preview` and `terminal`'s shared per-boot bound).
#[allow(clippy::too_many_arguments)]
pub async fn claim_stops(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    idle_secs: f64,
    cap_secs: f64,
    lease_secs: f64,
    awake_cap_secs: f64,
    limit: i64,
) -> Result<Vec<StopClaim>, DbError> {
    let sql = format!(
        "WITH cand AS (
           SELECT s.id,
             CASE
               WHEN s.state = 'stopping' THEN coalesce(s.stop_reason, 'idle')
               WHEN {frozen} THEN 'frozen'
               WHEN s.restart_requested_at IS NOT NULL THEN 'restart'
               WHEN s.stop_requested_at IS NOT NULL THEN coalesce(s.stop_reason, 'idle')
               WHEN {ACTIVE_TURN} THEN NULL
               -- (P4-PRV-1, D-145) keep-awake reason `preview`: stream activity refreshes
               -- `preview_active_until`; the cap (`preview_awake_since`) bounds a boot, so an
               -- open WebSocket cannot hold the sandbox awake forever.
               -- (P4-TERM, C-620) keep-awake reason `terminal` in the same shape: byte activity
               -- refreshes `terminal_active_until`, bounded by the same cap clock per boot.
               WHEN {PREVIEW_KEEP} OR {TERMINAL_KEEP} THEN NULL
               WHEN {BACKGROUND} THEN CASE WHEN {ENGAGED_AGE} >= $4 THEN 'cap' END
               WHEN {ENGAGED_AGE} >= $3 THEN 'idle'
             END AS reason
           FROM sandboxes s
           WHERE s.runner_id = $1 AND s.owner_replica_id = $2
             AND (s.state = 'running' OR (s.state = 'stopping'
                  AND (s.lifecycle_lease_until IS NULL OR s.lifecycle_lease_until < now())))
         ), pick AS (
           SELECT s.id, c.reason FROM cand c JOIN sandboxes s ON s.id = c.id
           WHERE c.reason IS NOT NULL ORDER BY s.updated_at LIMIT $5
           FOR UPDATE OF s SKIP LOCKED)
         UPDATE sandboxes s SET state = 'stopping', stop_reason = p.reason,
             lifecycle_lease_until = now() + make_interval(secs => $6), updated_at = now()
         FROM pick p WHERE s.id = p.id AND s.state IN ('running','stopping')
         RETURNING s.id AS sandbox_id, s.boot_id, p.reason",
        // (phase 2, RFE-006) A frozen agent's sandbox is parked regardless of keep-awake reasons.
        frozen = super::freeze::frozen_agent_sql("s.agent_id"),
        // (P4-PRV-1) `preview_active_until > now()` (relay heartbeat) and the boot's cap clock:
        // `preview_awake_since` was set at the boot's first heartbeat and `now() - it` must still
        // be under `PREVIEW_AWAKE_CAP` ($7).
        PREVIEW_KEEP = "s.preview_active_until > now() \
             AND (s.preview_awake_since IS NULL \
                  OR now() - s.preview_awake_since < make_interval(secs => $7))",
        // (P4-TERM, C-620) the terminal's stamp and cap clock, same bound ($7 = the awake cap).
        TERMINAL_KEEP = "s.terminal_active_until > now() \
             AND (s.terminal_awake_since IS NULL \
                  OR now() - s.terminal_awake_since < make_interval(secs => $7))",
    );
    Ok(sqlx::query_as(&sql)
        .bind(runner)
        .bind(replica)
        .bind(idle_secs)
        .bind(cap_secs)
        .bind(limit)
        .bind(lease_secs)
        .bind(awake_cap_secs)
        .fetch_all(conn)
        .await?)
}

/// Arm 2 eviction: when a start is waiting and running-but-not-working sandboxes exceed the
/// held-awake `ceiling`, stop the longest-idle one (`evict`).
pub async fn claim_evict(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    ceiling: i64,
    default_harness: &str,
    lease_secs: f64,
    wake_window_secs: f64,
) -> Result<Option<StopClaim>, DbError> {
    let held: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM sandboxes s WHERE s.runner_id = $1 AND s.state = 'running'
         AND NOT {ACTIVE_TURN}"
    ))
    .bind(runner)
    .fetch_one(&mut *conn)
    .await?;
    if held <= ceiling
        || !pending_start(conn, runner, replica, default_harness, wake_window_secs).await?
    {
        return Ok(None);
    }
    Ok(sqlx::query_as(&format!(
        "UPDATE sandboxes SET state = 'stopping', stop_reason = 'evict',
             lifecycle_lease_until = now() + make_interval(secs => $3), updated_at = now()
         WHERE id = (SELECT s.id FROM sandboxes s
             WHERE s.runner_id = $1 AND s.owner_replica_id = $2 AND s.state = 'running'
               AND NOT {ACTIVE_TURN}
             ORDER BY coalesce(s.last_engaged_at, s.last_active_at, s.created_at)
             LIMIT 1 FOR UPDATE SKIP LOCKED)
         RETURNING id AS sandbox_id, boot_id, 'evict'::text AS reason"
    ))
    .bind(runner)
    .bind(replica)
    .bind(lease_secs)
    .fetch_optional(conn)
    .await?)
}

/// Is a start waiting for this runner/replica (ignoring capacity)?
pub async fn pending_start(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    default_harness: &str,
    wake_window_secs: f64,
) -> Result<bool, DbError> {
    // `start_eligible()` is shared with `claim_starts`, where `$4`/`$5` are the limit and the
    // lease and `$6` the wake window: bind the unused `$4`/`$5` so `$6` stays the wake window.
    Ok(sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM sandboxes s JOIN agents a ON a.id = s.agent_id
                        WHERE {})",
        start_eligible()
    ))
    .bind(runner)
    .bind(replica)
    .bind(default_harness)
    .bind(0_i64)
    .bind(0_f64)
    .bind(wake_window_secs)
    .fetch_one(conn)
    .await?)
}

/// (P4-TERM, C-620) The keep-awake stamp of reason `terminal`: the runner sends it while a
/// terminal session carries bytes. `now() + TERMINAL_AWAKE_GRACE` on each stamp; the cap clock
/// (`terminal_awake_since`) starts at the boot's first session byte. Never a turn, never an
/// engagement stamp — the same shape as the relay's `keep_preview_awake`.
pub async fn keep_terminal_awake<'e, E: PgExecutor<'e>>(
    exec: E,
    sandbox: Uuid,
    grace_secs: i64,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE sandboxes SET terminal_active_until = now() + make_interval(secs => $2), \
             terminal_awake_since = coalesce(terminal_awake_since, now()), updated_at = now() \
           WHERE id = $1",
    )
    .bind(sandbox)
    .bind(grace_secs)
    .execute(exec)
    .await?;
    Ok(())
}

/// (P4-TERM, C-621) Set the wake demand (`terminal_wake_at`): a terminal open found the
/// sandbox parked; the start arm treats it as due work for `TERMINAL_WAKE_WINDOW`.
pub async fn set_terminal_wake<'e, E: PgExecutor<'e>>(
    exec: E,
    sandbox: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE sandboxes SET terminal_wake_at = now(), updated_at = now() WHERE id = $1")
        .bind(sandbox)
        .execute(exec)
        .await?;
    Ok(())
}

/// Sandbox rows are created with hosted agents; this heals agents that lack one but need it.
pub async fn ensure_rows(conn: &mut PgConnection) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "INSERT INTO sandboxes (id, agent_id, state)
         SELECT gen_random_uuid(), a.id, 'unprovisioned' FROM agents a
         WHERE a.kind = 'hosted' AND NOT EXISTS (SELECT 1 FROM sandboxes x WHERE x.agent_id = a.id)
           AND (EXISTS (SELECT 1 FROM turns t WHERE t.agent_id = a.id AND (
                  (t.role = 'turn' AND t.status IN ('queued','dispatched','running'))
                  OR (t.role = 'followup' AND t.status = 'joining')))
                OR EXISTS (SELECT 1 FROM schedules sc WHERE sc.agent_id = a.id AND sc.enabled
                  AND sc.next_fire_at <= now()))
         LIMIT 16
         ON CONFLICT (agent_id) DO NOTHING",
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// Serialise capacity decisions of one runner inside the current transaction.
pub async fn lock_runner(conn: &mut PgConnection, runner: Uuid) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(runner)
        .execute(conn)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StartClaim {
    pub sandbox_id: Uuid,
    pub agent_id: Uuid,
}

/// Arm 3: claim up to `limit` sandboxes to start on this runner.
pub async fn claim_starts(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    default_harness: &str,
    limit: i64,
    lease_secs: f64,
    wake_window_secs: f64,
) -> Result<Vec<StartClaim>, DbError> {
    let sql = format!(
        "WITH cand AS (SELECT s.id FROM sandboxes s JOIN agents a ON a.id = s.agent_id
             WHERE {}
             ORDER BY coalesce(s.last_engaged_at, s.created_at), s.id
             LIMIT $4 FOR UPDATE OF s SKIP LOCKED)
         UPDATE sandboxes s SET state = 'starting', runner_id = $1, owner_replica_id = $2,
             -- (FIX-093) the attempt clock starts at the claim (not at the previous boot's stamp);
             -- a re-claim of a still-`starting` row keeps it, so the deadline cannot be renewed.
             start_attempt_at = CASE WHEN s.state = 'starting' THEN s.start_attempt_at ELSE now() END,
             lifecycle_lease_until = now() + make_interval(secs => $5), updated_at = now()
         FROM cand WHERE s.id = cand.id
         RETURNING s.id AS sandbox_id, s.agent_id",
        start_eligible()
    );
    Ok(sqlx::query_as(&sql)
        .bind(runner)
        .bind(replica)
        .bind(default_harness)
        .bind(limit)
        .bind(lease_secs)
        .bind(wake_window_secs)
        .fetch_all(conn)
        .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StartAgent {
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub handle: String,
    pub name: String,
    pub harness: Option<String>,
    pub resource_overrides: Option<serde_json::Value>,
    pub home_ref: Option<String>,
}

pub async fn start_agent(
    conn: &mut PgConnection,
    sandbox: Uuid,
) -> Result<Option<StartAgent>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.org_id, a.workspace_id, a.handle, a.name, a.harness, a.resource_overrides, s.home_ref
         FROM sandboxes s JOIN agents a ON a.id = s.agent_id WHERE s.id = $1",
    )
    .bind(sandbox)
    .fetch_optional(conn)
    .await?)
}

/// Stamp the per-boot fields of a freshly claimed start and mint its boot credential.
/// Returns the ids of the previous boot credentials that were revoked.
#[allow(clippy::too_many_arguments)]
pub async fn begin_boot(
    conn: &mut PgConnection,
    sandbox: Uuid,
    agent: Uuid,
    boot_id: Uuid,
    bootstrap_hash: &[u8],
    home_ref: &str,
    profile: &serde_json::Value,
    boot_hash: &[u8],
    boot_prefix: &str,
    capabilities: &[String],
) -> Result<Vec<Uuid>, DbError> {
    sqlx::query(
        "UPDATE sandboxes SET boot_id = $2, bootstrap_token_hash = $3, bootstrap_used_at = NULL,
             channel_held_at = NULL, channel_replica_id = NULL, listening_ports = '{}', home_applied_generation = 0,
             stop_reason = NULL, stop_requested_at = NULL, restart_requested_at = NULL,
             failure_code = NULL, failure_message = NULL, container_ref = NULL, home_ref = $4,
             resource_profile = $5::jsonb,
             start_attempt_at = now(), start_phase = 'claimed', start_detail = NULL,
             start_events = '[]'::jsonb, start_progress_at = NULL, running_at = NULL,
             start_log = '[]'::jsonb,
             -- (P4-PRV-1) every new boot restarts the preview keep-awake cap clock.
             preview_awake_since = NULL, preview_start_requested_at = NULL,
             -- (P4-TERM, C-620) and the terminal's.
             terminal_awake_since = NULL, terminal_active_until = NULL,
             updated_at = now()
         WHERE id = $1",
    )
    .bind(sandbox)
    .bind(boot_id)
    .bind(bootstrap_hash)
    .bind(home_ref)
    .bind(profile)
    .execute(&mut *conn)
    .await?;
    let revoked = revoke_boot_credentials(conn, agent, None).await?;
    sqlx::query(
        "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, boot_id, capabilities)
         VALUES ($1,'boot',$2,$3,$4,$5,$6)",
    )
    .bind(Uuid::now_v7())
    .bind(boot_hash)
    .bind(boot_prefix)
    .bind(agent)
    .bind(boot_id)
    .bind(capabilities)
    .execute(conn)
    .await?;
    Ok(revoked)
}

/// Revoke live boot credentials of the agent (optionally only those of `boot`).
pub async fn revoke_boot_credentials(
    conn: &mut PgConnection,
    agent: Uuid,
    boot: Option<Uuid>,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE gateway_credentials SET revoked_at = now(), updated_at = now()
         WHERE agent_id = $1 AND kind = 'boot' AND revoked_at IS NULL
           AND ($2::uuid IS NULL OR boot_id = $2)
         RETURNING id",
    )
    .bind(agent)
    .bind(boot)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Applied,
    Duplicate,
    Fenced,
}

#[derive(Debug, Clone)]
pub struct StatusUpdate<'a> {
    /// `running`, `stopped`, `failed`, `held` or (FIX-025) `starting`.
    pub state: &'a str,
    pub container_ref: Option<&'a str>,
    pub reason_code: Option<&'a str>,
    pub message: Option<&'a str>,
    /// (FIX-025, C-580) Substrate warning events (already bounded by the protocol).
    pub events: &'a [String],
    /// (FIX-025, D-158) A `starting`/`held` progress report renews the start lease to at least
    /// `now() + lease_secs`.
    pub lease_secs: f64,
}

/// (FIX-025) Phase recorded for a `held` report (the runner waits for capacity).
pub const PHASE_WAITING_CAPACITY: &str = "waiting_capacity";
/// (FIX-025) Phase after the runner reported the container running: the supervisor must connect.
pub const PHASE_WAITING_SUPERVISOR: &str = "waiting_supervisor";
/// (FIX-025) Phase before any runner progress: the start item is claimed, not yet acknowledged.
pub const PHASE_CLAIMED: &str = "claimed";

/// Phase vocabulary accepted from a runner; anything else is stored as `starting`.
const RUNNER_PHASES: [&str; 8] = [
    "queued",
    "provisioning",
    "waiting_capacity",
    "scheduling",
    "pulling",
    "creating",
    "pending",
    "starting",
];

/// (FIX-025, C-583) Running sandboxes whose supervisor never connected (`bootstrap_used_at`
/// unset) within `secs` of the runner's `running` report: `(sandbox, boot, start_detail)`.
pub async fn hello_overdue(
    conn: &mut PgConnection,
    secs: f64,
    limit: i64,
) -> Result<Vec<(Uuid, Uuid, Option<String>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, boot_id, start_detail FROM sandboxes
         WHERE state = 'running' AND bootstrap_used_at IS NULL AND boot_id IS NOT NULL
           AND running_at < now() - make_interval(secs => $1)
         ORDER BY running_at LIMIT $2",
    )
    .bind(secs)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// (FIX-093, C-580) A `starting` attempt with no waiting turn needs a deadline too (due schedules,
/// preview and terminal demand claim starts as well).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StaleStart {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub runner_live: bool,
    pub start_phase: Option<String>,
    pub start_detail: Option<String>,
}

/// (FIX-093, C-580) `starting` sandboxes whose attempt is older than `secs` (the readiness
/// deadline), or whose lifecycle lease lapsed while the runner holding them is no longer live
/// (replica seen within `online_secs`). No locks: `fail_attempt` fences.
pub async fn stale_starts(
    conn: &mut PgConnection,
    secs: f64,
    online_secs: f64,
    limit: i64,
) -> Result<Vec<StaleStart>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id AS sandbox_id, boot_id, runner_live, start_phase, start_detail FROM (
             SELECT s.id, s.boot_id, s.start_phase, s.start_detail, s.start_attempt_at,
                    s.lifecycle_lease_until,
                    EXISTS (SELECT 1 FROM runner_replicas rr WHERE rr.runner_id = s.runner_id
                        AND rr.retired_at IS NULL
                        AND rr.last_seen_at > now() - make_interval(secs => $2)) AS runner_live
             FROM sandboxes s WHERE s.state = 'starting' AND s.boot_id IS NOT NULL) x
         WHERE (start_attempt_at < now() - make_interval(secs => $1))
            OR (NOT runner_live AND (lifecycle_lease_until IS NULL OR lifecycle_lease_until < now()))
         ORDER BY start_attempt_at NULLS FIRST LIMIT $3",
    )
    .bind(secs)
    .bind(online_secs)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// (FIX-093) Whether the sandbox is ready for dispatch right now (supervisor connected, channel
/// held, harness ready, home synced) — the predicate `fail_attempt` fences on. Call inside the
/// transaction that holds the sandbox row.
pub async fn is_ready(conn: &mut PgConnection, sandbox: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT coalesce(bool_or(s.state = 'running' AND s.bootstrap_used_at IS NOT NULL
                AND s.channel_replica_id IS NOT NULL AND s.resource_profile ? 'harness'
                AND s.home_applied_generation >=
                    (a.home_sync_generation + w.home_sync_generation + 1)), false)
         FROM sandboxes s JOIN agents a ON a.id = s.agent_id JOIN workspaces w ON w.id = a.workspace_id
         WHERE s.id = $1",
    )
    .bind(sandbox)
    .fetch_one(conn)
    .await?)
}

/// (FIX-071) The hello-overdue give-up: `fail_attempt`, additionally fenced on the supervisor
/// still not having connected — the predicate `hello_overdue` selects on (`running`,
/// `bootstrap_used_at IS NULL`). The row is locked first (held to the end of the caller's
/// transaction), so a supervisor that connected between the selection and this update leaves the
/// sandbox to the ordinary readiness deadline, which names the correct later-phase reason.
/// Call inside a transaction.
pub async fn fail_attempt_unconnected(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
    code: &str,
    message: &str,
) -> Result<Option<Vec<Uuid>>, DbError> {
    fail_attempt_unconnected_detail(conn, sandbox, boot, code, message, message).await
}

/// [`fail_attempt_unconnected`] with a separate `admin_message` for the start log and the admin
/// start-failure rows (FIX-093: it may name the runner endpoint, the member text never does).
pub async fn fail_attempt_unconnected_detail(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
    code: &str,
    message: &str,
    admin_message: &str,
) -> Result<Option<Vec<Uuid>>, DbError> {
    let still: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM sandboxes
         WHERE id = $1 AND boot_id = $2 AND state = 'running' AND bootstrap_used_at IS NULL
         FOR UPDATE",
    )
    .bind(sandbox)
    .bind(boot)
    .fetch_optional(&mut *conn)
    .await?;
    if still.is_none() {
        return Ok(None);
    }
    fail_attempt_detail(conn, sandbox, boot, code, message, false, admin_message).await
}

/// (FIX-025, D-158) The API gives up on the current start attempt of `boot`: the sandbox moves to
/// `stopping` (reason `failure`, lease cleared so arm 2 sends the stop at the next poll) with the
/// failure recorded; the `stopped` report then lands it in `failed` with the backoff. Fenced on
/// state `starting`/`running` and the boot, and (FIX-058) on "still not ready": a sandbox that
/// became ready between the sweep's snapshot and this update (supervisor connected, channel held,
/// harness ready, home synced) is left alone.
///
/// (FIX-058) With `runner_gone` — no live runner holds the sandbox, so nothing would ever send the
/// stop — the attempt ends terminally instead: the sandbox goes straight to `failed` with the
/// backoff `apply_status` uses for a `failed` report (the boot ends, its credentials are
/// revoked), and a `stopping` row is resolved the same way instead of wedging. Returns `None`
/// when nothing changed, else the boot-credential ids revoked (empty unless `runner_gone`; the
/// caller notifies the gateway).
pub async fn fail_attempt(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
    code: &str,
    message: &str,
    runner_gone: bool,
) -> Result<Option<Vec<Uuid>>, DbError> {
    fail_attempt_detail(conn, sandbox, boot, code, message, runner_gone, message).await
}

/// [`fail_attempt`] with a separate `admin_message` for the start log and the admin
/// start-failure rows (FIX-093: it may name the runner endpoint; `message` is member-facing and
/// never carries an address).
pub async fn fail_attempt_detail(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
    code: &str,
    message: &str,
    runner_gone: bool,
    admin_message: &str,
) -> Result<Option<Vec<Uuid>>, DbError> {
    let row: Option<(Uuid, i32)> = sqlx::query_as(
        "SELECT s.agent_id, s.start_failures FROM sandboxes s
           JOIN agents a ON a.id = s.agent_id JOIN workspaces w ON w.id = a.workspace_id
         WHERE s.id = $1 AND s.boot_id = $2
           AND (s.state IN ('starting','running') OR ($3 AND s.state = 'stopping'))
           AND NOT (s.state = 'running' AND s.bootstrap_used_at IS NOT NULL
                AND s.channel_replica_id IS NOT NULL AND s.resource_profile ? 'harness'
                AND s.home_applied_generation >=
                    (a.home_sync_generation + w.home_sync_generation + 1))
         FOR UPDATE OF s",
    )
    .bind(sandbox)
    .bind(boot)
    .bind(runner_gone)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((agent, failures)) = row else {
        return Ok(None);
    };
    let mut revoked = Vec::new();
    if runner_gone {
        sqlx::query(
            "UPDATE sandboxes SET state = 'failed', stop_reason = 'failure',
                 failure_code = $2, failure_message = $3, start_failures = $4,
                 lifecycle_lease_until = now() + make_interval(secs => $5),
                 -- (FIX-093) no runner holds it: the next attempt may go to any runner.
                 runner_id = NULL, owner_replica_id = NULL,
                 channel_replica_id = NULL, listening_ports = '{}', container_ref = NULL,
                 stop_requested_at = NULL, restart_requested_at = NULL, updated_at = now()
             WHERE id = $1",
        )
        .bind(sandbox)
        .bind(code)
        .bind(message)
        .bind(failures + 1)
        .bind(backoff_secs(failures + 1))
        .execute(&mut *conn)
        .await?;
        end_boot(conn, sandbox, boot, "failure", "parked").await?;
        revoked = revoke_boot_credentials(conn, agent, Some(boot)).await?;
    } else {
        sqlx::query(
            "UPDATE sandboxes SET state = 'stopping', stop_reason = 'failure',
                 failure_code = $2, failure_message = $3, start_failures = start_failures + 1,
                 lifecycle_lease_until = NULL, updated_at = now()
             WHERE id = $1",
        )
        .bind(sandbox)
        .bind(code)
        .bind(message)
        .execute(&mut *conn)
        .await?;
    }
    // (FIX-029, C-612) The readiness sweep's verdict enters the boot's start log.
    append_start_log(conn, sandbox, "phase", Some("failed"), Some(admin_message)).await?;
    record_start_failure(
        conn,
        sandbox,
        Some(boot),
        code,
        admin_message,
        &serde_json::json!([]),
    )
    .await?;
    Ok(Some(revoked))
}

fn events_json(events: &[String]) -> serde_json::Value {
    serde_json::Value::from(events.to_vec())
}

/// Record a start failure for the admin view (FIX-025, C-586) and prune rows older than 30 days.
#[allow(clippy::too_many_arguments)]
pub async fn record_start_failure(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Option<Uuid>,
    code: &str,
    message: &str,
    events: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO sandbox_start_failures
             (id, runner_id, sandbox_id, agent_id, boot_id, code, message, phase, events, elapsed_secs)
         SELECT $1, s.runner_id, s.id, s.agent_id, $3, $4, $5,
                CASE WHEN s.boot_id IS NOT DISTINCT FROM $3 THEN s.start_phase END, $6,
                CASE WHEN s.boot_id IS NOT DISTINCT FROM $3 AND s.start_attempt_at IS NOT NULL
                     THEN extract(epoch from now() - s.start_attempt_at)::int END
         FROM sandboxes s WHERE s.id = $2",
    )
    .bind(Uuid::now_v7())
    .bind(sandbox)
    .bind(boot)
    .bind(code)
    .bind(message)
    .bind(events)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM sandbox_start_failures WHERE created_at < now() - interval '30 days'")
        .execute(conn)
        .await?;
    Ok(())
}

/// Failure backoff after the n-th consecutive start failure: 30 s, 2 min, 10 min.
pub fn backoff_secs(failures: i32) -> f64 {
    match failures {
        ..=1 => 30.0,
        2 => 120.0,
        _ => 600.0,
    }
}

/// (FIX-029, D-160, C-612) One entry of a boot's start log (`sandboxes.start_log`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StartLogEntry {
    pub ts: DateTime<Utc>,
    /// `phase` (a runner report or a failure verdict) or `note` (a retry).
    pub kind: String,
    pub phase: Option<String>,
    pub detail: Option<String>,
}

/// (FIX-029, C-612) Append one entry to a sandbox's start log: newest first, `detail` sanitised
/// and clamped, bounded to [`limits::SANDBOX_START_LOG_MAX`] entries and
/// [`limits::SANDBOX_START_LOG_BYTES`] total (oldest dropped).
pub async fn append_start_log(
    conn: &mut PgConnection,
    sandbox: Uuid,
    kind: &str,
    phase: Option<&str>,
    detail: Option<&str>,
) -> Result<(), DbError> {
    let detail = detail.map(|d| aurora_core::sanitize::reason(d, limits::SANDBOX_START_LOG_DETAIL));
    let entry = serde_json::json!({
        "ts": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "kind": kind,
        "phase": phase,
        "detail": detail,
    });
    // Read-modify-write the newest-first ring: prepend, then drop the oldest entries until
    // both the entry cap and the total byte budget hold.
    let current: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT start_log FROM sandboxes WHERE id = $1")
            .bind(sandbox)
            .fetch_optional(&mut *conn)
            .await?;
    let mut entries: Vec<serde_json::Value> = match current {
        Some(serde_json::Value::Array(entries)) => entries,
        _ => Vec::new(),
    };
    entries.insert(0, entry);
    entries.truncate(limits::SANDBOX_START_LOG_MAX);
    while serde_json::to_vec(&entries).is_ok_and(|b| b.len() > limits::SANDBOX_START_LOG_BYTES)
        && !entries.is_empty()
    {
        entries.pop();
    }
    sqlx::query("UPDATE sandboxes SET start_log = $2, updated_at = now() WHERE id = $1")
        .bind(sandbox)
        .bind(serde_json::Value::Array(entries))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// (FIX-029, C-610) Retry a failed start from `failed`: clear the failure fields, reset
/// `start_failures`, park the sandbox back in `stopped` and clear the backoff lease — all in the
/// caller's transaction. The ordinary due-work start arm (`claim_starts`, notified right after)
/// then claims the sandbox with a fresh `boot_id` and the ordinary `lifecycle_lease_until` —
/// the retry itself never sets a lease, because a live lease would block that claim (and the
/// readiness deadline is shorter than a start lease). (FIX-049, D-164) An explicit retry overrides
/// the failure-backoff lease (it stops only *automatic* storms, C-583; a live lease on a `failed`
/// row fences no in-flight start, which would leave the row `starting`). Returns `false` when
/// nothing was retried (the state is not `failed`) so the caller answers 409
/// `sandbox_not_failed`; the conditional UPDATE also fences concurrent retries.
pub async fn retry_failed_start(
    conn: &mut PgConnection,
    agent: Uuid,
    note: &str,
) -> Result<bool, DbError> {
    let n = sqlx::query(
        "UPDATE sandboxes SET
             state = 'stopped', failure_code = NULL, failure_message = NULL,
             start_failures = 0, lifecycle_lease_until = NULL,
             stop_reason = NULL, stop_requested_at = NULL, restart_requested_at = NULL,
             updated_at = now()
         WHERE agent_id = $1 AND state = 'failed'",
    )
    .bind(agent)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if n == 0 {
        // Another caller won, or the state is not `failed`: 409 `sandbox_not_failed`.
        return Ok(false);
    }
    let sandbox: Uuid = sqlx::query_scalar("SELECT id FROM sandboxes WHERE agent_id = $1")
        .bind(agent)
        .fetch_one(&mut *conn)
        .await?;
    append_start_log(conn, sandbox, "note", Some("retry"), Some(note)).await?;
    Ok(true)
}

/// (FIX-029, C-613) The agent's recent start failures (newest first, ≤ `limit`) with the runner
/// named, read from `sandbox_start_failures` — the same rows Admin › Runners lists per runner
/// (C-586, C-613, C-614).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentStartFailureRow {
    pub id: Uuid,
    pub sandbox_id: Uuid,
    pub runner_id: Option<Uuid>,
    pub agent_id: Uuid,
    pub boot_id: Option<Uuid>,
    pub code: String,
    pub message: String,
    pub phase: Option<String>,
    pub events: serde_json::Value,
    pub elapsed_secs: Option<i32>,
    pub created_at: DateTime<Utc>,
}

/// (FIX-029, C-613) Typed query for [`AgentStartFailureRow`].
pub async fn agent_start_failures(
    conn: &mut PgConnection,
    agent: Uuid,
    limit: i64,
) -> Result<Vec<AgentStartFailureRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, sandbox_id, runner_id, agent_id, boot_id, code, message, phase, events,
                elapsed_secs, created_at
         FROM sandbox_start_failures WHERE agent_id = $1
         ORDER BY created_at DESC, id DESC LIMIT $2",
    )
    .bind(agent)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?)
}

/// `(state, boot, owner, agent, stop_reason, start_failures, failure_code)`.
type StatusRow = (
    String,
    Option<Uuid>,
    Option<Uuid>,
    Uuid,
    Option<String>,
    i32,
    Option<String>,
);

/// Apply a lifecycle `sandbox.status` report, fenced on `(sandbox, boot, owner replica)`.
/// Returns the outcome and the boot-credential ids revoked (for gateway invalidation).
pub async fn apply_status(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
    replica: Uuid,
    u: &StatusUpdate<'_>,
) -> Result<(Applied, Vec<Uuid>), DbError> {
    let row: Option<StatusRow> = sqlx::query_as(
        "SELECT s.state, s.boot_id, s.owner_replica_id, s.agent_id, s.stop_reason,
                    s.start_failures, s.failure_code
             FROM sandboxes s WHERE s.id = $1 FOR UPDATE",
    )
    .bind(sandbox)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((state, cur_boot, owner, agent, stop_reason, failures, failure_code)) = row else {
        return Ok((Applied::Fenced, vec![]));
    };
    if cur_boot != Some(boot) || owner != Some(replica) {
        // (FIX-025) A start failure of a superseded boot is still kept for operators.
        if u.state == "failed" && u.reason_code != Some("agent_stop_failed") {
            record_start_failure(
                conn,
                sandbox,
                Some(boot),
                u.reason_code.unwrap_or("failed"),
                u.message.unwrap_or(""),
                &events_json(u.events),
            )
            .await?;
        }
        return Ok((Applied::Fenced, vec![]));
    }
    match u.state {
        // (FIX-025, D-158) Progress of the current start: renew the lease, record the phase.
        "starting" | "held" => {
            if state != "starting" {
                return Ok((Applied::Duplicate, vec![]));
            }
            let phase = if u.state == "held" {
                PHASE_WAITING_CAPACITY
            } else {
                u.reason_code
                    .filter(|p| RUNNER_PHASES.contains(p))
                    .unwrap_or("starting")
            };
            sqlx::query(
                "UPDATE sandboxes SET
                     lifecycle_lease_until = greatest(coalesce(lifecycle_lease_until, now()),
                                                      now() + make_interval(secs => $2)),
                     start_phase = $3, start_detail = coalesce($4, start_detail),
                     start_events = CASE WHEN jsonb_array_length($5) > 0 THEN $5 ELSE start_events END,
                     start_progress_at = now(), updated_at = now()
                 WHERE id = $1",
            )
            .bind(sandbox)
            .bind(u.lease_secs)
            .bind(phase)
            .bind(u.message)
            .bind(events_json(u.events))
            .execute(&mut *conn)
            .await?;
            // (FIX-029, C-612) The phase report enters the boot's start log.
            append_start_log(conn, sandbox, "phase", Some(phase), u.message).await?;
            Ok((Applied::Applied, vec![]))
        }
        "running" => match state.as_str() {
            "running" => Ok((Applied::Duplicate, vec![])),
            "starting" => {
                sqlx::query(
                    "UPDATE sandboxes SET state='running', container_ref=$2, lifecycle_lease_until=NULL,
                         last_active_at=now(),
                         running_at = now(), start_phase = $3,
                         start_detail = coalesce($4, start_detail), start_progress_at = now(),
                         updated_at=now() WHERE id=$1",
                )
                .bind(sandbox)
                .bind(u.container_ref)
                .bind(PHASE_WAITING_SUPERVISOR)
                .bind(u.message)
                .execute(&mut *conn)
                .await?;
                sqlx::query(
                    "INSERT INTO runtime_intervals (id, org_id, workspace_id, agent_id, sandbox_id, boot_id, started_at)
                     SELECT $1, a.org_id, a.workspace_id, a.id, $2, $3, now() FROM agents a WHERE a.id = $4
                     ON CONFLICT DO NOTHING",
                )
                .bind(Uuid::now_v7())
                .bind(sandbox)
                .bind(boot)
                .bind(agent)
                .execute(&mut *conn)
                .await?;
                Ok((Applied::Applied, vec![]))
            }
            _ => Ok((Applied::Fenced, vec![])),
        },
        "stopped" => {
            if state == "stopped" {
                return Ok((Applied::Duplicate, vec![]));
            }
            if state == "failed" {
                return Ok((Applied::Fenced, vec![]));
            }
            // (FIX-025, D-158) A stop the API ordered because the start failed (readiness
            // deadline, supervisor never connected) ends in `failed` with the start backoff.
            let failed_start = stop_reason.as_deref() == Some("failure");
            sqlx::query(
                "UPDATE sandboxes SET state = CASE WHEN $2 THEN 'failed' ELSE 'stopped' END,
                     lifecycle_lease_until = CASE WHEN $2 THEN now() + make_interval(secs => $3) END,
                     channel_replica_id=NULL,
                     listening_ports='{}', container_ref=NULL, stop_requested_at=NULL,
                     restart_requested_at=NULL, updated_at=now() WHERE id=$1",
            )
            .bind(sandbox)
            .bind(failed_start)
            .bind(backoff_secs(failures))
            .execute(&mut *conn)
            .await?;
            end_boot(
                conn,
                sandbox,
                boot,
                stop_reason.as_deref().unwrap_or("stopped"),
                "parked",
            )
            .await?;
            let revoked = revoke_boot_credentials(conn, agent, Some(boot)).await?;
            Ok((Applied::Applied, revoked))
        }
        "failed" => {
            if state == "failed" || state == "stopped" {
                return Ok((Applied::Duplicate, vec![]));
            }
            // (FIX-093) The API already gave this boot up (`fail_attempt`: `stopping`, reason
            // `failure`, code recorded, failure counted once): the runner's late `failed` for the
            // same boot only lands the sandbox in `failed`: no second count, the code and message
            // the API recorded stay.
            let late = state == "stopping"
                && stop_reason.as_deref() == Some("failure")
                && failure_code.is_some();
            let n = if late { failures } else { failures + 1 };
            sqlx::query(
                "UPDATE sandboxes SET state='failed',
                     failure_code = CASE WHEN $6 THEN failure_code ELSE $2 END,
                     failure_message = CASE WHEN $6 THEN failure_message ELSE $3 END,
                     channel_replica_id=NULL, listening_ports='{}', container_ref=NULL,
                     lifecycle_lease_until = now() + make_interval(secs => $5),
                     start_failures = $4,
                     updated_at=now() WHERE id=$1",
            )
            .bind(sandbox)
            .bind(u.reason_code.unwrap_or("failed"))
            .bind(u.message)
            .bind(n)
            .bind(backoff_secs(n))
            .bind(late)
            .execute(&mut *conn)
            .await?;
            // (FIX-025) A start failure keeps its events on the attempt and in the history.
            if matches!(state.as_str(), "starting" | "running")
                && u.reason_code != Some("agent_stop_failed")
            {
                if !u.events.is_empty() {
                    sqlx::query("UPDATE sandboxes SET start_events = $2 WHERE id = $1")
                        .bind(sandbox)
                        .bind(events_json(u.events))
                        .execute(&mut *conn)
                        .await?;
                }
                record_start_failure(
                    conn,
                    sandbox,
                    Some(boot),
                    u.reason_code.unwrap_or("failed"),
                    u.message.unwrap_or(""),
                    &events_json(u.events),
                )
                .await?;
                // (FIX-029, C-612) The failure verdict enters the boot's start log.
                append_start_log(
                    conn,
                    sandbox,
                    "phase",
                    Some("failed"),
                    u.message.or(Some(u.reason_code.unwrap_or("failed"))),
                )
                .await?;
            }
            end_boot(conn, sandbox, boot, "failure", "lost").await?;
            let revoked = revoke_boot_credentials(conn, agent, Some(boot)).await?;
            Ok((Applied::Applied, revoked))
        }
        // `held` carries no lifecycle transition here.
        _ => Ok((Applied::Applied, vec![])),
    }
}

async fn end_boot(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
    reason: &str,
    process_status: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE runtime_intervals SET ended_at = now(), end_reason = $2, updated_at = now()
         WHERE boot_id = $1 AND ended_at IS NULL",
    )
    .bind(boot)
    .bind(reason)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE processes SET status = $3, ended_at = coalesce(ended_at, now()), updated_at = now()
         WHERE sandbox_id = $1 AND boot_id = $2 AND status = 'running'",
    )
    .bind(sandbox)
    .bind(boot)
    .bind(process_status)
    .execute(&mut *conn)
    .await?;
    // docs/10 §Processes and watches: armed watches of the ended boot convert likewise and
    // post a notice to the arming conversation.
    let converted: Vec<(Uuid, String)> = sqlx::query_as(
        "UPDATE watches w SET status = $3, updated_at = now()
         FROM processes p
         WHERE p.id = w.process_id AND p.sandbox_id = $1 AND p.boot_id = $2 AND w.status = 'armed'
         RETURNING w.conversation_id, p.name",
    )
    .bind(sandbox)
    .bind(boot)
    .bind(process_status)
    .fetch_all(&mut *conn)
    .await?;
    for (conversation, name) in converted {
        crate::repo::transcript::append_durable_event(
            conn,
            conversation,
            None,
            "notice",
            &serde_json::json!({
                "code": format!("watch_{process_status}"),
                "text": format!("The process \"{name}\" was {process_status}; its watch was cancelled."),
            }),
        )
        .await?;
    }
    Ok(())
}

/// `home.synced`: fenced on the channel holder.
pub async fn apply_home_synced(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
    replica: Uuid,
    generation: i64,
) -> Result<Applied, DbError> {
    let r = sqlx::query(
        "UPDATE sandboxes SET home_applied_generation = greatest(home_applied_generation, $4),
             updated_at = now()
         WHERE id=$1 AND boot_id=$2 AND channel_replica_id=$3",
    )
    .bind(sandbox)
    .bind(boot)
    .bind(replica)
    .bind(generation)
    .execute(conn)
    .await?;
    Ok(if r.rows_affected() > 0 {
        Applied::Applied
    } else {
        Applied::Fenced
    })
}

pub async fn apply_ports(
    conn: &mut PgConnection,
    sandbox: Uuid,
    boot: Uuid,
    replica: Uuid,
    ports: &[i32],
) -> Result<Applied, DbError> {
    let r = sqlx::query(
        "UPDATE sandboxes SET listening_ports = $4, updated_at = now()
         WHERE id=$1 AND boot_id=$2 AND channel_replica_id=$3",
    )
    .bind(sandbox)
    .bind(boot)
    .bind(replica)
    .bind(ports)
    .execute(conn)
    .await?;
    Ok(if r.rows_affected() > 0 {
        Applied::Applied
    } else {
        Applied::Fenced
    })
}

/// Channel claim: consume the bootstrap token. `Ok(None)` => unknown token. The claim is
/// idempotent while the boot has never held a channel (`channel_held_at IS NULL`, FIX-094,
/// SEC-25): a claim whose response was lost, or whose `channel/hold` then failed, is answered
/// again with the same `(sandbox, boot)`; once a channel was held the token is spent.
pub async fn claim_bootstrap(
    conn: &mut PgConnection,
    runner: Uuid,
    hash: &[u8],
) -> Result<Option<Result<(Uuid, Uuid), ()>>, DbError> {
    let won: Option<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE sandboxes SET bootstrap_used_at = coalesce(bootstrap_used_at, now()), updated_at = now()
         WHERE bootstrap_token_hash = $1 AND runner_id = $2
           AND (bootstrap_used_at IS NULL OR channel_held_at IS NULL)
           AND state IN ('starting','running') AND boot_id IS NOT NULL
         RETURNING id, boot_id",
    )
    .bind(hash)
    .bind(runner)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(w) = won {
        return Ok(Some(Ok(w)));
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sandboxes WHERE bootstrap_token_hash = $1 AND runner_id = $2)",
    )
    .bind(hash)
    .bind(runner)
    .fetch_one(conn)
    .await?;
    Ok(exists.then_some(Err(())))
}

/// `/channel/hold`: record the replica holding the supervisor socket.
pub async fn hold_channel(
    conn: &mut PgConnection,
    runner: Uuid,
    sandbox: Uuid,
    boot: Uuid,
    replica: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        // (FIX-025) The supervisor connected: the attempt moves on to `booting` until `ready`.
        "UPDATE sandboxes SET channel_replica_id = $4,
             channel_held_at = coalesce(channel_held_at, now()),
             start_phase = CASE WHEN start_phase = 'ready' THEN start_phase ELSE 'booting' END,
             updated_at = now()
         WHERE id=$1 AND boot_id=$2 AND runner_id=$3 AND state IN ('starting','running')",
    )
    .bind(sandbox)
    .bind(boot)
    .bind(runner)
    .bind(replica)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

pub async fn release_channel(
    conn: &mut PgConnection,
    runner: Uuid,
    sandbox: Uuid,
    boot: Uuid,
    replica: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE sandboxes SET channel_replica_id = NULL, updated_at = now()
         WHERE id=$1 AND boot_id=$2 AND runner_id=$3 AND channel_replica_id=$4",
    )
    .bind(sandbox)
    .bind(boot)
    .bind(runner)
    .bind(replica)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Assignment {
    pub sandbox_id: Uuid,
    pub agent_id: Uuid,
    pub boot_id: Option<Uuid>,
    pub desired: String,
    pub home_ref: Option<String>,
}

/// Reconcile: the control plane's expected set for a runner. `desired` is `running` while the
/// sandbox is starting/running, otherwise `stopped`.
pub async fn assignments(
    conn: &mut PgConnection,
    runner: Uuid,
) -> Result<Vec<Assignment>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id AS sandbox_id, agent_id, boot_id,
                CASE WHEN state IN ('starting','running') THEN 'running' ELSE 'stopped' END AS desired,
                home_ref
         FROM sandboxes WHERE runner_id = $1 ORDER BY id",
    )
    .bind(runner)
    .fetch_all(conn)
    .await?)
}

/// Which of `ids` are sandboxes of `runner` (FIX-094: another runner's ids are not "known" —
/// `sandboxes/check` decides what a runner may reap).
pub async fn known_ids(
    conn: &mut PgConnection,
    runner: Uuid,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM sandboxes WHERE id = ANY($1) AND runner_id = $2")
            .bind(ids)
            .bind(runner)
            .fetch_all(conn)
            .await?,
    )
}

/// Sandboxes of the runner that are occupying slots.
pub async fn occupied(conn: &mut PgConnection, runner: Uuid) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM sandboxes WHERE runner_id=$1 AND state IN ('starting','running','stopping')",
    )
    .bind(runner)
    .fetch_one(conn)
    .await?)
}

/// Recompute-once wait helper: does any sandbox of the runner have a stop/start in flight?
/// (kept for diagnostics and tests)
pub async fn lifecycle_in_flight(conn: &mut PgConnection, runner: Uuid) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM sandboxes WHERE runner_id=$1 AND state IN ('starting','stopping')",
    )
    .bind(runner)
    .fetch_one(conn)
    .await?)
}
