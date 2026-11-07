//! Tenancy domain logic. `create_organization` / `create_workspace_in_tx` are public:
//! sign-up (BE-011) and OIDC JIT (BE-015) call them inside their own transaction.

use super::dto::{MemberStatus, OrgRole, PatchMemberRequest, PatchOrgRequest};
use crate::app::AppState;
use aurora_config::ApiConfig;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::repo::agents::{self, AgentRow, NewDefaultAgent};
use aurora_db::repo::members::{self, MemberRow};
use aurora_db::repo::orgs::{self, OrgRow};
use aurora_db::repo::workspaces::{self, WorkspaceRow};
use aurora_db::repo::{notify_auth_revoked, policy};
use aurora_db::{PgPool, conflict_code};
use sqlx::PgConnection;
use uuid::Uuid;

const ORG_SLUG_CONSTRAINT: &str = "organizations_slug_key";

pub struct CreatedOrg {
    pub org: OrgRow,
    pub workspace: WorkspaceRow,
    pub default_agent: AgentRow,
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, msg)])
}

fn slug_taken() -> ApiError {
    ApiError::conflict("slug_taken", "That slug is already in use.")
}

/// Trimmed name, 1-80 chars, no control characters.
pub fn validate_name(path: &str, s: &str) -> Result<String, ApiError> {
    let t = s.trim();
    let n = t.chars().count();
    if n == 0 || n > 80 {
        return Err(bad(path, "must be 1 to 80 characters"));
    }
    if t.chars().any(char::is_control) {
        return Err(bad(path, "must not contain control characters"));
    }
    Ok(t.to_owned())
}

/// Lowercase, non-`[a-z0-9]` -> `-`, collapsed and trimmed; at most `max` chars.
fn slugify(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(max);
    out.trim_matches('-').to_owned()
}

fn derive_slug_base(name: &str, fallback: &str) -> String {
    let mut s = slugify(name, 40);
    if s.is_empty() {
        s = fallback.to_owned();
    }
    if s.len() < 2 {
        s.push_str("-1");
    }
    s
}

fn explicit_slug(s: &str) -> Result<&str, ApiError> {
    if ids::is_valid_slug(s) {
        Ok(s)
    } else {
        Err(bad(
            "/slug",
            "must match [a-z0-9][a-z0-9-]{1,47} (2 to 48 characters)",
        ))
    }
}

fn agent_handle(name: &str) -> String {
    let mut h = slugify(name, 36);
    if h.is_empty() {
        h = "agent".to_owned();
    }
    if !h.starts_with(|c: char| c.is_ascii_lowercase()) {
        h = format!("agent-{h}");
    }
    h.truncate(40);
    h.trim_end_matches('-').to_owned()
}

fn random_hex32() -> String {
    let t = token::generate(TokenPrefix::Session);
    token::hash(t.expose())[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Org creation transaction body (docs/04 §Sign-up and organization creation, steps 1-4).
/// `slug = None` derives one from `name`, suffixing `-2`, `-3`... on collision;
/// an explicit colliding slug is 409 `slug_taken`.
pub async fn create_organization(
    tx: &mut PgConnection,
    cfg: &ApiConfig,
    creator: Uuid,
    name: &str,
    slug: Option<&str>,
) -> Result<CreatedOrg, ApiError> {
    let name = validate_name("/name", name)?;
    let slug = match slug {
        Some(s) => {
            let s = explicit_slug(s)?;
            if orgs::slug_exists(&mut *tx, s).await? {
                return Err(slug_taken());
            }
            s.to_owned()
        }
        None => {
            let base = derive_slug_base(&name, "org");
            let mut candidate = base.clone();
            let mut n = 2;
            while orgs::slug_exists(&mut *tx, &candidate).await? {
                let suffix = format!("-{n}");
                let keep = 48 - suffix.len();
                candidate = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
                n += 1;
            }
            candidate
        }
    };
    let org_id = ids::new_id();
    orgs::insert(&mut *tx, org_id, &slug, &name, &random_hex32())
        .await
        .map_err(|e| conflict_code(e, &[(ORG_SLUG_CONSTRAINT, "slug_taken")]))?;
    members::insert(&mut *tx, org_id, creator, "owner").await?;
    policy::insert_empty_generation(&mut *tx, org_id, None, 1, true).await?;
    policy::insert_empty_generation(&mut *tx, org_id, None, 2, false).await?;
    let (workspace, default_agent) = create_workspace_in_tx(
        tx,
        cfg,
        org_id,
        creator,
        "Default",
        Some("default"),
        None,
        None,
    )
    .await?;
    let org = orgs::get(&mut *tx, org_id, creator)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    Ok(CreatedOrg {
        org,
        workspace,
        default_agent,
    })
}

/// Workspace creation (steps 2-4 without the org scope): workspace, creator bound as
/// `owner`, workspace policy generations (1 published, 2 draft), default agent.
#[allow(clippy::too_many_arguments)]
pub async fn create_workspace_in_tx(
    tx: &mut PgConnection,
    cfg: &ApiConfig,
    org_id: Uuid,
    creator: Uuid,
    name: &str,
    slug: Option<&str>,
    timezone: Option<&str>,
    memory_language: Option<&str>,
) -> Result<(WorkspaceRow, AgentRow), ApiError> {
    let name = validate_name("/name", name)?;
    let timezone = validate_timezone(timezone.unwrap_or(&cfg.hosted.default_timezone))?;
    let language = validate_language(&mut *tx, memory_language.unwrap_or("english")).await?;
    let slug = match slug {
        Some(s) => {
            let s = explicit_slug(s)?;
            if workspaces::slug_exists(&mut *tx, org_id, s).await? {
                return Err(slug_taken());
            }
            s.to_owned()
        }
        None => {
            let base = derive_slug_base(&name, "workspace");
            let mut candidate = base.clone();
            let mut n = 2;
            while workspaces::slug_exists(&mut *tx, org_id, &candidate).await? {
                let suffix = format!("-{n}");
                let keep = 48 - suffix.len();
                candidate = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
                n += 1;
            }
            candidate
        }
    };
    let ws_id = ids::new_id();
    let ws = workspaces::insert(&mut *tx, ws_id, org_id, &slug, &name, &timezone, &language)
        .await
        .map_err(|e| conflict_code(e, &[(workspaces::SLUG_CONSTRAINT, "slug_taken")]))?;
    workspaces::bind_user(&mut *tx, ws_id, creator, "owner").await?;
    policy::insert_empty_generation(&mut *tx, org_id, Some(ws_id), 1, true).await?;
    policy::insert_empty_generation(&mut *tx, org_id, Some(ws_id), 2, false).await?;
    let agent = agents::insert_default(
        &mut *tx,
        &NewDefaultAgent {
            id: ids::new_id(),
            org_id,
            workspace_id: ws_id,
            handle: &agent_handle(&cfg.agents.default_name),
            name: &cfg.agents.default_name,
            hosted: cfg.hosted.enabled,
            harness: &cfg.hosted.default_harness,
            brief: &cfg.agents.default_brief,
            created_by: creator,
        },
    )
    .await?;
    Ok((ws, agent))
}

pub fn validate_timezone(tz: &str) -> Result<String, ApiError> {
    tz.parse::<chrono_tz::Tz>()
        .map(|_| tz.to_owned())
        .map_err(|_| bad("/timezone", "unknown IANA timezone"))
}

pub async fn validate_language(conn: &mut PgConnection, lang: &str) -> Result<String, ApiError> {
    if lang == "simple" || workspaces::memory_language_exists(&mut *conn, lang).await? {
        Ok(lang.to_owned())
    } else {
        Err(bad("/memory_language", "unsupported text-search language"))
    }
}

/// Org owner patch (name, slug, settings).
pub async fn patch_org(
    pool: &PgPool,
    org_id: Uuid,
    user: Uuid,
    req: PatchOrgRequest,
) -> Result<OrgRow, ApiError> {
    let name = req
        .name
        .as_deref()
        .map(|n| validate_name("/name", n))
        .transpose()?;
    let slug = req.slug.as_deref().map(explicit_slug).transpose()?;
    let mut tx = pool.begin().await.map_err(aurora_db::DbError::from)?;
    let current = orgs::get(&mut *tx, org_id, user)
        .await?
        .ok_or_else(|| ApiError::not_found("Organization not found."))?;
    let settings = match req.settings {
        None => None,
        Some(p) => {
            let mut s = current.settings.clone();
            let obj = s
                .as_object_mut()
                .ok_or_else(|| ApiError::internal("Internal error."))?;
            if let Some(b) = p.members_can_create_workspaces {
                obj.insert("members_can_create_workspaces".into(), b.into());
            }
            for (key, v) in [("max_members", p.max_members), ("max_agents", p.max_agents)] {
                match v {
                    None => {}
                    Some(None) => {
                        obj.remove(key);
                    }
                    Some(Some(n)) if n >= 1 => {
                        obj.insert(key.into(), n.into());
                    }
                    Some(Some(_)) => {
                        return Err(bad(&format!("/settings/{key}"), "must be at least 1"));
                    }
                }
            }
            Some(s)
        }
    };
    if let Some(s) = slug
        && s != current.slug
        && orgs::slug_exists(&mut *tx, s).await?
    {
        return Err(slug_taken());
    }
    orgs::update(&mut *tx, org_id, name.as_deref(), slug, settings.as_ref())
        .await
        .map_err(|e| conflict_code(e, &[(ORG_SLUG_CONSTRAINT, "slug_taken")]))?;
    let out = orgs::get(&mut *tx, org_id, user)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(out)
}

pub fn confirmation_mismatch() -> ApiError {
    ApiError::new(
        http::StatusCode::BAD_REQUEST,
        "confirmation_mismatch",
        "Confirmation does not match.",
    )
}

pub async fn delete_org(pool: &PgPool, org_id: Uuid) -> Result<(), ApiError> {
    let mut tx = pool.begin().await.map_err(aurora_db::DbError::from)?;
    crate::domains::channels::presences::enqueue_deprovision_for_org(&mut tx, org_id).await?;
    orgs::delete_cascade(&mut tx, org_id).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(())
}

fn last_owner() -> ApiError {
    ApiError::conflict(
        "last_owner",
        "An organization needs at least one active owner.",
    )
}

fn admin_vs_owner() -> ApiError {
    ApiError::forbidden("Admins cannot modify owners or grant the owner role.")
}

pub async fn patch_member(
    state: &AppState,
    org_id: Uuid,
    actor_role: OrgRole,
    target: Uuid,
    req: &PatchMemberRequest,
) -> Result<MemberRow, ApiError> {
    let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
    let owners = members::lock_active_owners(&mut tx, org_id).await?;
    let cur = members::get(&mut *tx, org_id, target)
        .await?
        .ok_or_else(|| ApiError::not_found("Member not found."))?;
    if actor_role != OrgRole::Owner && (cur.role == "owner" || req.role == Some(OrgRole::Owner)) {
        return Err(admin_vs_owner());
    }
    let loses_owner = req.role.is_some_and(|r| r != OrgRole::Owner)
        || req.status == Some(MemberStatus::Suspended);
    if cur.role == "owner" && cur.status == "active" && loses_owner && owners.len() <= 1 {
        return Err(last_owner());
    }
    members::update(
        &mut *tx,
        org_id,
        target,
        req.role.map(OrgRole::as_str),
        req.status.map(MemberStatus::as_str),
    )
    .await?;
    if req.status.is_some_and(|s| s.as_str() != cur.status) {
        notify_auth_revoked(&mut *tx, "user", target).await?;
    }
    let out = members::get(&mut *tx, org_id, target)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(out)
}

pub async fn remove_member(
    state: &AppState,
    org_id: Uuid,
    actor_role: OrgRole,
    target: Uuid,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
    let owners = members::lock_active_owners(&mut tx, org_id).await?;
    let cur = members::get(&mut *tx, org_id, target)
        .await?
        .ok_or_else(|| ApiError::not_found("Member not found."))?;
    if actor_role != OrgRole::Owner && cur.role == "owner" {
        return Err(admin_vs_owner());
    }
    if cur.role == "owner" && cur.status == "active" && owners.len() <= 1 {
        return Err(last_owner());
    }
    members::remove(&mut tx, org_id, target).await?;
    notify_auth_revoked(&mut *tx, "user", target).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(derive_slug_base("Alex's  Org!", "org"), "alex-s-org");
        assert_eq!(derive_slug_base("!!!", "org"), "org");
        assert_eq!(derive_slug_base("a", "org"), "a-1");
        assert!(ids::is_valid_slug(&derive_slug_base(
            &"x".repeat(80),
            "org"
        )));
        assert_eq!(agent_handle("Daneel"), "daneel");
        assert_eq!(agent_handle("42 Bot"), "agent-42-bot");
        assert_eq!(random_hex32().len(), 32);
    }

    #[test]
    fn names() {
        assert!(validate_name("/name", "  ").is_err());
        assert!(validate_name("/name", "a\nb").is_err());
        assert!(validate_name("/name", &"x".repeat(81)).is_err());
        assert_eq!(validate_name("/name", " ok ").unwrap(), "ok");
        assert!(validate_timezone("Europe/Zurich").is_ok());
        assert!(validate_timezone("Mars/Base").is_err());
    }
}
