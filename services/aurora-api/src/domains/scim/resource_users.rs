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
    let by_email = repo::users_by_email(&mut *tx, org.org_id, &email)
        .await
        .map_err(db)?;
    if by_email.iter().any(|u| u.state != "removed") {
        return Err(ScimError::uniqueness(
            "A user with this e-mail already exists in this organization.",
        ));
    }
    // A removed link with this e-mail is revived (C-474: "POST of a removed user
    // re-provisions"): the tombstone keeps its id and its pinned `user_id` under
    // `UNIQUE (org_id, user_id)`, so the row is updated in place instead of re-inserted, on
    // the person it was removed from — the e-mail is not re-resolved (D-167: the link's
    // `userName` may differ from that person's `users.email` after an IdP rename, and the
    // revival must neither 500 nor re-point). Its global user always still exists (a
    // `users` delete cascades the tombstone away).
    let removed = by_email
        .iter()
        .find(|u| u.state == "removed")
        .map(|u| (u.id, u.user_id));
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
    // Resolve the global user by e-mail only (never `externalId`, 04 §SCIM) — unless a
    // tombstone is revived, which stays on its pinned user.
    let (user_id, created_user) = match removed {
        Some((_, pinned)) => (pinned, false),
        None => match users_repo::find_by_email(&mut *tx, &email)
            .await
            .map_err(db)?
        {
            Some(u) => (u.id, false),
            None => {
                let id = ids::new_id();
                let name = global_display(&input, &names).unwrap_or_else(|| email.clone());
                repo::insert_preprovisioned_user(&mut tx, id, &email, &name)
                    .await
                    .map_err(uniqueness)?;
                (id, true)
            }
        },
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
    // A revived tombstone is updated in place (its `scim_users.id` survives); otherwise a
    // fresh link row is inserted.
    let row = match removed {
        Some((id, _)) => repo::revive_user(
            &mut tx,
            org.org_id,
            id,
            external_id.as_deref(),
            &email,
            display.as_deref(),
            names.0.as_deref(),
            names.1.as_deref(),
            state_str,
            !active && existing.is_none_or(|m| m.status != "suspended"),
        )
        .await
        .map_err(db)?
        .ok_or_else(ScimError::internal)?,
        None => repo::insert_user(
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
        .map_err(uniqueness)?,
    };
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
    /// `PUT` (a full replacement: refused for an owner whatever the body, C-473) or `PATCH`
    /// (refused for an owner only when it deactivates).
    put: bool,
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
    if !email_domain_allowed(
        &caller.org,
        &state.config.scim.allowed_email_domains,
        &email,
    ) {
        return Err(ScimError::invalid_value(
            "The e-mail domain is not allowed for this organization.",
        ));
    }
    let external_id = clean_external_id(input.external_id.as_deref())?;
    let names = names_of(&input)?;
    let display = display_of(&input, &names);
    apply_update(
        state,
        caller,
        id,
        Update {
            put: true,
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
        // The stored e-mail *is* `userName` (rendered as the one primary `emails` entry), so
        // it is seeded once: a `replace userName` must change it (FIX-062 — a seeded
        // `emails` copy would win in `resolved_email` and silently ignore the operation),
        // while an `emails` operation still wins over `userName` like on POST.
        emails: None,
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
            put: false,
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
///
/// **D-167: an update never moves a link to another account.** The link's `user_id` is
/// immutable, with one exception that cannot hurt a person:
/// - (a) the linked user signed in (an identity or a verified e-mail), or the membership is
///   an owner's: a new `userName`/e-mail is stored on the link only; `users` is untouched.
/// - (b) the linked user is a still-unbound placeholder (no identity, unverified e-mail):
///   a new e-mail that is free globally renames the placeholder in place when no other org
///   holds it (membership or link in any state); otherwise — the address belongs to another
///   user, or the placeholder is shared — the link moves exactly like a `POST` would link
///   it ([`move_link`]): to that user, or to a new pre-provisioned one. The placeholder's
///   membership in this org is removed; the `users` row is never deleted (SEC-73).
async fn apply_update(
    state: &AppState,
    caller: ScimToken,
    id: Uuid,
    u: Update,
) -> ScimResult<axum::Json<UserResource>> {
    let org = caller.org.clone();
    let actor = service::actor(&caller.token.name);
    let mut tx = state.pool.begin().await.map_err(sq)?;
    // The link row is locked: concurrent updates of one link serialize.
    let current = repo::lock_user(&mut tx, org.org_id, id)
        .await
        .map_err(db)?
        .filter(|r| r.state != "removed")
        .ok_or_else(ScimError::not_found)?;
    // Owners are never mutated by SCIM (C-473, SEC-72): a `PUT` (whatever its body) and a
    // deactivating `PATCH` are refused before any write.
    let member = members::get(&mut *tx, org.org_id, current.user_id)
        .await
        .map_err(db)?;
    let is_owner = member.as_ref().is_some_and(|m| m.role == "owner");
    let want_active = u.active.map(|a| a.0).unwrap_or(current.state == "active");
    if is_owner && (u.put || !want_active) {
        return Err(ScimError::forbidden(
            "Organization owners are managed in Aurora.",
        ));
    }
    // Uniqueness: the new e-mail / externalId must belong to this very link (or be free)
    // among the org's links. A removed link in the result is a tombstone that still owns
    // its `userName` (a POST of it revives the tombstone on its pinned user); an update
    // claiming it is a deterministic 409 here — never a raw index error (FIX-040). POST
    // treats the tombstone differently (revival), PUT/PATCH cannot.
    for other in repo::users_by_email(&mut *tx, org.org_id, &u.email)
        .await
        .map_err(db)?
    {
        if other.id != id {
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
    // D-167: what the asserted e-mail means for the account behind the link. The account row
    // is locked so a concurrent sign-in binding it serializes with this decision.
    let account = repo::lock_account(&mut tx, current.user_id)
        .await
        .map_err(db)?
        .ok_or_else(ScimError::internal)?;
    let email_differs = !account.email.eq_ignore_ascii_case(&u.email);
    let mut account_renamed = false;
    let move_to: Option<Uuid> = if is_owner || account.signed_in || !email_differs {
        // (a) — or nothing to change: the e-mail lands on the link only.
        None
    } else {
        // (b) a still-unbound placeholder (no identity, unverified e-mail).
        match users_repo::find_by_email(&mut *tx, &u.email)
            .await
            .map_err(db)?
        {
            Some(other) => Some(other.id),
            None if !repo::user_elsewhere(&mut tx, org.org_id, current.user_id)
                .await
                .map_err(db)? =>
            {
                // Free globally and the placeholder is this org's alone: renamed in place.
                if !repo::set_placeholder_email(&mut tx, current.user_id, &u.email)
                    .await
                    .map_err(uniqueness)?
                {
                    // The locked row was unbound a statement ago; refuse rather than guess.
                    return Err(ScimError::internal());
                }
                account_renamed = true;
                None
            }
            None => {
                // Free globally but the placeholder is shared with another org: a new
                // pre-provisioned user, created exactly like a POST would.
                let nid = ids::new_id();
                let name = u
                    .display
                    .clone()
                    .map(|d| sanitize::one_line(&d, 80))
                    .unwrap_or_else(|| u.email.clone());
                repo::insert_preprovisioned_user(&mut tx, nid, &u.email, &name)
                    .await
                    .map_err(uniqueness)?;
                Some(nid)
            }
        }
    };
    let row = repo::update_user(
        &mut tx,
        org.org_id,
        id,
        u.external_id.as_deref(),
        &u.email,
        u.display.as_deref(),
        u.names.0.as_deref(),
        u.names.1.as_deref(),
    )
    .await
    .map_err(db)?
    .ok_or_else(ScimError::not_found)?;
    let mut evs = Vec::with_capacity(2);
    let mut flush: Vec<(Uuid, Uuid)> = Vec::new();
    let change: &str;
    if let Some(to) = move_to {
        flush = move_link(&mut tx, &actor, org.org_id, &row, to, want_active, &mut evs).await?;
        change = "moved";
    } else {
        // The link keeps its user, whose membership exists (a link without one is a broken
        // invariant: a human removal tombstones the link, FIX-037).
        if member.is_none() {
            tracing::error!(scim_id = %id, "scim: unremoved link without a membership");
            return Err(ScimError::internal());
        }
        change = if want_active && row.state == "suspended" {
            "reactivated"
        } else if !want_active && row.state != "suspended" {
            "suspended"
        } else {
            "attributes"
        };
    }
    if move_to.is_some() {
        // State was decided by the move (POST semantics).
    } else if !want_active && row.state != "suspended" {
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
        deprovision::cancel_invitations(&mut tx, org.org_id, &row.user_name, row.user_id)
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
    evs.push(
        AuditEvent::new(actor.clone(), "scim.update", Outcome::Success)
            .org(org.org_id)
            .target("scim_user", id)
            .metadata(json!({
                "via": VIA,
                "change": change,
                // D-167(b): the placeholder's own e-mail was renamed in place.
                "account_email": account_renamed,
            })),
    );
    crate::audit::commit_critical_many(tx, &evs)
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

/// D-167(b): move the link of a still-unbound placeholder to `to` — exactly what a `POST`
/// of the new e-mail would do (adopt `to`'s membership as it is, or insert one under the
/// seat cap; `active: false` suspends through the C-474 path), plus what keeps the IdP's
/// view whole: the placeholder's membership in this org is removed (audited, its bindings
/// cascade) and its SCIM-group memberships follow the link (the group resources reference
/// the link, not the account; `GROUPS_PER_USER` applies). The placeholder's `users` row is
/// never touched or deleted (SEC-73). Returns the turns to flush after commit.
async fn move_link(
    tx: &mut sqlx::PgConnection,
    actor: &crate::audit::AuditActor,
    org: Uuid,
    link: &repo::UserRow,
    to: Uuid,
    want_active: bool,
    evs: &mut Vec<AuditEvent>,
) -> ScimResult<Vec<(Uuid, Uuid)>> {
    let from = link.user_id;
    // The org lock first: the seat cap and the group cap below count under it (the lock
    // `ensure_member_limit` documents).
    groups::lock_org(&mut *tx, org).await.map_err(db)?;
    // `to` already linked in this org (any state, a tombstone included): the same 409 a
    // POST would answer — never two links of one person.
    if repo::user_link(&mut *tx, org, to)
        .await
        .map_err(db)?
        .is_some()
    {
        return Err(ScimError::uniqueness(
            "A user with this e-mail already exists in this organization.",
        ));
    }
    let moved_groups: Vec<Uuid> = sqlx::query_scalar(
        "SELECT m.group_id FROM group_members m
         JOIN groups g ON g.id = m.group_id AND g.org_id = m.org_id
         WHERE m.org_id = $1 AND m.user_id = $2 AND g.source = 'scim'
         ORDER BY m.group_id",
    )
    .bind(org)
    .bind(from)
    .fetch_all(&mut *tx)
    .await
    .map_err(sq)?;
    // The placeholder leaves this org (it never signed in: no session, key or turn to end).
    deprovision::remove_membership(&mut *tx, org, from, actor)
        .await
        .map_err(scim_of_api)?;
    for g in &moved_groups {
        evs.push(
            AuditEvent::new(actor.clone(), "group_member.remove", Outcome::Success)
                .org(org)
                .target("group", *g)
                .metadata(json!({"via": VIA, "user": from, "reason": "scim_link_moved"})),
        );
    }
    // `to`'s membership, exactly like `create_user`.
    let existing = members::get(&mut *tx, org, to).await.map_err(db)?;
    let mut flush = Vec::new();
    match &existing {
        None => {
            ensure_seat(&mut *tx, org).await?;
            members::insert(&mut *tx, org, to, "member")
                .await
                .map_err(db)?;
            if !want_active {
                members::update(&mut *tx, org, to, None, Some("suspended"))
                    .await
                    .map_err(db)?;
            }
            evs.push(
                AuditEvent::new(actor.clone(), "member.add", Outcome::Success)
                    .org(org)
                    .target("user", to)
                    .metadata(json!({
                        "via": VIA,
                        "scim_id": link.id,
                        "role": "member",
                        "status": if want_active { "active" } else { "suspended" },
                    })),
            );
        }
        Some(m) if m.role == "owner" => {
            if !want_active {
                return Err(ScimError::forbidden(
                    "Organization owners are managed in Aurora.",
                ));
            }
        }
        Some(m) => {
            if !want_active && m.status == "active" {
                deprovision::suspend_membership(&mut *tx, org, to, actor)
                    .await
                    .map_err(scim_of_api)?;
                flush = deprovision::abort_turns(&mut *tx, org, to)
                    .await
                    .map_err(scim_of_api)?;
                deprovision::revoke_org_keys(&mut *tx, org, to)
                    .await
                    .map_err(scim_of_api)?;
                deprovision::revoke_sessions_if_last(&mut *tx, to)
                    .await
                    .map_err(scim_of_api)?;
                deprovision::cancel_invitations(&mut *tx, org, &link.user_name, to)
                    .await
                    .map_err(scim_of_api)?;
            }
        }
    }
    repo::move_user(&mut *tx, org, link.id, to)
        .await
        .map_err(db)?
        .ok_or_else(ScimError::not_found)?;
    let by_scim = !want_active && existing.as_ref().is_none_or(|m| m.status != "suspended");
    repo::set_user_state(
        &mut *tx,
        org,
        link.id,
        if want_active { "active" } else { "suspended" },
        by_scim,
    )
    .await
    .map_err(db)?;
    // The link's SCIM groups follow it.
    for g in &moved_groups {
        let in_groups = groups::groups_of_user_count(&mut *tx, org, to)
            .await
            .map_err(db)?;
        if in_groups >= groups::GROUPS_PER_USER {
            return Err(ScimError::invalid_value(format!(
                "A user can be in at most {} groups of this organization.",
                groups::GROUPS_PER_USER
            )));
        }
        if groups::add_member(&mut *tx, *g, org, to, "scim", None)
            .await
            .map_err(db)?
        {
            evs.push(
                AuditEvent::new(actor.clone(), "group_member.add", Outcome::Success)
                    .org(org)
                    .target("group", *g)
                    .metadata(json!({"via": VIA, "user": to, "reason": "scim_link_moved"})),
            );
        }
    }
    groups::notify_member_changed(&mut *tx, org, to)
        .await
        .map_err(db)?;
    Ok(flush)
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
        deprovision::cancel_invitations(&mut tx, org.org_id, &row.user_name, row.user_id)
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
