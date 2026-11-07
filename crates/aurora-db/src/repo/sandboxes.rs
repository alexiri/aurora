//! Sandbox lifecycle persistence and the due-work claims for arms 1-3
//! (docs/05 §The due-work engine, docs/11 §Lifecycle).
//!
//! Claims follow the `aurora_db::claim` pattern (`UPDATE … WHERE id IN (… FOR UPDATE SKIP
//! LOCKED)` with a lease in `lifecycle_lease_until`) but also set several columns, so they are
//! spelled out here. No network call happens inside a claim.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
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
                    OR coalesce(s.last_engaged_at, '-infinity'::timestamptz) > s.updated_at)))
         AND ({ACTIVE_TURN} OR {DUE_SCHEDULE})
         AND coalesce((SELECT capabilities->'harnesses' FROM runners WHERE id = $1) ? coalesce(a.harness, $3), false)"
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
pub async fn claim_stops(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    idle_secs: f64,
    cap_secs: f64,
    lease_secs: f64,
    limit: i64,
) -> Result<Vec<StopClaim>, DbError> {
    let sql = format!(
        "WITH cand AS (
           SELECT s.id,
             CASE
               WHEN s.state = 'stopping' THEN coalesce(s.stop_reason, 'idle')
               WHEN s.restart_requested_at IS NOT NULL THEN 'restart'
               WHEN s.stop_requested_at IS NOT NULL THEN coalesce(s.stop_reason, 'idle')
               WHEN {ACTIVE_TURN} THEN NULL
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
         RETURNING s.id AS sandbox_id, s.boot_id, p.reason"
    );
    Ok(sqlx::query_as(&sql)
        .bind(runner)
        .bind(replica)
        .bind(idle_secs)
        .bind(cap_secs)
        .bind(limit)
        .bind(lease_secs)
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
) -> Result<Option<StopClaim>, DbError> {
    let held: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM sandboxes s WHERE s.runner_id = $1 AND s.state = 'running'
         AND NOT {ACTIVE_TURN}"
    ))
    .bind(runner)
    .fetch_one(&mut *conn)
    .await?;
    if held <= ceiling || !pending_start(conn, runner, replica, default_harness).await? {
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
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM sandboxes s JOIN agents a ON a.id = s.agent_id
                        WHERE {})",
        start_eligible()
    ))
    .bind(runner)
    .bind(replica)
    .bind(default_harness)
    .fetch_one(conn)
    .await?)
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
) -> Result<Vec<StartClaim>, DbError> {
    let sql = format!(
        "WITH cand AS (SELECT s.id FROM sandboxes s JOIN agents a ON a.id = s.agent_id
             WHERE {}
             ORDER BY coalesce(s.last_engaged_at, s.created_at), s.id
             LIMIT $4 FOR UPDATE OF s SKIP LOCKED)
         UPDATE sandboxes s SET state = 'starting', runner_id = $1, owner_replica_id = $2,
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
             channel_replica_id = NULL, listening_ports = '{}', home_applied_generation = 0,
             stop_reason = NULL, stop_requested_at = NULL, restart_requested_at = NULL,
             failure_code = NULL, failure_message = NULL, container_ref = NULL, home_ref = $4,
             resource_profile = $5::jsonb,
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
    /// `running`, `stopped`, `failed` or `held`.
    pub state: &'a str,
    pub container_ref: Option<&'a str>,
    pub reason_code: Option<&'a str>,
    pub message: Option<&'a str>,
}

/// Failure backoff after the n-th consecutive start failure: 30 s, 2 min, 10 min.
pub fn backoff_secs(failures: i32) -> f64 {
    match failures {
        ..=1 => 30.0,
        2 => 120.0,
        _ => 600.0,
    }
}

/// `(state, boot, owner, agent, stop_reason, start_failures)`.
type StatusRow = (
    String,
    Option<Uuid>,
    Option<Uuid>,
    Uuid,
    Option<String>,
    i32,
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
                    s.start_failures
             FROM sandboxes s WHERE s.id = $1 FOR UPDATE",
    )
    .bind(sandbox)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((state, cur_boot, owner, agent, stop_reason, failures)) = row else {
        return Ok((Applied::Fenced, vec![]));
    };
    if cur_boot != Some(boot) || owner != Some(replica) {
        return Ok((Applied::Fenced, vec![]));
    }
    match u.state {
        "running" => match state.as_str() {
            "running" => Ok((Applied::Duplicate, vec![])),
            "starting" => {
                sqlx::query(
                    "UPDATE sandboxes SET state='running', container_ref=$2, lifecycle_lease_until=NULL,
                         last_active_at=now(),
                         start_failures=0,
                         updated_at=now() WHERE id=$1",
                )
                .bind(sandbox)
                .bind(u.container_ref)
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
            sqlx::query(
                "UPDATE sandboxes SET state='stopped', lifecycle_lease_until=NULL, channel_replica_id=NULL,
                     listening_ports='{}', container_ref=NULL, stop_requested_at=NULL,
                     restart_requested_at=NULL, updated_at=now() WHERE id=$1",
            )
            .bind(sandbox)
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
            let n = failures + 1;
            sqlx::query(
                "UPDATE sandboxes SET state='failed', failure_code=$2, failure_message=$3,
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
            .execute(&mut *conn)
            .await?;
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

/// Channel claim: consume the single-use bootstrap token. `Ok(None)` => unknown token.
pub async fn claim_bootstrap(
    conn: &mut PgConnection,
    runner: Uuid,
    hash: &[u8],
) -> Result<Option<Result<(Uuid, Uuid), ()>>, DbError> {
    let won: Option<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE sandboxes SET bootstrap_used_at = now(), updated_at = now()
         WHERE bootstrap_token_hash = $1 AND runner_id = $2 AND bootstrap_used_at IS NULL
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
        "UPDATE sandboxes SET channel_replica_id = $4, updated_at = now()
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

pub async fn known_ids(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM sandboxes WHERE id = ANY($1)")
            .bind(ids)
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
