use super::dto::{Account, AccountRefType, AccountsQuery, TestAnswer};
use super::service;
use super::test;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_workspace_accounts))
        .routes(routes!(list_org_accounts))
        .routes(routes!(test_account))
}

/// Check one account now (C-943, SEC-129): the connect method's probe, a gateway database login, a hook mint or the AI-key
/// verify, in that order of what the account has. `can_own` of the account's workspace, or an organization owner or admin
/// for an organization's account. Never returns anything the upstream sent; stores the result (except `not_testable`) and
/// is audited `account.test`.
#[utoipa::path(
    post, path = "/accounts/{type}/{id}/test", operation_id = "testAccount", tag = "accounts",
    params(("type" = String, Path, description = "`connection` or `secret`"),
           ("id" = Uuid, Path, description = "Account id")),
    responses((status = 200, description = "What the check found", body = TestAnswer),
              (status = 400, description = "Unknown account type", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 429, description = "Rate limited (class `check`)", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn test_account(
    caller: Caller,
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, Uuid)>,
) -> Result<Json<TestAnswer>, ApiError> {
    let kind = match kind.as_str() {
        "connection" => AccountRefType::Connection,
        "secret" => AccountRefType::Secret,
        _ => {
            return Err(ApiError::invalid_request(
                "Invalid account type.",
                vec![aurora_core::FieldError::new(
                    "/type",
                    "must be connection or secret",
                )],
            ));
        }
    };
    Ok(Json(test::test(&state, &caller, kind, id).await?))
}

/// One list of what a workspace's agents can use (`can_use`; C-917): the workspace's own apps,
/// websites and AI keys, and the organization's accounts its agents can be given. With
/// `agent_id` the list is read for that agent (`given`, given first).
#[utoipa::path(
    get, path = "/workspaces/{ws}/accounts", operation_id = "listWorkspaceAccounts", tag = "accounts",
    params(("ws" = Uuid, Path, description = "Workspace id"), AccountsQuery),
    responses((status = 200, description = "Accounts", body = Page<Account>),
              (status = 400, description = "Invalid filter", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_workspace_accounts(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<AccountsQuery>,
) -> Result<Json<Page<Account>>, ApiError> {
    Ok(Json(
        service::list_workspace(&state, &caller, ws, &q).await?,
    ))
}

/// The accounts shared with the whole organization (owners and admins; C-917), each with the
/// agents of every workspace that use it and its limit.
#[utoipa::path(
    get, path = "/orgs/{org}/accounts", operation_id = "listOrgAccounts", tag = "accounts",
    params(("org" = Uuid, Path, description = "Organization id"), AccountsQuery),
    responses((status = 200, description = "Accounts", body = Page<Account>),
              (status = 400, description = "Invalid filter", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_org_accounts(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<AccountsQuery>,
) -> Result<Json<Page<Account>>, ApiError> {
    Ok(Json(service::list_org(&state, &caller, org, &q).await?))
}
