//! New-user placement (P4-ONB-1, D-140, SEC-79): the admin's settings and the sign-in step that
//! applies them inside the sign-in transaction.

use super::dto::*;
use super::template::{self, Lookup, Sample, TokenClaims};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::grants::service as grants;
use crate::domains::scim::gates;
use crate::domains::tenancy::service as tenancy;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::repo::placement::{self as repo, PlacementRow};
use aurora_db::repo::users::UserRow;
use aurora_db::repo::{connections, identities, members, secrets, users, workspaces};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use uuid::Uuid;

/// Default template of a new placement (D-162: name from the OIDC-unique `sub`).
pub const DEFAULT_TEMPLATE: &str = "{sub}|{email_local}";
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
    ApiError::invalid_field("Invalid request.", path, msg)
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
    /// Nothing saved on a fresh instance (FIX-116: even after an administrator's own sign-in
    /// created an organization): only instance admins may sign in until a placement is saved.
    SetupPending,
}

pub async fn open_plan(
    conn: &mut sqlx::PgConnection,
    privileged: bool,
) -> Result<OpenPlan, aurora_db::DbError> {
    Ok(match repo::get(&mut *conn).await? {
        None => {
            if !privileged && repo::setup_pending(&mut *conn).await? {
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
    let mut member = members::get(&mut **tx, org_id, user.id).await?;
    if member.is_none() && created {
        // (FIX-080, SEC-73) Placement creates a membership like JIT and invitations do, so it
        // passes the same admission gate and seat cap, under the org lock (taken first, the
        // membership re-read under it).
        aurora_db::repo::groups::lock_org(tx, org_id).await?;
        member = members::get(&mut **tx, org_id, user.id).await?;
        if member.is_none()
            && let Some(reason) = gates::admit_new_member(tx, org_id, user.id).await?
        {
            // The person signs in without this membership (nothing else of the placement
            // applies without it); the denied row goes on its own connection.
            gates::audit_denied_detached(&state.pool, org_id, user.id, "placement", reason).await?;
            return Ok(());
        }
    }
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
            || repo::is_personal(&mut **tx, ws.id).await?
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
    let lookup = TokenClaims {
        claims: who.claims,
        email: who.email,
    };
    // The template still sets the slug (D-162); the name is the person's own (C-923).
    let slug_text = template::parse(&row.name_template)
        .ok()
        .and_then(|t| t.render(&lookup).name)
        .unwrap_or_else(|| template::fallback_name(user.id));
    // (C-1064) "<First>'s workspace"; the slug is still the template's.
    let name = template::personal_workspace_name(&lookup).unwrap_or_else(|| slug_text.clone());
    let (ws, agent) = tenancy::create_workspace_named_in_tx(
        tx,
        &state.ops(),
        org_id,
        user.id,
        &name,
        tenancy::SlugFrom::Text(&slug_text),
        None,
        None,
    )
    .await?;
    repo::set_personal_owner(&mut **tx, ws.id, user.id).await?;
    // (FIX-234, D-480) The person is always the owner of their personal workspace: the placement
    // setting's `personal_role` no longer picks the binding, the creation path's `owner` binding
    // stands. The setting is retired (`GET` says `owner`, `PUT` ignores it) and a later cleanup
    // drops the column.
    audit_tx(
        tx,
        AuditEvent::new(actor(), "workspace.create", Outcome::Success)
            .org(org_id)
            .workspace(ws.id)
            .target("workspace", ws.id)
            .metadata(json!({
                "slug": ws.slug, "via": "placement", "personal_owner_user_id": user.id,
                "role": "owner"
            })),
    )
    .await?;
    let starters: Vec<Starter> =
        serde_json::from_value(row.starter_grants.clone()).unwrap_or_default();
    apply_starters(tx, state, &agent, &starters, "signin", false).await?;
    Ok(())
}

/// What happened to one starter credential on one agent.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Applied {
    Granted,
    AlreadyHad,
    Skipped,
}

/// Give `agent` the starter credentials (C-1061): the one function the sign-in placement, a new
/// agent, a placement save and the backfill share. One savepoint per grant inside the caller's
/// transaction (a refusal, even one that was a failed statement, is rolled back to it and
/// skipped, and the transaction stays usable), the grant written by the same
/// `grant_*_as_system` the sign-in always used, audited `grant.create` with
/// `actor_label = placement`, `starter: true` and `via`; a refused credential is skipped and
/// audited `Denied`. Returns one outcome per starter, in order.
async fn apply_starters(
    tx: &mut audit::Tx,
    state: &AppState,
    agent: &aurora_db::repo::agents::AgentRow,
    starters: &[Starter],
    via: &str,
    backfill: bool,
) -> Result<Vec<Applied>, ApiError> {
    let mut out = Vec::with_capacity(starters.len());
    for s in starters {
        sqlx::query("SAVEPOINT starter_grant")
            .execute(&mut **tx)
            .await
            .map_err(aurora_db::DbError::from)?;
        let res = match s.kind.as_str() {
            "secret" => grants::grant_secret_as_system(tx, agent, s.id, actor())
                .await
                .map(|w| w.event),
            "connection" => grants::grant_connection_as_system(tx, state, agent, s.id, actor())
                .await
                .map(|w| w.event),
            _ => Err(ApiError::not_found("Unknown credential kind.")),
        };
        match res {
            Ok(w) => {
                sqlx::query("RELEASE SAVEPOINT starter_grant")
                    .execute(&mut **tx)
                    .await
                    .map_err(aurora_db::DbError::from)?;
                if let Some(mut ev) = w {
                    ev.metadata["starter"] = json!(true);
                    ev.metadata["via"] = json!(via);
                    if backfill {
                        ev.metadata["backfill"] = json!(true);
                    }
                    audit_tx(tx, ev).await?;
                    out.push(Applied::Granted);
                } else {
                    out.push(Applied::AlreadyHad);
                }
            }
            // A server-side failure (database, internal) is not a refusal: it propagates and
            // the caller fails with its real cause instead of logging a skipped grant.
            Err(e) if e.status().is_server_error() => return Err(e),
            // A credential that was deleted or disabled since the admin saved, or an app that
            // became `custom_only` after the starter was saved, must not lock people out: skip
            // it and leave a trail with the real reason.
            Err(e) => {
                sqlx::query("ROLLBACK TO SAVEPOINT starter_grant")
                    .execute(&mut **tx)
                    .await
                    .map_err(aurora_db::DbError::from)?;
                let reason = if e.code() == "full_grant_forbidden" {
                    "full_grant_forbidden"
                } else {
                    "credential_unavailable"
                };
                tracing::warn!(
                    "placement: starter grant {} {} skipped ({reason}): {e:?}",
                    s.kind,
                    s.id
                );
                audit_tx(
                    tx,
                    AuditEvent::new(actor(), "grant.create", Outcome::Denied)
                        .org(agent.org_id)
                        .workspace(agent.workspace_id)
                        .target("agent", agent.id)
                        .metadata(json!({"starter": true, "via": via, "kind": s.kind,
                                         "id": s.id, "reason": reason})),
                )
                .await?;
                out.push(Applied::Skipped);
            }
        }
    }
    Ok(out)
}

/// A new hosted agent of the placement's default organization gets the starter grants in its
/// creation transaction (C-1061, `via: "new_agent"`). Nothing for a BYO agent, another
/// organization, or a placement that is not `shared_org` with personal workspaces on.
pub async fn apply_new_agent_starters(
    tx: &mut audit::Tx,
    state: &AppState,
    agent: &aurora_db::repo::agents::AgentRow,
) -> Result<(), ApiError> {
    if agent.kind != "hosted" {
        return Ok(());
    }
    let Some(row) = repo::get(&mut **tx).await? else {
        return Ok(());
    };
    if PlacementMode::parse(&row.mode) != PlacementMode::SharedOrg
        || !row.personal_enabled
        || row.default_org_id != Some(agent.org_id)
    {
        return Ok(());
    }
    let starters: Vec<Starter> =
        serde_json::from_value(row.starter_grants.clone()).unwrap_or_default();
    if starters.is_empty() {
        return Ok(());
    }
    apply_starters(tx, state, agent, &starters, "new_agent", false).await?;
    Ok(())
}

// ---- admin ------------------------------------------------------------------------------

fn signup_mode_name(state: &AppState) -> &'static str {
    match state.auth().signup_mode {
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

pub async fn view(state: &AppState) -> Result<PlacementView, ApiError> {
    let row = repo::get(&state.pool).await?;
    let saved = row.is_some();
    let open = state.auth().signup_mode == aurora_config::SignupMode::Open;
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
        starter_applied: vec![],
        starter_pending: 0,
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
            // (FIX-234, D-480) The person is always the owner of their personal workspace: the
            // retired `personal_role` setting reads back `owner` for one release, then the field
            // goes.
            role: PersonalRole::Owner,
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
                        tenancy::create_organization(&mut tx, &state.ops(), c.user_id, name, None)
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
        if repo::is_personal(&mut *tx, ws.id).await? {
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
            StarterKind::Connection => {
                match connections::get(&mut *tx, s.id).await? {
                    // (FIX-038, P4-GRANTS D-123) A `custom_only` app is never granted `full`
                    // anywhere, and that is the mode a starter connection is granted in: saving
                    // one would make every sign-in skip it. Refused at save time, decided by the
                    // same app restrictions the grants writer reads.
                    Some(x)
                        if Some(x.org_id) == org_id
                            && x.workspace_id.is_none()
                            && x.status != "orphaned" =>
                    {
                        if grants::app_restrictions(state, &x.app_id)
                            .await?
                            .custom_only
                        {
                            return Err(bad(
                                &path,
                                &format!(
                                    "{} only allows custom access: a starter grant would be full access.",
                                    x.app_id
                                ),
                            ));
                        }
                        true
                    }
                    _ => false,
                }
            }
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
        // (FIX-234, D-480) `personal_role` is retired: the field is accepted and stored for one
        // release but nothing reads it — the person is always their personal workspace's owner.
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
    // C-1061: a grant this save adds reaches the agents that already exist.
    let (applied, pending) = match org_id {
        Some(org) if personal_enabled => {
            // A different default organization starts from nothing: every starter is new there.
            let previous: Vec<Starter> = before
                .as_ref()
                .filter(|b| b.default_org_id == Some(org))
                .map(|b| serde_json::from_value(b.starter_grants.clone()).unwrap_or_default())
                .unwrap_or_default();
            let added: Vec<Starter> = starters
                .iter()
                .filter(|s| !previous.iter().any(|p| p.kind == s.kind && p.id == s.id))
                .cloned()
                .collect();
            if added.is_empty() {
                (vec![], 0)
            } else {
                apply_added_starters(&mut tx, state, org, &added).await?
            }
        }
        _ => (vec![], 0),
    };
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
    let mut v = view(state).await?;
    v.starter_applied = applied;
    v.starter_pending = pending;
    Ok(v)
}

/// Most agents one placement save reaches (C-1061); the rest are `starter_pending`.
const MAX_AGENTS_PER_SAVE: i64 = 2_000;

/// Give the starter grants a save **added** to every existing hosted agent of the default
/// organization (C-1061, `via: "placement_save"`), inside the save's transaction.
async fn apply_added_starters(
    tx: &mut audit::Tx,
    state: &AppState,
    org: Uuid,
    added: &[Starter],
) -> Result<(Vec<StarterApplied>, u32), ApiError> {
    let ids = repo::hosted_agents_of_org(&mut **tx, org, MAX_AGENTS_PER_SAVE + 1).await?;
    let pending =
        u32::try_from(ids.len().saturating_sub(MAX_AGENTS_PER_SAVE as usize)).unwrap_or(u32::MAX);
    let mut out: Vec<StarterApplied> = Vec::new();
    for s in added {
        let (kind, name) = match s.kind.as_str() {
            "secret" => (
                StarterKind::Secret,
                secrets::get(&mut **tx, s.id).await?.map(|x| x.name),
            ),
            _ => (
                StarterKind::Connection,
                connections::get(&mut **tx, s.id).await?.map(|x| x.label),
            ),
        };
        out.push(StarterApplied {
            kind,
            id: s.id,
            name: name.unwrap_or_default(),
            agents_granted: 0,
            agent_names: vec![],
            already_had: 0,
        });
    }
    for agent_id in ids.into_iter().take(MAX_AGENTS_PER_SAVE as usize) {
        let Some(agent) = aurora_db::repo::agents::get(&mut **tx, agent_id).await? else {
            continue;
        };
        let res = apply_starters(tx, state, &agent, added, "placement_save", true).await?;
        for (a, r) in out.iter_mut().zip(res) {
            match r {
                Applied::Granted => {
                    a.agents_granted += 1;
                    if a.agent_names.len() < 3 {
                        a.agent_names.push(agent.name.clone());
                    }
                }
                Applied::AlreadyHad => a.already_had += 1,
                Applied::Skipped => {}
            }
        }
    }
    Ok((out, pending))
}

/// The saved placement's starter credentials as `(secret ids, connection ids)`.
pub fn starter_ids(row: &PlacementRow) -> (Vec<Uuid>, Vec<Uuid>) {
    let starters: Vec<Starter> =
        serde_json::from_value(row.starter_grants.clone()).unwrap_or_default();
    let pick = |kind: &str| {
        starters
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.id)
            .collect()
    };
    (pick("secret"), pick("connection"))
}

/// How many personal workspaces of the default organization lack a starter grant (C-922).
pub async fn personal_without_starter(state: &AppState) -> Result<i64, ApiError> {
    let Some(row) = repo::get(&state.pool).await? else {
        return Ok(0);
    };
    let Some(org) = row.default_org_id else {
        return Ok(0);
    };
    let (secrets, connections) = starter_ids(&row);
    if secrets.is_empty() && connections.is_empty() {
        return Ok(0);
    }
    Ok(repo::count_personal_without_starter(&state.pool, org, &secrets, &connections).await?)
}

/// How many hosted agents of the default organization lack a starter grant (C-1061).
pub async fn agents_without_starter(state: &AppState) -> Result<i64, ApiError> {
    let Some(row) = repo::get(&state.pool).await? else {
        return Ok(0);
    };
    let Some(org) = row.default_org_id else {
        return Ok(0);
    };
    let (secrets, connections) = starter_ids(&row);
    if secrets.is_empty() && connections.is_empty() {
        return Ok(0);
    }
    Ok(repo::count_agents_without_starter(&state.pool, org, &secrets, &connections).await?)
}

/// Give the placement's starter grants to every hosted agent of the default organization
/// (C-922, C-1061). Idempotent: a grant that exists is left as
/// it is and counted as skipped; one the normal grant path refuses is skipped too. Each grant is
/// its own transaction with its own audit row (`actor_label = placement`, `backfill = true`).
pub async fn backfill_starter_grants(
    state: &AppState,
    c: &Caller,
    req: BackfillStarterGrants,
) -> Result<BackfillResult, ApiError> {
    let row = repo::get(&state.pool)
        .await?
        .filter(|r| PlacementMode::parse(&r.mode) == PlacementMode::SharedOrg)
        .ok_or_else(|| bad("/", "save a shared organization placement first"))?;
    let org_id = row
        .default_org_id
        .ok_or_else(|| bad("/", "the default organization no longer exists"))?;
    let (mut secret_ids, mut connection_ids) = starter_ids(&row);
    if let Some(only) = &req.credential_ids {
        for (i, id) in only.iter().enumerate() {
            if !secret_ids.contains(id) && !connection_ids.contains(id) {
                return Err(bad(
                    &format!("/credential_ids/{i}"),
                    "is not one of the placement's starter grants",
                ));
            }
        }
        secret_ids.retain(|id| only.contains(id));
        connection_ids.retain(|id| only.contains(id));
    }
    let mut out = BackfillResult {
        granted: 0,
        skipped: 0,
    };
    // (C-1061) every hosted agent of the default organization, not only the default agent of
    // each personal workspace.
    for agent_id in repo::hosted_agents_of_org(&state.pool, org_id, 100_000).await? {
        let Some(agent) = aurora_db::repo::agents::get(&state.pool, agent_id).await? else {
            continue;
        };
        let wanted = secret_ids
            .iter()
            .map(|id| ("secret", *id))
            .chain(connection_ids.iter().map(|id| ("connection", *id)));
        for (kind, id) in wanted {
            let mut tx = audit::begin(state).await?;
            let res = if kind == "secret" {
                grants::grant_secret_as_system(&mut tx, &agent, id, actor())
                    .await
                    .map(|w| w.event)
            } else {
                grants::grant_connection_as_system(&mut tx, state, &agent, id, actor())
                    .await
                    .map(|w| w.event)
            };
            match res {
                Ok(Some(mut ev)) => {
                    ev.metadata["starter"] = json!(true);
                    ev.metadata["backfill"] = json!(true);
                    ev.metadata["via"] = json!("backfill");
                    ev.metadata["requested_by"] = json!(c.user_id);
                    audit::commit_critical(tx, &ev).await?;
                    out.granted += 1;
                }
                // Already granted: nothing was written.
                Ok(None) => out.skipped += 1,
                Err(e) if e.status().is_server_error() => return Err(e),
                // A credential deleted or disabled since, or an app that became custom-only:
                // skipped, like the sign-in path does.
                Err(_) => out.skipped += 1,
            }
        }
    }
    Ok(out)
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
    /// The caller's first stored identity `(provider_id, subject)` (lowest provider id,
    /// D-162); `None` when the user has no linked identity (a pre-provisioned admin).
    identity: Option<(String, String)>,
}

impl Lookup for Profile<'_> {
    fn get(&self, v: &template::Var) -> Option<String> {
        match v {
            template::Var::Sub => self.identity.as_ref().map(|(_, s)| s.to_owned()),
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
            // `{sub}` resolves from the caller's stored identity row: already persisted
            // `(provider_id, subject)` (docs/03 §user_identities), no new claims retention;
            // lowest `provider_id` when the caller has several (D-162). `{claim:…}` stays
            // unresolved here (D-151(b)).
            let identity = identities::by_user(&state.pool, c.user_id)
                .await?
                .into_iter()
                .next();
            tpl.render(&Profile {
                name: &u.display_name,
                email: &u.email,
                identity,
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
