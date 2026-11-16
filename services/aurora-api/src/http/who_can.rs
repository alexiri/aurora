//! `WhoCan` (P5-UX-2, D-349, C-981): the one answer to "who can fix this?".
//!
//! Shape: the action, the role that holds it, up to three display names (owners first, then by
//! name; never an e-mail; never an instance admin), and a dashboard path when the caller can do it
//! themselves. The server sends the *role*; the dashboard owns the phrase (`14 §Who can fix this`).
//!
//! Refusals: the shared role helpers (`http::auth`) attach a `who_can` stub with the role and the
//! scope to every 403 they raise ([`forbid`]); they have no database handle, so [`middleware`]
//! fills in the names on the way out and drops the scope. Routes never build it themselves.

use crate::app::AppState;
use crate::http::error::ApiResult;
use aurora_core::ApiError;
use aurora_db::repo::contacts;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

/// The role that can fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WhoCanRole {
    WorkspaceOwner,
    WorkspaceApprover,
    OrgAdmin,
    OrgOwner,
    InstanceAdmin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WhoCanPerson {
    /// Display name, never an e-mail.
    pub name: String,
}

/// Who can do something, and where (C-981).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WhoCan {
    /// A snake_case key for what is asked (`view_this` / `change_this` when a refusal does not
    /// know better); the dashboard words it.
    pub action: String,
    pub role: WhoCanRole,
    /// At most three display names; always empty for `instance_admin`.
    pub people: Vec<WhoCanPerson>,
    /// A dashboard path (`aurora_core::dash_paths`), present only when `you_can`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub you_can: bool,
}

/// `contacts` on a workspace or an organization (C-981): the people to ask where nothing was
/// refused. Readable by every member of that level.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Contacts {
    /// Owners, at most three.
    pub owners: Vec<WhoCanPerson>,
    /// Administrators (organization level only), at most three.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admins: Option<Vec<WhoCanPerson>>,
}

fn persons(names: Vec<String>) -> Vec<WhoCanPerson> {
    names
        .into_iter()
        .take(contacts::CONTACTS_MAX as usize)
        .map(|name| WhoCanPerson { name })
        .collect()
}

/// The people who hold `role` in this scope. Never an instance admin; a query failure yields
/// no names (the sentence then names only the role).
pub async fn people(
    s: &AppState,
    role: WhoCanRole,
    org: Uuid,
    workspace: Option<Uuid>,
) -> Vec<WhoCanPerson> {
    let names = match (role, workspace) {
        (WhoCanRole::InstanceAdmin, _) => return Vec::new(),
        (WhoCanRole::OrgOwner, _) => contacts::org_contacts(&s.pool, org, true).await,
        (WhoCanRole::OrgAdmin, _) => contacts::org_contacts(&s.pool, org, false).await,
        // Without a workspace (an org-level route) the organization's administrators decide.
        (WhoCanRole::WorkspaceOwner | WhoCanRole::WorkspaceApprover, None) => {
            contacts::org_contacts(&s.pool, org, false).await
        }
        (WhoCanRole::WorkspaceOwner, Some(w)) => {
            contacts::workspace_contacts(&s.pool, w, false).await
        }
        (WhoCanRole::WorkspaceApprover, Some(w)) => {
            contacts::workspace_contacts(&s.pool, w, true).await
        }
    };
    persons(names.unwrap_or_default())
}

impl WhoCan {
    /// The one builder. `path` is `Some` when the caller can do it themselves; the names are
    /// given either way (a page can offer both the action and the people who share it).
    pub async fn build(
        s: &AppState,
        role: WhoCanRole,
        org: Uuid,
        workspace: Option<Uuid>,
        action: &str,
        path: Option<String>,
    ) -> Self {
        let you_can = path.is_some();
        Self {
            action: action.to_owned(),
            role,
            people: people(s, role, org, workspace).await,
            path,
            you_can,
        }
    }
}

/// The `contacts` of a workspace.
pub async fn workspace_contacts(s: &AppState, workspace: Uuid) -> ApiResult<Contacts> {
    Ok(Contacts {
        owners: persons(contacts::workspace_contacts(&s.pool, workspace, false).await?),
        admins: None,
    })
}

/// The `contacts` of an organization.
pub async fn org_contacts(s: &AppState, org: Uuid) -> ApiResult<Contacts> {
    Ok(Contacts {
        owners: persons(contacts::org_contacts(&s.pool, org, true).await?),
        admins: Some(persons(contacts::org_contacts(&s.pool, org, false).await?)),
    })
}

/// The refusal stub: `who_can` with the role and (until [`middleware`] resolves it) the scope.
/// Use it as the `details` of a 4xx whose sentence needs "who can fix this" (C-958 uses it for
/// `oauth_client_required`).
pub fn stub(role: WhoCanRole, org: Option<Uuid>, workspace: Option<Uuid>) -> Value {
    let mut who = json!({
        "action": "", "role": role, "people": [], "you_can": false,
    });
    if org.is_some() || workspace.is_some() {
        who["scope"] = json!({ "org_id": org, "workspace_id": workspace });
    }
    json!({ "who_can": who })
}

/// 403 `forbidden` carrying `who_can` (the shared role helpers use this).
pub fn forbid(
    message: &str,
    role: WhoCanRole,
    org: Option<Uuid>,
    workspace: Option<Uuid>,
) -> ApiError {
    ApiError::forbidden(message).with_details(stub(role, org, workspace))
}

/// 403 with a caller-chosen code (`approver_required`, `instance_admin_required`) carrying `who_can`.
pub fn forbid_code(
    code: &str,
    message: &str,
    role: WhoCanRole,
    org: Option<Uuid>,
    workspace: Option<Uuid>,
) -> ApiError {
    ApiError::new(http::StatusCode::FORBIDDEN, code, message)
        .with_details(stub(role, org, workspace))
}

/// Fills the names into every 403 whose `details.who_can` carries a scope, and drops the scope.
pub async fn middleware(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let res = next.run(req).await;
    // 403 refusals and the 422 "an admin must set this up first" answers carry a stub.
    if !matches!(
        res.status(),
        http::StatusCode::FORBIDDEN | http::StatusCode::UNPROCESSABLE_ENTITY
    ) {
        return res;
    }
    let (parts, body) = res.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024).await else {
        return ApiError::forbidden("Forbidden.").into_response();
    };
    let resolved = resolve(&s, &method, &bytes).await;
    let mut res = Response::from_parts(
        parts,
        axum::body::Body::from(resolved.unwrap_or(bytes.to_vec())),
    );
    res.headers_mut().remove(http::header::CONTENT_LENGTH);
    res
}

async fn resolve(s: &AppState, method: &http::Method, bytes: &[u8]) -> Option<Vec<u8>> {
    let mut v: Value = serde_json::from_slice(bytes).ok()?;
    let who = v.pointer_mut("/error/details/who_can")?.as_object_mut()?;
    let scope = who.remove("scope")?;
    let role: WhoCanRole = serde_json::from_value(who.get("role")?.clone()).ok()?;
    let id = |k: &str| scope.get(k)?.as_str()?.parse::<Uuid>().ok();
    let (org, ws) = (id("org_id"), id("workspace_id"));
    let names = match org {
        Some(o) => people(s, role, o, ws).await,
        None => Vec::new(),
    };
    who.insert("people".into(), serde_json::to_value(names).ok()?);
    if who.get("action").and_then(Value::as_str) == Some("") {
        let default = if matches!(*method, http::Method::GET | http::Method::HEAD) {
            "view_this"
        } else {
            "change_this"
        };
        who.insert("action".into(), default.into());
    }
    serde_json::to_vec(&v).ok()
}
