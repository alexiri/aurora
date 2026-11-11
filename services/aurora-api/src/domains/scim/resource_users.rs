//! SCIM Users handlers (`16 §W15`): the org's links. Plain functions over the orchestration
//! in [`super::protocol`] (which reads the body, authenticates and renders errors); every
//! write is one transaction with its security-critical audit row ("no audit, no change").
//!
//! Rules (`04 §SCIM`): the e-mail resolves the global user; a missing one is created
//! pre-provisioned (`email_verified_at` stays NULL). The e-mail domain must be in the org's
//! allow-list (and the instance's). Owners are linked but never mutated. `active: false`
//! suspends (deprovisioning matrix in [`super::deprovision`]); reactivation only when SCIM
//! made the suspension. `DELETE` tombstones the link (idempotent 204).

use super::deprovision;
use super::dto::{Active, Email, ListResponse, Name, PatchOp, UserInput, UserResource};
use super::error::{ScimError, ScimResult};
use super::service::{
    self, Filter, USER_REFUSED_ATTRS, clean_external_id, clean_name, email_domain_allowed,
    page_params, parse_filter,
};
use super::{auth::ScimToken, dto, service::VIA};
use crate::app::AppState;
use crate::audit::{AuditEvent, Outcome};
use crate::domains::hosted::lifecycle;
use aurora_core::{ids, limits, sanitize};
use aurora_db::repo::{groups, members, scim as repo, users as users_repo};
use axum::http::header;
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
            "A resource with the same userName or externalId already exists in this organization.",
        )
    } else {
        db(e)
    }
}

fn audit_unavailable() -> ScimError {
    ScimError::audit_unavailable()
}

/// Location header value of a resource.
fn location(external_url: &str, kind: &str, id: Uuid) -> header::HeaderValue {
    dto::location(external_url, kind, id)
        .parse()
        .unwrap_or_else(|_| header::HeaderValue::from_static("/scim/v2"))
}

// ---- input shaping ------------------------------------------------------------------

/// Parse the query string leniently (shared with the Groups list): unknown keys are ignored,
/// a malformed pair is 400 `invalidSyntax`.
pub fn parse_query(query: Option<&str>) -> ScimResult<dto::ListQuery> {
    let Some(q) = query else {
        return Ok(dto::ListQuery::default());
    };
    let mut out = dto::ListQuery::default();
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = percent_decode(k);
        let v = percent_decode(v);
        match k.as_str() {
            "filter" => out.filter = Some(v),
            "startIndex" => {
                out.start_index =
                    Some(v.parse().map_err(|_| {
                        ScimError::invalid_syntax("`startIndex` must be a number.")
                    })?);
            }
            "count" => {
                out.count = Some(
                    v.parse()
                        .map_err(|_| ScimError::invalid_syntax("`count` must be a number."))?,
                );
            }
            "attributes" => out.attributes = Some(v),
            _ => {}
        }
    }
    Ok(out)
}

/// Minimal `%xx` / `+` decoding of a query component (the subset's values are ASCII; a bad
/// escape is kept literally rather than an error).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The primary (else first) e-mail of the body.
fn primary_email(input: &UserInput) -> Option<String> {
    let emails = input.emails.as_ref()?;
    let pick = emails
        .iter()
        .find(|e| e.primary)
        .or_else(|| emails.first())?;
    Some(pick.value.trim().to_ascii_lowercase())
}

/// The e-mail a SCIM user carries: primary `emails[].value`, else `userName` (the stored
/// `user_name`; `04 §SCIM` Users step 1).
fn resolved_email(input: &UserInput) -> ScimResult<String> {
    let e = match primary_email(input) {
        Some(e) if !e.is_empty() => e,
        _ => input
            .user_name
            .as_deref()
            .map(|u| u.trim().to_ascii_lowercase())
            .filter(|u| !u.is_empty())
            .ok_or_else(|| {
                ScimError::invalid_value("`userName` or an `emails[].value` is required.")
            })?,
    };
    if !service::valid_email(&e) {
        return Err(ScimError::invalid_value(
            "The e-mail address is not a valid address.",
        ));
    }
    Ok(e)
}

/// Refused attributes of the raw body (`roles`, `password`, … → 400 `invalidValue`).
fn bad_input(body: &serde_json::Value) -> ScimResult<()> {
    if !body.is_object() {
        return Err(ScimError::invalid_syntax("The body must be a JSON object."));
    }
    if let Some(a) = service::refused_attr(body, USER_REFUSED_ATTRS) {
        return Err(ScimError::invalid_value(format!(
            "`{a}` is not supported: Aurora roles are managed in Aurora, passwords and \
             certificates are not part of provisioning."
        )));
    }
    Ok(())
}

type Names = (Option<String>, Option<String>);

fn names_of(input: &UserInput) -> ScimResult<Names> {
    let n = input.name.clone().unwrap_or_default();
    Ok((
        clean_name(n.given_name.as_deref(), 120),
        clean_name(n.family_name.as_deref(), 120),
    ))
}

/// The name shown for the link: `displayName`, else `name.formatted`, else given + family.
fn display_of(input: &UserInput, names: &Names) -> Option<String> {
    if let Some(d) = clean_name(input.display_name.as_deref(), 120) {
        return Some(d);
    }
    if let Some(n) = input.name.as_ref()
        && let Some(f) = clean_name(n.formatted.as_deref(), 120)
    {
        return Some(f);
    }
    match (&names.0, &names.1) {
        (Some(g), Some(f)) => Some(format!("{g} {f}")),
        (Some(g), None) => Some(g.clone()),
        (None, Some(f)) => Some(f.clone()),
        (None, None) => None,
    }
}

/// The stored `display_name` for a *global* `users` row (≤ 80, sanitized).
fn global_display(input: &UserInput, names: &Names) -> Option<String> {
    display_of(input, names).map(|d| sanitize::one_line(&d, 80))
}

// ---- rendering ----------------------------------------------------------------------

/// The SCIM groups of the org the user is in (`readOnly`; the list and single GET share it).
async fn render_user(state: &AppState, org: Uuid, row: &repo::UserRow) -> ScimResult<UserResource> {
    let group_rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT g.id, g.name::text FROM group_members m
         JOIN groups g ON g.id = m.group_id AND g.org_id = m.org_id
         WHERE m.org_id = $1 AND m.user_id = $2 AND g.source = 'scim'
         ORDER BY g.created_at, g.id",
    )
    .bind(org)
    .bind(row.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(sq)?;
    Ok(UserResource {
        schemas: [dto::USER_SCHEMA],
        id: row.id.to_string(),
        external_id: row.external_id.clone(),
        user_name: row.user_name.clone(),
        display_name: row.display_name.clone(),
        name: Some(Name {
            given_name: row.given_name.clone(),
            family_name: row.family_name.clone(),
            formatted: row.display_name.clone(),
        }),
        emails: vec![Email {
            value: row.user_name.clone(),
            type_: Some("work".to_owned()),
            primary: true,
        }],
        active: row.state == "active",
        groups: group_rows
            .into_iter()
            .map(|(id, name)| dto::GroupRef {
                value: id.to_string(),
                display: Some(name),
                ref_: Some(dto::location(&state.config.external_url, "Groups", id)),
            })
            .collect(),
        meta: dto::Meta {
            resource_type: "User",
            created: service::rfc3339(row.created_at),
            last_modified: service::rfc3339(row.updated_at),
            location: dto::location(&state.config.external_url, "Users", row.id),
        },
    })
}

/// The churn column of the token (`last_used_at`, ≤ 1 write/min; best-effort).
async fn touch(state: &AppState, caller: &ScimToken) {
    if let Err(e) = repo::touch_token(&state.pool, caller.token.id).await {
        tracing::debug!(error = %e, "scim: last_used_at update failed");
    }
}

/// The seat cap of the org (`organizations.settings.max_members`; `None` = no cap) against
/// the active members. 403 `member_limit_reached` when full.
async fn ensure_seat(conn: &mut sqlx::PgConnection, org: Uuid) -> ScimResult<()> {
    let cap: Option<i64> = sqlx::query_scalar(
        "SELECT (settings ->> 'max_members')::int8 FROM organizations WHERE id = $1",
    )
    .bind(org)
    .fetch_one(&mut *conn)
    .await
    .map_err(sq)?;
    let Some(cap) = cap else {
        return Ok(());
    };
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM org_members WHERE org_id = $1 AND status = 'active'",
    )
    .bind(org)
    .fetch_one(&mut *conn)
    .await
    .map_err(sq)?;
    if used >= cap {
        return Err(ScimError::forbidden("member_limit_reached"));
    }
    Ok(())
}

// ---- handlers -----------------------------------------------------------------------

/// `POST /Users` (`16 §W15`): non-idempotent; 201 with the resource and `Location`.
pub async fn create_user(
    state: &AppState,
    caller: ScimToken,
    body: serde_json::Value,
) -> ScimResult<axum::response::Response> {
    use axum::response::IntoResponse;
    let input: UserInput = serde_json::from_value(body.clone())
        .map_err(|e| ScimError::invalid_syntax(e.to_string()))?;
    bad_input(&body)?;
    let org = caller.org.clone();
    let email = resolved_email(&input)?;
    if !email_domain_allowed(&org, &state.config.scim.allowed_email_domains, &email) {
        return Err(ScimError::invalid_value(
            "The e-mail domain is not allowed for this organization.",
        ));
    }
    let external_id = clean_external_id(input.external_id.as_deref())?;
    let names = names_of(&input)?;
    let display = display_of(&input, &names);

    let mut tx = state.pool.begin().await.map_err(sq)?;
    // (c) an unremoved link of this org with the same e-mail or externalId → 409 uniqueness.
    let dup = repo::users_by_email(&mut *tx, org.org_id, &email)
        .await
        .map_err(db)?
        .into_iter()
        .any(|u| u.state != "removed");
    if dup {
        return Err(ScimError::uniqueness(
            "A user with this e-mail already exists in this organization.",
        ));
    }
    if let Some(ext) = &external_id
        && repo::user_by_external_id(&mut *tx, org.org_id, ext)
            .await
            .map_err(db)?
            .is_some()
    {
        return Err(ScimError::uniqueness(
            "A user with this externalId already exists in this organization.",
        ));
    }
    // Resolve the global user by e-mail only (never `externalId`, 04 §SCIM).
    let (user_id, created_user) = match users_repo::find_by_email(&mut *tx, &email)
        .await
        .map_err(db)?
    {
        Some(u) => (u.id, false),
        None => {
            let id = ids::new_id();
            let name = global_display(&input, &names).unwrap_or_else(|| email.clone());
            sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2::citext, $3)")
                .bind(id)
                .bind(&email)
                .bind(&name)
                .execute(&mut *tx)
                .await
                .map_err(|e| uniqueness(aurora_db::DbError::from(e)))?;
            (id, true)
        }
    };
    // The org_members row: created (role member) or adopted as it is; owners never mutated.
    let active = input.active.map(|a| a.0).unwrap_or(true);
    let state_str = if active { "active" } else { "suspended" };
    let existing = members::get(&mut *tx, org.org_id, user_id)
        .await
        .map_err(db)?;
    let mut flush: Vec<(Uuid, Uuid)> = Vec::new();
    match &existing {
        None => {
            ensure_seat(&mut tx, org.org_id).await?;
            members::insert(&mut *tx, org.org_id, user_id, "member")
                .await
                .map_err(db)?;
            if !active {
                members::update(&mut *tx, org.org_id, user_id, None, Some("suspended"))
                    .await
                    .map_err(db)?;
            }
        }
        Some(m) if m.role == "owner" => {
            if !active {
                return Err(ScimError::forbidden(
                    "Organization owners are managed in Aurora.",
                ));
            }
        }
        Some(m) => {
            // Adopted member: `active: false` means the same thing as on the fresh path —
            // an active membership is suspended through the full C-474 path (the effects
            // run inside this transaction and audited with `via = "scim"`); an already
            // suspended one stays untouched (a human suspension SCIM must not claim).
            if !active && m.status == "active" {
                deprovision::suspend_membership(
                    &mut tx,
                    org.org_id,
                    user_id,
                    &service::actor(&caller.token.name),
                )
                .await
                .map_err(scim_of_api)?;
                flush = deprovision::abort_turns(&mut tx, org.org_id, user_id)
                    .await
                    .map_err(scim_of_api)?;
                deprovision::revoke_org_keys(&mut tx, org.org_id, user_id)
                    .await
                    .map_err(scim_of_api)?;
                deprovision::revoke_sessions_if_last(&mut tx, user_id)
                    .await
                    .map_err(scim_of_api)?;
            }
        }
    }
    let row = repo::insert_user(
        &mut *tx,
        &repo::NewScimUser {
            id: ids::new_id(),
            org_id: org.org_id,
            user_id,
            external_id: external_id.as_deref(),
            user_name: &email,
            display_name: display.as_deref(),
            given_name: names.0.as_deref(),
            family_name: names.1.as_deref(),
            state: state_str,
            suspended_by_scim: !active && existing.is_none_or(|m| m.status != "suspended"),
        },
    )
    .await
    .map_err(uniqueness)?;
    let ev = AuditEvent::new(
        service::actor(&caller.token.name),
        "member.add",
        Outcome::Success,
    )
    .org(org.org_id)
    .target("user", user_id)
    .metadata(json!({
        "via": VIA,
        "scim_id": row.id,
        "role": "member",
        "status": state_str,
        "user_created": created_user,
    }));
    crate::audit::commit_critical(tx, &ev)
        .await
        .map_err(|_| audit_unavailable())?;
    for (c, t) in flush {
        lifecycle::flush_deltas(state, c, t).await;
    }
    // The membership change invalidates like any other (gateway identities + streams).
    let mut conn = state.pool.acquire().await.map_err(sq)?;
    groups::notify_member_changed(&mut conn, org.org_id, user_id)
        .await
        .map_err(db)?;
    let rendered = render_user(state, org.org_id, &row).await?;
    let mut res = (axum::http::StatusCode::CREATED, axum::Json(rendered)).into_response();
    res.headers_mut().insert(
        header::LOCATION,
        location(&state.config.external_url, "Users", row.id),
    );
    touch(state, &caller).await;
    Ok(res)
}

/// `GET /Users/{id}`: 404 for unknown, foreign or removed (the tombstone is invisible).
pub async fn get_user(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
) -> ScimResult<axum::Json<UserResource>> {
    let row = repo::user_by_id(&state.pool, caller.org.org_id, id)
        .await
        .map_err(db)?
        .filter(|r| r.state != "removed")
        .ok_or_else(ScimError::not_found)?;
    touch(state, &caller).await;
    Ok(axum::Json(
        render_user(state, caller.org.org_id, &row).await?,
    ))
}

/// `GET /Users?filter=&startIndex=&count=`: pages of the org's non-removed links. The query
/// is parsed leniently (unknown keys ignored; a malformed one is 400 `invalidSyntax`).
pub async fn list_users(
    state: &AppState,
    headers: &http::HeaderMap,
    query: Option<&str>,
) -> ScimResult<axum::Json<ListResponse>> {
    let caller = ScimToken::authenticate(state, headers).await?;
    let q = parse_query(query)?;
    let (start, count) = page_params(q.start_index, q.count)?;
    let filter = parse_filter(q.filter.as_deref().unwrap_or(""))?;
    touch(state, &caller).await;

    // The filter subset runs on indexed lookups; `displayName` scans the org's links
    // (bounded: the set is the org's directory, not the instance).
    let rows: Vec<repo::UserRow> = match &filter {
        None => repo::users_page(&state.pool, caller.org.org_id, None, 10_000)
            .await
            .map_err(db)?,
        Some(Filter::UserName(v)) => repo::users_by_user_name(&state.pool, caller.org.org_id, v)
            .await
            .map_err(db)?,
        Some(Filter::Email(v)) => repo::users_by_email(&state.pool, caller.org.org_id, v)
            .await
            .map_err(db)?,
        Some(Filter::ExternalId(v)) => {
            repo::users_by_external_id(&state.pool, caller.org.org_id, v)
                .await
                .map_err(db)?
        }
        Some(Filter::Id(id)) => repo::users_by_scim_id(&state.pool, caller.org.org_id, *id)
            .await
            .map_err(db)?,
        Some(Filter::DisplayName(v)) => {
            repo::users_page(&state.pool, caller.org.org_id, None, 10_000)
                .await
                .map_err(db)?
                .into_iter()
                .filter(|r| {
                    r.display_name
                        .as_deref()
                        .is_some_and(|d| d.eq_ignore_ascii_case(v))
                })
                .collect()
        }
    };
    let rows: Vec<repo::UserRow> = rows.into_iter().filter(|r| r.state != "removed").collect();
    let total = rows.len() as u64;
    let skip = usize::try_from(start - 1).unwrap_or(usize::MAX);
    let items: Vec<&repo::UserRow> = rows.iter().skip(skip).take(count).collect();
    let mut resources = Vec::with_capacity(items.len());
    for r in items {
        resources.push(
            serde_json::to_value(render_user(state, caller.org.org_id, r).await?)
                .unwrap_or_else(|_| json!({})),
        );
    }
    Ok(axum::Json(ListResponse {
        schemas: [dto::LIST_RESPONSE_SCHEMA],
        total_results: total,
        start_index: start,
        items_per_page: resources.len() as u64,
        resources,
    }))
}

/// One PUT/PATCH update after the operations merged over the current state.
struct Update {
    email: String,
    external_id: Option<String>,
    display: Option<String>,
    names: Names,
    active: Option<Active>,
}

/// `PUT /Users/{id}`: full replacement of the stored attributes; idempotent.
pub async fn put_user(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
    body: serde_json::Value,
) -> ScimResult<axum::Json<UserResource>> {
    let input: UserInput = serde_json::from_value(body.clone())
        .map_err(|e| ScimError::invalid_syntax(e.to_string()))?;
    bad_input(&body)?;
    let email = resolved_email(&input)?;
    let external_id = clean_external_id(input.external_id.as_deref())?;
    let names = names_of(&input)?;
    let display = display_of(&input, &names);
    apply_update(
        state,
        caller,
        id,
        Update {
            email,
            external_id,
            display,
            names,
            active: input.active,
        },
    )
    .await
}

/// `PATCH /Users/{id}`: the operations subset, applied atomically (one bad operation applies
/// none; `16 §W15`).
pub async fn patch_user(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
    body: serde_json::Value,
) -> ScimResult<axum::Json<UserResource>> {
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
    let current = repo::user_by_id(&state.pool, caller.org.org_id, id)
        .await
        .map_err(db)?
        .filter(|r| r.state != "removed")
        .ok_or_else(ScimError::not_found)?;
    // Merge the operations over the current resource, then apply like a PUT (atomicity is
    // free: nothing is written until every operation parsed and merged).
    let mut merged = UserInput {
        user_name: Some(current.user_name.clone()),
        external_id: current.external_id.clone(),
        display_name: current.display_name.clone(),
        name: Some(Name {
            given_name: current.given_name.clone(),
            family_name: current.family_name.clone(),
            formatted: current.display_name.clone(),
        }),
        emails: Some(vec![Email {
            value: current.user_name.clone(),
            type_: Some("work".to_owned()),
            primary: true,
        }]),
        active: Some(Active(current.state == "active")),
        raw: json!({}),
    };
    for op in &patch.operations {
        apply_patch_op(&mut merged, op)?;
    }
    // A removed `displayName` stores NULL — it must not come back through `display_of`'s
    // fallbacks (`name.formatted`, given + family): updates store exactly what was sent.
    let email = resolved_email(&merged)?;
    if !email_domain_allowed(
        &caller.org,
        &state.config.scim.allowed_email_domains,
        &email,
    ) {
        return Err(ScimError::invalid_value(
            "The e-mail domain is not allowed for this organization.",
        ));
    }
    let external_id = clean_external_id(merged.external_id.as_deref())?;
    let names = names_of(&merged)?;
    // Exactly what the IdP asserts: a removed/absent displayName stays absent (the
    // given+family fallback is a POST-creation default, not an update behavior).
    let display = clean_name(merged.display_name.as_deref(), 120);
    apply_update(
        state,
        caller,
        id,
        Update {
            email,
            external_id,
            display,
            names,
            active: merged.active,
        },
    )
    .await
}

/// One PATCH operation against the merged input. Paths supported: `active`, `userName`,
/// `displayName`, `externalId`, `name.givenName`, `name.familyName`, `name.formatted`,
/// `emails`, `emails[type eq "work"].value` (and `primary eq true`), or no path with an
/// object value naming those attributes. `op` is case-insensitive.
fn apply_patch_op(target: &mut UserInput, op: &PatchOp) -> ScimResult<()> {
    let kind = op.op.trim().to_ascii_lowercase();
    if !matches!(kind.as_str(), "add" | "replace" | "remove") {
        return Err(ScimError::invalid_value(
            "`op` must be add, replace or remove.",
        ));
    }
    let path = op.path.as_deref().unwrap_or("").trim();
    if path.is_empty() {
        // No path: the value is an object naming the attributes (Okta, Entra).
        let Some(v) = &op.value else {
            return Err(ScimError::invalid_value(
                "An operation without a path carries the value object.",
            ));
        };
        if kind == "remove" {
            return Err(ScimError::no_target(
                "A `remove` without a path has no target.",
            ));
        }
        let Some(obj) = v.as_object() else {
            return Err(ScimError::invalid_value(
                "The value of an operation without a path must be an object.",
            ));
        };
        if let Some(a) = service::refused_attr(v, USER_REFUSED_ATTRS) {
            return Err(ScimError::invalid_value(format!("`{a}` is not supported.")));
        }
        for key in ["active", "userName", "displayName", "externalId"] {
            if let Some(v) = obj.get(key) {
                apply_leaf(target, &key.to_ascii_lowercase(), v)?;
            }
        }
        if let Some(n) = obj.get("name").and_then(|n| n.as_object()) {
            for key in ["givenName", "familyName", "formatted"] {
                if let Some(v) = n.get(key) {
                    apply_leaf(target, &format!("name.{}", key.to_ascii_lowercase()), v)?;
                }
            }
        }
        if let Some(e) = obj.get("emails") {
            apply_leaf(target, "emails", e)?;
        }
        return Ok(());
    }
    // `emails[type eq "work"].value` (and `emails[primary eq true].value`).
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("emails[") {
        if kind == "remove" {
            return Err(ScimError::invalid_path(
                "`emails[…].value` cannot be removed; replace `emails` instead.",
            ));
        }
        let Some(v) = &op.value else {
            return Err(ScimError::invalid_value("The operation carries no value."));
        };
        let s = v.as_str().ok_or_else(|| {
            ScimError::invalid_value("The value of an e-mail operation must be a string.")
        })?;
        target.emails = Some(vec![Email {
            value: s.to_owned(),
            type_: Some("work".to_owned()),
            primary: true,
        }]);
        return Ok(());
    }
    if kind == "remove" && matches!(lower.as_str(), "username" | "active") {
        return Err(ScimError::no_target(
            "`userName` and `active` cannot be removed.",
        ));
    }
    if kind == "remove" {
        // Removing an optional attribute: back to null.
        match lower.as_str() {
            "displayname" => target.display_name = None,
            "externalid" => target.external_id = None,
            "name.givenname" => {
                if let Some(n) = target.name.as_mut() {
                    n.given_name = None;
                }
            }
            "name.familyname" => {
                if let Some(n) = target.name.as_mut() {
                    n.family_name = None;
                }
            }
            "name.formatted" => {
                if let Some(n) = target.name.as_mut() {
                    n.formatted = None;
                }
            }
            "emails" => target.emails = None,
            "name" => target.name = None,
            other => {
                return Err(ScimError::invalid_path(format!(
                    "The path `{other}` is not supported."
                )));
            }
        }
        return Ok(());
    }
    let Some(v) = &op.value else {
        return Err(ScimError::invalid_value("The operation carries no value."));
    };
    apply_leaf(target, &lower, v)
}

/// Apply one leaf value (`add` and `replace` behave identically on scalar attributes).
fn apply_leaf(target: &mut UserInput, lower_path: &str, v: &serde_json::Value) -> ScimResult<()> {
    match lower_path {
        "active" => {
            let a: Active = serde_json::from_value(v.clone())
                .map_err(|_| ScimError::invalid_value("`active` must be true or false."))?;
            target.active = Some(a);
        }
        "username" => {
            target.user_name = Some(
                v.as_str()
                    .ok_or_else(|| ScimError::invalid_value("`userName` must be a string."))?
                    .to_owned(),
            );
        }
        "displayname" => {
            target.display_name = v.as_str().map(str::to_owned);
        }
        "externalid" => {
            target.external_id = v.as_str().map(str::to_owned);
        }
        "name.givenname" => {
            target.name.get_or_insert_with(Default::default).given_name =
                v.as_str().map(str::to_owned);
        }
        "name.familyname" => {
            target.name.get_or_insert_with(Default::default).family_name =
                v.as_str().map(str::to_owned);
        }
        "name.formatted" => {
            target.name.get_or_insert_with(Default::default).formatted =
                v.as_str().map(str::to_owned);
        }
        "emails" => {
            let emails: Vec<Email> = serde_json::from_value(v.clone())
                .map_err(|_| ScimError::invalid_value("`emails` must be a list."))?;
            target.emails = Some(emails);
        }
        other => {
            return Err(ScimError::invalid_path(format!(
                "The path `{other}` is not supported."
            )));
        }
    }
    Ok(())
}

/// The shared body of `PUT`/`PATCH` (one transaction; the state change with its critical
/// audit rows and the full deprovisioning matrix).
async fn apply_update(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
    u: Update,
) -> ScimResult<axum::Json<UserResource>> {
    let org = caller.org.clone();
    let mut tx = state.pool.begin().await.map_err(sq)?;
    let _current = repo::user_by_id(&mut *tx, org.org_id, id)
        .await
        .map_err(db)?
        .filter(|r| r.state != "removed")
        .ok_or_else(ScimError::not_found)?;
    // Uniqueness: the new e-mail / externalId must belong to this very link (or be free).
    for other in repo::users_by_email(&mut *tx, org.org_id, &u.email)
        .await
        .map_err(db)?
    {
        if other.id != id && other.state != "removed" {
            return Err(ScimError::uniqueness(
                "A user with this e-mail already exists in this organization.",
            ));
        }
    }
    if let Some(ext) = &u.external_id
        && let Some(other) = repo::user_by_external_id(&mut *tx, org.org_id, ext)
            .await
            .map_err(db)?
        && other.id != id
    {
        return Err(ScimError::uniqueness(
            "A user with this externalId already exists in this organization.",
        ));
    }
    // Resolve the (possibly new) global user by e-mail. Aurora never changes `users.email`
    // or the display name of a user with an identity or a verified e-mail; a new address
    // creates a new pre-provisioned user and the link follows it (04 §SCIM). The new global
    // user needs an org_members row of its own (a member; seats like any other provision).
    let user_id = match users_repo::find_by_email(&mut *tx, &u.email)
        .await
        .map_err(db)?
    {
        Some(existing) => existing.id,
        None => {
            let id = ids::new_id();
            let name = u
                .display
                .clone()
                .map(|d| sanitize::one_line(&d, 80))
                .unwrap_or_else(|| u.email.clone());
            sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2::citext, $3)")
                .bind(id)
                .bind(&u.email)
                .bind(&name)
                .execute(&mut *tx)
                .await
                .map_err(|e| uniqueness(aurora_db::DbError::from(e)))?;
            ensure_seat(&mut tx, org.org_id).await?;
            members::insert(&mut *tx, org.org_id, id, "member")
                .await
                .map_err(db)?;
            id
        }
    };
    let row = repo::update_user(
        &mut tx,
        org.org_id,
        id,
        user_id,
        u.external_id.as_deref(),
        &u.email,
        u.display.as_deref(),
        u.names.0.as_deref(),
        u.names.1.as_deref(),
    )
    .await
    .map_err(db)?
    .ok_or_else(ScimError::not_found)?;
    // `active` drives deprovisioning; owners are never mutated (403).
    let member = members::get(&mut *tx, org.org_id, row.user_id)
        .await
        .map_err(db)?
        .ok_or_else(ScimError::internal)?;
    if member.role == "owner" && u.active.is_some_and(|a| !a.0) {
        return Err(ScimError::forbidden(
            "Organization owners are managed in Aurora.",
        ));
    }
    let want_active = u.active.map(|a| a.0).unwrap_or(row.state == "active");
    let mut flush: Vec<(Uuid, Uuid)> = Vec::new();
    if !want_active && row.state != "suspended" {
        // Suspend (deprovisioning matrix, D-135).
        repo::set_user_state(&mut tx, org.org_id, id, "suspended", true)
            .await
            .map_err(db)?;
        deprovision::suspend_membership(
            &mut tx,
            org.org_id,
            row.user_id,
            &service::actor(&caller.token.name),
        )
        .await
        .map_err(scim_of_api)?;
        flush = deprovision::abort_turns(&mut tx, org.org_id, row.user_id)
            .await
            .map_err(scim_of_api)?;
        deprovision::revoke_org_keys(&mut tx, org.org_id, row.user_id)
            .await
            .map_err(scim_of_api)?;
        deprovision::revoke_sessions_if_last(&mut tx, row.user_id)
            .await
            .map_err(scim_of_api)?;
    } else if want_active && row.state == "suspended" {
        // Reactivation only when SCIM suspended the membership (04 §SCIM).
        if !row.suspended_by_scim {
            return Err(ScimError::forbidden(
                "This member was suspended in Aurora; reactivation is an Aurora decision.",
            ));
        }
        repo::set_user_state(&mut tx, org.org_id, id, "active", false)
            .await
            .map_err(db)?;
        deprovision::reactivate_membership(
            &mut tx,
            org.org_id,
            row.user_id,
            &service::actor(&caller.token.name),
        )
        .await
        .map_err(scim_of_api)?;
    }
    // Attribute update + state change + audit rows + invalidations commit atomically.
    crate::audit::commit_critical_many(
        tx,
        &[AuditEvent::new(
            service::actor(&caller.token.name),
            "scim.update",
            Outcome::Success,
        )
        .org(org.org_id)
        .target("scim_user", id)
        .metadata(json!({
            "via": VIA,
            "change": if want_active && row.state == "suspended" { "reactivated" }
                      else if !want_active && row.state != "suspended" { "suspended" }
                      else { "attributes" },
        }))],
    )
    .await
    .map_err(|_| audit_unavailable())?;
    for (c, t) in flush {
        lifecycle::flush_deltas(state, c, t).await;
    }
    touch(state, &caller).await;
    let row = repo::user_by_id(&state.pool, org.org_id, id)
        .await
        .map_err(db)?
        .ok_or_else(ScimError::not_found)?;
    Ok(axum::Json(render_user(state, org.org_id, &row).await?))
}

fn scim_of_api(e: aurora_core::ApiError) -> ScimError {
    tracing::error!(code = e.code(), "scim: effect failed");
    if e.status() == axum::http::StatusCode::SERVICE_UNAVAILABLE {
        ScimError::audit_unavailable()
    } else {
        ScimError::internal()
    }
}

/// `DELETE /Users/{id}`: the link becomes a tombstone; the membership is removed with the
/// full deprovisioning matrix. Idempotent (204 again).
pub async fn delete_user(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
) -> ScimResult<axum::http::StatusCode> {
    use axum::http::StatusCode;
    let org = caller.org.clone();
    let mut tx = state.pool.begin().await.map_err(sq)?;
    let row = repo::user_by_id(&mut *tx, org.org_id, id)
        .await
        .map_err(db)?
        .ok_or_else(ScimError::not_found)?;
    if row.state == "removed" {
        // Idempotent: 204 again.
        return Ok(StatusCode::NO_CONTENT);
    }
    let member = members::get(&mut *tx, org.org_id, row.user_id)
        .await
        .map_err(db)?;
    if member.as_ref().is_some_and(|m| m.role == "owner") {
        return Err(ScimError::forbidden(
            "Organization owners are managed in Aurora.",
        ));
    }
    repo::remove_user(&mut tx, org.org_id, id)
        .await
        .map_err(db)?;
    let actor = service::actor(&caller.token.name);
    let mut flush = Vec::new();
    if member.is_some() {
        deprovision::remove_membership(&mut tx, org.org_id, row.user_id, &actor)
            .await
            .map_err(scim_of_api)?;
        flush = deprovision::abort_turns(&mut tx, org.org_id, row.user_id)
            .await
            .map_err(scim_of_api)?;
        deprovision::revoke_org_keys(&mut tx, org.org_id, row.user_id)
            .await
            .map_err(scim_of_api)?;
        deprovision::revoke_sessions_if_last(&mut tx, row.user_id)
            .await
            .map_err(scim_of_api)?;
        deprovision::cancel_invitations(&mut tx, &row.user_name)
            .await
            .map_err(scim_of_api)?;
    }
    // Tombstone + membership removal + invalidations commit atomically; the invalidation
    // NOTIFY rides on the same transaction (delivered on commit).
    crate::audit::commit_critical_many(
        tx,
        &[AuditEvent::new(
            service::actor(&caller.token.name),
            "scim.update",
            Outcome::Success,
        )
        .org(org.org_id)
        .target("scim_user", id)
        .metadata(json!({"via": VIA, "change": "removed"}))],
    )
    .await
    .map_err(|_| audit_unavailable())?;
    for (c, t) in flush {
        lifecycle::flush_deltas(state, c, t).await;
    }
    touch(state, &caller).await;
    Ok(StatusCode::NO_CONTENT)
}
