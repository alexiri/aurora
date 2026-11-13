//! (FIX-025, D-158, C-582, C-583) Sandbox readiness deadline: a turn never waits forever for a
//! sandbox. Runs on every API replica every `SANDBOX_READINESS_SWEEP` (singleton per run), so it
//! works with no runner connected at all (the due-work arms only run inside a runner's poll).
//!
//! 1. Supervisor hello: a sandbox the runner reported `running` whose supervisor did not connect
//!    within `RUNNER_SUPERVISOR_HELLO` fails with `supervisor_unreachable` and is stopped.
//! 2. Per turn: a queued hosted turn still without a ready sandbox `hosted.sandbox_ready_timeout_secs`
//!    after it was queued fails with the most specific reason; a start attempt still in flight is
//!    given up and stopped. Follow-ups are promoted as for any finished turn (BE-051) and the next
//!    message retries the start (after the failure backoff).
//!
//! Lock order follows `runner_plane::service::apply`: sandbox row first, then conversation locks.

use crate::app::AppState;
use crate::domains::hosted::lifecycle;
use crate::domains::runner_plane::service as plane;
use aurora_core::{ApiError, limits, sanitize};
use aurora_db::DbError;
use aurora_db::repo::conversations as convs;
use aurora_db::repo::hosted::{self, OverdueTurn};
use aurora_db::repo::sandboxes;
use chrono::{DateTime, Utc};
use uuid::Uuid;

const SWEEP_JOB: &str = "sandbox_readiness";
const LIMIT: i64 = 32;
/// Bound of a reason shown in the chat.
const REASON_MAX: usize = 1000;

/// What the sweep did (for logs and tests).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Swept {
    pub unreachable: usize,
    pub turns_failed: usize,
}

/// The `supervisor_unreachable` reason; `detail` is the runner's `running` message
/// ("waiting for the supervisor to connect to the runner at <endpoint>").
pub fn unreachable_message(detail: Option<&str>) -> String {
    let at = detail
        .and_then(|d| {
            d.split_once("runner at ")
                .map(|(_, ep)| ep.trim().to_owned())
        })
        .filter(|ep| !ep.is_empty())
        .unwrap_or_else(|| {
            format!(
                "its supervisor port :{}",
                crate::domains::runner_plane::boot::SUPERVISOR_PORT
            )
        });
    format!(
        "The agent's computer started, but its supervisor could not reach the runner at {at} within {} s. \
         Check NetworkPolicy/CNI egress from the sandboxes namespace to the runner on port {} \
         (Cilium: networkPolicy.cilium.enabled).",
        limits::RUNNER_SUPERVISOR_HELLO.as_secs(),
        crate::domains::runner_plane::boot::SUPERVISOR_PORT,
    )
}

fn utc(t: DateTime<Utc>) -> String {
    t.format("%H:%M:%S UTC").to_string()
}

fn phase_text(phase: Option<&str>, detail: Option<&str>) -> String {
    let p = phase.unwrap_or("claimed");
    match detail.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => format!("{p}: {d}"),
        None => p.to_owned(),
    }
}

/// What happens to a turn past its deadline: `(code, message, give_up_attempt)`.
pub fn classify(
    t: &OverdueTurn,
    ready_secs: u64,
    now: DateTime<Utc>,
) -> Option<(String, String, bool)> {
    let state = t.state.as_deref().unwrap_or("unprovisioned");
    let runner_gone = |name: &Option<String>| {
        match name {
        Some(n) => format!(
            "The runner `{n}` that hosts the agent's computer is not connected, so the computer cannot start. \
             An instance admin can check Admin › Runners."
        ),
        None => "No runner is connected, so the agent's computer cannot start. An instance admin can check Admin › Runners."
            .to_owned(),
    }
    };
    let r = match state {
        // The sandbox is ready: dispatch takes the turn; nothing to explain.
        "running" if t.channel_held && t.harness_ready && t.home_synced => return None,
        // (FIX-058) No live runner holds the attempt: give it up terminally (`fail_attempt` with
        // `runner_gone` lands the sandbox in `failed`, never `stopping`, which nothing would drive).
        _ if !t.any_runner_live => ("runner_unavailable".to_owned(), runner_gone(&None), true),
        "starting" | "running" | "stopping"
            if t.sandbox_id.is_some() && t.runner_name.is_some() && !t.runner_live =>
        {
            (
                "runner_unavailable".to_owned(),
                runner_gone(&t.runner_name),
                true,
            )
        }
        "starting" => (
            "sandbox_start_failed".to_owned(),
            format!(
                "The agent's computer did not start within {ready_secs} s (last phase: {}).",
                phase_text(t.start_phase.as_deref(), t.start_detail.as_deref())
            ),
            true,
        ),
        "running" if !t.bootstrap_used => (
            "supervisor_unreachable".to_owned(),
            unreachable_message(t.start_detail.as_deref()),
            true,
        ),
        "running" => {
            let what = if !t.channel_held {
                "it connected and then disconnected"
            } else if !t.harness_ready {
                "it never reported ready"
            } else {
                "the home sync was not applied"
            };
            (
                "supervisor_boot_failed".to_owned(),
                format!(
                    "The agent's supervisor connected but did not finish booting within {ready_secs} s ({what}). \
                     The sandbox logs show the supervisor's error."
                ),
                true,
            )
        }
        "failed" => {
            let code = t
                .failure_code
                .clone()
                .filter(|c| plane::START_FAILURE_CODES.contains(&c.as_str()))
                .unwrap_or_else(|| "sandbox_start_failed".to_owned());
            let last = t
                .failure_message
                .clone()
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| "The last start of the agent's computer failed.".to_owned());
            let retry = match t.lease_until.filter(|l| *l > now) {
                Some(l) if t.start_failures >= 3 => format!(
                    " Automatic retries stopped after {} failed starts; a message sent after {} retries the start.",
                    t.start_failures,
                    utc(l)
                ),
                Some(l) => format!(
                    " The next start attempt is allowed after {} (backoff after {} failed start{}).",
                    utc(l),
                    t.start_failures,
                    if t.start_failures == 1 { "" } else { "s" }
                ),
                None if t.start_failures >= 3 => format!(
                    " Automatic retries stopped after {} failed starts; send a new message to try again.",
                    t.start_failures
                ),
                None => " A new start attempt is pending.".to_owned(),
            };
            (code, format!("{last}{retry}"), false)
        }
        "stopping" => (
            "sandbox_start_failed".to_owned(),
            format!(
                "The agent's computer was still stopping after {ready_secs} s; send the message again in a moment."
            ),
            false,
        ),
        // unprovisioned / stopped with a live runner: never claimed, so no slot was free.
        _ => (
            "at_capacity".to_owned(),
            format!(
                "The agent's computer did not start within {ready_secs} s: no sandbox slot became free on the runner."
            ),
            false,
        ),
    };
    Some((r.0, sanitize::reason(&r.1, REASON_MAX), r.2))
}

/// Tells the gateway the boot credentials revoked by a terminal give-up are gone.
async fn invalidate(tx: &mut sqlx::PgConnection, revoked: Vec<Uuid>) -> Result<(), ApiError> {
    for id in revoked {
        aurora_db::repo::credentials::notify_gateway_invalidate(&mut *tx, "credential", id).await?;
    }
    Ok(())
}

/// One sweep (singleton across API replicas). `Ok(None)` when another replica holds the lock.
pub async fn sweep(s: &AppState) -> Result<Option<Swept>, ApiError> {
    let ran = aurora_db::jobs::try_singleton(&s.pool, SWEEP_JOB, || sweep_once(s)).await?;
    ran.transpose()
}

/// One pass without the singleton lock (tests call this directly).
pub async fn sweep_once(s: &AppState) -> Result<Swept, ApiError> {
    let mut out = Swept::default();
    let mut woke = false;

    // 1. Supervisor never connected.
    let overdue = {
        let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
        sandboxes::hello_overdue(
            &mut conn,
            limits::RUNNER_SUPERVISOR_HELLO.as_secs_f64(),
            LIMIT,
        )
        .await?
    };
    for (sandbox, boot, detail) in overdue {
        let msg = sanitize::reason(&unreachable_message(detail.as_deref()), REASON_MAX);
        let mut tx = s.pool.begin().await.map_err(DbError::from)?;
        // (FIX-071) Fenced on "supervisor still not connected" as well as on "not ready".
        if let Some(revoked) = sandboxes::fail_attempt_unconnected(
            &mut tx,
            sandbox,
            boot,
            "supervisor_unreachable",
            &msg,
        )
        .await?
        {
            invalidate(&mut tx, revoked).await?;
            plane::fail_start(&mut tx, sandbox, Some("supervisor_unreachable"), Some(&msg)).await?;
            out.unreachable += 1;
            woke = true;
        }
        tx.commit().await.map_err(DbError::from)?;
    }

    // 2. Turns past the readiness deadline.
    let ready_secs = s.config.hosted.sandbox_ready_timeout_secs;
    let turns = {
        let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
        hosted::overdue_turns(
            &mut conn,
            ready_secs as f64,
            limits::REPLICA_ONLINE_THRESHOLD.as_secs_f64(),
            LIMIT,
        )
        .await?
    };
    let now = Utc::now();
    let mut given_up: Vec<Uuid> = Vec::new();
    for t in turns {
        let Some((code, message, give_up)) = classify(&t, ready_secs, now) else {
            continue;
        };
        let mut tx = s.pool.begin().await.map_err(DbError::from)?;
        if give_up
            && let (Some(sb), Some(boot)) = (t.sandbox_id, t.boot_id)
            && !given_up.contains(&sb)
        {
            given_up.push(sb);
            if let Some(revoked) = sandboxes::fail_attempt(
                &mut tx,
                sb,
                boot,
                &code,
                &message,
                code == "runner_unavailable",
            )
            .await?
            {
                invalidate(&mut tx, revoked).await?;
                // Every queued turn of the agent fails with the same reason (10 §Failure semantics).
                plane::fail_start(&mut tx, sb, Some(&code), Some(&message)).await?;
                woke = true;
            }
        }
        convs::lock(&mut tx, t.conversation_id).await?;
        // A no-op when `fail_start` already ended it (fenced).
        let failed = lifecycle::fail(&mut tx, t.turn_id, t.conversation_id, &code, &message)
            .await?
            .is_some();
        tx.commit().await.map_err(DbError::from)?;
        lifecycle::flush_deltas(s, t.conversation_id, t.turn_id).await;
        if failed || give_up {
            out.turns_failed += 1;
        }
        tracing::warn!(turn = %t.turn_id, agent = %t.agent_id, code = %code, reason = %message,
            "turn failed: sandbox not ready before the deadline");
    }
    if woke {
        plane::notify_work(s, "readiness").await;
    }
    Ok(out)
}

/// Run [`sweep`] every `SANDBOX_READINESS_SWEEP` on this replica.
pub fn spawn(s: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(limits::SANDBOX_READINESS_SWEEP);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match sweep(&s).await {
                Ok(Some(r)) if r != Swept::default() => {
                    tracing::info!(?r, "sandbox readiness sweep")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e.message(), "sandbox readiness sweep failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: Option<&str>) -> OverdueTurn {
        OverdueTurn {
            turn_id: Uuid::nil(),
            conversation_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            sandbox_id: state.map(|_| Uuid::nil()),
            state: state.map(str::to_owned),
            boot_id: state.map(|_| Uuid::nil()),
            runner_name: state.map(|_| "r1".to_owned()),
            runner_live: true,
            any_runner_live: true,
            bootstrap_used: false,
            channel_held: false,
            harness_ready: false,
            home_synced: false,
            start_phase: None,
            start_detail: None,
            failure_code: None,
            failure_message: None,
            lease_until: None,
            start_failures: 0,
            attempt_secs: None,
        }
    }

    #[test]
    fn every_waiting_state_has_a_specific_reason() {
        let now = Utc::now();
        let code = |r: &OverdueTurn| classify(r, 300, now).map(|c| (c.0, c.2));
        let mut r = row(None);
        r.any_runner_live = false;
        assert_eq!(code(&r), Some(("runner_unavailable".into(), true)));
        let mut r = row(Some("starting"));
        r.runner_live = false;
        assert_eq!(code(&r), Some(("runner_unavailable".into(), true)));
        let mut r = row(Some("starting"));
        r.start_phase = Some("creating".into());
        r.start_detail = Some("ContainerCreating".into());
        let c = classify(&r, 300, now).unwrap();
        assert_eq!((c.0.as_str(), c.2), ("sandbox_start_failed", true));
        assert!(
            c.1.contains("300 s") && c.1.contains("creating: ContainerCreating"),
            "{}",
            c.1
        );
        let mut r = row(Some("running"));
        r.start_detail = Some("waiting for the supervisor to connect to the runner at aurora-runner.aurora (10.0.0.9):4105".into());
        let c = classify(&r, 300, now).unwrap();
        assert_eq!(c.0, "supervisor_unreachable");
        assert!(
            c.1.contains("aurora-runner.aurora (10.0.0.9):4105")
                && c.1.contains("networkPolicy.cilium.enabled"),
            "{}",
            c.1
        );
        let mut r = row(Some("running"));
        r.bootstrap_used = true;
        r.channel_held = true;
        assert_eq!(code(&r), Some(("supervisor_boot_failed".into(), true)));
        r.harness_ready = true;
        r.home_synced = true;
        assert_eq!(code(&r), None, "ready: dispatch takes it");
        assert_eq!(
            code(&row(Some("stopped"))),
            Some(("at_capacity".into(), false))
        );
        assert_eq!(
            code(&row(Some("stopping"))).map(|c| c.0),
            Some("sandbox_start_failed".into())
        );
    }

    #[test]
    fn backoff_says_until_when_and_keeps_the_failure_code() {
        let now = Utc::now();
        let mut r = row(Some("failed"));
        r.failure_code = Some("home_volume_unbound".into());
        r.failure_message = Some("the sandbox home volume cannot be bound".into());
        r.start_failures = 2;
        r.lease_until = Some(now + chrono::Duration::seconds(90));
        let c = classify(&r, 300, now).unwrap();
        assert_eq!(c.0, "home_volume_unbound");
        assert!(
            c.1.contains("next start attempt is allowed after") && c.1.contains("UTC"),
            "{}",
            c.1
        );
        assert!(!c.2);
        r.start_failures = 5;
        let c = classify(&r, 300, now).unwrap();
        assert!(
            c.1.contains("Automatic retries stopped after 5 failed starts"),
            "{}",
            c.1
        );
    }

    #[test]
    fn reasons_never_carry_credentials() {
        let now = Utc::now();
        let mut r = row(Some("failed"));
        r.failure_message =
            Some("pull https://bob:hunter2@reg/x failed token=abc aub_0123456789".into());
        let c = classify(&r, 300, now).unwrap();
        assert!(
            !c.1.contains("hunter2") && !c.1.contains("abc") && !c.1.contains("0123456789"),
            "{}",
            c.1
        );
    }
}
