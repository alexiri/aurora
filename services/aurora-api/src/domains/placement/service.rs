//! New-user placement (P4-ONB-1, D-140, SEC-79): the admin's settings and the sign-in step that
//! applies them inside the sign-in transaction.

use super::dto::*;
use super::template::{self, Lookup, Sample, TokenClaims};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::grants::service as grants;
use crate::domains::tenancy::service as tenancy;
use crate::http::auth::Caller;
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::placement::{self as repo, PlacementRow};
use aurora_db::repo::users::UserRow;
use aurora_db::repo::{connections, members, secrets, users, workspaces};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use uuid::Uuid;

/// Default template of a new placement.
pub const DEFAULT_TEMPLATE: &str = "{preferred_username}|{email_local}";
const MAX_SHARED_WORKSPACES: usize = 50;
const MAX_STARTER_GRANTS: usize = 50;
const PERSONAL_ORG_LIST_CAP: i64 = 100;

#[derive(Serialize, Deserialize, Clone)]
struct SharedWs {
    workspace_id: Uuid,
    role: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct Starter {
    kind: String,
    id: Uuid,
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, msg)])
}

/// The actor of everything placement writes at sign-in.
pub fn actor() -> AuditActor {
    AuditActor::system("placement")
}

// ---- sign-in ----------------------------------------------------------------------------

/// How a **new** user under `auth.signup_mode = open` is handled (D-140 d).
#[derive(Debug, PartialEq, Eq)]
pub enum OpenPlan {
    /// Unsaved or `personal_org`: the user gets their own organization (the pre-D-140 behaviour).
    OwnOrg,
    /// `shared_org` with its default organization: the placement step admits and places.
    Placed,
    /// `none`, or `shared_org` whose default org is gone: only invitation, role claim or JIT admit.
    Closed,
    /// Nothing saved and no organization exists: only instance admins may sign in yet.
    SetupPending,
}

pub async fn open_plan(
    conn: &mut sqlx::PgConnection,
    privileged: bool,
) -> Result<OpenPlan, aurora_db::DbError> {
    Ok(match repo::get(&mut *conn).await? {
        None => {
            if !privileged && repo::org_count(&mut *conn).await? == 0 {
                OpenPlan::SetupPending
            } else {
                OpenPlan::OwnOrg
            }
        }
        Some(p) => match PlacementMode::parse(&p.mode) {
            PlacementMode::PersonalOrg => OpenPlan::OwnOrg,
            PlacementMode::SharedOrg if p.default_org_id.is_some() => OpenPlan::Placed,
            _ => OpenPlan::Closed,
        },
    })
}

/// What the sign-in knows about the user from the **validated** token.
pub struct SignInClaims<'a> {
    pub email: &'a str,
    /// The ID-token payload (`Null` when it could not be read: template variables then have no
    /// value and the fallback chain applies).
    pub claims: &'a Value,
}

async fn audit_tx(tx: &mut audit::Tx, ev: AuditEvent) -> Result<(), ApiError> {
    audit::insert_tx(tx, &ev).await.map_err(|e| {
        tracing::error!("placement: audit: {e}");
        audit::unavailable()
    })?;
    Ok(())
}

/// The placement step of the sign-in transaction (after invitation, claims and JIT).
///
/// * A user created by this sign-in joins the default organization and the shared workspaces.
/// * Every sign-in (new or later) gives an active member of the default organization their
///   personal workspace when personal workspaces are on and none is recorded for them. The
///   workspace is found **only** by `personal_owner_user_id` (SEC-79).
///
/// Existing users are never moved into the organization.
pub async fn apply_signin(
    tx: &mut audit::Tx,
    state: &AppState,
    user: &mut UserRow,
    created: bool,
    who: &SignInClaims<'_>,
) -> Result<(), ApiError> {
    let Some(row) = repo::get(&mut **tx).await? else {
        return Ok(());
    };
    if PlacementMode::parse(&row.mode) != PlacementMode::SharedOrg {
        return Ok(());
    }
    let Some(org_id) = row.default_org_id else {
        return Ok(());
    };
    let member = members::get(&mut **tx, org_id, user.id).await?;
    match member {
        None if created => {
            members::insert(&mut **tx, org_id, user.id, &row.default_org_role).await?;
            audit_tx(
                tx,
                AuditEvent::new(actor(), "member.add", Outcome::Success)
                    .org(org_id)
                    .target("user", user.id)
                    .metadata(json!({"role": row.default_org_role, "via": "placement"})),
            )
            .await?;
        }
        None => return Ok(()),
        Some(m) if m.status != "active" => return Ok(()),
        Some(_) => {}
    }
    if created {
        place_in_shared_workspaces(tx, &row, org_id, user.id).await?;
        if user.default_org_id.is_none() {
            users::set_default_org(&mut **tx, user.id, org_id).await?;
            user.default_org_id = Some(org_id);
        }
    }
    if row.personal_enabled {
        ensure_personal_workspace(tx, state, &row, org_id, user, who).await?;
    }
    Ok(())
}

async fn place_in_shared_workspaces(
    tx: &mut audit::Tx,
    row: &PlacementRow,
    org_id: Uuid,
    user: Uuid,
) -> Result<(), ApiError> {
    let shared: Vec<SharedWs> =
        serde_json::from_value(row.shared_workspaces.clone()).unwrap_or_default();
    let ids: Vec<Uuid> = shared.iter().map(|s| s.workspace_id).collect();
    let have: HashSet<Uuid> = workspaces::roles_of(&mut **tx, user, &ids)
        .await?
        .into_iter()
        .map(|(w, _)| w)
        .collect();
    for s in shared {
        // Never downgrade a role another source gave; never cross organizations; never hand out
        // somebody's personal workspace.
        if have.contains(&s.workspace_id) {
            continue;
        }
        let Some(ws) = workspaces::get(&mut **tx, s.workspace_id).await? else {
            tracing::warn!("placement: shared workspace {} is gone", s.workspace_id);
            continue;
        };
        if ws.org_id != org_id
            || repo::personal_owner_of(&mut **tx, ws.id).await?.is_some()
            || !matches!(s.role.as_str(), "member" | "approver")
        {
            tracing::warn!("placement: shared workspace {} is not usable", ws.id);
            continue;
        }
        workspaces::bind_user(&mut **tx, ws.id, user, &s.role).await?;
        audit_tx(
            tx,
            AuditEvent::new(actor(), "workspace_access.set", Outcome::Success)
                .org(org_id)
                .workspace(ws.id)
                .target("user", user)
                .metadata(json!({"role": s.role, "via": "placement"})),
        )
        .await?;
    }
    Ok(())
}

async fn ensure_personal_workspace(
    tx: &mut audit::Tx,
    state: &AppState,
    row: &PlacementRow,
    org_id: Uuid,
    user: &UserRow,
    who: &SignInClaims<'_>,
) -> Result<(), ApiError> {
    // Serialize per organization: two first sign-ins never pick the same slug, and one user's
    // concurrent sign-ins never create two workspaces.
    repo::lock_org(&mut **tx, org_id).await?;
    if repo::personal_workspace(&mut **tx, org_id, user.id)
        .await?
        .is_some()
    {
        return Ok(());
    }
    let name = template::parse(&row.name_template)
        .ok()
        .and_then(|t| {
            t.render(&TokenClaims {
                claims: who.claims,
                email: who.email,
            })
            .name
        })
        .unwrap_or_else(|| template::fallback_name(user.id));
    let (ws, agent) = tenancy::create_workspace_in_tx(
        tx,
        &state.config,
        org_id,
        user.id,
        &name,
        None,
        None,
        None,
    )
    .await?;
    repo::set_personal_owner(&mut **tx, ws.id, user.id).await?;
    if row.personal_role != "owner" {
        workspaces::bind_user(&mut **tx, ws.id, user.id, &row.personal_role).await?;
    }
    audit_tx(
        tx,
        AuditEvent::new(actor(), "workspace.create", Outcome::Success)
            .org(org_id)
            .workspace(ws.id)
            .target("workspace", ws.id)
            .metadata(json!({
                "slug": ws.slug, "via": "placement", "personal_owner_user_id": user.id,
                "role": row.personal_role
            })),
    )
    .await?;
    let starters: Vec<Starter> =
        serde_json::from_value(row.starter_grants.clone()).unwrap_or_default();
    for s in starters {
        let res = match s.kind.as_str() {
            "secret" => grants::grant_secret_as_system(tx, state, &agent, s.id, actor())
                .await
                .map(|w| w.event),
            "connection" => grants::grant_connection_as_system(tx, state, &agent, s.id, actor())
                .await
                .map(|w| w.event),
            _ => Err(ApiError::not_found("Unknown credential kind.")),
        };
        match res {
            Ok(Some(mut ev)) => {
                ev.metadata["starter"] = json!(true);
                audit_tx(tx, ev).await?;
            }
            Ok(None) => {}
            // A credential that was deleted or disabled since the admin saved must not lock
            // people out: skip it and leave a trail.
            Err(e) => {
                tracing::warn!(
                    "placement: starter grant {} {} skipped: {e:?}",
                    s.kind,
                    s.id
                );
                audit_tx(
                    tx,
                    AuditEvent::new(actor(), "grant.create", Outcome::Denied)
                        .org(org_id)
                        .workspace(ws.id)
                        .target("agent", agent.id)
                        .metadata(json!({"starter": true, "kind": s.kind, "id": s.id,
                                         "reason": "credential_unavailable"})),
                )
                .await?;
            }
        }
    }
    Ok(())
}

// ---- admin ------------------------------------------------------------------------------

fn signup_mode_name(state: &AppState) -> &'static str {
    match state.config.signup_mode {
        aurora_config::SignupMode::Open => "open",
        aurora_config::SignupMode::InviteOnly => "invite_only",
        aurora_config::SignupMode::SsoJit => "sso_jit",
    }
}

fn default_org_role(s: &str) -> DefaultOrgRole {
    if s == "admin" {
        DefaultOrgRole::Admin
    } else {
        DefaultOrgRole::Member
    }
}

fn personal_role(s: &str) -> PersonalRole {
    match s {
        "approver" => PersonalRole::Approver,
        "member" => PersonalRole::Member,
        _ => PersonalRole::Owner,
    }
}

pub async fn view(state: &AppState) -> Result<PlacementView, ApiError> {
    let row = repo::get(&state.pool).await?;
    let saved = row.is_some();
    let open = state.config.signup_mode == aurora_config::SignupMode::Open;
    let mut warnings = Vec::new();
    let mut v = PlacementView {
        saved,
        signup_mode: signup_mode_name(state).to_owned(),
        // Unsaved: today's behaviour (`personal_org` under `open`, otherwise no placement).
        mode: if open {
            PlacementMode::PersonalOrg
        } else {
            PlacementMode::None
        },
        default_org: None,
        default_org_role: DefaultOrgRole::Member,
        shared_workspaces: vec![],
        personal_workspace: PersonalWorkspaceView {
            enabled: false,
            role: PersonalRole::Owner,
            name_template: DEFAULT_TEMPLATE.to_owned(),
        },
        starter_grants: vec![],
        personal_orgs: PersonalOrgs {
            count: repo::count_personal_orgs(&state.pool).await?,
            items: repo::list_personal_orgs(&state.pool, PERSONAL_ORG_LIST_CAP)
                .await?
                .into_iter()
                .map(|o| PersonalOrgView {
                    org_id: o.org_id,
                    slug: o.slug,
                    name: o.name,
                    owner_email: o.owner_email,
                    created_at: o.created_at,
                })
                .collect(),
        },
        warnings: vec![],
        updated_at: None,
        updated_by: None,
    };
    if let Some(r) = row {
        v.mode = PlacementMode::parse(&r.mode);
        v.default_org_role = default_org_role(&r.default_org_role);
        v.personal_workspace = PersonalWorkspaceView {
            enabled: r.personal_enabled,
            role: personal_role(&r.personal_role),
            name_template: r.name_template.clone(),
        };
        v.updated_at = Some(r.updated_at);
        v.updated_by = r.updated_by;
        if let Some(org) = r.default_org_id {
            if let Some((slug, name)) = repo::org_brief(&state.pool, org).await? {
                v.default_org = Some(OrgRef {
                    id: org,
                    slug,
                    name,
                });
            }
        } else if v.mode == PlacementMode::SharedOrg {
            warnings.push("default_org_missing".to_owned());
        }
        let shared: Vec<SharedWs> = serde_json::from_value(r.shared_workspaces).unwrap_or_default();
        for s in shared {
            if let Some(ws) = workspaces::get(&state.pool, s.workspace_id).await? {
                v.shared_workspaces.push(SharedWorkspaceView {
                    workspace_id: ws.id,
                    slug: ws.slug,
                    name: ws.name,
                    role: if s.role == "approver" {
                        SharedWorkspaceRole::Approver
                    } else {
                        SharedWorkspaceRole::Member
                    },
                });
            }
        }
        let starters: Vec<Starter> = serde_json::from_value(r.starter_grants).unwrap_or_default();
        for s in starters {
            match s.kind.as_str() {
                "secret" => {
                    if let Some(x) = secrets::get(&state.pool, s.id).await? {
                        v.starter_grants.push(StarterGrantView {
                            kind: StarterKind::Secret,
                            id: x.id,
                            name: x.name,
                            detail: x.kind,
                        });
                    }
                }
                "connection" => {
                    if let Some(x) = connections::get(&state.pool, s.id).await? {
                        v.starter_grants.push(StarterGrantView {
                            kind: StarterKind::Connection,
                            id: x.id,
                            name: x.label,
                            detail: x.app_id,
                        });
                    }
                }
                _ => {}
            }
        }
    } else {
        warnings.push("placement_unsaved".to_owned());
    }
    if open && v.mode == PlacementMode::PersonalOrg {
        warnings.push("per_user_orgs_active".to_owned());
    }
    v.warnings = warnings;
    Ok(v)
}

pub async fn put(
    state: &AppState,
    c: &Caller,
    req: PutPlacement,
) -> Result<PlacementView, ApiError> {
    let shared_mode = req.mode == PlacementMode::SharedOrg;
    let personal = req.personal_workspace.clone();
    if !shared_mode
        && (req.default_org_id.is_some()
            || req.create_default_org.is_some()
            || !req.shared_workspaces.is_empty()
            || !req.starter_grants.is_empty()
            || personal.as_ref().is_some_and(|p| p.enabled))
    {
        return Err(bad(
            "/mode",
            "organization, workspaces, personal workspace and starter grants apply only to shared_org",
        ));
    }
    let personal_enabled = personal.as_ref().is_some_and(|p| p.enabled);
    let name_template = personal
        .as_ref()
        .and_then(|p| p.name_template.clone())
        .unwrap_or_else(|| DEFAULT_TEMPLATE.to_owned());
    if let Err(m) = template::parse(&name_template) {
        return Err(bad("/personal_workspace/name_template", &m));
    }
    if req.shared_workspaces.len() > MAX_SHARED_WORKSPACES {
        return Err(bad("/shared_workspaces", "too many workspaces"));
    }
    if req.starter_grants.len() > MAX_STARTER_GRANTS {
        return Err(bad("/starter_grants", "too many starter grants"));
    }
    if !personal_enabled && !req.starter_grants.is_empty() {
        return Err(bad(
            "/starter_grants",
            "starter grants go to personal workspaces; turn them on first",
        ));
    }

    let mut tx = audit::begin(state).await?;
    let before = repo::get(&mut *tx).await?;
    let mut org_id = None;
    let mut created_org = None;
    if shared_mode {
        org_id = Some(
            match (req.default_org_id, req.create_default_org.as_deref()) {
                (Some(id), None) => {
                    if repo::org_brief(&mut *tx, id).await?.is_none() {
                        return Err(bad("/default_org_id", "unknown organization"));
                    }
                    id
                }
                (None, Some(name)) => {
                    let o =
                        tenancy::create_organization(&mut tx, &state.config, c.user_id, name, None)
                            .await?;
                    created_org = Some(o.org.clone());
                    o.org.id
                }
                _ => {
                    return Err(bad(
                        "/default_org_id",
                        "give exactly one of default_org_id and create_default_org",
                    ));
                }
            },
        );
    }
    let mut seen = HashSet::new();
    let mut shared = Vec::new();
    for (i, s) in req.shared_workspaces.iter().enumerate() {
        let path = format!("/shared_workspaces/{i}/workspace_id");
        if !seen.insert(s.workspace_id) {
            return Err(bad(&path, "listed twice"));
        }
        let ws = workspaces::get(&mut *tx, s.workspace_id)
            .await?
            .filter(|w| Some(w.org_id) == org_id)
            .ok_or_else(|| bad(&path, "not a workspace of the default organization"))?;
        if repo::personal_owner_of(&mut *tx, ws.id).await?.is_some() {
            return Err(bad(
                &path,
                "a personal workspace cannot be shared with everyone",
            ));
        }
        shared.push(SharedWs {
            workspace_id: ws.id,
            role: s.role.as_str().to_owned(),
        });
    }
    let mut starters = Vec::new();
    let mut seen = HashSet::new();
    for (i, s) in req.starter_grants.iter().enumerate() {
        let path = format!("/starter_grants/{i}/id");
        if !seen.insert((s.kind.as_str(), s.id)) {
            return Err(bad(&path, "listed twice"));
        }
        let ok = match s.kind {
            StarterKind::Secret => secrets::get(&mut *tx, s.id).await?.is_some_and(|x| {
                Some(x.org_id) == org_id && x.workspace_id.is_none() && x.status == "active"
            }),
            StarterKind::Connection => connections::get(&mut *tx, s.id).await?.is_some_and(|x| {
                Some(x.org_id) == org_id && x.workspace_id.is_none() && x.status != "orphaned"
            }),
        };
        if !ok {
            return Err(bad(
                &path,
                "must be an active organization-level credential of the default organization",
            ));
        }
        starters.push(Starter {
            kind: s.kind.as_str().to_owned(),
            id: s.id,
        });
    }
    let row = PlacementRow {
        mode: req.mode.as_str().to_owned(),
        default_org_id: org_id,
        default_org_role: req
            .default_org_role
            .unwrap_or(DefaultOrgRole::Member)
            .as_str()
            .to_owned(),
        shared_workspaces: serde_json::to_value(&shared).unwrap_or_else(|_| json!([])),
        personal_enabled,
        personal_role: personal
            .as_ref()
            .and_then(|p| p.role)
            .unwrap_or(PersonalRole::Owner)
            .as_str()
            .to_owned(),
        name_template,
        starter_grants: serde_json::to_value(&starters).unwrap_or_else(|_| json!([])),
        updated_by: Some(c.user_id),
        updated_at: chrono::Utc::now(),
    };
    repo::save(&mut *tx, &row, c.user_id).await?;
    if let Some(o) = &created_org {
        audit_tx(
            &mut tx,
            AuditEvent::new(
                AuditActor::instance_admin(c),
                "org.create",
                Outcome::Success,
            )
            .org(o.id)
            .target("organization", o.id)
            .metadata(json!({"slug": o.slug, "via": "placement"})),
        )
        .await?;
    }
    let ev = AuditEvent::new(
        AuditActor::instance_admin(c),
        "placement.update",
        Outcome::Success,
    )
    .target("placement", "instance")
    .metadata(json!({
        "from_mode": before.as_ref().map(|b| b.mode.clone()),
        "mode": row.mode, "default_org_id": row.default_org_id,
        "default_org_role": row.default_org_role,
        "shared_workspaces": shared.len(), "personal_enabled": row.personal_enabled,
        "personal_role": row.personal_role, "name_template": row.name_template,
        "starter_grants": starters.len(),
    }));
    audit::commit_critical(tx, &ev).await?;
    view(state).await
}

pub async fn choices(state: &AppState, org: Uuid) -> Result<PlacementChoices, ApiError> {
    if repo::org_brief(&state.pool, org).await?.is_none() {
        return Err(ApiError::not_found("Organization not found."));
    }
    let c = repo::choices(&state.pool, org).await?;
    Ok(PlacementChoices {
        workspaces: c
            .workspaces
            .into_iter()
            .map(|(id, slug, name)| ChoiceWorkspace { id, slug, name })
            .collect(),
        secrets: c
            .secrets
            .into_iter()
            .map(|(id, name, kind)| ChoiceSecret { id, name, kind })
            .collect(),
        connections: c
            .connections
            .into_iter()
            .map(|(id, label, app_id)| ChoiceConnection { id, label, app_id })
            .collect(),
    })
}

struct Profile<'a> {
    name: &'a str,
    email: &'a str,
}

impl Lookup for Profile<'_> {
    fn get(&self, v: &template::Var) -> Option<String> {
        match v {
            template::Var::Name => Some(self.name.to_owned()),
            template::Var::EmailLocal => self.email.split('@').next().map(str::to_owned),
            _ => None,
        }
    }
}

pub async fn preview(
    state: &AppState,
    c: &Caller,
    q: PreviewQuery,
) -> Result<PlacementPreview, ApiError> {
    let tpl_text = match q.template {
        Some(t) => t,
        None => repo::get(&state.pool)
            .await?
            .map_or_else(|| DEFAULT_TEMPLATE.to_owned(), |r| r.name_template),
    };
    let tpl = template::parse(&tpl_text).map_err(|m| bad("/template", &m))?;
    let sample = q.sample.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if sample.is_some_and(|s| s.chars().count() > 200 || s.chars().any(char::is_control)) {
        return Err(bad(
            "/sample",
            "must be at most 200 characters, no control characters",
        ));
    }
    let rendered = match sample {
        Some(s) => tpl.render(&Sample(s)),
        None => {
            let u = users::find_by_id(&state.pool, c.user_id)
                .await?
                .ok_or_else(|| ApiError::not_found("User not found."))?;
            tpl.render(&Profile {
                name: &u.display_name,
                email: &u.email,
            })
        }
    };
    let name = rendered
        .name
        .clone()
        .unwrap_or_else(|| template::fallback_name(c.user_id));
    Ok(PlacementPreview {
        slug: tenancy::derive_slug_base(&name, "workspace"),
        name,
        used: rendered.used,
        unresolved: rendered.unresolved,
    })
}
