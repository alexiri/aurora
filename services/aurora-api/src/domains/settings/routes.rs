use super::dto::*;
use super::service::{self, Kind, SectionRequest, Via};
use crate::app::AppState;
use crate::http::auth::InstanceAdmin;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::{ApiError, limits};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_index))
        .routes(routes!(export))
        .routes(routes!(import))
        .routes(routes!(get_section, put_section))
        .routes(routes!(preview))
        .routes(routes!(governance_orgs))
        .routes(routes!(llm_endpoint_test))
        .routes(routes!(llm_endpoint_warnings))
        .routes(routes!(sign_in_provider_test))
        .routes(routes!(email_test))
}

/// Discover an OIDC issuer without saving anything (C-780): the discovery document through the
/// destination guard, 10 s. No secret is needed or used.
#[utoipa::path(post, path = "/admin/settings/sign_in/provider-test", operation_id = "adminTestOidcProvider",
    tag = "admin", request_body = super::sign_in::ProviderTestRequest,
    responses((status = 200, body = super::sign_in::ProviderTestResult),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn sign_in_provider_test(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<super::sign_in::ProviderTestRequest>,
) -> Result<Json<super::sign_in::ProviderTestResult>, ApiError> {
    Ok(Json(super::sign_in::provider_test(&s, req).await?))
}

/// Send one test message to the caller's own address with the proposed or effective e-mail
/// settings (C-781). Nothing is saved; the password is never logged or returned.
#[utoipa::path(post, path = "/admin/settings/email/test", operation_id = "adminTestEmail",
    tag = "admin", request_body = super::sign_in::EmailTestRequest,
    responses((status = 200, body = super::sign_in::EmailTestResult),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn email_test(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<super::sign_in::EmailTestRequest>,
) -> Result<Json<super::sign_in::EmailTestResult>, ApiError> {
    Ok(Json(super::sign_in::email_test(&s, &c.0.email, req).await?))
}

/// Probe an LLM endpoint without a credential (C-779): one GET of its models path through the
/// destination guard, 10 s. Nothing is saved; no re-authentication (nothing changes).
#[utoipa::path(post, path = "/admin/settings/llm/endpoint-test", operation_id = "adminTestLlmEndpoint",
    tag = "admin", request_body = super::endpoint_test::EndpointTestRequest,
    responses((status = 200, body = super::endpoint_test::EndpointTestResult),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn llm_endpoint_test(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<super::endpoint_test::EndpointTestRequest>,
) -> Result<Json<super::endpoint_test::EndpointTestResult>, ApiError> {
    Ok(Json(super::endpoint_test::run(&s, req).await?))
}

/// (P5-POLISH-3, D-385, C-1034) A role an endpoint leaves unset (or not allowed), for the endpoint's row in
/// Administration › AI models.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct EndpointRoleWarning {
    /// `large` | `medium` | `small`.
    pub role: String,
    /// `role_unset` | `role_not_allowed`.
    pub warning: String,
}

/// An endpoint with at least one role warning.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct EndpointWarnings {
    pub endpoint_id: String,
    pub roles: Vec<EndpointRoleWarning>,
}

/// Which model roles each configured LLM endpoint leaves unset, from the same resolution as an agent's
/// turn (C-1034). Only endpoints with at least one warning are listed; agents' own overrides are not
/// considered. Instance admins only.
#[utoipa::path(get, path = "/admin/llm-endpoint-warnings", operation_id = "adminListLlmEndpointWarnings",
    tag = "admin",
    responses((status = 200, body = Vec<EndpointWarnings>), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn llm_endpoint_warnings(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<Vec<EndpointWarnings>>, ApiError> {
    let harness = s.ops().hosted.default_harness.clone();
    let llm = s.llm();
    let out = llm
        .endpoints()
        .iter()
        .filter_map(|ep| {
            let roles: Vec<EndpointRoleWarning> =
                crate::domains::hosted::model::endpoint_role_warnings(ep, &harness)
                    .into_iter()
                    .map(|(role, warning)| EndpointRoleWarning {
                        role: role.to_owned(),
                        warning: warning.to_owned(),
                    })
                    .collect();
            (!roles.is_empty()).then(|| EndpointWarnings {
                endpoint_id: ep.id.clone(),
                roles,
            })
        })
        .collect();
    Ok(Json(out))
}

/// Where organizations stand against the instance governance ceiling (C-778): effective postures
/// and the settings where an organization is stricter than the floor or clamped by it.
#[utoipa::path(get, path = "/admin/settings/governance/orgs", operation_id = "adminListGovernanceOrgs",
    tag = "admin", params(crate::http::pagination::PageParams),
    responses((status = 200, body = crate::http::pagination::Page<crate::domains::governance::standing::OrgStandingRow>),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn governance_orgs(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Query(p): Query<crate::http::pagination::PageParams>,
) -> Result<
    Json<crate::http::pagination::Page<crate::domains::governance::standing::OrgStandingRow>>,
    ApiError,
> {
    let page = p.resolve()?;
    Ok(Json(
        crate::domains::governance::standing::list(&s.pool, &s.gov_instance(), page).await?,
    ))
}

/// The settings sections and the keys the deployment configuration locks (C-771).
#[utoipa::path(get, path = "/admin/settings", operation_id = "adminListSettings", tag = "admin",
    responses((status = 200, body = SettingsIndex), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_index(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<SettingsIndex>, ApiError> {
    Ok(Json(service::index(&s)))
}

/// One section: every setting's effective value, source and lock (C-772).
#[utoipa::path(get, path = "/admin/settings/{section}", operation_id = "adminGetSettingsSection",
    tag = "admin",
    params(("section" = String, Path, description = "Section id")),
    responses((status = 200, body = SettingsSection), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_section(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(section): Path<String>,
) -> Result<Json<SettingsSection>, ApiError> {
    Ok(Json(
        service::section_view(&s, &section, Some(c.0.user_id)).await?,
    ))
}

/// Replace a section's dashboard values (C-773): compare-and-set on `generation`, session only
/// (no re-authentication, FIX-157, D-317), audited with a before/after diff.
#[utoipa::path(put, path = "/admin/settings/{section}", operation_id = "adminPutSettingsSection",
    tag = "admin",
    params(("section" = String, Path, description = "Section id")),
    request_body = PutSection,
    responses((status = 200, body = SettingsSection), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn put_section(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(section): Path<String>,
    Json(req): Json<PutSection>,
) -> Result<Json<SettingsSection>, ApiError> {
    let user = c.0.user_id;
    let via = Via::Admin(c.0);
    service::apply(
        &s,
        &via,
        Kind::Update,
        vec![SectionRequest {
            section: section.clone(),
            expected: Some(req.generation),
            values: req.values,
        }],
        false,
    )
    .await?;
    Ok(Json(service::section_view(&s, &section, Some(user)).await?))
}

/// What a PUT would change, which values loosen and which organizations are affected (C-774).
/// Writes nothing and needs no re-authentication.
#[utoipa::path(post, path = "/admin/settings/{section}/preview", operation_id = "adminPreviewSettingsSection",
    tag = "admin",
    params(("section" = String, Path, description = "Section id")),
    request_body = PutSection,
    responses((status = 200, body = SettingsPreview), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn preview(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(section): Path<String>,
    Json(req): Json<PutSection>,
) -> Result<Json<SettingsPreview>, ApiError> {
    Ok(Json(
        service::preview(&s, &section, req.generation, &req.values, Some(c.0.user_id)).await?,
    ))
}

/// The dashboard values as a YAML or JSON document, without secrets (C-775).
#[utoipa::path(get, path = "/admin/settings/export", operation_id = "adminExportSettings", tag = "admin",
    params(ExportQuery),
    responses((status = 200, description = "The settings document", content_type = "application/yaml", body = String),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn export(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let only: Option<Vec<String>> = q.sections.as_deref().map(|v| {
        v.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    });
    let doc = service::export_document(&s, only.as_deref())?;
    let (ext, ctype, body) = match q.format.as_deref().unwrap_or("yaml") {
        "json" => (
            "json",
            "application/json",
            serde_json::to_string_pretty(&doc).unwrap_or_default(),
        ),
        "yaml" => (
            "yaml",
            "application/yaml",
            serde_yaml::to_string(&doc).unwrap_or_default(),
        ),
        _ => {
            return Err(ApiError::invalid_request(
                "format must be `yaml` or `json`.",
                vec![aurora_core::FieldError::new("/format", "yaml or json")],
            ));
        }
    };
    let name = format!(
        "attachment; filename=\"aurora-settings-{}.{ext}\"",
        chrono::Utc::now().format("%Y-%m-%d")
    );
    let mut r = (StatusCode::OK, body).into_response();
    r.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(ctype));
    if let Ok(v) = HeaderValue::from_str(&name) {
        r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(r)
}

/// Import a settings document: a dry run shows the diff, applying writes every section in one
/// transaction (C-776).
#[utoipa::path(post, path = "/admin/settings/import", operation_id = "adminImportSettings", tag = "admin",
    request_body = ImportRequest,
    responses((status = 200, body = ImportResult), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 413, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn import(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<ImportRequest>,
) -> Result<Json<ImportResult>, ApiError> {
    if req.document.len() > limits::SETTINGS_IMPORT_BYTES_MAX {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "settings_too_large",
            "The settings document is too large.",
        ));
    }
    let merge = match req.mode.as_deref().unwrap_or("replace") {
        "replace" => false,
        "merge" => true,
        _ => {
            return Err(ApiError::invalid_request(
                "mode must be `replace` or `merge`.",
                vec![aurora_core::FieldError::new("/mode", "replace or merge")],
            ));
        }
    };
    let reqs = service::import_requests(
        &s,
        &req.document,
        &req.format,
        merge,
        req.generations.as_ref(),
    )?;
    if req.dry_run {
        return Ok(Json(service::import_dry_run(&s, &reqs).await));
    }
    // Applying needs the generation of every section the document touches (C-776).
    let missing: Vec<String> = reqs
        .iter()
        .filter(|r| r.expected.is_none())
        .map(|r| r.section.clone())
        .collect();
    if !missing.is_empty() {
        return Err(ApiError::invalid_request(
            "`generations` must list every section the document touches.",
            missing
                .iter()
                .map(|m| aurora_core::FieldError::new(format!("/generations/{m}"), "required"))
                .collect(),
        ));
    }
    let via = Via::Admin(c.0);
    let applied = service::apply(&s, &via, Kind::Import, reqs, true).await?;
    let sections = applied
        .changed
        .iter()
        .map(|(section, generation)| ImportSection {
            section: section.clone(),
            generation: *generation,
            diff: Vec::new(),
            loosening: Vec::new(),
            warnings: Vec::new(),
        })
        .collect();
    Ok(Json(ImportResult {
        applied: true,
        sections,
        errors: Vec::new(),
    }))
}
