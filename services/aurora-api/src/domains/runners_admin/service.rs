use super::dto::*;
use crate::app::AppState;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, limits};
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
