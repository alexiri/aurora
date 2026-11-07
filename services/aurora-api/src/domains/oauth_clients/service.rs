use super::dto::*;
use crate::app::AppState;
use crate::domains::plugins::registry;
use crate::domains::secrets::service::secret_err;
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::repo::oauth_clients::{self as repo, ClientRow, NewClient};
use aurora_db::{DbError, conflict_code};
use aurora_registry::snapshot::OauthClientEntry;
use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

const VALUE_MAX: usize = 4096;

/// Where a client lives.
#[derive(Clone, Copy)]
pub struct Scope {
    pub org: Option<Uuid>,
    pub ws: Option<Uuid>,
}

fn ctx(id: Uuid, org: Option<Uuid>, ws: Option<Uuid>) -> SealContext {
    SealContext {
        purpose: Purpose::OauthClientSecret,
        owner_id: id,
        org_id: org.unwrap_or_else(Uuid::nil),
        workspace_id: ws,
    }
}

fn bad(errs: Vec<FieldError>) -> ApiError {
    ApiError::invalid_request("Invalid OAuth client.", errs)
}

async fn manifest_client(
    state: &AppState,
    package_id: &str,
    key: &str,
) -> Result<OauthClientEntry, ApiError> {
    registry::active_typed(state)
        .await?
        .and_then(|s| {
            s.oauth_clients
                .into_iter()
                .find(|c| c.package_id == package_id && c.client_key == key)
        })
        .ok_or_else(|| {
            bad(vec![FieldError::new(
                "/client_key",
                "no such OAuth client in an active package",
            )])
        })
}

/// Validate `fields` against the manifest declaration. With `partial`, missing fields are
/// allowed (PATCH); unknown names and blank values never are.
fn validate(
    decl: &OauthClientEntry,
    fields: &BTreeMap<String, String>,
    partial: bool,
) -> Result<(), ApiError> {
    let mut errs = Vec::new();
    for (k, v) in fields {
        let path = format!("/fields/{k}");
        if !decl.fields.iter().any(|f| &f.name == k) {
            errs.push(FieldError::new(path, "unknown field"));
        } else if v.trim().is_empty() {
            errs.push(FieldError::new(path, "must not be empty"));
        } else if v.len() > VALUE_MAX {
            errs.push(FieldError::new(path, "too long"));
        }
    }
    if !partial {
        for f in &decl.fields {
            if !fields.contains_key(&f.name) {
                errs.push(FieldError::new(format!("/fields/{}", f.name), "required"));
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(bad(errs))
    }
}

fn split(
    decl: &OauthClientEntry,
    fields: &BTreeMap<String, String>,
) -> (Map<String, Value>, Map<String, Value>) {
    let (mut public, mut secret) = (Map::new(), Map::new());
    for f in &decl.fields {
        if let Some(v) = fields.get(&f.name) {
            let dst = if f.secret { &mut secret } else { &mut public };
            dst.insert(f.name.clone(), Value::String(v.clone()));
        }
    }
    (public, secret)
}

async fn seal(
    state: &AppState,
    c: &SealContext,
    secret: &Map<String, Value>,
) -> Result<SecretRef, ApiError> {
    let bytes = serde_json::to_vec(secret).map_err(|_| ApiError::internal("Internal error."))?;
    state
        .vault
        .seal(c, aurora_core::Secret::new(bytes))
        .await
        .map_err(secret_err)
}

pub async fn list(
    state: &AppState,
    scope: Scope,
    q: &ListQuery,
) -> Result<crate::http::pagination::Page<OAuthClient>, ApiError> {
    use crate::http::pagination::{Page, Pagination};
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = repo::list_scope(
        &state.pool,
        scope.org,
        scope.ws,
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Page {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next_cursor,
    })
}

pub async fn create(
    state: &AppState,
    scope: Scope,
    req: CreateOAuthClientRequest,
) -> Result<ClientRow, ApiError> {
    let decl = manifest_client(state, &req.package_id, &req.client_key).await?;
    validate(&decl, &req.fields, false)?;
    let (public, secret) = split(&decl, &req.fields);
    let id = ids::new_id();
    let c = ctx(id, scope.org, scope.ws);
    let sref = seal(state, &c, &secret).await?;
    let public = Value::Object(public);
    let col = sref.to_column();
    let res = repo::insert(
        &state.pool,
        &NewClient {
            id,
            org_id: scope.org,
            workspace_id: scope.ws,
            package_id: &req.package_id,
            client_key: &req.client_key,
            fields: &public,
            secret_ref: &col,
        },
    )
    .await;
    if let Err(e) = res {
        discard(state, &c, &sref).await;
        return Err(conflict_code(e, &[(repo::SCOPE_CONSTRAINT, "conflict")]));
    }
    repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))
}

async fn discard(state: &AppState, c: &SealContext, r: &SecretRef) {
    if let Err(e) = state.vault.discard(c, r, ValueSource::Managed).await {
        tracing::warn!(error = %e, "discarding OAuth client secret failed");
    }
}

pub async fn patch(
    state: &AppState,
    row: &ClientRow,
    req: PatchOAuthClientRequest,
) -> Result<ClientRow, ApiError> {
    let mut new_public = None;
    let mut new_ref: Option<(SecretRef, String)> = None;
    let c = ctx(row.id, row.org_id, row.workspace_id);
    let old_ref: SecretRef = row
        .secret_ref
        .parse()
        .map_err(|_| ApiError::internal("Stored secret reference is unreadable."))?;
    if let Some(fields) = req.fields.filter(|f| !f.is_empty()) {
        let decl = manifest_client(state, &row.package_id, &row.client_key).await?;
        validate(&decl, &fields, true)?;
        let (public, secret) = split(&decl, &fields);
        if !public.is_empty() {
            let mut merged = row.fields.as_object().cloned().unwrap_or_default();
            merged.extend(public);
            new_public = Some(Value::Object(merged));
        }
        if !secret.is_empty() {
            let cur = state.vault.open(&c, &old_ref).await.map_err(secret_err)?;
            let mut merged: Map<String, Value> =
                serde_json::from_slice(cur.expose()).unwrap_or_default();
            merged.extend(secret);
            let r = seal(state, &c, &merged).await?;
            let col = r.to_column();
            new_ref = Some((r, col));
        }
    }
    let res = repo::update(
        &state.pool,
        row.id,
        new_public.as_ref(),
        new_ref.as_ref().map(|(_, c)| c.as_str()),
        req.enabled,
    )
    .await;
    if let Err(e) = res {
        if let Some((r, _)) = &new_ref {
            discard(state, &c, r).await;
        }
        return Err(e.into());
    }
    if new_ref.is_some() {
        discard(state, &c, &old_ref).await;
    }
    repo::get(&state.pool, row.id)
        .await?
        .ok_or_else(|| ApiError::not_found("OAuth client not found."))
}

fn in_use(count: i64) -> ApiError {
    ApiError::conflict("client_in_use", "Connections still use this OAuth client.")
        .with_details(serde_json::json!({"count": count}))
}

pub async fn delete(state: &AppState, row: &ClientRow) -> Result<(), ApiError> {
    if row.connection_count > 0 {
        return Err(in_use(row.connection_count));
    }
    match repo::delete(&state.pool, row.id).await {
        Ok(_) => {}
        Err(DbError::ForeignKeyViolation { .. }) => return Err(in_use(1)),
        Err(e) => return Err(e.into()),
    }
    if let Ok(r) = row.secret_ref.parse::<SecretRef>() {
        discard(state, &ctx(row.id, row.org_id, row.workspace_id), &r).await;
    }
    Ok(())
}
