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
/// `invalidValue`). Runs inside the writing transaction.
async fn resolve_members(
    conn: &mut sqlx::PgConnection,
    org: Uuid,
    members: &[String],
) -> ScimResult<Vec<Uuid>> {
    let mut out = Vec::with_capacity(members.len());
    for m in members {
        let Ok(id) = Uuid::parse_str(m.trim()) else {
            return Err(ScimError::invalid_value(
                "A member `value` is not a user id of this organization.",
            ));
        };
        let row = repo::user_by_id(&mut *conn, org, id)
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
/// through the same cap as the dashboard). The count is only a cap under concurrency when
/// the caller holds the org lock ([`grepo::lock_org`], `organizations` row `FOR UPDATE`),
/// taken as the transaction's first lock, before any group row lock (FIX-062): every SCIM
/// write that adds group members takes it, so two requests adding one user to different
/// groups cannot both pass the count.
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
    let values: Vec<String> = members_in.iter().map(|m| m.value.clone()).collect();
    let user_ids = resolve_members(&mut tx, org.org_id, &values).await?;
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
    // The list, `displayName` and `externalId` pages in SQL (a count and an `OFFSET`/`LIMIT`
    // window: no bound on the number of groups, FIX-081); `id` is one lookup.
    let org = caller.org.org_id;
    let skip = i64::try_from(start - 1).unwrap_or(i64::MAX);
    let limit = i64::try_from(count).unwrap_or(i64::MAX);
    let (total, rows): (u64, Vec<grepo::GroupRow>) = match &filter {
        None | Some(service::Filter::DisplayName(_)) | Some(service::Filter::ExternalId(_)) => {
            let (name, ext) = match &filter {
                Some(service::Filter::DisplayName(v)) => (Some(v.as_str()), None),
                Some(service::Filter::ExternalId(v)) => (None, Some(v.as_str())),
                _ => (None, None),
            };
            let total = repo::scim_groups_count(&state.pool, org, name, ext)
                .await
                .map_err(db)?;
            let rows = repo::scim_groups_window(&state.pool, org, name, ext, skip, limit)
                .await
                .map_err(db)?;
            (u64::try_from(total).unwrap_or(0), rows)
        }
        Some(service::Filter::Id(id)) => {
            let found = match scim_group(&state.pool, org, *id).await {
                Ok(r) => vec![r],
                Err(e) if e.status == axum::http::StatusCode::NOT_FOUND => vec![],
                Err(e) => return Err(e),
            };
            let total = found.len() as u64;
            let skip = usize::try_from(skip).unwrap_or(usize::MAX);
            (total, found.into_iter().skip(skip).take(count).collect())
        }
        Some(_) => {
            return Err(ScimError::invalid_filter(
                "Groups support displayName, externalId and id.",
            ));
        }
    };
    let mut resources = Vec::with_capacity(rows.len());
    for r in &rows {
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

/// One member operation of a `PUT` (one `Replace`) or a `PATCH` (in request order), as SCIM
/// user ids. Folded over the members read **inside** the writing transaction after the group
/// row lock, so concurrent PATCHes compose instead of overwriting each other (FIX-062).
#[derive(Debug, Clone, PartialEq, Eq)]
enum MemberOp {
    Replace(Vec<String>),
    Add(Vec<String>),
    Remove(Vec<String>),
}

impl MemberOp {
    fn values(&self) -> &[String] {
        match self {
            Self::Replace(v) | Self::Add(v) | Self::Remove(v) => v,
        }
    }
}

/// A SCIM id as compared while folding (a UUID's canonical form; anything else verbatim —
/// it is refused by [`resolve_members`] if it survives the fold).
fn norm_id(v: &str) -> String {
    let v = v.trim();
    Uuid::parse_str(v).map_or_else(|_| v.to_owned(), |u| u.to_string())
}

/// Apply the operations in order over `current` (SCIM ids).
fn fold_ops(current: Vec<String>, ops: &[MemberOp]) -> Vec<String> {
    let mut out = current;
    for op in ops {
        match op {
            MemberOp::Replace(v) => {
                out = Vec::with_capacity(v.len());
                for m in v {
                    let m = norm_id(m);
                    if !out.contains(&m) {
                        out.push(m);
                    }
                }
            }
            MemberOp::Add(v) => {
                for m in v {
                    let m = norm_id(m);
                    if !out.contains(&m) {
                        out.push(m);
                    }
                }
            }
            MemberOp::Remove(v) => {
                let gone: Vec<String> = v.iter().map(|m| norm_id(m)).collect();
                out.retain(|e| !gone.contains(e));
            }
        }
    }
    out
}

fn group_full() -> ScimError {
    ScimError::invalid_value(format!(
        "A group can have at most {} members.",
        grepo::GROUP_MEMBERS_MAX
    ))
}

/// The shared body of `PUT` and the folded `PATCH` operations: rename and/or change
/// members, atomically, one critical audit row per change plus `group.update` when the
/// name/externalId actually changed (C-478: a no-op — the request asserts the stored
/// values — writes nothing, no `revision` bump and no audit row). `None` leaves that part
/// as stored (read under the lock, never from a pre-transaction snapshot).
async fn apply_group_change(
    state: &AppState,
    caller: &ScimToken,
    id: Uuid,
    name: Option<String>,
    external_id: Option<Option<String>>,
    member_ops: Option<Vec<MemberOp>>,
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
    if let Some(ops) = &member_ops
        && ops.iter().map(|o| o.values().len()).sum::<usize>()
            > limits::SCIM_MAX_MEMBERS_PER_REQUEST
    {
        return Err(ScimError::too_many(format!(
            "At most {} members per request.",
            limits::SCIM_MAX_MEMBERS_PER_REQUEST
        )));
    }
    let org = caller.org.clone();
    let mut tx = state.pool.begin().await.map_err(sq)?;
    // Lock order: the org first (the `GROUPS_PER_USER` cap, see `ensure_member_limit`; and, for
    // every write, the sign-in group sync that inserts members under the same lock, FIX-083),
    // then the group row.
    grepo::lock_org(&mut tx, org.org_id).await.map_err(db)?;
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
    // Current members, read under the group row lock: a concurrent change has committed
    // (or is waiting behind us), never half-seen.
    let before: Vec<Uuid> = grepo::member_ids(&mut *tx, id).await.map_err(db)?;
    let target: Vec<Uuid> = match &member_ops {
        None => before.clone(),
        Some(ops) => {
            let current: Vec<String> = sqlx::query_scalar(
                "SELECT s.id::text FROM group_members m
                 JOIN scim_users s ON s.org_id = m.org_id AND s.user_id = m.user_id
                 WHERE m.org_id = $1 AND m.group_id = $2
                 ORDER BY s.created_at, s.id",
            )
            .bind(org.org_id)
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(sq)?;
            let folded = fold_ops(current, ops);
            if folded.len() as i64 > grepo::GROUP_MEMBERS_MAX {
                return Err(group_full());
            }
            resolve_members(&mut tx, org.org_id, &folded).await?
        }
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
        input.members.as_ref().map(|ms| {
            vec![MemberOp::Replace(
                ms.iter().map(|m| m.value.clone()).collect(),
            )]
        }),
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
    // 404 before parsing for an unknown/foreign group (the authoritative check is repeated
    // under the lock). Nothing of the group's current state is read here: the operations are
    // folded inside the writing transaction (FIX-062 — a pre-transaction snapshot of the
    // members made two concurrent `add`s overwrite each other).
    scim_group(&state.pool, caller.org.org_id, id).await?;
    let mut name: Option<String> = None;
    let mut ext: Option<Option<String>> = None;
    let mut member_ops: Vec<MemberOp> = Vec::new();
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
                name =
                    Some(v.as_str().map(str::to_owned).ok_or_else(|| {
                        ScimError::invalid_value("`displayName` must be a string.")
                    })?);
            }
            if let Some(v) = obj.get("externalId") {
                ext = Some(v.as_str().map(str::to_owned));
            }
            if let Some(v) = obj.get("members") {
                saw_members_op = true;
                member_ops.push(member_op(v, &kind)?);
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
            name = Some(
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| ScimError::invalid_value("`displayName` must be a string."))?,
            );
            continue;
        }
        if lower == "externalid" {
            if kind == "remove" {
                ext = Some(None);
            } else {
                ext = Some(
                    op.value
                        .as_ref()
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                );
            }
            continue;
        }
        if lower == "members" {
            saw_members_op = true;
            let Some(v) = &op.value else {
                return Err(ScimError::invalid_value("The operation carries no value."));
            };
            member_ops.push(member_op(v, &kind)?);
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
            member_ops.push(MemberOp::Remove(vec![id_str.to_owned()]));
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
        name,
        ext,
        saw_members_op.then_some(member_ops),
    )
    .await?;
    Ok(axum::Json(out))
}

/// One `members` value as an operation (`add` appends, `replace` overwrites, `remove`
/// deletes), folded later under the group lock.
fn member_op(v: &serde_json::Value, kind: &str) -> ScimResult<MemberOp> {
    let list = v
        .as_array()
        .ok_or_else(|| ScimError::invalid_value("`members` must be a list."))?;
    let parsed: Vec<String> = list
        .iter()
        .map(|m| serde_json::from_value::<dto::MemberInput>(m.clone()).map(|m| m.value))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ScimError::invalid_value(format!("`members`: {e}")))?;
    Ok(match kind {
        "replace" => MemberOp::Replace(parsed),
        "add" => MemberOp::Add(parsed),
        _ => MemberOp::Remove(parsed),
    })
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
    // (FIX-083) The org lock before the group lock, like every other group writer: a sign-in
    // sync inserting this group's member cannot race the delete into a foreign-key failure.
    grepo::lock_org(&mut tx, org.org_id).await.map_err(db)?;
    let Some(row) = grepo::lock(&mut tx, id).await.map_err(db)? else {
        return Ok(StatusCode::NO_CONTENT);
    };
    if row.org_id != org.org_id || row.source != "scim" {
        return Ok(StatusCode::NO_CONTENT);
    }
    // What the group carried, read under its lock before the delete cascades it away: the
    // workspaces its bindings touched, its members and its org-role mapping — exactly the
    // dashboard delete's invalidation set (SEC-71: effective for the next request, FIX-062).
    let wss: Vec<Uuid> = grepo::bound_workspaces(&mut *tx, id).await.map_err(db)?;
    let members: Vec<Uuid> = grepo::member_ids(&mut *tx, id).await.map_err(db)?;
    grepo::delete(&mut tx, id).await.map_err(db)?;
    crate::domains::groups::service::notify_group_deleted(
        &mut tx,
        org.org_id,
        &wss,
        &members,
        row.org_role.is_some(),
    )
    .await
    .map_err(db)?;
    let ev = AuditEvent::new(
        service::actor(&caller.token.name),
        "group.delete",
        Outcome::Success,
    )
    .org(org.org_id)
    .target("group", id)
    .metadata(json!({"via": VIA, "bindings": wss.len(), "members": members.len()}));
    crate::audit::commit_critical(tx, &ev)
        .await
        .map_err(|_| ScimError::audit_unavailable())?;
    Ok(StatusCode::NO_CONTENT)
}
