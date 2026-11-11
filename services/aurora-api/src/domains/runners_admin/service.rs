use super::dto::*;
use crate::app::AppState;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, limits};
use aurora_db::repo::agents;
use aurora_db::repo::runners::{self, RunnerRow};
use aurora_db::{DbError, conflict_code};
use uuid::Uuid;

fn fingerprint(tok: &str) -> String {
    token::hash(tok)[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn summary(s: &AppState, r: RunnerRow) -> Result<RunnerSummary, ApiError> {
    let online = limits::REPLICA_ONLINE_THRESHOLD.as_secs_f64();
    let replicas = runners::list_replicas(&s.pool, r.id, online).await?;
    let stats = runners::stats(&s.pool, online)
        .await?
        .into_iter()
        .find(|x| x.runner_id == r.id);
    let max = r
        .capabilities
        .as_ref()
        .and_then(|c| c.get("max_sandboxes"))
        .and_then(serde_json::Value::as_i64);
    Ok(RunnerSummary {
        id: r.id,
        name: r.name,
        installation_fingerprint: r.installation_fingerprint,
        backend: r.backend,
        capabilities: r.capabilities,
        last_registered_at: r.last_registered_at,
        created_at: r.created_at,
        live_replicas: stats.as_ref().map_or(0, |x| x.live_replicas),
        sandboxes: stats.as_ref().map_or(0, |x| x.sandboxes),
        running_sandboxes: stats.as_ref().map_or(0, |x| x.running),
        max_sandboxes: max,
        replicas: replicas
            .into_iter()
            .map(|p| Replica {
                id: p.id,
                advertised_host: p.advertised_host,
                version: p.version,
                started_at: p.started_at,
                last_seen_at: p.last_seen_at,
                retired_at: p.retired_at,
                live: p.live,
            })
            .collect(),
    })
}

pub async fn list(s: &AppState) -> Result<RunnerList, ApiError> {
    let mut items = Vec::new();
    for r in runners::list(&s.pool).await? {
        items.push(summary(s, r).await?);
    }
    Ok(RunnerList { items })
}

/// (FIX-025, C-586) The runner's recent start failures (Admin › Runners).
pub async fn start_failures(s: &AppState, id: Uuid) -> Result<RunnerStartFailures, ApiError> {
    if runners::get(&s.pool, id).await?.is_none() {
        return Err(ApiError::not_found("Runner not found."));
    }
    let rows = runners::recent_start_failures(&s.pool, id, 50).await?;
    Ok(RunnerStartFailures {
        items: rows
            .into_iter()
            .map(|r| RunnerStartFailure {
                id: r.id,
                sandbox_id: r.sandbox_id,
                agent_id: r.agent_id,
                code: r.code,
                message: r.message,
                phase: r.phase,
                events: r
                    .events
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| e.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                elapsed_secs: r.elapsed_secs,
                created_at: r.created_at,
            })
            .collect(),
    })
}

/// (FIX-029, C-613/C-614) Map stored start-failure rows to the C-586 shape.
pub fn agent_failure_items(
    rows: Vec<aurora_db::repo::sandboxes::AgentStartFailureRow>,
) -> Vec<crate::domains::hosted::dto::AgentStartFailure> {
    rows.into_iter()
        .map(|r| crate::domains::hosted::dto::AgentStartFailure {
            id: r.id,
            sandbox_id: r.sandbox_id,
            runner_id: r.runner_id,
            agent_id: r.agent_id,
            code: r.code,
            message: r.message,
            phase: r.phase,
            events: r
                .events
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            elapsed_secs: r.elapsed_secs,
            created_at: r.created_at,
        })
        .collect()
}

/// (FIX-029, C-614) An agent's recent start failures with the runner named (instance admins).
pub async fn agent_start_failures(
    s: &AppState,
    agent: Uuid,
) -> Result<crate::domains::hosted::dto::AgentStartFailures, ApiError> {
    if agents::get(&s.pool, agent).await?.is_none() {
        return Err(ApiError::not_found("Agent not found."));
    }
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let rows = aurora_db::repo::sandboxes::agent_start_failures(&mut conn, agent, 20).await?;
    drop(conn);
    Ok(crate::domains::hosted::dto::AgentStartFailures {
        items: agent_failure_items(rows),
    })
}

/// Security-critical audit event of a runner change (RFE-008): committed with the change.
fn event(actor: &crate::audit::AuditActor, action: &str, id: Uuid) -> crate::audit::AuditEvent {
    crate::audit::AuditEvent::new(actor.clone(), action, crate::audit::Outcome::Success)
        .target("runner", id)
}

pub async fn create(
    s: &AppState,
    name: &str,
    actor: &crate::audit::AuditActor,
) -> Result<RunnerWithToken, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(ApiError::invalid_request(
            "name must be 1-64 characters.",
            vec![],
        ));
    }
    let tok = token::generate(TokenPrefix::Runner);
    let mut tx = crate::audit::begin(s).await?;
    let row = runners::insert(
        &mut *tx,
        Uuid::now_v7(),
        name,
        &token::hash(tok.expose()),
        &fingerprint(tok.expose()),
    )
    .await
    .map_err(|e| conflict_code(e, &[("runners_name_key", "runner_name_taken")]))?;
    crate::audit::commit_critical(tx, &event(actor, "admin.runner_create", row.id)).await?;
    Ok(RunnerWithToken {
        runner: summary(s, row).await?,
        token: tok.expose().clone(),
    })
}

pub async fn rotate(
    s: &AppState,
    id: Uuid,
    actor: &crate::audit::AuditActor,
) -> Result<RunnerWithToken, ApiError> {
    let tok = token::generate(TokenPrefix::Runner);
    let mut tx = crate::audit::begin(s).await?;
    if !runners::set_token(
        &mut *tx,
        id,
        &token::hash(tok.expose()),
        &fingerprint(tok.expose()),
    )
    .await?
    {
        return Err(ApiError::not_found("Runner not found."));
    }
    crate::audit::commit_critical(tx, &event(actor, "admin.runner_rotate", id)).await?;
    let row = runners::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Runner not found."))?;
    Ok(RunnerWithToken {
        runner: summary(s, row).await?,
        token: tok.expose().clone(),
    })
}

pub async fn delete(
    s: &AppState,
    id: Uuid,
    actor: &crate::audit::AuditActor,
) -> Result<(), ApiError> {
    if runners::occupied_sandboxes(&s.pool, id).await? > 0 {
        return Err(ApiError::conflict(
            "runner_in_use",
            "The runner still has running sandboxes.",
        ));
    }
    let mut tx = crate::audit::begin(s).await?;
    if runners::delete(&mut *tx, id).await? {
        crate::audit::commit_critical(tx, &event(actor, "admin.runner_delete", id)).await?;
        Ok(())
    } else {
        Err(ApiError::not_found("Runner not found."))
    }
}

/// `AURORA_BOOTSTRAP_RUNNER_TOKEN`: upsert runner `default` with this token's hash.
pub async fn bootstrap_default(s: &AppState) -> Result<(), DbError> {
    if let Some(t) = &s.config.bootstrap_runner_token {
        let t = t.expose();
        runners::upsert_named(&s.pool, "default", &token::hash(t), &fingerprint(t)).await?;
    }
    Ok(())
}
