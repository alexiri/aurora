//! SCIM Groups handlers (`16 §W15`): the org's `source = scim` groups only. Manual and
//! IdP-linked groups are invisible here (404 on their ids). Members are the SCIM user ids of
//! this org's links; a member must have an `org_members` row (the composite foreign key
//! enforces it — a group never creates a membership). `org_role` is never set from SCIM.

use super::auth::ScimToken;
use super::dto;
use super::error::{ScimError, ScimResult};
use super::service::{self, VIA};
use crate::app::AppState;
use crate::audit::{AuditEvent, Outcome};
use aurora_core::{ids, limits};
use aurora_db::repo::groups as grepo;
use aurora_db::repo::scim as repo;
use serde_json::json;
use uuid::Uuid;

fn db(e: aurora_db::DbError) -> ScimError {
    tracing::error!(error = %e, "scim: database error");
    ScimError::internal()
}

/// A repo error (already `DbError`) → `ScimError`.
fn sq(e: sqlx::Error) -> ScimError {
    tracing::error!(error = %e, "scim: database error");
    ScimError::internal()
}

fn uniqueness(e: aurora_db::DbError) -> ScimError {
    if matches!(e, aurora_db::DbError::UniqueViolation { .. }) {
        ScimError::uniqueness(
            "A resource with the same displayName or externalId already exists in this organization.",
        )
    } else {
        db(e)
    }
}

/// The SCIM group of this org, or 404 (a manual or IdP-linked group is invisible here).
async fn scim_group(
    exec: impl sqlx::PgExecutor<'_>,
    org: Uuid,
    id: Uuid,
) -> ScimResult<grepo::GroupRow> {
    grepo::get(exec, id)
        .await
        .map_err(db)?
        .filter(|g| g.org_id == org && g.source == "scim")
        .ok_or_else(ScimError::not_found)
}

/// The members of the group as SCIM user references.
async fn members_of(
    exec: impl sqlx::PgExecutor<'_> + Copy,
    org: Uuid,
    group: Uuid,
) -> ScimResult<Vec<dto::GroupMemberRef>> {
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT s.id, s.display_name FROM group_members m
         JOIN scim_users s ON s.org_id = m.org_id AND s.user_id = m.user_id
         WHERE m.org_id = $1 AND m.group_id = $2
         ORDER BY s.created_at, s.id",
    )
    .bind(org)
    .bind(group)
    .fetch_all(exec)
    .await
    .map_err(sq)?;
    Ok(rows
        .into_iter()
        .map(|(id, display)| dto::GroupMemberRef {
            value: id.to_string(),
            display,
            ref_: None,
        })
        .collect())
}

fn render(
    external_url: &str,
    row: &grepo::GroupRow,
    members: Option<Vec<dto::GroupMemberRef>>,
) -> dto::GroupResource {
    dto::GroupResource {
        schemas: [dto::GROUP_SCHEMA],
        id: row.id.to_string(),
        external_id: row.external_id.clone(),
        display_name: row.name.clone(),
        members,
        meta: dto::Meta {
            resource_type: "Group",
            created: service::rfc3339(row.created_at),
            last_modified: service::rfc3339(row.updated_at),
            location: dto::location(external_url, "Groups", row.id),
        },
    }
}

/// The SCIM user ids of `members[].value` (SCIM ids, not Aurora ids) resolved to Aurora
/// user ids of **this org's links** (an unknown id, a removed link, or another org → 400
/// `invalidValue`).
async fn resolve_members(
    exec: &sqlx::PgPool,
    org: Uuid,
    members: &[dto::MemberInput],
) -> ScimResult<Vec<Uuid>> {
    let mut out = Vec::with_capacity(members.len());
    for m in members {
        let Ok(id) = Uuid::parse_str(m.value.trim()) else {
            return Err(ScimError::invalid_value(
                "A member `value` is not a user id of this organization.",
            ));
        };
        let row = repo::user_by_id(exec, org, id)
            .await
            .map_err(db)?
            .ok_or_else(|| {
                ScimError::invalid_value("A member is not a user of this organization.")
            })?;
        if row.state == "removed" {
            return Err(ScimError::invalid_value(
                "A member's link is removed; provision the user again first.",
            ));
        }
        out.push(row.user_id);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// `GROUPS_PER_USER` (400 `invalidValue` with a detail, `02 §Limits`; SCIM adds members
/// through the same cap as the dashboard).
async fn ensure_member_limit(
    exec: impl sqlx::PgExecutor<'_>,
    org: Uuid,
    user: Uuid,
) -> ScimResult<()> {
    let in_groups = grepo::groups_of_user_count(exec, org, user)
        .await
        .map_err(db)?;
    if in_groups >= grepo::GROUPS_PER_USER {
        return Err(ScimError::invalid_value(format!(
            "A user can be in at most {} groups of this organization.",
            grepo::GROUPS_PER_USER
        )));
    }
    Ok(())
}

/// `POST /Groups`.
pub async fn create_group(
    state: &AppState,
    caller: ScimToken,
    body: serde_json::Value,
) -> ScimResult<axum::response::Response> {
    use axum::response::IntoResponse;
    let input: dto::GroupInput = serde_json::from_value(body.clone())
        .map_err(|e| ScimError::invalid_syntax(e.to_string()))?;
    if let Some(a) = service::refused_attr(&body, service::GROUP_REFUSED_ATTRS) {
        return Err(ScimError::invalid_value(format!("`{a}` is not supported.")));
    }
    let org = caller.org.clone();
    let name = service::clean_name(input.display_name.as_deref(), 80)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ScimError::invalid_value("`displayName` is required."))?;
    let external_id = service::clean_external_id(input.external_id.as_deref())?;
    let members_in = input.members.clone().unwrap_or_default();
    if members_in.len() > limits::SCIM_MAX_MEMBERS_PER_REQUEST {
        return Err(ScimError::too_many(format!(
            "At most {} members per request.",
            limits::SCIM_MAX_MEMBERS_PER_REQUEST
        )));
    }
    let mut tx = state.pool.begin().await.map_err(sq)?;
    // Serialize creation per org so `GROUPS_PER_ORG` holds under concurrency.
    grepo::lock_org(&mut tx, org.org_id).await.map_err(db)?;
    if grepo::count_in_org(&mut *tx, org.org_id)
        .await
        .map_err(db)?
        >= grepo::GROUPS_PER_ORG
    {
        return Err(ScimError::too_many(
            "This organization has reached its group limit.",
        ));
    }
    let user_ids = resolve_members(&state.pool, org.org_id, &members_in).await?;
    for uid in &user_ids {
        ensure_member_limit(&mut *tx, org.org_id, *uid).await?;
    }
    let id = ids::new_id();
    grepo::insert(
        &mut tx,
        id,
        org.org_id,
        &name,
        None,
        "scim",
        None,
        external_id.as_deref(),
        None,
        None,
    )
    .await
    .map_err(uniqueness)?;
    let mut evs = Vec::new();
    for uid in &user_ids {
        ensure_member_limit(&mut *tx, org.org_id, *uid).await?;
        grepo::add_member(&mut tx, id, org.org_id, *uid, "scim", None)
            .await
            .map_err(db)?;
        grepo::notify_member_changed(&mut tx, org.org_id, *uid)
            .await
            .map_err(db)?;
        evs.push(
            AuditEvent::new(
                service::actor(&caller.token.name),
                "group_member.add",
                Outcome::Success,
            )
            .org(org.org_id)
            .target("group", id)
            .metadata(json!({"via": VIA, "user": uid})),
        );
    }
    crate::audit::commit_critical_many(tx, &evs)
        .await
        .map_err(|_| ScimError::audit_unavailable())?;
    let row = scim_group(&state.pool, org.org_id, id).await?;
    let ms = members_of(&state.pool, org.org_id, id).await?;
    let mut res = (
        axum::http::StatusCode::CREATED,
        axum::Json(render(&state.config.external_url, &row, Some(ms))),
    )
        .into_response();
    if let Ok(v) = dto::location(&state.config.external_url, "Groups", id)
        .parse::<axum::http::header::HeaderValue>()
    {
        res.headers_mut().insert(axum::http::header::LOCATION, v);
    }
    Ok(res)
}

/// `GET /Groups/{id}`.
pub async fn get_group(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
) -> ScimResult<axum::Json<dto::GroupResource>> {
    let row = scim_group(&state.pool, caller.org.org_id, id).await?;
    let ms = members_of(&state.pool, caller.org.org_id, id).await?;
    Ok(axum::Json(render(
        &state.config.external_url,
        &row,
        Some(ms),
    )))
}

/// `GET /Groups?filter=&startIndex=&count=&attributes=`. `attributes=members` adds the
/// member lists (omitted otherwise, `16 §W15`).
pub async fn list_groups(
    state: &AppState,
    headers: &http::HeaderMap,
    query: Option<&str>,
) -> ScimResult<axum::Json<dto::ListResponse>> {
    let caller = ScimToken::authenticate(state, headers).await?;
    let q = super::resource_users::parse_query(query)?;
    let (start, count) = service::page_params(q.start_index, q.count)?;
    let filter = service::parse_filter(q.filter.as_deref().unwrap_or(""))?;
    let want_members = q.attributes.as_deref().is_some_and(|a| {
        a.split(',')
            .any(|s| s.trim().eq_ignore_ascii_case("members"))
    });
    let rows: Vec<grepo::GroupRow> = match &filter {
        None => repo::scim_groups(&state.pool, caller.org.org_id, None, 10_000)
            .await
            .map_err(db)?,
        Some(service::Filter::DisplayName(v)) => {
            repo::scim_groups(&state.pool, caller.org.org_id, None, 10_000)
                .await
                .map_err(db)?
                .into_iter()
                .filter(|r| r.name.eq_ignore_ascii_case(v))
                .collect()
        }
        Some(service::Filter::ExternalId(v)) => {
            repo::scim_groups(&state.pool, caller.org.org_id, None, 10_000)
                .await
                .map_err(db)?
                .into_iter()
                .filter(|r| r.external_id.as_deref() == Some(v.as_str()))
                .collect()
        }
        Some(service::Filter::Id(id)) => {
            match scim_group(&state.pool, caller.org.org_id, *id).await {
                Ok(r) => vec![r],
                Err(e) if e.status == axum::http::StatusCode::NOT_FOUND => vec![],
                Err(e) => return Err(e),
            }
        }
        Some(_) => {
            return Err(ScimError::invalid_filter(
                "Groups support displayName, externalId and id.",
            ));
        }
    };
    let total = rows.len() as u64;
    let skip = usize::try_from(start - 1).unwrap_or(usize::MAX);
    let items: Vec<&grepo::GroupRow> = rows.iter().skip(skip).take(count).collect();
    let mut resources = Vec::with_capacity(items.len());
    for r in items {
        let ms = if want_members {
            Some(members_of(&state.pool, caller.org.org_id, r.id).await?)
        } else {
            None
        };
        resources.push(
            serde_json::to_value(render(&state.config.external_url, r, ms))
                .unwrap_or_else(|_| json!({})),
        );
    }
    Ok(axum::Json(dto::ListResponse {
        schemas: [dto::LIST_RESPONSE_SCHEMA],
        total_results: total,
        start_index: start,
        items_per_page: resources.len() as u64,
        resources,
    }))
}

/// One audit event per member change (`group_member.add|remove`, critical).
fn member_evs(
    caller: &ScimToken,
    group: Uuid,
    added: &[Uuid],
    removed: &[Uuid],
) -> Vec<AuditEvent> {
    let mut evs = Vec::new();
    for u in added {
        evs.push(
            AuditEvent::new(
                service::actor(&caller.token.name),
                "group_member.add",
                Outcome::Success,
            )
            .org(caller.org.org_id)
            .target("group", group)
            .metadata(json!({"via": VIA, "user": u})),
        );
    }
    for u in removed {
        evs.push(
            AuditEvent::new(
                service::actor(&caller.token.name),
                "group_member.remove",
                Outcome::Success,
            )
            .org(caller.org.org_id)
            .target("group", group)
            .metadata(json!({"via": VIA, "user": u})),
        );
    }
    evs
}

/// The shared body of `PUT` and the folded `PATCH` operations: rename and/or replace
/// members, atomically, one critical audit row per change plus `group.update` when the
/// name/externalId actually changed (C-478: a no-op — the request asserts the stored
/// values — writes nothing, no `revision` bump and no audit row).
async fn apply_group_change(
    state: &AppState,
    caller: &ScimToken,
    id: Uuid,
    name: Option<String>,
    external_id: Option<Option<String>>,
    members_in: Option<Vec<dto::MemberInput>>,
) -> ScimResult<dto::GroupResource> {
    if let Some(n) = &name {
        if n.is_empty() {
            return Err(ScimError::invalid_value("`displayName` cannot be empty."));
        }
        if n.chars().count() > 80 {
            return Err(ScimError::invalid_value(
                "`displayName` must be at most 80 characters.",
            ));
        }
    }
    if let Some(ms) = &members_in
        && ms.len() > limits::SCIM_MAX_MEMBERS_PER_REQUEST
    {
        return Err(ScimError::too_many(format!(
            "At most {} members per request.",
            limits::SCIM_MAX_MEMBERS_PER_REQUEST
        )));
    }
    let org = caller.org.clone();
    let mut tx = state.pool.begin().await.map_err(sq)?;
    let row = grepo::lock(&mut tx, id)
        .await
        .map_err(db)?
        .ok_or_else(ScimError::not_found)?;
    if row.org_id != org.org_id || row.source != "scim" {
        return Err(ScimError::not_found());
    }
    if let Some(Some(ext)) = &external_id {
        if ext.chars().count() > 256 {
            return Err(ScimError::invalid_value(
                "externalId must be at most 256 characters.",
            ));
        }
        if grepo::existing_in_org_external(&mut *tx, org.org_id, id, ext)
            .await
            .map_err(db)?
            .is_some()
        {
            return Err(ScimError::uniqueness(
                "A group with this externalId already exists in this organization.",
            ));
        }
    }
    if let Some(n) = &name
        && grepo::existing_in_org_name(&mut *tx, org.org_id, id, n)
            .await
            .map_err(db)?
            .is_some()
    {
        return Err(ScimError::uniqueness(
            "A group with this name already exists in this organization.",
        ));
    }
    let before: Vec<Uuid> = grepo::member_ids(&mut *tx, id).await.map_err(db)?;
    let target: Vec<Uuid> = match &members_in {
        None => before.clone(),
        Some(ms) => resolve_members(&state.pool, org.org_id, ms).await?,
    };
    for uid in &target {
        if !before.contains(uid) {
            ensure_member_limit(&mut *tx, org.org_id, *uid).await?;
        }
    }
    let renamed = match &name {
        // A no-op rename (the request asserts the stored name) must not bump
        // `revision` or churn `updated_at` (C-478).
        Some(n) if *n == row.name => false,
        Some(n) => {
            grepo::rename(&mut tx, id, n).await.map_err(db)?;
            true
        }
        None => false,
    };
    let external_changed = match &external_id {
        // `Some(None)` (an explicit clear) equals a stored NULL; the stored value is
        // compared so a repeated `externalId` writes nothing.
        Some(ext) => ext.as_deref() != row.external_id.as_deref(),
        None => false,
    };
    if let Some(ext) = external_id
        && external_changed
    {
        grepo::set_external_id(&mut tx, id, ext.as_deref())
            .await
            .map_err(db)?;
    }
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for u in &target {
        if !before.contains(u) {
            grepo::add_member(&mut tx, id, org.org_id, *u, "scim", None)
                .await
                .map_err(db)?;
            added.push(*u);
        }
    }
    for u in &before {
        if !target.contains(u) {
            grepo::remove_member(&mut tx, id, *u).await.map_err(db)?;
            removed.push(*u);
        }
    }
    for u in added.iter().chain(removed.iter()) {
        grepo::notify_member_changed(&mut tx, org.org_id, *u)
            .await
            .map_err(db)?;
    }
    let mut evs = member_evs(caller, id, &added, &removed);
    if renamed || external_changed {
        evs.insert(
            0,
            AuditEvent::new(
                service::actor(&caller.token.name),
                "group.update",
                Outcome::Success,
            )
            .org(org.org_id)
            .target("group", id)
            .metadata(json!({
                "via": VIA,
                "rename": renamed,
                "external_id": external_changed,
            })),
        );
    }
    crate::audit::commit_critical_many(tx, &evs)
        .await
        .map_err(|_| ScimError::audit_unavailable())?;
    let row = scim_group(&state.pool, org.org_id, id).await?;
    let ms = members_of(&state.pool, org.org_id, id).await?;
    Ok(render(&state.config.external_url, &row, Some(ms)))
}

/// `PUT /Groups/{id}`: replaces the name and the whole member list.
pub async fn put_group(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
    body: serde_json::Value,
) -> ScimResult<axum::Json<dto::GroupResource>> {
    let input: dto::GroupInput = serde_json::from_value(body.clone())
        .map_err(|e| ScimError::invalid_syntax(e.to_string()))?;
    if let Some(a) = service::refused_attr(&body, service::GROUP_REFUSED_ATTRS) {
        return Err(ScimError::invalid_value(format!("`{a}` is not supported.")));
    }
    let name = service::clean_name(input.display_name.as_deref(), 80)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ScimError::invalid_value("`displayName` is required."))?;
    let ext = service::clean_external_id(input.external_id.as_deref())?;
    let out = apply_group_change(
        state,
        &caller,
        id,
        Some(name),
        Some(ext),
        input.members.clone(),
    )
    .await?;
    Ok(axum::Json(out))
}

/// `PATCH /Groups/{id}`: the operations subset — `displayName`, `externalId`, `members`
/// (add/replace/remove of the list), `members[value eq "<id>"]` for `remove`, or no path
/// with an object value. Folded into one change; atomic (`16 §W15`).
pub async fn patch_group(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
    body: serde_json::Value,
) -> ScimResult<axum::Json<dto::GroupResource>> {
    let patch: dto::PatchInput = serde_json::from_value(body.clone())
        .map_err(|e| ScimError::invalid_syntax(e.to_string()))?;
    if patch.schemas.iter().all(|s| s != dto::PATCH_OP_SCHEMA) && !patch.schemas.is_empty() {
        return Err(ScimError::invalid_value("Unknown `schemas` value."));
    }
    if patch.operations.len() > limits::SCIM_MAX_PATCH_OPS {
        return Err(ScimError::too_many(format!(
            "A PATCH carries at most {} operations.",
            limits::SCIM_MAX_PATCH_OPS
        )));
    }
    let current = scim_group(&state.pool, caller.org.org_id, id).await?;
    let mut name = current.name.clone();
    let mut ext = current.external_id.clone();
    let mut members: Vec<dto::MemberInput> = members_of(&state.pool, caller.org.org_id, id)
        .await?
        .into_iter()
        .map(|m| dto::MemberInput {
            value: m.value,
            type_: None,
            display: None,
        })
        .collect();
    let mut saw_members_op = false;
    for op in &patch.operations {
        let kind = op.op.trim().to_ascii_lowercase();
        if !matches!(kind.as_str(), "add" | "replace" | "remove") {
            return Err(ScimError::invalid_value(
                "`op` must be add, replace or remove.",
            ));
        }
        let path = op.path.as_deref().unwrap_or("").trim();
        let lower = path.to_ascii_lowercase();
        if path.is_empty() {
            let Some(v) = &op.value else {
                return Err(ScimError::invalid_value(
                    "An operation without a path carries the value object.",
                ));
            };
            let Some(obj) = v.as_object() else {
                return Err(ScimError::invalid_value(
                    "The value of an operation without a path must be an object.",
                ));
            };
            if let Some(a) = service::refused_attr(v, service::GROUP_REFUSED_ATTRS) {
                return Err(ScimError::invalid_value(format!("`{a}` is not supported.")));
            }
            if let Some(v) = obj.get("displayName") {
                name = v
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| ScimError::invalid_value("`displayName` must be a string."))?;
            }
            if let Some(v) = obj.get("externalId") {
                ext = v.as_str().map(str::to_owned);
            }
            if let Some(v) = obj.get("members") {
                saw_members_op = true;
                members = fold_members(members, v, &kind)?;
            }
            continue;
        }
        if lower == "displayname" {
            if kind == "remove" {
                return Err(ScimError::no_target("`displayName` cannot be removed."));
            }
            let Some(v) = &op.value else {
                return Err(ScimError::invalid_value("The operation carries no value."));
            };
            name = v
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| ScimError::invalid_value("`displayName` must be a string."))?;
            continue;
        }
        if lower == "externalid" {
            if kind == "remove" {
                ext = None;
            } else {
                ext = op
                    .value
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
            }
            continue;
        }
        if lower == "members" {
            saw_members_op = true;
            let Some(v) = &op.value else {
                return Err(ScimError::invalid_value("The operation carries no value."));
            };
            members = fold_members(members, v, &kind)?;
            continue;
        }
        if lower.starts_with("members[value") && lower.ends_with(']') {
            // `members[value eq "<id>"]` — supported for `remove` only.
            if kind != "remove" {
                return Err(ScimError::invalid_path(
                    "`members[value eq …]` supports `remove` only; replace `members` instead.",
                ));
            }
            saw_members_op = true;
            let id_str = path
                .split('"')
                .nth(1)
                .ok_or_else(|| ScimError::invalid_path("Malformed `members[value eq …]` path."))?;
            members.retain(|m| m.value != id_str);
            continue;
        }
        return Err(ScimError::invalid_path(format!(
            "The path `{path}` is not supported."
        )));
    }
    let out = apply_group_change(
        state,
        &caller,
        id,
        Some(name),
        Some(ext),
        saw_members_op.then_some(members),
    )
    .await?;
    Ok(axum::Json(out))
}

/// Fold a `members` value (`add` appends, `replace` overwrites, `remove` deletes).
fn fold_members(
    current: Vec<dto::MemberInput>,
    v: &serde_json::Value,
    kind: &str,
) -> ScimResult<Vec<dto::MemberInput>> {
    let list = v
        .as_array()
        .ok_or_else(|| ScimError::invalid_value("`members` must be a list."))?;
    let parsed: Vec<dto::MemberInput> = list
        .iter()
        .map(|m| serde_json::from_value(m.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ScimError::invalid_value(format!("`members`: {e}")))?;
    match kind {
        "replace" => Ok(parsed),
        "add" => {
            let mut out = current;
            for m in parsed {
                if !out.iter().any(|e| e.value == m.value) {
                    out.push(m);
                }
            }
            Ok(out)
        }
        // remove
        _ => {
            let mut out = current;
            for m in parsed {
                out.retain(|e| e.value != m.value);
            }
            Ok(out)
        }
    }
}

/// `DELETE /Groups/{id}`: deletes the group with its bindings and membership rows
/// (idempotent: unknown, foreign or already deleted → 204 again).
pub async fn delete_group(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
) -> ScimResult<axum::http::StatusCode> {
    use axum::http::StatusCode;
    let org = caller.org.clone();
    let mut tx = state.pool.begin().await.map_err(sq)?;
    let Some(row) = grepo::lock(&mut tx, id).await.map_err(db)? else {
        return Ok(StatusCode::NO_CONTENT);
    };
    if row.org_id != org.org_id || row.source != "scim" {
        return Ok(StatusCode::NO_CONTENT);
    }
    // The workspaces the bindings touched, for the invalidation.
    let wss: Vec<Uuid> = sqlx::query_scalar(
        "SELECT workspace_id FROM group_workspace_access WHERE group_id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org.org_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(sq)?;
    grepo::delete(&mut tx, id).await.map_err(db)?;
    grepo::notify_workspaces_changed(&mut tx, org.org_id, &wss)
        .await
        .map_err(db)?;
    let ev = AuditEvent::new(
        service::actor(&caller.token.name),
        "group.delete",
        Outcome::Success,
    )
    .org(org.org_id)
    .target("group", id)
    .metadata(json!({"via": VIA, "bindings": wss.len()}));
    crate::audit::commit_critical(tx, &ev)
        .await
        .map_err(|_| ScimError::audit_unavailable())?;
    Ok(StatusCode::NO_CONTENT)
}
