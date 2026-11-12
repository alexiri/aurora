//! Roles from IdP claims (P4-IDP-1, D-139, SEC-78): claim extraction, mapping and the sign-in sync.
//!
//! Values come only from the provider's **validated** ID token (or its userinfo response fetched
//! with that flow's access token) and are matched exactly against the operator's
//! `role_mappings`. The sync adds and removes only assignments whose `idp_source` is
//! `idp:<this provider>:…`; manual, JIT, invitation, e-mail-list and `bootstrap-admin` roles are
//! never changed or removed. Every change is audited in the sign-in transaction.
use super::service::Fail;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_config::OidcProvider;
use aurora_db::repo::idp_roles::{self as db, RoleRow, org_rank, ws_rank};
use aurora_db::repo::users::UserRow;
use aurora_db::repo::{members, notify_auth_revoked, orgs, users, workspaces};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

const MAX_VALUES: usize = 200;
const MAX_VALUE_LEN: usize = 200;

/// Read the role values at `path` (dot path) of a validated claims object. An array of strings,
/// or one string with space- or comma-separated values; anything else counts as no values
/// (logged once per provider).
pub fn extract_values(provider: &str, payload: &Value, path: &str) -> Vec<String> {
    let mut cur = payload;
    for seg in path.split('.') {
        match cur.get(seg) {
            Some(v) => cur = v,
            None => return vec![],
        }
    }
    let raw: Vec<String> = match cur {
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Value::String(s) => s
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => {
            warn_once(provider, path);
            vec![]
        }
    };
    let mut seen = HashSet::new();
    raw.into_iter()
        .filter(|v| {
            !v.is_empty()
                && v.len() <= MAX_VALUE_LEN
                && !v.chars().any(char::is_control)
                && seen.insert(v.clone())
        })
        .take(MAX_VALUES)
        .collect()
}

fn warn_once(provider: &str, path: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let set = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut s) = set.lock()
        && s.insert(provider.to_owned())
    {
        tracing::warn!(
            "oidc: provider `{provider}`: role claim `{path}` is neither a string array nor a string; ignored"
        );
    }
}

/// What the matched values grant, strongest per target.
#[derive(Debug, Default)]
pub struct Desired {
    /// The value that grants instance admin.
    pub admin: Option<String>,
    /// org slug → (role, value).
    pub orgs: BTreeMap<String, (String, String)>,
    /// (org slug, workspace slug) → (role, value).
    pub workspaces: BTreeMap<(String, String), (String, String)>,
    /// One entry per matched value (the claim debugger).
    pub granted: Vec<Value>,
}

impl Desired {
    /// At least one actual grant (the admission and `require_role` test).
    pub fn grants_anything(&self) -> bool {
        self.admin.is_some() || !self.orgs.is_empty() || !self.workspaces.is_empty()
    }
}

/// Exact, case-sensitive match of `values` against the provider's mappings.
pub fn compute(p: &OidcProvider, values: &[String]) -> Desired {
    let mut d = Desired::default();
    for m in &p.role_mappings {
        if !values.contains(&m.value) {
            continue;
        }
        if m.instance_admin && d.admin.is_none() {
            d.admin = Some(m.value.clone());
        }
        if let Some(o) = &m.org {
            let stronger = d
                .orgs
                .get(&o.slug)
                .is_none_or(|(r, _)| org_rank(&o.role) > org_rank(r));
            if stronger {
                d.orgs
                    .insert(o.slug.clone(), (o.role.clone(), m.value.clone()));
            }
            for w in &m.workspaces {
                let key = (o.slug.clone(), w.slug.clone());
                let stronger = d
                    .workspaces
                    .get(&key)
                    .is_none_or(|(r, _)| ws_rank(&w.role) > ws_rank(r));
                if stronger {
                    d.workspaces.insert(key, (w.role.clone(), m.value.clone()));
                }
            }
        }
        d.granted.push(json!({
            "value": m.value,
            "instance_admin": m.instance_admin,
            "org": m.org.as_ref().map(|o| json!({"slug": o.slug, "role": o.role})),
            "workspaces": m.workspaces.iter().map(|w| json!({"slug": w.slug, "role": w.role})).collect::<Vec<_>>(),
        }));
    }
    d
}

fn actor(p: &OidcProvider) -> AuditActor {
    AuditActor {
        kind: "service",
        id: None,
        label: format!("idp:{}", p.id),
        source: "system",
        ip: None,
    }
}

async fn log(tx: &mut sqlx::PgConnection, ev: AuditEvent) -> Result<(), Fail> {
    audit::insert_tx(&mut *tx, &ev)
        .await
        .map(|_| ())
        .map_err(|e| {
            tracing::error!("oidc: role sync audit: {e}");
            Fail("oidc_failed")
        })
}

fn title(slug: &str) -> String {
    let mut c = slug.replace('-', " ").chars().collect::<Vec<_>>();
    if let Some(f) = c.first_mut() {
        *f = f.to_ascii_uppercase();
    }
    c.into_iter().collect()
}

fn api_err(e: aurora_core::ApiError) -> Fail {
    tracing::error!("oidc: role sync: {e:?}");
    Fail("oidc_failed")
}

async fn is_last_owner(tx: &mut sqlx::PgConnection, org: Uuid, user: Uuid) -> Result<bool, Fail> {
    let owners = members::lock_active_owners(&mut *tx, org).await?;
    Ok(owners.len() <= 1 && owners.contains(&user))
}

async fn ensure_org(
    tx: &mut sqlx::PgConnection,
    state: &AppState,
    user: Uuid,
    slug: &str,
) -> Result<(Uuid, bool), Fail> {
    if let Some((id, _, _)) = orgs::get_by_slug(&mut *tx, slug).await? {
        return Ok((id, false));
    }
    let created = crate::domains::tenancy::service::create_organization(
        tx,
        &state.config,
        user,
        &title(slug),
        Some(slug),
    )
    .await
    .map_err(api_err)?;
    // The creating user is not an owner because of this: the mapping decides their role.
    members::remove(tx, created.org.id, user).await?;
    Ok((created.org.id, true))
}

async fn ensure_ws(
    tx: &mut sqlx::PgConnection,
    state: &AppState,
    org: Uuid,
    user: Uuid,
    slug: &str,
) -> Result<Uuid, Fail> {
    if let Some(id) = db::ws_lookup(&mut *tx, org, slug).await? {
        return Ok(id);
    }
    let (ws, _) = crate::domains::tenancy::service::create_workspace_in_tx(
        tx,
        &state.config,
        org,
        user,
        &title(slug),
        Some(slug),
        None,
        None,
    )
    .await
    .map_err(api_err)?;
    workspaces::unbind_user(&mut *tx, ws.id, user).await?;
    Ok(ws.id)
}

/// (FIX-050, D-165, C-630) The owner-integrity floor: after the sync's org step, an
/// organization the mappings of this provider touched (the orgs in `org_ids`) that ends the
/// sign-in transaction with no active owner gets its syncing user promoted to `owner` —
/// inside the transaction, with the claim-derived columns kept and audited
/// `member.update { via: "idp_sync", reason: "ownerless_org" }`. A suspended (or absent)
/// membership of the syncing user is never promoted: logged and skipped, the org stays
/// ownerless until an active member signs in and the admin overview surfaces it. The
/// promotion is what `apply_org` wrote for this sign-in (role and value passed in), so the
/// last-owner guard protects the floor exactly like any other owner.
async fn promote_to_owner_floor(
    tx: &mut sqlx::PgConnection,
    p: &OidcProvider,
    user: Uuid,
    org_ids: &BTreeMap<String, Uuid>,
) -> Result<(), Fail> {
    for (slug, &org) in org_ids {
        if members::org_has_active_owner(&mut *tx, org).await? {
            continue;
        }
        let Some(row) = db::org_row(&mut *tx, org, user).await? else {
            tracing::warn!(
                org = slug,
                "ownerless org after sync: the syncing user holds no membership; not promoted"
            );
            continue;
        };
        if row.status != "active" {
            tracing::warn!(
                org = slug,
                status = row.status,
                "ownerless org: the syncing user's membership is not active; not promoted"
            );
            continue;
        }
        // Keep the claim-derived columns (the claim still decides the base role on later
        // sign-ins; the row stays read-only per C-514) and raise only the effective role.
        let previous = row.role.clone();
        let new = RoleRow {
            role: "owner".to_owned(),
            status: row.status.clone(),
            idp_role: row.idp_role.clone(),
            idp_source: row.idp_source.clone(),
            base_role: row.base_role.clone(),
        };
        db::org_write(&mut *tx, org, user, &new).await?;
        notify_auth_revoked(&mut *tx, "user", user).await?;
        let mut meta = json!({
            "role": "owner",
            "previous_role": previous,
            "provider": p.id,
            "via": "idp_sync",
            "reason": "ownerless_org"
        });
        if let Some(v) = row
            .idp_source
            .as_deref()
            .and_then(|s| s.strip_prefix(&db::source_prefix(&p.id)))
        {
            meta["claim_value"] = v.into();
        }
        log(
            tx,
            AuditEvent::new(actor(p), "member.update", Outcome::Success)
                .org(org)
                .target("user", user)
                .metadata(meta),
        )
        .await?;
    }
    Ok(())
}

/// Run the sync for `user` in the sign-in transaction. `values` are the validated claim values.
pub async fn sync(
    tx: &mut sqlx::PgConnection,
    state: &AppState,
    p: &OidcProvider,
    user: &mut UserRow,
    values: &[String],
    d: &Desired,
) -> Result<(), Fail> {
    let prefix = db::source_prefix(&p.id);
    let mut first_org: Option<Uuid> = None;

    // ---- instance admin
    let cur = db::admin_source(&mut *tx, user.id).await?;
    let ours = cur.as_deref().is_some_and(|s| s.starts_with(&prefix));
    match (&d.admin, ours) {
        (Some(v), _) if !user.is_instance_admin => {
            let src = db::source(&p.id, v);
            db::set_admin(&mut *tx, user.id, true, Some(&src)).await?;
            user.is_instance_admin = true;
            log(
                tx,
                AuditEvent::new(actor(p), "instance_admin.grant", Outcome::Success)
                    .target("user", user.id)
                    .metadata(json!({"claim_value": v, "provider": p.id})),
            )
            .await?;
        }
        (Some(v), true) => {
            // Still claim-granted; the value may have changed.
            db::set_admin(&mut *tx, user.id, true, Some(&db::source(&p.id, v))).await?;
        }
        (None, true) => {
            db::set_admin(&mut *tx, user.id, false, None).await?;
            user.is_instance_admin = false;
            let v = cur.as_deref().map_or("", |s| &s[prefix.len()..]).to_owned();
            log(
                tx,
                AuditEvent::new(actor(p), "instance_admin.revoke", Outcome::Success)
                    .target("user", user.id)
                    .metadata(json!({"claim_value": v, "provider": p.id})),
            )
            .await?;
        }
        _ => {}
    }

    // ---- workspaces first (an org removal deletes its bindings), then orgs
    let mut ws_ids: BTreeMap<(String, String), Uuid> = BTreeMap::new();
    let mut org_ids: BTreeMap<String, Uuid> = BTreeMap::new();
    let mut created_orgs: BTreeSet<String> = BTreeSet::new();
    for (slug, (role, value)) in &d.orgs {
        let (id, created) = ensure_org(tx, state, user.id, slug).await?;
        org_ids.insert(slug.clone(), id);
        if created {
            created_orgs.insert(slug.clone());
        }
        first_org.get_or_insert(id);
        apply_org(tx, p, user.id, id, role, value, created).await?;
    }
    for ((o, w), (role, value)) in &d.workspaces {
        let Some(&org) = org_ids.get(o) else { continue };
        let active = db::org_row(&mut *tx, org, user.id)
            .await?
            .is_some_and(|r| r.status == "active");
        if !active {
            continue;
        }
        let ws = ensure_ws(tx, state, org, user.id, w).await?;
        ws_ids.insert((o.clone(), w.clone()), ws);
        apply_ws(tx, p, user.id, org, ws, role, value).await?;
    }

    // ---- removals: claim-sourced rows of this provider that are no longer desired
    for r in db::ws_claim_rows(&mut *tx, user.id, &p.id).await? {
        if d.workspaces
            .contains_key(&(r.org_slug.clone(), r.slug.clone()))
        {
            continue;
        }
        let Some(row) = db::ws_row(&mut *tx, r.workspace_id, user.id).await? else {
            continue;
        };
        let value = r.idp_source[prefix.len()..].to_owned();
        match row.base_role.clone() {
            None => {
                workspaces::unbind_user(&mut *tx, r.workspace_id, user.id).await?;
                log(
                    tx,
                    AuditEvent::new(actor(p), "workspace_access.remove", Outcome::Success)
                        .org(r.org_id)
                        .workspace(r.workspace_id)
                        .target("user", user.id)
                        .metadata(json!({"claim_value": value, "provider": p.id})),
                )
                .await?;
            }
            Some(base) => {
                let new = RoleRow {
                    role: base.clone(),
                    status: row.status,
                    idp_role: None,
                    idp_source: None,
                    base_role: None,
                };
                db::ws_write(&mut *tx, r.workspace_id, user.id, &new).await?;
                log(
                    tx,
                    AuditEvent::new(actor(p), "workspace_access.set", Outcome::Success)
                        .org(r.org_id)
                        .workspace(r.workspace_id)
                        .target("user", user.id)
                        .metadata(json!({"role": base, "claim_value": value, "provider": p.id})),
                )
                .await?;
            }
        }
        notify_auth_revoked(&mut *tx, "user", user.id).await?;
    }
    for r in db::org_claim_rows(&mut *tx, user.id, &p.id).await? {
        if d.orgs.contains_key(&r.slug) {
            continue;
        }
        let Some(row) = db::org_row(&mut *tx, r.org_id, user.id).await? else {
            continue;
        };
        let value = r.idp_source[prefix.len()..].to_owned();
        let new_role = row.base_role.clone();
        let loses_owner =
            row.role == "owner" && row.status == "active" && new_role.as_deref() != Some("owner");
        if loses_owner && is_last_owner(tx, r.org_id, user.id).await? {
            blocked(tx, p, r.org_id, user.id, &value, "last_owner").await?;
            continue;
        }
        match new_role {
            None => {
                members::remove(tx, r.org_id, user.id).await?;
                log(
                    tx,
                    AuditEvent::new(actor(p), "member.remove", Outcome::Success)
                        .org(r.org_id)
                        .target("user", user.id)
                        .metadata(
                            json!({"claim_value": value, "provider": p.id, "via": "idp_sync"}),
                        ),
                )
                .await?;
            }
            Some(base) => {
                let new = RoleRow {
                    role: base.clone(),
                    status: row.status,
                    idp_role: None,
                    idp_source: None,
                    base_role: None,
                };
                db::org_write(&mut *tx, r.org_id, user.id, &new).await?;
                log(
                    tx,
                    AuditEvent::new(actor(p), "member.update", Outcome::Success)
                        .org(r.org_id)
                        .target("user", user.id)
                        .metadata(json!({"role": base, "claim_value": value, "provider": p.id, "via": "idp_sync"})),
                )
                .await?;
            }
        }
        notify_auth_revoked(&mut *tx, "user", user.id).await?;
    }

    // ---- (FIX-050, D-165, C-630) owner-integrity floor: after creations and removals, no
    // org the provider's mappings touched may end the transaction without an active owner.
    promote_to_owner_floor(tx, p, user.id, &org_ids).await?;

    if user.default_org_id.is_none()
        && let Some(org) = first_org
        && db::org_row(&mut *tx, org, user.id).await?.is_some()
    {
        users::set_default_org(&mut *tx, user.id, org).await?;
        user.default_org_id = Some(org);
    }
    db::save_state(tx, user.id, &p.id, values, &Value::Array(d.granted.clone())).await?;
    Ok(())
}

async fn blocked(
    tx: &mut sqlx::PgConnection,
    p: &OidcProvider,
    org: Uuid,
    user: Uuid,
    value: &str,
    reason: &str,
) -> Result<(), Fail> {
    log(
        tx,
        AuditEvent::new(actor(p), "org_role.idp_sync_blocked", Outcome::Denied)
            .org(org)
            .target("user", user)
            .metadata(json!({"claim_value": value, "provider": p.id, "reason": reason})),
    )
    .await
}

async fn apply_org(
    tx: &mut sqlx::PgConnection,
    p: &OidcProvider,
    user: Uuid,
    org: Uuid,
    role: &str,
    value: &str,
    created_org: bool,
) -> Result<(), Fail> {
    let src = db::source(&p.id, value);
    let prefix = db::source_prefix(&p.id);
    let meta = |extra: Value| {
        let mut m =
            json!({"role": role, "claim_value": value, "provider": p.id, "via": "idp_sync"});
        if created_org {
            m["created_org"] = true.into();
        }
        if let (Some(a), Some(b)) = (m.as_object_mut(), extra.as_object()) {
            a.extend(b.clone());
        }
        m
    };
    let Some(row) = db::org_row(&mut *tx, org, user).await? else {
        db::org_insert(&mut *tx, org, user, role, &src).await?;
        log(
            tx,
            AuditEvent::new(actor(p), "member.add", Outcome::Success)
                .org(org)
                .target("user", user)
                .metadata(meta(json!({}))),
        )
        .await?;
        return Ok(());
    };
    if row.status != "active" {
        return Ok(());
    }
    match row.idp_source.as_deref() {
        Some(s) if s.starts_with(&prefix) => {
            let base = row.base_role.clone();
            let eff = match &base {
                Some(b) if org_rank(b) > org_rank(role) => b.clone(),
                _ => role.to_owned(),
            };
            if row.role == "owner" && eff != "owner" && is_last_owner(tx, org, user).await? {
                blocked(tx, p, org, user, value, "last_owner").await?;
                return Ok(());
            }
            let changed = eff != row.role || row.idp_source.as_deref() != Some(&src);
            let new = RoleRow {
                role: eff.clone(),
                status: row.status.clone(),
                idp_role: Some(role.to_owned()),
                idp_source: Some(src),
                base_role: base,
            };
            db::org_write(&mut *tx, org, user, &new).await?;
            if changed {
                if eff != row.role {
                    notify_auth_revoked(&mut *tx, "user", user).await?;
                }
                log(
                    tx,
                    AuditEvent::new(actor(p), "member.update", Outcome::Success)
                        .org(org)
                        .target("user", user)
                        .metadata(meta(json!({"role": eff}))),
                )
                .await?;
            }
        }
        // Another provider's claim owns this row: not ours to change.
        Some(_) => {}
        None => {
            // A manual (or JIT/invitation) role: the claim may only raise it, and the manual
            // role is remembered so that losing the claim restores it.
            if org_rank(role) > org_rank(&row.role) {
                let new = RoleRow {
                    role: role.to_owned(),
                    status: row.status.clone(),
                    idp_role: Some(role.to_owned()),
                    idp_source: Some(src),
                    base_role: Some(row.role.clone()),
                };
                db::org_write(&mut *tx, org, user, &new).await?;
                log(
                    tx,
                    AuditEvent::new(actor(p), "member.update", Outcome::Success)
                        .org(org)
                        .target("user", user)
                        .metadata(meta(json!({"previous_role": row.role}))),
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn apply_ws(
    tx: &mut sqlx::PgConnection,
    p: &OidcProvider,
    user: Uuid,
    org: Uuid,
    ws: Uuid,
    role: &str,
    value: &str,
) -> Result<(), Fail> {
    let src = db::source(&p.id, value);
    let prefix = db::source_prefix(&p.id);
    let ev = |role: &str| {
        AuditEvent::new(actor(p), "workspace_access.set", Outcome::Success)
            .org(org)
            .workspace(ws)
            .target("user", user)
            .metadata(json!({"role": role, "claim_value": value, "provider": p.id}))
    };
    let Some(row) = db::ws_row(&mut *tx, ws, user).await? else {
        db::ws_insert(&mut *tx, ws, user, role, &src).await?;
        return log(tx, ev(role)).await;
    };
    match row.idp_source.as_deref() {
        Some(s) if s.starts_with(&prefix) => {
            let eff = match &row.base_role {
                Some(b) if ws_rank(b) > ws_rank(role) => b.clone(),
                _ => role.to_owned(),
            };
            let changed = eff != row.role || row.idp_source.as_deref() != Some(&src);
            let new = RoleRow {
                role: eff.clone(),
                status: row.status.clone(),
                idp_role: Some(role.to_owned()),
                idp_source: Some(src),
                base_role: row.base_role.clone(),
            };
            db::ws_write(&mut *tx, ws, user, &new).await?;
            if changed {
                if eff != row.role {
                    notify_auth_revoked(&mut *tx, "user", user).await?;
                }
                log(tx, ev(&eff)).await?;
            }
        }
        Some(_) => {}
        None => {
            if ws_rank(role) > ws_rank(&row.role) {
                let new = RoleRow {
                    role: role.to_owned(),
                    status: row.status.clone(),
                    idp_role: Some(role.to_owned()),
                    idp_source: Some(src),
                    base_role: Some(row.role.clone()),
                };
                db::ws_write(&mut *tx, ws, user, &new).await?;
                log(tx, ev(role)).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_shapes() {
        let p = json!({"roles": ["a", "b", 3, "a"], "s": "x y,z", "n": 5,
            "ra": {"aurora": {"roles": ["r"]}}});
        assert_eq!(extract_values("t", &p, "roles"), ["a", "b"]);
        assert_eq!(extract_values("t", &p, "s"), ["x", "y", "z"]);
        assert!(extract_values("t", &p, "n").is_empty());
        assert!(extract_values("t", &p, "missing").is_empty());
        assert_eq!(extract_values("t", &p, "ra.aurora.roles"), ["r"]);
        assert!(extract_values("t", &p, "ra.aurora.roles.x").is_empty());
    }
}
