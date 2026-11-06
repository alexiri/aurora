//! Runner-plane operations other than the work engine: register, reports, channel, reconcile.

use super::work;
use crate::app::AppState;
use aurora_core::token;
use aurora_core::{ApiError, limits};
use aurora_db::bus::{self, Channel};
use aurora_db::repo::{
    credentials,
    runners::{self, RunnerRow},
    sandboxes,
};
use aurora_db::{DbError, repo::sandboxes::Applied};
use aurora_protocol::common::Validate;
use aurora_protocol::runner::*;
use http::StatusCode;
use uuid::Uuid;

pub async fn register(
    s: &AppState,
    runner: &RunnerRow,
    req: RegisterRequest,
) -> Result<RegisterResponse, ApiError> {
    req.validate()
        .map_err(|e| ApiError::invalid_request(e.to_string(), vec![]))?;
    let replica = Uuid::now_v7();
    let caps =
        serde_json::to_value(&req.capabilities).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    runners::register(
        &mut tx,
        runner.id,
        replica,
        &req.advertised_host,
        &req.version,
        &req.backend,
        &caps,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(RegisterResponse {
        runner_id: runner.id,
        replica_id: replica,
        protocol: 1,
        config: RunnerConfig {
            poll_wait_secs: MAX_WAIT_SECS,
            reconcile_interval_secs: limits::RECONCILE_INTERVAL.as_secs() as u32,
            orphan_grace_secs: limits::ORPHAN_GRACE.as_secs() as u32,
            idle_stop_secs: s.config.hosted.idle_stop_secs as u32,
        },
    })
}

pub async fn retire(s: &AppState, runner: &RunnerRow, replica: Uuid) -> Result<(), ApiError> {
    runners::retire_replica(&s.pool, runner.id, replica).await?;
    // Wake other replicas so ownership takeover happens promptly.
    notify_work(s, "retire").await;
    Ok(())
}

pub async fn notify_work(s: &AppState, reason: &str) {
    if let Err(e) = bus::notify(
        &s.pool,
        Channel::Work,
        &serde_json::json!({"reason": reason}),
    )
    .await
    {
        tracing::warn!(error = %e, "work notify failed");
    }
}

// Report idempotency is by construction in the database (decision I-25): lifecycle reports are
// boot/owner-fenced conditional transitions whose replays are no-ops (`duplicate`/`fenced`), and
// `home.synced`/`ports.state` are monotonic/overwriting updates fenced on the channel holder.

fn result(report_id: Uuid, status: ReportStatus, detail: Option<&str>) -> ReportResult {
    ReportResult {
        report_id,
        status,
        detail: detail.map(str::to_owned),
    }
}

pub async fn reports(
    s: &AppState,
    runner: &RunnerRow,
    replica: Uuid,
    req: ReportsRequest,
) -> Result<ReportsResponse, ApiError> {
    req.validate()
        .map_err(|e| ApiError::invalid_request(e.to_string(), vec![]))?;
    let mut results = Vec::with_capacity(req.reports.len());
    for r in &req.reports {
        let res = match apply(s, runner, replica, r).await {
            Ok(res) => res,
            Err(e) => {
                tracing::warn!(error = %e.code(), "report apply failed");
                return Err(e);
            }
        };
        results.push(res);
    }
    Ok(ReportsResponse { results })
}

fn from_applied(id: Uuid, a: Applied) -> ReportResult {
    let st = match a {
        Applied::Applied => ReportStatus::Applied,
        Applied::Duplicate => ReportStatus::Duplicate,
        Applied::Fenced => ReportStatus::Fenced,
    };
    result(id, st, None)
}

async fn apply(
    s: &AppState,
    runner: &RunnerRow,
    replica: Uuid,
    r: &Report,
) -> Result<ReportResult, ApiError> {
    // The sandbox must be placed on the reporting runner.
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    match sandboxes::get(&mut tx, r.sandbox_id).await? {
        Some(row) if row.runner_id == Some(runner.id) => {}
        _ => {
            return Ok(result(
                r.report_id,
                ReportStatus::Fenced,
                Some("unknown_sandbox"),
            ));
        }
    }
    let out = match &r.body {
        ReportBody::SandboxStatus(st) => {
            let upd = sandboxes::StatusUpdate {
                state: st.state.as_str(),
                container_ref: st.container_ref.as_deref(),
                reason_code: st.reason_code.as_ref().map(|c| c.as_str()),
                message: st.message.as_deref(),
            };
            let (a, revoked) =
                sandboxes::apply_status(&mut tx, r.sandbox_id, r.boot_id, replica, &upd).await?;
            for id in revoked {
                credentials::notify_gateway_invalidate(&mut *tx, "credential", id).await?;
            }
            if a == Applied::Applied && matches!(upd.state, "stopped" | "failed") {
                bus::notify(
                    &mut *tx,
                    Channel::Work,
                    &serde_json::json!({"reason": "slot_free"}),
                )
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
            }
            from_applied(r.report_id, a)
        }
        // Full home sync scheduling belongs to BE-051 (arm 5); nothing to schedule yet.
        ReportBody::SupervisorReady(_) => result(r.report_id, ReportStatus::Applied, None),
        ReportBody::HomeSynced(h) => from_applied(
            r.report_id,
            sandboxes::apply_home_synced(
                &mut tx,
                r.sandbox_id,
                r.boot_id,
                replica,
                i64::try_from(h.generation).unwrap_or(i64::MAX),
            )
            .await?,
        ),
        ReportBody::PortsState(p) => {
            let ports: Vec<i32> = p.ports.iter().map(|p| i32::from(*p)).collect();
            from_applied(
                r.report_id,
                sandboxes::apply_ports(&mut tx, r.sandbox_id, r.boot_id, replica, &ports).await?,
            )
        }
        ReportBody::Other(u) => {
            tracing::warn!(kind = %u.kind.as_str(), "unknown report kind skipped");
            result(r.report_id, ReportStatus::Invalid, Some("unknown_kind"))
        }
        // Turn/process/watch reports are applied by BE-051.
        _ => result(r.report_id, ReportStatus::Invalid, Some("not_implemented")),
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(out)
}

pub async fn channel_claim(
    s: &AppState,
    runner: &RunnerRow,
    req: ChannelClaimRequest,
) -> Result<ChannelClaimResponse, ApiError> {
    let hash = token::hash(req.bootstrap_token.expose());
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    match sandboxes::claim_bootstrap(&mut conn, runner.id, &hash).await? {
        Some(Ok((sandbox_id, boot_id))) => Ok(ChannelClaimResponse {
            sandbox_id,
            boot_id,
        }),
        Some(Err(())) => Err(ApiError::conflict(
            "bootstrap_spent",
            "The bootstrap token was already used.",
        )),
        None => Err(ApiError::not_found("Unknown bootstrap token.")),
    }
}

pub async fn channel_hold(
    s: &AppState,
    runner: &RunnerRow,
    replica: Uuid,
    r: ChannelRef,
) -> Result<(), ApiError> {
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    if sandboxes::hold_channel(&mut conn, runner.id, r.sandbox_id, r.boot_id, replica).await? {
        Ok(())
    } else {
        Err(ApiError::conflict("fenced", "Sandbox boot is not current."))
    }
}

pub async fn channel_release(
    s: &AppState,
    runner: &RunnerRow,
    replica: Uuid,
    r: ChannelRef,
) -> Result<(), ApiError> {
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    sandboxes::release_channel(&mut conn, runner.id, r.sandbox_id, r.boot_id, replica).await?;
    Ok(())
}

pub async fn sandboxes_list(
    s: &AppState,
    runner: &RunnerRow,
) -> Result<SandboxesResponse, ApiError> {
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let rows = sandboxes::assignments(&mut conn, runner.id).await?;
    Ok(SandboxesResponse {
        sandboxes: rows
            .into_iter()
            .map(|a| SandboxAssignment {
                sandbox_id: a.sandbox_id,
                agent_id: a.agent_id,
                boot_id: a.boot_id,
                desired: DesiredState::from_wire(&a.desired).unwrap_or(DesiredState::Stopped),
                home_ref: a.home_ref.unwrap_or_default(),
            })
            .collect(),
    })
}

pub async fn sandboxes_check(
    s: &AppState,
    req: SandboxesCheckRequest,
) -> Result<SandboxesCheckResponse, ApiError> {
    if req.sandbox_ids.len() > 10_000 {
        return Err(ApiError::invalid_request("Too many sandbox ids.", vec![]));
    }
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    Ok(SandboxesCheckResponse {
        known: sandboxes::known_ids(&mut conn, &req.sandbox_ids).await?,
    })
}

/// Strays are logged for operators (failing/recording them is a follow-up).
pub fn strays(runner: &RunnerRow, req: &StraysRequest) {
    for o in &req.objects {
        tracing::warn!(runner = %runner.id, sandbox = ?o.sandbox_id, boot = ?o.boot_id,
            container = %o.container_ref, "stray container reported");
    }
}

pub fn not_implemented() -> ApiError {
    ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "not_implemented",
        "This runner-plane call is not implemented yet.",
    )
}

pub use work::poll;
