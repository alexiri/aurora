//! `GET /agents/{agent}/sandbox`, `POST /agents/{agent}/restart`, `GET /agents/{agent}/models`
//! (contract M4).

use super::dto::{ModelOption, ModelOptions, SandboxPort, SandboxProcess, SandboxView};
use super::{lifecycle, model};
use crate::app::AppState;
use crate::domains::conversations::service::{self as conv, load_agent};
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::{conversations as convs, hosted, secrets};
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_sandbox))
        .routes(routes!(restart))
        .routes(routes!(list_models))
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

async fn view(s: &AppState, agent: Uuid) -> Result<SandboxView, ApiError> {
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
    Ok(SandboxView {
        agent_id: agent,
        state: r.state,
        boot_id: r.boot_id,
        runner_id: r.runner_id,
        image: running.then(|| format!("aurora-sandbox:{}", env!("CARGO_PKG_VERSION"))),
        started_at: r.started_at,
        keep_awake_reasons: reasons,
        failure_code: r.failure_code,
        failure_message: r.failure_message,
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
        restart_to_update: false,
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
    hosted_agent(&s, &c, agent).await?;
    Ok(Json(view(&s, agent).await?))
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
    hosted_agent(&s, &c, agent).await?;
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
    Ok((StatusCode::ACCEPTED, Json(view(&s, agent).await?)))
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
