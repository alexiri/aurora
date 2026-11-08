use super::dto::*;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::plugins::{ingest, registry};
use crate::http::pagination::{Page, Pagination};
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::repo::app_instances::{self as repo, InstanceRow, NewInstance};
use aurora_db::repo::credentials;
use aurora_db::{DbError, conflict_code};
use aurora_policy::HostPattern;
use aurora_registry::claims::hosts_overlap;
use aurora_registry::manifest::{InstanceProbe, ProbeMethod};
use aurora_registry::snapshot::Snapshot;
use aurora_registry::validate::plain_dns_name;
use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
use serde_json::{Value, json};
use uuid::Uuid;

/// 422 with `code` and `details.fields[0] = { path, message }` (C-241).
fn field_error(code: &str, path: &str, message: &str, extra: Option<Value>) -> ApiError {
    let mut details = json!({ "fields": [FieldError::new(path, message)] });
    if let (Some(Value::Object(x)), Some(d)) = (extra, details.as_object_mut()) {
        d.extend(x);
    }
    ApiError::unprocessable(code, message).with_details(details)
}

fn bad(path: &str, message: &str) -> ApiError {
    ApiError::invalid_request(
        "Invalid app instance.",
        vec![FieldError::new(path, message)],
    )
}

fn clean_label(s: &str) -> Result<String, ApiError> {
    let t: String = s.chars().filter(|c| !c.is_control()).collect();
    let t = t.trim().to_owned();
    if t.is_empty() || t.chars().count() > 120 {
        return Err(bad("/label", "label must be 1 to 120 characters"));
    }
    Ok(t)
}

/// A host an admin may register: a lower-case DNS name with at least two labels, never an IP
/// literal, port, path or userinfo, and not under `aurora.internal` (09 §3.4).
pub fn check_host_shape(host: &str) -> Result<(), ApiError> {
    let internal = host == "aurora.internal" || host.ends_with(".aurora.internal");
    if !plain_dns_name(host) || internal {
        return Err(field_error(
            "instance_host_invalid",
            "/host",
            "The host must be a lower-case DNS name without port, path, userinfo or IP literal.",
            None,
        ));
    }
    Ok(())
}

/// The conflict an instance host would create with the active static claims or a governed LLM host
/// (a claim overlaps when some request host matches both, ignoring ports).
fn static_conflict(state: &AppState, snap: &Snapshot, host: &str) -> Option<ApiError> {
    let conflict = |kind: &str, app_id: Option<&str>| {
        let c = match app_id {
            Some(a) => json!({ "kind": kind, "app_id": a }),
            None => json!({ "kind": kind }),
        };
        field_error(
            "instance_host_conflict",
            "/host",
            "The host is already claimed.",
            Some(json!({ "conflict": c })),
        )
    };
    if let Some(e) = snap
        .hosts
        .iter()
        .find(|e| hosts_overlap(&e.host, host) || HostPattern::lenient(&e.host).matches(host, 443))
    {
        return Some(conflict("static_claim", Some(&e.app_id)));
    }
    let llm = state.llm.is_endpoint_host(host)
        || snap.reserved_hosts.iter().any(|p| hosts_overlap(p, host))
        || ingest::reserved_names(state)
            .iter()
            .any(|p| hosts_overlap(p, host));
    llm.then(|| conflict("llm_host", None))
}

/// Run the manifest's probe against the host through the destination guard.
async fn run_probe(state: &AppState, host: &str, probe: &InstanceProbe) -> Result<(), ApiError> {
    let fail = |reason: &str, status: Option<u16>| {
        field_error(
            "instance_probe_failed",
            "/host",
            "The instance did not answer the probe as expected.",
            Some(json!({ "status": status, "reason": reason })),
        )
    };
    let env = &state.connect;
    let raw = format!("https://{host}{}", probe.path);
    let url = env
        .prepare(&raw)
        .map_err(|_| fail("destination_refused", None))?;
    let method = match probe.method {
        ProbeMethod::Get => reqwest::Method::GET,
        ProbeMethod::Head => reqwest::Method::HEAD,
        ProbeMethod::Post => reqwest::Method::POST,
    };
    let resp = env
        .http_instance
        .request(method, url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            let refused = format!("{e:?}").contains("destination refused");
            fail(
                if refused {
                    "destination_refused"
                } else {
                    "unreachable"
                },
                None,
            )
        })?;
    let status = resp.status().as_u16();
    if probe.expect_status.contains(&status) {
        Ok(())
    } else {
        Err(fail("unexpected_status", Some(status)))
    }
}

fn invalidate_scope(org: Option<Uuid>) -> (&'static str, Uuid) {
    match org {
        Some(o) => ("org", o),
        None => ("all", Uuid::nil()),
    }
}

fn event(actor: &AuditActor, action: &str, row: &InstanceRow, meta: Value) -> AuditEvent {
    let mut ev = AuditEvent::new(actor.clone(), action, Outcome::Success)
        .target("app_instance", row.id)
        .metadata(meta);
    if let Some(o) = row.org_id {
        ev = ev.org(o);
    }
    ev
}

pub async fn list_for_org(
    state: &AppState,
    org: Uuid,
    q: &ListQuery,
) -> Result<Page<AppInstance>, ApiError> {
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = repo::list_for_org(
        &state.pool,
        org,
        q.app_id.as_deref(),
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    Ok(map_page(Page::from_rows(rows, p.limit, |r| {
        (r.created_at, r.id)
    })))
}

pub async fn list_instance_wide(
    state: &AppState,
    q: &ListQuery,
) -> Result<Page<AppInstance>, ApiError> {
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = repo::list_instance_wide(
        &state.pool,
        q.app_id.as_deref(),
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    Ok(map_page(Page::from_rows(rows, p.limit, |r| {
        (r.created_at, r.id)
    })))
}

fn map_page(page: Page<InstanceRow>) -> Page<AppInstance> {
    Page {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next_cursor,
    }
}

/// Register an instance (`org = None`: instance-wide). The caller has authorized the scope.
pub async fn create(
    state: &AppState,
    actor: &AuditActor,
    org: Option<Uuid>,
    req: CreateAppInstanceRequest,
) -> Result<InstanceRow, ApiError> {
    let label = clean_label(&req.label)?;
    let snap = registry::active_typed(state)
        .await?
        .ok_or_else(|| field_error("app_not_instance_bound", "/app_id", "Unknown app.", None))?;
    let app = snap
        .app(&req.app_id)
        .filter(|a| a.instances.is_some())
        .ok_or_else(|| {
            field_error(
                "app_not_instance_bound",
                "/app_id",
                "The app is unknown, inactive or does not support self-hosted instances.",
                None,
            )
        })?;
    check_host_shape(&req.host)?;
    if let Some(e) = static_conflict(state, &snap, &req.host) {
        return Err(e);
    }
    let instance_conflict = |app_id: String| {
        field_error(
            "instance_host_conflict",
            "/host",
            "The host is already registered as an app instance.",
            Some(json!({ "conflict": { "kind": "instance", "app_id": app_id } })),
        )
    };
    if let Some((a, _)) = repo::visible_with_host(&state.pool, org, &req.host).await? {
        return Err(instance_conflict(a));
    }
    if let Some(p) = app.instances.as_ref().and_then(|i| i.probe.as_ref()) {
        run_probe(state, &req.host, p).await?;
    }
    let id = ids::new_id();
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    repo::lock_host(&mut tx, &req.host).await?;
    if let Some((a, _)) = repo::visible_with_host(&mut *tx, org, &req.host).await? {
        return Err(instance_conflict(a));
    }
    let res = repo::insert(
        &mut *tx,
        &NewInstance {
            id,
            org_id: org,
            package_id: &app.package_id,
            app_id: &app.app_id,
            host: &req.host,
            label: &label,
            created_by: actor.id,
        },
    )
    .await;
    if let Err(e) = res {
        return Err(conflict_code(e, &[("app_instances_scope_uq", "conflict")]));
    }
    let row = repo::get(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    audit::insert_tx(
        &mut tx,
        &event(
            actor,
            "app_instance.create",
            &row,
            json!({"app_id": row.app_id, "package_id": row.package_id, "host": row.host, "label": row.label}),
        ),
    )
    .await?;
    let (scope, sid) = invalidate_scope(org);
    credentials::notify_gateway_invalidate(&mut *tx, scope, sid).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(row)
}

pub async fn patch(
    state: &AppState,
    actor: &AuditActor,
    row: &InstanceRow,
    req: PatchAppInstanceRequest,
) -> Result<InstanceRow, ApiError> {
    let label = req.label.as_deref().map(clean_label).transpose()?;
    if let Some(s) = req.status.as_deref()
        && !["active", "disabled"].contains(&s)
    {
        return Err(bad("/status", "status must be active or disabled"));
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    repo::update(&mut *tx, row.id, label.as_deref(), req.status.as_deref()).await?;
    let updated = repo::get(&mut *tx, row.id)
        .await?
        .ok_or_else(|| ApiError::not_found("App instance not found."))?;
    audit::insert_tx(
        &mut tx,
        &event(
            actor,
            "app_instance.update",
            &updated,
            json!({"app_id": updated.app_id, "host": updated.host,
                   "change": {"label": label.as_ref().map(|_| json!([row.label, updated.label])),
                              "status": req.status.as_ref().map(|_| json!([row.status, updated.status]))}}),
        ),
    )
    .await?;
    let (scope, sid) = invalidate_scope(updated.org_id);
    credentials::notify_gateway_invalidate(&mut *tx, scope, sid).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(updated)
}

/// Delete (C-242): one transaction disables the connections, deletes the BYOC clients (cascade) and
/// the row, and audits.
pub async fn delete(
    state: &AppState,
    actor: &AuditActor,
    row: &InstanceRow,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    repo::lock_host(&mut tx, &row.host).await?;
    let clients = repo::client_secret_refs(&mut tx, row.id).await?;
    let disabled = repo::disable_connections(&mut tx, row.id).await?;
    if !repo::delete(&mut tx, row.id).await? {
        return Err(ApiError::not_found("App instance not found."));
    }
    audit::insert_tx(
        &mut tx,
        &event(
            actor,
            "app_instance.delete",
            row,
            json!({"app_id": row.app_id, "host": row.host,
                   "connections_disabled": disabled.len(), "oauth_clients_deleted": clients.len()}),
        ),
    )
    .await?;
    let (scope, sid) = invalidate_scope(row.org_id);
    credentials::notify_gateway_invalidate(&mut *tx, scope, sid).await?;
    // The connections' own caches too (their status changed).
    if let Some(c) = disabled.first() {
        credentials::notify_gateway_invalidate(&mut *tx, "connection", *c).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    for (cid, org, ws, sref) in clients {
        if let Ok(r) = sref.parse::<SecretRef>() {
            let ctx = SealContext {
                purpose: Purpose::OauthClientSecret,
                owner_id: cid,
                org_id: org.unwrap_or_else(Uuid::nil),
                workspace_id: ws,
            };
            if let Err(e) = state.vault.discard(&ctx, &r, ValueSource::Managed).await {
                tracing::warn!(error = ?e, "discarding instance OAuth client secret failed");
            }
        }
    }
    Ok(())
}
