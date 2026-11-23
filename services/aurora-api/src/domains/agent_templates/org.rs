//! The organizations' template switches and the person-facing list (C-1109). Org owners/admins
//! only, session or api key; the audit is ordinary (`org.template_availability`). The kill
//! switch (`agents.templates.enabled`) answers 404 `templates_disabled` here.

use super::admin;
use super::dto::{OrgTemplateItem, OrgTemplateList, SetOrgTemplateRequest};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

/// The kill switch for the person-facing template routes (C-1109, C-1110, C-1111): 404
/// `templates_disabled`; the admin routes keep working.
pub(crate) fn ensure_enabled(state: &AppState) -> Result<(), ApiError> {
    if state.ops().agents.templates.enabled {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "templates_disabled",
            "Agent templates are disabled on this instance.",
        ))
    }
}

/// The organization's view of every offered template (C-1109): on by default, `enabled: false`
/// when the organization turned it off.
pub async fn list_for_org(state: &AppState, org: Uuid) -> Result<OrgTemplateList, ApiError> {
    let disabled = aurora_db::repo::agent_templates::org_disabled(&state.pool, org).await?;
    let mut items = Vec::new();
    for t in aurora_db::repo::agent_templates::all_templates(&state.pool).await? {
        if !t.offered {
            continue;
        }
        let version = admin::offered_version(&state.pool, t.id).await?;
        items.push(OrgTemplateItem {
            enabled: !disabled.iter().any(|id| id == &t.template_id),
            version: version.map(|v| v.version),
            template_id: t.template_id,
            name: t.name,
            summary: t.summary,
        });
    }
    items.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.template_id.cmp(&b.template_id))
    });
    Ok(OrgTemplateList { items })
}

/// Turn a template off or on for the organization (C-1109). The stored versions and the
/// instance-wide `offered` flag are untouched; the audit is ordinary and best-effort.
pub async fn set_org_template(
    state: &AppState,
    actor: &AuditActor,
    org: Uuid,
    template_id: &str,
    req: &SetOrgTemplateRequest,
) -> Result<(), ApiError> {
    let t = aurora_db::repo::agent_templates::template_by_template_id(&state.pool, template_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Template not found."))?;
    if req.enabled {
        aurora_db::repo::agent_templates::set_org_enabled(&state.pool, org, &t.template_id).await?;
    } else {
        aurora_db::repo::agent_templates::set_org_disabled(
            &state.pool,
            org,
            &t.template_id,
            actor.id.unwrap_or_default(),
        )
        .await?;
    }
    audit::record(
        state,
        AuditEvent::new(actor.clone(), "org.template_availability", Outcome::Success)
            .target("organization", org)
            .metadata(json!({"template": t.template_id, "enabled": req.enabled})),
    );
    Ok(())
}

/// The organization's offered templates with their switches (C-1109).
#[utoipa::path(get, path = "/orgs/{org}/agent-templates", operation_id = "listOrgTemplates", tag = "agent-templates",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = OrgTemplateList), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_org_templates(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<OrgTemplateList>, ApiError> {
    ensure_enabled(&s)?;
    c.org(&s, org).await?.require_admin()?;
    Ok(Json(list_for_org(&s, org).await?))
}

/// Turn a template off or on for the organization (C-1109); 404 when the template does not
/// exist (a switch for an unknown id is never stored).
#[utoipa::path(put, path = "/orgs/{org}/agent-templates/{template_id}", operation_id = "setOrgTemplate", tag = "agent-templates",
    params(("org" = Uuid, Path, description = "Organization id"), ("template_id" = String, Path, description = "Template id")),
    request_body = SetOrgTemplateRequest,
    responses((status = 204), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn set_org_template_route(
    c: Caller,
    State(s): State<AppState>,
    Path((org, template_id)): Path<(Uuid, String)>,
    Json(req): Json<SetOrgTemplateRequest>,
) -> Result<StatusCode, ApiError> {
    ensure_enabled(&s)?;
    let org_auth = c.org(&s, org).await?;
    org_auth.require_admin()?;
    set_org_template(&s, &AuditActor::from_caller(&c), org, &template_id, &req).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_org_templates))
        .routes(routes!(set_org_template_route))
}
