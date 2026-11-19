//! Tenancy domain logic. `create_organization` / `create_workspace_in_tx` are public:
//! sign-up (BE-011) and OIDC JIT (BE-015) call them inside their own transaction.

use super::dto::{MemberStatus, OrgRole, PatchMemberRequest, PatchOrgRequest};
use crate::app::AppState;
use crate::domains::automations::service::{To, reassign_org_member};
use aurora_config::ApiConfig;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, ids};
use aurora_db::repo::agents::{self, AgentRow, NewDefaultAgent};
use aurora_db::repo::members::{self, MemberRow};
use aurora_db::repo::orgs::{self, OrgRow};
use aurora_db::repo::workspaces::{self, WorkspaceRow};
use aurora_db::repo::{notify_auth_revoked, policy, scim};
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
    ApiError::invalid_field("Invalid request.", path, msg)
}

fn slug_taken() -> ApiError {
    ApiError::conflict("slug_taken", "That slug is already in use.")
}

/// Trimmed name, 1-80 chars, no control or invisible formatting characters
/// (`aurora_core::names`, FIX-181 F6).
pub fn validate_name(path: &str, s: &str) -> Result<String, ApiError> {
    aurora_core::names::clean(s, 80).map_err(|e| bad(path, &e.to_string()))
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

pub fn derive_slug_base(name: &str, fallback: &str) -> String {
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
    create_workspace_named_in_tx(
        tx,
        cfg,
        org_id,
        creator,
        name,
        slug.map_or(SlugFrom::Name, SlugFrom::Explicit),
        timezone,
        memory_language,
    )
    .await
}

/// Where a new workspace's slug comes from.
#[derive(Clone, Copy)]
pub enum SlugFrom<'a> {
    /// Derived from the workspace name (the default).
    Name,
    /// Exactly this slug (validated, must be free).
    Explicit(&'a str),
    /// Derived from this text, not from the name (a personal workspace: the name is the
    /// person's, the slug comes from the placement template, C-923, D-162).
    Text(&'a str),
}

/// [`create_workspace_in_tx`] with the slug's source chosen (C-923).
#[allow(clippy::too_many_arguments)]
pub async fn create_workspace_named_in_tx(
    tx: &mut PgConnection,
    cfg: &ApiConfig,
    org_id: Uuid,
    creator: Uuid,
    name: &str,
    slug_from: SlugFrom<'_>,
    timezone: Option<&str>,
    memory_language: Option<&str>,
) -> Result<(WorkspaceRow, AgentRow), ApiError> {
    let name = validate_name("/name", name)?;
    let slug = match slug_from {
        SlugFrom::Explicit(s) => Some(s),
        _ => None,
    };
    let slug_text = match slug_from {
        SlugFrom::Text(t) => t.to_owned(),
        _ => name.clone(),
    };
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
            let base = derive_slug_base(&slug_text, "workspace");
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

/// Org owner patch (name, slug, settings). `previews_ev` writes the extra security-critical
/// `org.previews.update` row into the same transaction when the switch was sent.
pub async fn patch_org(
    pool: &PgPool,
    org_id: Uuid,
    user: Uuid,
    req: PatchOrgRequest,
    ev: &crate::audit::AuditEvent,
    previews_ev: Option<&crate::audit::AuditEvent>,
    webhook_ceiling: (u32, u32),
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
            // (P4-PRV-1, C-549) The org's Port previews switch; switching off keeps the shares
            // but ends access (`11 §Port previews`).
            if let Some(b) = p.previews_enabled {
                obj.insert("previews_enabled".into(), b.into());
            }
            // (P5-CALINV, C-844) The org switch of the calendar-invitation tools.
            if let Some(b) = p.calendar_invitations_enabled {
                obj.insert(
                    "calendar_invitations".into(),
                    serde_json::json!({ "enabled": b }),
                );
            }
            // (P5-WEBHOOK-2, C-912) Outgoing webhooks: the switch and the org's lowered ceilings.
            if p.outgoing_webhooks_enabled.is_some()
                || p.outgoing_webhooks_per_minute_max.is_some()
                || p.outgoing_webhooks_per_day_max.is_some()
            {
                let mut o = obj
                    .get("outgoing_webhooks")
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                if let Some(b) = p.outgoing_webhooks_enabled {
                    o.insert("enabled".into(), b.into());
                }
                for (key, v, ceiling) in [
                    (
                        "per_minute_max",
                        p.outgoing_webhooks_per_minute_max,
                        i64::from(webhook_ceiling.0),
                    ),
                    (
                        "per_day_max",
                        p.outgoing_webhooks_per_day_max,
                        i64::from(webhook_ceiling.1),
                    ),
                ] {
                    match v {
                        None => {}
                        Some(None) => {
                            o.remove(key);
                        }
                        Some(Some(n)) if n >= 1 && n <= ceiling => {
                            o.insert(key.into(), n.into());
                        }
                        Some(Some(n)) if n >= 1 => {
                            return Err(ApiError::unprocessable(
                                "above_instance_ceiling",
                                format!("The instance allows at most {ceiling} here."),
                            ));
                        }
                        Some(Some(_)) => {
                            return Err(bad(
                                &format!("/settings/outgoing_webhooks_{key}"),
                                "must be at least 1",
                            ));
                        }
                    }
                }
                obj.insert("outgoing_webhooks".into(), serde_json::Value::Object(o));
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
    // (P4-PRV-1, C-549, FIX-042) The next home sync must carry the (possibly empty) `previews`
    // part so a running sandbox's share list follows the switch at once: bump every agent of
    // the org (the `bump_org` pattern) inside the same transaction.
    if previews_ev.is_some() {
        aurora_db::repo::skills::bump_org(&mut tx, org_id).await?;
    }
    let out = orgs::get(&mut *tx, org_id, user)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    // (P4-PRV-1, C-549) Both rows commit together or not at all: the settings write and the
    // security-critical `org.previews.update` switch (docs/04 §Audit).
    match previews_ev {
        Some(pev) => crate::audit::commit_critical_many(tx, &[ev.clone(), pev.clone()]).await?,
        None => crate::audit::commit_critical(tx, ev).await?,
    }
    Ok(out)
}

pub fn confirmation_mismatch() -> ApiError {
    ApiError::new(
        http::StatusCode::BAD_REQUEST,
        "confirmation_mismatch",
        "Confirmation does not match.",
    )
}

/// Discard managed secret values whose rows are gone (docs/08 §Secret stores): after the
/// commit, best effort. A failure leaves an orphan in the store, which is logged — never a
/// reason to undo a committed deletion.
pub async fn discard_managed(state: &AppState, refs: Vec<orgs::ManagedRef>) {
    use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
    for r in refs {
        let Ok(sref) = r.value_ref.parse::<SecretRef>() else {
            continue;
        };
        let ctx = SealContext {
            purpose: match r.kind {
                orgs::ManagedKind::SecretValue => Purpose::SecretValue,
                orgs::ManagedKind::ConnectionCredentials => Purpose::ConnectionCredentials,
                orgs::ManagedKind::OauthClientSecret => Purpose::OauthClientSecret,
                orgs::ManagedKind::WebhookVerification => Purpose::WebhookVerification,
                orgs::ManagedKind::IntegrationCredentials => Purpose::IntegrationCredentials,
                orgs::ManagedKind::PresenceCredentials => Purpose::PresenceCredentials,
            },
            owner_id: r.owner_id,
            org_id: r.org_id.unwrap_or_else(Uuid::nil),
            workspace_id: r.workspace_id,
        };
        if let Err(e) = state.vault.discard(&ctx, &sref, ValueSource::Managed).await {
            tracing::warn!(error = %e, owner = %r.owner_id, "managed secret value not discarded");
        }
    }
}

/// A workspace deletion in three steps, so a caller can combine several (or other deletes) in
/// one transaction: [`WorkspaceDeletion::prepare`] seals the deprovision copies,
/// [`WorkspaceDeletion::run`] deletes inside the caller's transaction, and after the commit
/// [`WorkspaceDeletion::committed`] discards what must not outlive the rows (managed secret
/// values, spare credential copies); [`WorkspaceDeletion::rollback`] on failure.
pub struct WorkspaceDeletion {
    dep: crate::domains::channels::deprovision::Prepared,
    refs: Vec<orgs::ManagedRef>,
    /// (P4-UTOK) Sealed values of person tokens granted to the workspace's agents (their posts
    /// end with the agents).
    token_discards: crate::domains::user_tokens::custody::Discards,
}

impl WorkspaceDeletion {
    pub async fn prepare(state: &AppState, ws: Uuid) -> Result<Self, ApiError> {
        use crate::domains::channels::deprovision::{self, Scope};
        // Self-contained deprovision ops (RFE-024): sealed first (503 when the secret store is
        // unavailable, nothing deleted), committed with the delete, before the cascade.
        Ok(Self {
            dep: deprovision::prepare(state, Scope::Workspace(ws)).await?,
            refs: Vec::new(),
            token_discards: Default::default(),
        })
    }

    pub async fn run(
        &mut self,
        tx: &mut PgConnection,
        org: Uuid,
        ws: Uuid,
    ) -> Result<(), ApiError> {
        self.dep.enqueue(tx).await?;
        // (P4-UTOK, D-284(a)) The grants of the workspace's agents end with the agents.
        self.token_discards.extend(
            crate::domains::user_tokens::end::for_workspace(
                tx,
                ws,
                &crate::audit::AuditActor::system("workspace-delete"),
            )
            .await?,
        );
        self.refs
            .extend(orgs::managed_refs(&mut *tx, org, Some(ws)).await?);
        // (P5-ADMDEL) A deleted workspace leaves the placement's shared list in this transaction.
        aurora_db::repo::placement::prune_shared_workspaces(&mut *tx, &[ws], None).await?;
        workspaces::delete(&mut *tx, ws).await?;
        // (FIX-097) Gateways hold the workspace's secrets, connections and policy warm.
        aurora_db::repo::credentials::notify_gateway_invalidate(&mut *tx, "workspace", ws).await?;
        Ok(())
    }

    pub async fn committed(self, state: &AppState) {
        self.dep.committed(state).await;
        self.token_discards.run(state).await;
        discard_managed(state, self.refs).await;
    }

    pub async fn rollback(&self, state: &AppState) {
        self.dep.rollback(state).await;
    }
}

pub async fn delete_org(
    state: &AppState,
    org_id: Uuid,
    ev: &crate::audit::AuditEvent,
) -> Result<(), ApiError> {
    use crate::domains::channels::deprovision::{self, Scope};
    // Self-contained deprovision ops (RFE-024): the integrations are removed by the cascade, so
    // each op carries its own sealed credential copy. The org's shared bot stays (C-364).
    let mut dep = deprovision::prepare(state, Scope::Org(org_id)).await?;
    let mut refs = Vec::new();
    let mut token_discards = crate::domains::user_tokens::custody::Discards::default();
    let done = async {
        let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
        dep.enqueue(&mut tx).await?;
        // (P4-UTOK, D-284(a)) Every person's token on the org's integrations ends with the org.
        token_discards = crate::domains::user_tokens::end::for_org(
            &mut tx,
            org_id,
            &crate::audit::AuditActor::system("org-delete"),
        )
        .await?;
        // (FIX-097) The managed values of the org's secrets, connections and OAuth clients
        // lose their rows with the cascade: collect them now, discard after the commit.
        refs = orgs::managed_refs(&mut tx, org_id, None).await?;
        // (P5-ADMDEL) Every workspace of the org leaves the placement's shared list.
        aurora_db::repo::placement::prune_shared_workspaces(&mut *tx, &[], Some(org_id)).await?;
        orgs::delete_cascade(&mut tx, org_id).await?;
        aurora_db::repo::credentials::notify_gateway_invalidate(&mut *tx, "org", org_id).await?;
        crate::audit::commit_critical(tx, ev).await
    }
    .await;
    if let Err(e) = done {
        dep.rollback(state).await;
        return Err(e);
    }
    dep.committed(state).await;
    token_discards.run(state).await;
    discard_managed(state, refs).await;
    Ok(())
}

/// Delete one workspace with its deprovisioning, secret discard and gateway invalidation,
/// writing `ev` (critical) in the same transaction. The owner route and the instance admin's
/// (P5-ADMDEL, C-733) both call this: one implementation.
pub async fn delete_workspace(
    state: &AppState,
    org_id: Uuid,
    ws: Uuid,
    ev: &crate::audit::AuditEvent,
) -> Result<(), ApiError> {
    let mut del = WorkspaceDeletion::prepare(state, ws).await?;
    let done = async {
        let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
        del.run(&mut tx, org_id, ws).await?;
        crate::audit::commit_critical(tx, ev).await
    }
    .await;
    if let Err(e) = done {
        del.rollback(state).await;
        return Err(e);
    }
    del.committed(state).await;
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
    actor: &crate::audit::AuditActor,
    mut ev: crate::audit::AuditEvent,
) -> Result<MemberRow, ApiError> {
    // RFE-021: validate `reassign_to` before touching anything.
    let reassign = match req.reassign_to.as_deref() {
        None => None,
        Some(_) if req.status != Some(MemberStatus::Suspended) => {
            return Err(ApiError::invalid_request(
                "Invalid request.",
                vec![aurora_core::FieldError::new(
                    "/reassign_to",
                    "only valid together with status suspended",
                )],
            ));
        }
        Some(v) => Some(To::parse("/reassign_to", v)?),
    };
    // (D-175, FIX-082) `pending` is written only by SCIM and left only by the person's own
    // verified sign-in: an admin can neither set it nor move a member out of it (a
    // suspend-then-reactivate would be an activation); removing the member is allowed.
    if req.status == Some(MemberStatus::Pending) {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![aurora_core::FieldError::new(
                "/status",
                "must be active or suspended",
            )],
        ));
    }
    let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
    let owners = members::lock_active_owners(&mut tx, org_id).await?;
    let cur = members::get(&mut *tx, org_id, target)
        .await?
        .ok_or_else(|| ApiError::not_found("Member not found."))?;
    if cur.status == "pending" && req.status.is_some() {
        return Err(ApiError::conflict(
            "member_pending",
            "This member has not signed in yet; the membership activates at their first sign-in. Remove it instead.",
        ));
    }
    if actor_role != OrgRole::Owner && (cur.role == "owner" || req.role == Some(OrgRole::Owner)) {
        return Err(admin_vs_owner());
    }
    // D-139: a role the IdP's claims give is edited in the IdP, not here.
    if req.role.is_some()
        && aurora_db::repo::idp_roles::org_is_managed(&mut *tx, org_id, target).await?
    {
        return Err(crate::domains::admin::service::managed_by_idp());
    }
    let loses_owner = req.role.is_some_and(|r| r != OrgRole::Owner)
        || req.status == Some(MemberStatus::Suspended);
    if cur.role == "owner" && cur.status == "active" && loses_owner && owners.len() <= 1 {
        return Err(last_owner());
    }
    if let Some(to) = reassign {
        reassign_org_member(&mut tx, actor, org_id, target, to).await?;
    }
    members::update(
        &mut *tx,
        org_id,
        target,
        req.role.map(OrgRole::as_str),
        req.status.map(MemberStatus::as_str),
    )
    .await?;
    // (FIX-083) A membership leaving `active` ends the access the IdP's group claim granted,
    // in this transaction; a reactivation does not bring it back, the next sign-in re-syncs.
    if req.status == Some(MemberStatus::Suspended) && cur.status == "active" {
        crate::domains::groups::sync::drop_idp_memberships(
            &mut tx,
            actor,
            org_id,
            target,
            "membership_suspended",
        )
        .await?;
    }
    // (FIX-037) A human status change of a SCIM-linked member reconciles the link in the same
    // transaction (04 §SCIM, D-135(f)): suspension → link `suspended` with
    // `suspended_by_scim = false` (a human suspension is never lifted by the IdP), manual
    // reactivation → link `active` again — the admission gate and the Directory badge keep
    // telling the truth. Only a real transition writes (a redundant suspend must not steal a
    // SCIM-made flag); the repo refuses `removed` rows, keeping a tombstone untouched.
    if let Some(st) = req.status
        && st.as_str() != cur.status
        && let Some(link) = scim::user_link(&mut *tx, org_id, target).await?
    {
        scim::set_user_state(&mut tx, org_id, link.id, st.as_str(), false).await?;
    }
    // A role change (owner/admin → member) can also end `can_use`, which ends speaker binding
    // (RFE-005).
    if req.status.is_some_and(|s| s.as_str() != cur.status)
        || req.role.is_some_and(|r| r.as_str() != cur.role)
    {
        notify_auth_revoked(&mut *tx, "user", target).await?;
    }
    let out = members::get(&mut *tx, org_id, target)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    // `metadata.change`: the changed fields as old → new (04 §Audit).
    let mut change = serde_json::Map::new();
    if let Some(r) = req.role {
        change.insert(
            "role".into(),
            format!("{} → {}", cur.role, r.as_str()).into(),
        );
    }
    if let Some(st) = req.status {
        change.insert(
            "status".into(),
            format!("{} → {}", cur.status, st.as_str()).into(),
        );
    }
    ev.metadata["change"] = change.into();
    crate::audit::commit_critical(tx, &ev).await?;
    Ok(out)
}

pub async fn remove_member(
    state: &AppState,
    org_id: Uuid,
    actor_role: OrgRole,
    target: Uuid,
    reassign: Option<To>,
    actor: &crate::audit::AuditActor,
    ev: &crate::audit::AuditEvent,
) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
    let owners = members::lock_active_owners(&mut tx, org_id).await?;
    let cur = members::get(&mut *tx, org_id, target)
        .await?
        .ok_or_else(|| ApiError::not_found("Member not found."))?;
    if actor_role != OrgRole::Owner && cur.role == "owner" {
        return Err(admin_vs_owner());
    }
    if aurora_db::repo::idp_roles::org_is_managed(&mut *tx, org_id, target).await? {
        return Err(crate::domains::admin::service::managed_by_idp());
    }
    if cur.role == "owner" && cur.status == "active" && owners.len() <= 1 {
        return Err(last_owner());
    }
    if let Some(to) = reassign {
        reassign_org_member(&mut tx, actor, org_id, target, to).await?;
    }
    members::remove(&mut tx, org_id, target).await?;
    // (FIX-108) The removal is remembered, so JIT provisioning never restores the membership.
    members::record_removal(&mut *tx, org_id, target).await?;
    // (FIX-037) A human removal of a SCIM-linked member tombstones the link in the same
    // transaction (04 §SCIM, D-135(f)): `state = removed` (with `removed_at`, flag false) so
    // JIT, invitations and sign-in re-admission are refused exactly as for a SCIM-side
    // delete. `set_user_state` refuses an already-`removed` row, keeping the tombstone
    // (and its `removed_at`) untouched.
    if let Some(link) = scim::user_link(&mut *tx, org_id, target).await? {
        scim::set_user_state(&mut tx, org_id, link.id, "removed", false).await?;
    }
    notify_auth_revoked(&mut *tx, "user", target).await?;
    crate::audit::commit_critical(tx, ev).await?;
    Ok(())
}

/// (phase 2, RFE-025) Sets the workspace's own BYO posture (`None` = inherit). Tighten-only: a
/// value looser than the instance floor or the org posture is refused with 422
/// `governance_below_instance_minimum`. The audit row `workspace.byo_posture.update` is written
/// in the same transaction and the `workspace` scope is invalidated in gateways.
pub async fn set_byo_posture(
    s: &AppState,
    c: &crate::http::auth::Caller,
    a: &aurora_db::authz::WorkspaceAuth,
    ws: Uuid,
    before: workspaces::WorkspaceRow,
    posture: Option<&str>,
) -> Result<workspaces::WorkspaceRow, ApiError> {
    use aurora_llm::governance::ByoPosture;
    let new = match posture {
        None => None,
        Some(p) => Some(ByoPosture::parse(p).ok_or_else(|| {
            ApiError::invalid_request(
                "Invalid workspace.",
                vec![aurora_core::FieldError::new(
                    "/byo_posture",
                    "must be enabled, owners_only, disabled or null",
                )],
            )
        })?),
    };
    if let Some(v) = new {
        // The inherited level: instance floor, strengthened by the org posture.
        let org = crate::domains::agents::byo::org_byo(s, a.org_id).await?;
        let inherited = org
            .posture
            .unwrap_or(ByoPosture::Enabled)
            .max(s.gov_instance().byo.minimum_posture);
        if v < inherited {
            return Err(ApiError::unprocessable(
                "governance_below_instance_minimum",
                "The workspace BYO posture cannot be looser than the instance or organization posture.",
            )
            .with_details(serde_json::json!({
                "fields": [{ "path": "/byo_posture", "message": format!("minimum is {}", inherited.as_str()) }],
                // (SET-2, C-783) The instance floor is a setting; named when it decides.
                "instance_setting": (inherited == s.gov_instance().byo.minimum_posture)
                    .then(|| serde_json::json!({ "section": "governance", "key": "byo.minimum_posture" })),
            })));
        }
    }
    let mut tx = s.pool.begin().await.map_err(aurora_db::DbError::from)?;
    let row = workspaces::set_byo_posture(&mut *tx, ws, new.map(ByoPosture::as_str))
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
    let ev = crate::audit::AuditEvent::by(c, "workspace.byo_posture.update")
        .org(a.org_id)
        .workspace(ws)
        .target("workspace", ws)
        .metadata(serde_json::json!({
            "byo_posture": new.map(ByoPosture::as_str),
            "previous": before.byo_posture,
        }));
    crate::audit::insert_tx(&mut tx, &ev).await.map_err(|e| {
        tracing::error!(error = %e, "workspace.byo_posture.update audit write failed");
        ApiError::unavailable("audit_unavailable", "The audit log is unavailable.")
    })?;
    aurora_db::repo::credentials::notify_gateway_invalidate(&mut *tx, "workspace", ws).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIX-116: a name with bidi or zero-width characters is refused like one with controls.
    #[test]
    fn names_refuse_format_characters() {
        assert!(validate_name("name", "Acme").is_ok());
        for bad in [
            "Ac\u{202E}me",
            "Ac\u{200B}me",
            "Ac\u{FEFF}me",
            "Ac\u{0007}me",
        ] {
            assert!(validate_name("name", bad).is_err(), "{bad:?}");
        }
    }

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
