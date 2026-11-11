//! `GET /agents/{agent}/sandbox`, `POST /agents/{agent}/restart`, `GET /agents/{agent}/models`
//! (contract M4).

use super::dto::{
    AgentStartFailure, AgentStartFailures, DefaultModel, HarnessList, HarnessView, KeyReadiness,
    ModelOption, ModelOptions, ModelReadiness, SandboxPort, SandboxProcess, SandboxStartAttempt,
    SandboxView, StartLogEntry,
};
use super::{lifecycle, model};
use crate::app::AppState;
use crate::audit::{AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::service::{self as conv, load_agent};
use crate::domains::runner_plane::boot;
use crate::http::auth::Caller;
use crate::http::auth::require_own;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::{conversations as convs, hosted, secrets};
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_sandbox))
        .routes(routes!(restart))
        .routes(routes!(retry_sandbox))
        .routes(routes!(start_failures))
        .routes(routes!(list_models))
        .routes(routes!(model_readiness))
        .routes(routes!(list_harnesses))
}

/// The harnesses agents may use on this instance (`hosted.enabled_harnesses`) and whether a
/// live runner replica offers each one (C-420).
#[utoipa::path(get, path = "/harnesses", operation_id = "listHarnesses", tag = "agents",
    responses((status = 200, body = HarnessList)),
    security(("session" = []), ("api_key" = [])))]
async fn list_harnesses(
    _c: Caller,
    State(s): State<AppState>,
) -> Result<Json<HarnessList>, ApiError> {
    let live = aurora_db::repo::runners::live_harnesses(
        &s.pool,
        aurora_core::limits::REPLICA_ONLINE_THRESHOLD.as_secs_f64(),
    )
    .await?;
    let items = s
        .config
        .hosted
        .enabled_harnesses
        .iter()
        .filter_map(|id| {
            let (_, name) = model::HARNESSES.iter().find(|(h, _)| h == id)?;
            Some(HarnessView {
                id: id.clone(),
                display_name: (*name).to_owned(),
                dialects: model::harness_dialects(id)
                    .iter()
                    .map(|d| d.as_str().to_owned())
                    .collect(),
                available: live.iter().any(|l| l == id),
            })
        })
        .collect();
    Ok(Json(HarnessList {
        items,
        default: s.config.hosted.default_harness.clone(),
    }))
}

async fn hosted_agent(
    s: &AppState,
    c: &Caller,
    id: Uuid,
) -> Result<aurora_db::repo::agents::AgentRow, ApiError> {
    let a = load_agent(s, c, id).await?;
    if a.kind != "hosted" {
        return Err(ApiError::not_found("Agent has no sandbox."));
    }
    Ok(a)
}

async fn view(s: &AppState, a: &AgentRow) -> Result<SandboxView, ApiError> {
    let agent = a.id;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let Some(r) = hosted::sandbox_view(&mut conn, agent).await? else {
        return Ok(SandboxView {
            agent_id: agent,
            state: "unprovisioned".into(),
            boot_id: None,
            runner_id: None,
            image: None,
            started_at: None,
            keep_awake_reasons: vec![],
            failure_code: None,
            failure_message: None,
            start_attempt: None,
            retryable: false,
            start_log: vec![],
            processes: vec![],
            ports: vec![],
            restart_to_update: false,
        });
    };
    let running = matches!(r.state.as_str(), "starting" | "running");
    let processes = match (r.boot_id, running) {
        (Some(b), true) => hosted::processes_of_boot(&mut conn, r.id, b).await?,
        _ => vec![],
    };
    let mut reasons = Vec::new();
    if r.active_turn {
        reasons.push("turn".to_owned());
    }
    if r.busy_process {
        reasons.push("process".to_owned());
    }
    if r.armed_watch {
        reasons.push("watch".to_owned());
    }
    // The image is whatever the owning runner advertised (`runner.image`).
    let runner_caps = if running {
        match r.runner_id {
            Some(rid) => aurora_db::repo::runners::get(&s.pool, rid)
                .await?
                .and_then(|row| crate::domains::runner_plane::work::caps_of(&row)),
            None => None,
        }
    } else {
        None
    };
    let image = if running {
        Some(
            runner_caps
                .as_ref()
                .and_then(|c| c.image.clone())
                .unwrap_or_else(boot::default_image),
        )
    } else {
        None
    };
    // Env/stub changes and a new runner image apply at the next boot: compare what a boot would
    // get now with what was recorded when the running boot started. Boots recorded before the
    // image was stored flag only on the env/stub hash.
    let restart_to_update = if running {
        let booted_hash = hosted::boot_app_inputs_hash(&mut conn, agent).await?;
        let booted_image = hosted::boot_image(&mut conn, agent).await?;
        let booted_caps = hosted::boot_capabilities(&mut conn, agent).await?;
        drop(conn);
        // (RFE-003) A boot started without the capability a boot would get now (older boots,
        // or the posture was loosened since) needs a restart to gain it.
        let caps_changed = boot::capabilities_for(s, a, runner_caps.as_ref()).await? != booted_caps;
        let image_changed = matches!((&booted_image, &image), (Some(b), Some(now)) if b != now);
        let hash_changed = match booted_hash {
            Some(h) => boot::app_boot_for(s, a).await?.hash != h,
            None => false,
        };
        image_changed || hash_changed || caps_changed
    } else {
        false
    };
    Ok(SandboxView {
        agent_id: agent,
        state: r.state.clone(),
        boot_id: r.boot_id,
        runner_id: r.runner_id,
        image,
        started_at: r.started_at,
        keep_awake_reasons: reasons,
        failure_code: r.failure_code,
        failure_message: r.failure_message,
        start_attempt: r.start_attempt_at.map(|started_at| SandboxStartAttempt {
            started_at,
            phase: r.start_phase,
            detail: r.start_detail,
            events: r
                .start_events
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            progress_at: r.start_progress_at,
        }),
        // (FIX-029, C-611) `retryable = state == "failed"`.
        retryable: r.state == "failed",
        start_log: start_log_entries(&r.start_log),
        processes: processes
            .into_iter()
            .map(|p| SandboxProcess {
                id: p.id.to_string(),
                name: p.name,
                state: p.status,
                busy: p.busy,
                started_at: Some(p.started_at),
            })
            .collect(),
        ports: r
            .listening_ports
            .into_iter()
            .map(|port| SandboxPort {
                port,
                protocol: "tcp".into(),
            })
            .collect(),
        restart_to_update,
    })
}

/// The agent's sandbox (computer) state.
#[utoipa::path(get, path = "/agents/{agent}/sandbox", operation_id = "getAgentSandbox", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = SandboxView), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_sandbox(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<SandboxView>, ApiError> {
    let a = hosted_agent(&s, &c, agent).await?;
    Ok(Json(view(&s, &a).await?))
}

/// (FIX-029, C-611) Parse the `start_log` jsonb into `StartLogEntry`s (already newest first).
fn start_log_entries(v: &serde_json::Value) -> Vec<StartLogEntry> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let ts = e.get("ts")?.as_str()?.parse().ok()?;
                    Some(StartLogEntry {
                        ts,
                        kind: e
                            .get("kind")
                            .and_then(|k| k.as_str())
                            .unwrap_or("phase")
                            .to_owned(),
                        phase: e.get("phase").and_then(|p| p.as_str()).map(str::to_owned),
                        detail: e.get("detail").and_then(|d| d.as_str()).map(str::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Map the stored start-failure rows to the C-586 shape (with or without `runner_id`).
fn failure_items(
    rows: Vec<aurora_db::repo::sandboxes::AgentStartFailureRow>,
    with_runner: bool,
) -> Vec<AgentStartFailure> {
    rows.into_iter()
        .map(|r| {
            let events = r
                .events
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            AgentStartFailure {
                id: r.id,
                sandbox_id: r.sandbox_id,
                runner_id: with_runner.then_some(r.runner_id).flatten(),
                agent_id: r.agent_id,
                code: r.code,
                message: r.message,
                phase: r.phase,
                events,
                elapsed_secs: r.elapsed_secs,
                created_at: r.created_at,
            }
        })
        .collect()
}

/// (FIX-029, C-613) The agent's recent start failures, newest first, ≤ 20 (C-586 shape).
#[utoipa::path(get, path = "/agents/{agent}/start-failures", operation_id = "agentStartFailures", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = AgentStartFailures), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn start_failures(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<AgentStartFailures>, ApiError> {
    let a = hosted_agent(&s, &c, agent).await?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let rows = aurora_db::repo::sandboxes::agent_start_failures(&mut conn, a.id, 20).await?;
    drop(conn);
    Ok(Json(AgentStartFailures {
        items: failure_items(rows, false),
    }))
}

/// (FIX-029, C-610) Retry a failed sandbox start from `failed`: clears the failure, resets the
/// automatic-retry bound, claims the ordinary lifecycle lease, appends a retry note to the start
/// log and notifies due work — the next start is the ordinary due-work start arm.
#[utoipa::path(post, path = "/agents/{agent}/sandbox/retry", operation_id = "retryAgentSandbox", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 202, body = SandboxView), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 423, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn retry_sandbox(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<(StatusCode, Json<SandboxView>), ApiError> {
    let a = hosted_agent(&s, &c, agent).await?;
    // 423 `frozen` on an effectively frozen agent (a retry would start work).
    crate::domains::freeze::service::ensure_agent_active_pool(&s, agent).await?;
    let label = AuditActor::from_caller(&c).label;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let (retried, previous_code) = {
        // The note names the failure code being retried; the row is fenced by the conditional
        // UPDATE inside `retry_failed_start`, so a concurrent retry cannot double-claim.
        let current: Option<String> = sqlx::query_scalar(
            "SELECT coalesce(failure_code, 'unknown') FROM sandboxes WHERE agent_id = $1",
        )
        .bind(agent)
        .fetch_optional(&mut *tx)
        .await
        .map_err(DbError::from)?
        .flatten();
        let note = format!(
            "Start retried by {label} after {}.",
            current.clone().unwrap_or_else(|| "unknown".into())
        );
        let ok = aurora_db::repo::sandboxes::retry_failed_start(&mut tx, agent, &note).await?;
        (ok, current.filter(|c| c != "unknown"))
    };
    if !retried {
        return Err(ApiError::conflict(
            "sandbox_not_failed",
            "The sandbox start is not failed (it may be in flight or its backoff lease is live).",
        ));
    }
    conv::notify_work(&mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    // Best-effort audit (docs/04 §Audit): metadata carries the previous failure code only.
    audit_event(&s, &c, &a, previous_code);
    Ok((StatusCode::ACCEPTED, Json(view(&s, &a).await?)))
}

/// Best-effort `agent.sandbox_retry` audit row.
fn audit_event(s: &AppState, c: &Caller, a: &AgentRow, previous_code: Option<String>) {
    crate::audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "agent.sandbox_retry",
            Outcome::Success,
        )
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("agent", a.id)
        .metadata(serde_json::json!({ "previous_failure_code": previous_code })),
    );
}

/// Restart the agent's computer; its active turns fail with `agent_restarted`.
#[utoipa::path(post, path = "/agents/{agent}/restart", operation_id = "restartAgent", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 202, body = SandboxView), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn restart(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<(StatusCode, Json<SandboxView>), ApiError> {
    let a = hosted_agent(&s, &c, agent).await?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    hosted::request_restart(&mut tx, agent).await?;
    let mut flush = Vec::new();
    for (turn, cid) in hosted::active_turns_of_agent(&mut tx, agent).await? {
        convs::lock(&mut tx, cid).await?;
        if lifecycle::fail(
            &mut tx,
            turn,
            cid,
            "agent_restarted",
            "The agent's computer was restarted.",
        )
        .await?
        .is_some()
        {
            flush.push((cid, turn));
        }
    }
    conv::notify_work(&mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    for (cid, turn) in flush {
        lifecycle::flush_deltas(&s, cid, turn).await;
    }
    Ok((StatusCode::ACCEPTED, Json(view(&s, &a).await?)))
}

/// Per-key readiness and the default model of a hosted agent (`can_own`; D-136, C-500). The
/// dispatcher decides `no_model_key` / `no_default_model` from the same function.
#[utoipa::path(get, path = "/agents/{agent}/model-readiness", operation_id = "getAgentModelReadiness", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = ModelReadiness), (status = 403, body = ErrorEnvelope),
              (status = 404, description = "Not found, not hosted, or not visible", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn model_readiness(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<ModelReadiness>, ApiError> {
    let a = hosted_agent(&s, &c, agent).await?;
    let auth = c.workspace(&s, a.workspace_id).await?;
    require_own(&auth)?;
    let harness = a
        .harness
        .clone()
        .unwrap_or_else(|| s.config.hosted.default_harness.clone());
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let r = model::readiness(&s, &mut conn, a.id, a.org_id, a.workspace_id, &harness).await?;
    drop(conn);
    let default = r.default.as_ref().and_then(|d| {
        let ep = r.keys.get(d.key)?.ep?;
        Some(DefaultModel {
            endpoint_id: ep.id.clone(),
            provider: ep.provider.clone(),
            model_id: d.model.clone(),
            source: d.source.as_str().to_owned(),
        })
    });
    Ok(Json(ModelReadiness {
        harness,
        keys: r
            .keys
            .iter()
            .map(|k| KeyReadiness {
                secret_id: k.secret.id,
                name: k.secret.name.clone(),
                scope: if k.secret.workspace_id.is_some() {
                    "workspace"
                } else {
                    "organization"
                }
                .to_owned(),
                endpoint_id: k.secret.llm_endpoint_id.clone(),
                provider: k.secret.llm_provider.clone(),
                usable: k.reason.is_none(),
                reason: k.reason.map(|r| r.as_str().to_owned()),
            })
            .collect(),
        default,
        override_dropped: r.override_dropped.is_some(),
    }))
}

/// Models selectable for the agent: usable granted keys only, allowed models only.
#[utoipa::path(get, path = "/agents/{agent}/models", operation_id = "listAgentModels", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = ModelOptions), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_models(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<ModelOptions>, ApiError> {
    let a = load_agent(&s, &c, agent).await?;
    let harness = a
        .harness
        .clone()
        .unwrap_or_else(|| s.config.hosted.default_harness.clone());
    let eff = crate::domains::secrets::gov::effective_for(&s, a.org_id).await?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let cands = hosted::granted_llm_secrets(&mut conn, a.id, a.org_id, a.workspace_id).await?;
    let usable = model::usable(&s, &eff, &harness, cands);
    drop(conn);
    let mut items: Vec<ModelOption> = Vec::new();
    for u in usable {
        // One entry set per endpoint (the first usable key in preference order serves it).
        if items.iter().any(|i| i.endpoint_id == u.ep.id) {
            continue;
        }
        let Some(row) = secrets::get(&s.pool, u.secret.id).await? else {
            continue;
        };
        let list = match crate::domains::secrets::service::models(&s, &row).await {
            Ok(m) => m
                .items
                .into_iter()
                .map(|m| (m.id, m.display_name))
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e.code(), "model listing failed; using static list");
                let allowed = eff
                    .endpoints
                    .get(&u.ep.id)
                    .map(|e| e.allowed_models.clone())
                    .unwrap_or_default();
                u.ep.models
                    .iter()
                    .chain(u.ep.default_model.iter())
                    .filter(|m| allowed.is_empty() || allowed.contains(m))
                    .map(|m| (m.clone(), None))
                    .collect::<Vec<_>>()
            }
        };
        let dialects: Vec<String> = s
            .llm
            .provider_for(u.ep)
            .map(|p| {
                p.dialects(u.ep)
                    .iter()
                    .map(|d| d.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        for (id, display_name) in list {
            if !items
                .iter()
                .any(|i| i.endpoint_id == u.ep.id && i.model_id == id)
            {
                items.push(ModelOption {
                    endpoint_id: u.ep.id.clone(),
                    provider: u.ep.provider.clone(),
                    model_id: id,
                    display_name,
                    dialects: dialects.clone(),
                });
            }
        }
    }
    Ok(Json(ModelOptions { items }))
}
