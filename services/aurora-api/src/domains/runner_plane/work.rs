//! The due-work engine for runners: arms 1-3 (docs/05 §The due-work engine).
//! No scheduler loop: the `POST /work` long-poll computes and claims work.

use super::boot::{self, BootInput};
use crate::app::AppState;
use crate::domains::hosted::dispatch;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, limits};
use aurora_db::DbError;
use aurora_db::bus::{Channel, Listener};
use aurora_db::repo::{credentials, runners::RunnerRow, sandboxes};
use aurora_protocol::runner::{
    Capacity, RunnerCapabilities, SandboxStart, SandboxStop, StopReason, WorkItem, WorkItemBody,
    WorkResponse,
};
use std::collections::BTreeMap;
use std::time::Duration;
use uuid::Uuid;

/// Lease on a claimed start (provision + boot) before it is re-driven.
pub const START_LEASE_SECS: f64 = 300.0;
/// Lease on a claimed stop before it is re-driven.
pub const STOP_LEASE_SECS: f64 = 120.0;
/// Grace given to the supervisor to flush before the container is stopped.
pub const STOP_GRACE_SECS: u32 = 10;
/// Max items per arm per poll (bounded arms).
const ARM_LIMIT: i64 = 16;

fn online_secs() -> f64 {
    limits::REPLICA_ONLINE_THRESHOLD.as_secs_f64()
}

/// Long-poll: compute, and if nothing is due wait for `aurora_work` / `wait_secs`, recompute once.
pub async fn poll(
    s: &AppState,
    runner: &RunnerRow,
    replica: Uuid,
    wait_secs: u32,
    capacity: Capacity,
) -> Result<WorkResponse, ApiError> {
    // Listen before the first computation so no notification between compute and wait is lost.
    let mut listener = if wait_secs > 0 {
        let mut l = Listener::connect(&s.pool, &[Channel::Work])
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let _ = l.next().await; // initial Resync
        Some(l)
    } else {
        None
    };
    let mut items = compute(s, runner, replica, capacity).await?;
    if items.is_empty()
        && let Some(l) = listener.as_mut()
    {
        let wait = Duration::from_secs(u64::from(wait_secs));
        // Any notification (or listener resync) wakes the poll; the timeout ends it.
        let _ = tokio::time::timeout(wait, l.next()).await;
        // Recompute once, whether woken or timed out (timestamps are the truth).
        items = compute(s, runner, replica, capacity).await?;
    }
    let reconciler =
        aurora_db::repo::runners::is_oldest_live(&s.pool, runner.id, replica, online_secs())
            .await?;
    Ok(WorkResponse { items, reconciler })
}

pub(crate) fn caps_of(runner: &RunnerRow) -> Option<RunnerCapabilities> {
    runner
        .capabilities
        .as_ref()
        .and_then(|c| serde_json::from_value(c.clone()).ok())
}

fn stop_item(c: sandboxes::StopClaim) -> Option<WorkItem> {
    let boot_id = c.boot_id?;
    let reason = StopReason::from_wire(&c.reason).ok()?;
    Some(WorkItem {
        item_id: Uuid::now_v7(),
        sandbox_id: c.sandbox_id,
        boot_id,
        body: WorkItemBody::SandboxStop(SandboxStop {
            reason,
            grace_secs: STOP_GRACE_SECS,
        }),
    })
}

/// Held-awake ceiling for eviction (docs/11 §Stop).
fn held_awake_ceiling(configured: Option<u32>, max_slots: i64) -> i64 {
    configured.map_or_else(|| (max_slots - 1).max(1), i64::from)
}

/// One evaluation of arms 1-3 in a single transaction.
async fn compute(
    s: &AppState,
    runner: &RunnerRow,
    replica: Uuid,
    capacity: Capacity,
) -> Result<Vec<WorkItem>, ApiError> {
    let caps = caps_of(runner);
    let max_slots = i64::from(if capacity.max > 0 {
        capacity.max
    } else {
        caps.as_ref().map_or(1, |c| c.max_sandboxes)
    });
    // The runner's registered `runner.held_awake_ceiling`, else `max(1, max_sandboxes − 1)`.
    let ceiling = held_awake_ceiling(caps.as_ref().and_then(|c| c.held_awake_ceiling), max_slots);
    let hosted = &s.config.hosted;
    let default_harness = hosted.default_harness.as_str();

    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let mut after = dispatch::After::default();
    // Arm 1: ownership takeover (the heartbeat itself was stamped by the extractor).
    sandboxes::take_over(&mut tx, runner.id, replica, online_secs()).await?;

    // Arm 2: stop.
    let mut stops = sandboxes::claim_stops(
        &mut tx,
        runner.id,
        replica,
        hosted.idle_stop_secs as f64,
        hosted.background_cap_secs as f64,
        STOP_LEASE_SECS,
        ARM_LIMIT,
    )
    .await?;
    sandboxes::ensure_rows(&mut tx).await?;
    if let Some(ev) = sandboxes::claim_evict(
        &mut tx,
        runner.id,
        replica,
        ceiling,
        default_harness,
        STOP_LEASE_SECS,
    )
    .await?
    {
        stops.push(ev);
    }
    let mut items: Vec<WorkItem> = stops.into_iter().filter_map(stop_item).collect();

    // `no_model_key` fails queued turns before any sandbox start is claimed for them.
    dispatch::sweep_no_model_key(s, &mut tx, &mut after).await?;

    // Arm 3: start (capacity decisions serialised per runner).
    sandboxes::lock_runner(&mut tx, runner.id).await?;
    let occupied = sandboxes::occupied(&mut tx, runner.id).await?;
    let free = (max_slots - occupied.max(i64::from(capacity.running))).max(0);
    if free > 0 {
        let claims = sandboxes::claim_starts(
            &mut tx,
            runner.id,
            replica,
            default_harness,
            free.min(ARM_LIMIT),
            START_LEASE_SECS,
        )
        .await?;
        if !claims.is_empty() {
            let ca_pem = boot::read_ca(s).await?;
            let host = aurora_db::repo::runners::replica_host(&mut *tx, replica)
                .await?
                .unwrap_or_default();
            let mut revoked = Vec::new();
            for c in claims {
                let (item, r) =
                    mint_start(s, &mut tx, runner, caps.as_ref(), &host, &ca_pem, &c).await?;
                revoked.extend(r);
                items.push(item);
            }
            for id in revoked {
                credentials::notify_gateway_invalidate(&mut *tx, "credential", id).await?;
            }
        }
    }
    // Arms 4-8 (hosted engine): abort/send-now, home sync, dispatch, steer, stale recovery.
    dispatch::arms(s, &mut tx, runner.id, replica, &mut items, &mut after).await?;
    // Arm 9 (schedules): claim due schedules and create their turns.
    crate::domains::schedules::fire::arm9(s, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    for (conv, turn) in after.flush {
        crate::domains::hosted::lifecycle::flush_deltas(s, conv, turn).await;
    }
    Ok(items)
}

async fn mint_start(
    s: &AppState,
    tx: &mut sqlx::PgConnection,
    runner: &RunnerRow,
    caps: Option<&RunnerCapabilities>,
    host: &str,
    ca_pem: &str,
    c: &sandboxes::StartClaim,
) -> Result<(WorkItem, Vec<Uuid>), ApiError> {
    let agent = sandboxes::start_agent(tx, c.sandbox_id)
        .await?
        .ok_or_else(|| ApiError::internal("sandbox agent vanished"))?;
    let boot_id = Uuid::now_v7();
    let boot_tok = token::generate(TokenPrefix::Boot);
    let bootstrap = token::generate(TokenPrefix::Supervisor);
    let (resources, home_gib) = boot::profile(caps, agent.resource_overrides.as_ref());
    let home_ref = agent
        .home_ref
        .clone()
        .unwrap_or_else(|| format!("home-{}", c.sandbox_id));
    let harness = agent
        .harness
        .clone()
        .unwrap_or_else(|| s.config.hosted.default_harness.clone());
    let agent_row = aurora_db::repo::agents::get(&mut *tx, c.agent_id)
        .await?
        .ok_or_else(|| ApiError::internal("sandbox agent vanished"))?;
    let app_boot = boot::app_boot_for(s, &agent_row).await?;
    let profile = serde_json::json!({
        "cpu_millis": resources.cpu_millis, "memory_mib": resources.memory_mib,
        "pids": resources.pids, "home_gib": home_gib,
        "app_inputs_hash": app_boot.hash,
    });
    let revoked = sandboxes::begin_boot(
        tx,
        c.sandbox_id,
        c.agent_id,
        boot_id,
        &token::hash(bootstrap.expose()),
        &home_ref,
        &profile,
        &token::hash(boot_tok.expose()),
        &token::display_prefix(boot_tok.expose()),
    )
    .await?;
    let secret = boot::boot_secret(
        s,
        &BootInput {
            sandbox_id: c.sandbox_id,
            boot_id,
            agent_id: c.agent_id,
            handle: &agent.handle,
            name: &agent.name,
            harness: &harness,
            replica_host: host,
            bootstrap_token: bootstrap.expose(),
            boot_token: boot_tok.expose(),
            ca_pem,
            files: &app_boot.files,
        },
    )?;
    let labels = BTreeMap::from([
        (
            "aurora.installation".to_owned(),
            runner.installation_fingerprint.clone(),
        ),
        ("aurora.runner".to_owned(), runner.id.to_string()),
        ("aurora.sandbox".to_owned(), c.sandbox_id.to_string()),
        ("aurora.boot".to_owned(), boot_id.to_string()),
    ]);
    let image = caps
        .and_then(|c| c.image.clone())
        .unwrap_or_else(boot::default_image);
    let spec = boot::spec(
        image,
        app_boot.env,
        secret,
        home_ref,
        home_gib,
        resources,
        labels,
    );
    Ok((
        WorkItem {
            item_id: Uuid::now_v7(),
            sandbox_id: c.sandbox_id,
            boot_id,
            body: WorkItemBody::SandboxStart(SandboxStart { spec }),
        },
        revoked,
    ))
}

#[cfg(test)]
mod tests {
    use super::held_awake_ceiling;

    #[test]
    fn ceiling_is_derived_unless_configured() {
        assert_eq!(held_awake_ceiling(None, 4), 3);
        assert_eq!(held_awake_ceiling(None, 1), 1);
        assert_eq!(held_awake_ceiling(Some(2), 4), 2);
        assert_eq!(held_awake_ceiling(Some(7), 1), 7);
    }
}
