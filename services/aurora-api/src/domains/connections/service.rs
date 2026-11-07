use super::dto::*;
use crate::app::AppState;
use crate::domains::plugins::registry;
use crate::http::auth::{Caller, require_own};
use crate::http::pagination::{Page, Pagination};
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::repo::connections::{self as repo, ConnectionRow};
use aurora_db::repo::credentials;
use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
use uuid::Uuid;

const LABEL_MAX: usize = 120;

/// Read access to a connection's scope (404 when invisible).
pub async fn visible(
    caller: &Caller,
    state: &AppState,
    id: Uuid,
) -> Result<(ConnectionRow, Access), ApiError> {
    let row = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connection not found."))?;
    let access = match row.workspace_id {
        Some(ws) => Access::Workspace(caller.workspace(state, ws).await?),
        None => {
            let org = caller.org(state, row.org_id).await?;
            // org-scope metadata is also readable by `can_own` workspace owners (I-27 amendment)
            let can_read = org.role.is_admin()
                || aurora_db::authz::owns_any_workspace(&state.pool, caller.user_id, row.org_id)
                    .await?;
            Access::Org {
                auth: org,
                can_read,
            }
        }
    };
    Ok((row, access))
}

pub enum Access {
    Workspace(aurora_db::authz::WorkspaceAuth),
    Org {
        auth: crate::http::auth::OrgAuth,
        /// Owner/admin, or `can_own` on some workspace of the org.
        can_read: bool,
    },
}

impl Access {
    /// `can_own` (workspace) or owner/admin (organization).
    pub fn require_write(&self) -> Result<(), ApiError> {
        match self {
            Self::Workspace(a) => require_own(a),
            Self::Org { auth, .. } => auth.require_admin(),
        }
    }

    /// Metadata read: `can_own` (workspace); organization scope also admits a `can_own`
    /// workspace owner of the org (so org connections can be granted to their agents).
    pub fn require_read(&self) -> Result<(), ApiError> {
        match self {
            Self::Workspace(a) => require_own(a),
            Self::Org { can_read: true, .. } => Ok(()),
            Self::Org { auth, .. } => auth.require_admin(),
        }
    }
}

pub async fn list(
    state: &AppState,
    org: Uuid,
    ws: Option<Uuid>,
    q: &ListQuery,
) -> Result<Page<Connection>, ApiError> {
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    if let Some(s) = &q.status
        && !matches!(
            s.as_str(),
            "active" | "reauth_required" | "disabled" | "orphaned"
        )
    {
        return Err(ApiError::invalid_request(
            "Invalid status.",
            vec![FieldError::new("/status", "unknown status")],
        ));
    }
    let rows = repo::list_scope(
        &state.pool,
        org,
        ws,
        q.app_id.as_deref(),
        q.status.as_deref(),
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    let snap = registry::active_typed(state).await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Page {
        items: page
            .items
            .into_iter()
            .map(|r| Connection::new(r, snap.as_ref()))
            .collect(),
        next_cursor: page.next_cursor,
    })
}

pub async fn view(state: &AppState, row: ConnectionRow) -> Result<Connection, ApiError> {
    let snap = registry::active_typed(state).await?;
    Ok(Connection::new(row, snap.as_ref()))
}

pub async fn patch(
    state: &AppState,
    row: &ConnectionRow,
    req: &PatchConnectionRequest,
) -> Result<ConnectionRow, ApiError> {
    if let Some(l) = &req.label {
        let n = l.trim().chars().count();
        if n == 0 || n > LABEL_MAX {
            return Err(ApiError::invalid_request(
                "Invalid label.",
                vec![FieldError::new("/label", "must be 1-120 characters")],
            ));
        }
    }
    let status = match req.status.as_deref() {
        None => None,
        Some(s @ ("active" | "disabled")) => {
            if row.status != s && !matches!(row.status.as_str(), "active" | "disabled") {
                return Err(ApiError::conflict(
                    "invalid_state",
                    "Only active and disabled connections can be switched.",
                ));
            }
            (row.status != s).then_some(s)
        }
        Some(_) => {
            return Err(ApiError::invalid_request(
                "Invalid status.",
                vec![FieldError::new("/status", "must be active or disabled")],
            ));
        }
    };
    let label = req.label.as_deref().map(str::trim);
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    repo::update(&mut *tx, row.id, label, status).await?;
    credentials::notify_gateway_invalidate(&mut *tx, "connection", row.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    repo::get(&state.pool, row.id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connection not found."))
}

/// Delete the connection (and connections derived from it) with its blocklist and grant
/// derived rules, then invalidate gateways. Returns the number of agents affected.
pub async fn delete(state: &AppState, row: &ConnectionRow) -> Result<usize, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let ids = repo::with_children(&mut tx, row.id).await?;
    let agents = repo::granted_agents(&mut tx, &ids).await?;
    repo::delete_derived_rules(&mut tx, &ids).await?;
    let refs = repo::credential_refs(&mut tx, &ids).await?;
    repo::delete_many(&mut tx, &ids).await?;
    for id in &ids {
        credentials::notify_gateway_invalidate(&mut *tx, "connection", *id).await?;
    }
    for a in &agents {
        credentials::notify_gateway_invalidate(&mut *tx, "agent", *a).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    for (id, org, ws, r) in refs {
        if let Ok(sref) = r.parse::<SecretRef>() {
            let ctx = SealContext {
                purpose: Purpose::ConnectionCredentials,
                owner_id: id,
                org_id: org,
                workspace_id: ws,
            };
            if let Err(e) = state.vault.discard(&ctx, &sref, ValueSource::Managed).await {
                tracing::warn!(error = %e, "discarding connection credentials failed");
            }
        }
    }
    Ok(agents.len())
}
