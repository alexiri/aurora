//! Create an agent from a template (C-1111, P6-TPL-3): every check runs before any write, then
//! **one transaction** through the existing writers — the agent, its starter grants, its own
//! skills, the chosen library skills, the helper files, the memories, the schedules, the
//! (always paused) webhooks, each account's access and the C-1112 provenance. Any failure rolls
//! everything back; nothing is half-created. The C-1110 plan is the single source of truth:
//! what the plan showed, create accepts. The network IO (library fetches), the webhook secret
//! seals and the avatar blob put all happen before the transaction.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use aurora_template::{helpers_hash, instructions_hash, skill_hash, skill_ref_key};

use crate::app::AppState;
use crate::audit::{self, AuditEvent};
use crate::domains::accounts::dto::AccountRefType;
use crate::domains::agent_helpers;
use crate::domains::agent_templates::admin;
use crate::domains::agent_templates::dto::{self};
use crate::domains::agent_templates::org as org_switch;
use crate::domains::agents::{self, dto::Agent};
use crate::domains::apps::service as apps;
use crate::domains::attachments::blob;
use crate::domains::grants::dto::{GrantMode, PutConnectionGrant};
use crate::domains::grants::{self, levels};
use crate::domains::memory;
use crate::domains::schedules;
use crate::domains::skill_libraries;
use crate::domains::skills::dto::{FromLibrary, Skill};
use crate::domains::skills::{self as skills_domain, NewSkill, library};
use crate::domains::webhooks::{self, dto::WebhookWithUrl};
use crate::http::auth::Caller;
use crate::http::who_can::{WhoCanRole, forbid};
use aurora_core::ApiError;
use aurora_core::limits;
use aurora_db::repo::agent_templates as trepo;
use aurora_db::repo::connections;
use aurora_db::repo::skills::Owner;

fn not_found() -> ApiError {
    ApiError::not_found("Template not found.")
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_field("Invalid request.", path, msg)
}

/// A 422 whose `details.fields` carries `/…` and the entry or reference index (C-1111).
fn at(code: &str, msg: &str, path: &str, index: usize) -> ApiError {
    ApiError::unprocessable(code, msg).with_details(json!({
        "fields": [{"path": path, "message": format!("entry {index}")}]
    }))
}

// ---- the request and the answer (C-1111) -------------------------------------------------------

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateFromTemplateRequest {
    pub template_id: String,
    /// The offered version the person planned against.
    pub version: String,
    /// The offered version's content hash the plan showed.
    pub content_sha256: String,
    pub name: String,
    pub handle: Option<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// One account per app entry (required entries must have exactly one).
    #[serde(default)]
    pub accounts: Vec<AccountChoice>,
    /// The library skill references to add (required ones must be here).
    #[serde(default)]
    pub skill_refs: Vec<SkillRefChoice>,
    /// The schedules to create (a left-out one is created with its template `enabled`).
    #[serde(default)]
    pub automations: Vec<AutomationChoice>,
}

/// One app entry's account (C-1111's `accounts` item): the entry, which of its apps, which
/// account, and optionally the person's own access choice (default the plan's level).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AccountChoice {
    /// The `apps:` entry's index.
    pub entry: usize,
    pub app_id: String,
    pub connection_id: Uuid,
    /// `read` | `ask` | `act`; default the plan's level for this app.
    pub level: Option<String>,
    /// A custom tool split (C-980's shape); replaces the level.
    pub custom: Option<CustomAccess>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomAccess {
    #[serde(default)]
    pub allow_tools: Vec<String>,
    #[serde(default)]
    pub ask_tools: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillRefChoice {
    /// The `skill_refs:` entry's index.
    pub index: usize,
    pub library_id: Uuid,
    /// The commit the person reviewed on the four steps (the fetch pins to it).
    pub commit_sha: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationChoice {
    pub kind: String,
    /// The template's automation name.
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Default the workspace's time zone (as a schedule create).
    pub timezone: Option<String>,
}

fn default_true() -> bool {
    true
}

/// (C-1111) One note: `tools_dropped`, `access_not_given` or `skill_skipped`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CreateNote {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    pub detail: String,
}

/// (C-1111) The 201 answer: the new agent and everything that was created with it; each
/// webhook's secret is shown once.
#[derive(Serialize, ToSchema)]
pub struct CreatedFromTemplate {
    pub agent: Agent,
    pub grants: Vec<grants::dto::ConnectionGrant>,
    pub skills: Vec<Skill>,
    pub webhooks: Vec<WebhookWithUrl>,
    pub notes: Vec<CreateNote>,
}

// ---- the checks (all before any write) ---------------------------------------------------------

/// One validated account choice: the plan's choice it landed on and the grant it writes.
struct ChosenAccount {
    connection_id: Uuid,
    grant: PutConnectionGrant,
}

/// The 409 of a create against anything but the offered version the request answered to
/// (C-1110, SEC-146 B): with the plan as the reload answer, when there is one.
fn version_changed(plan: Option<&dto::TemplatePlan>) -> ApiError {
    let e = ApiError::conflict(
        "template_version_changed",
        "This template was updated. Check the changes, then create.",
    );
    match plan {
        Some(plan) => e.with_details(json!({ "plan": plan })),
        None => e,
    }
}

/// [`version_changed`] once the offer has moved under the request: the plan is built again, so
/// the answer carries the current one. A template that is not available any more (withdrawn,
/// or turned off by the organization) has no plan.
async fn current_version_changed(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    template_id: &str,
) -> ApiError {
    match super::workspace::plan(state, caller, ws, template_id).await {
        Ok(plan) => version_changed(Some(&plan)),
        Err(e) if e.code() == "not_found" => version_changed(None),
        Err(e) => e,
    }
}

/// A rendered skill keeps the limits of an edit: filling in the inputs and names can push a
/// text over them after the version was validated (422 `template_too_large`).
pub(crate) fn rendered_skill_ok(s: &aurora_template::RenderedSkill) -> Result<(), ApiError> {
    let files: Vec<(&str, &[u8])> = s
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.bytes.as_slice()))
        .collect();
    skills_domain::service::check_composed(&s.name, &s.use_it_when, &s.body, &files)
        .and_then(|()| skills_domain::service::clean_title(s.title.as_deref()).map(|_| ()))
        .map_err(|e| {
            // An empty description is not a size problem: it keeps the edit's own error.
            if s.use_it_when.trim().is_empty() {
                return e;
            }
            let field = e.details().and_then(|d| d.get("fields")?.get(0));
            let limit = match field {
                Some(f) => format!(
                    "{}: {}",
                    f["path"].as_str().unwrap_or("").trim_start_matches('/'),
                    f["message"].as_str().unwrap_or("")
                ),
                None => e.message().to_owned(),
            };
            ApiError::unprocessable(
                "template_too_large",
                format!(
                    "The skill {} is too large once the template is filled in ({}).",
                    s.name,
                    limit.trim_end_matches('.')
                ),
            )
        })
}

/// Every automation choice names one of the template's schedules (C-1111): a choice that
/// matched nothing would leave the real schedule created as the template has it.
fn automations_ok(
    choices: &[AutomationChoice],
    rendered: &aurora_template::RenderedTemplate,
) -> Result<(), ApiError> {
    for (i, c) in choices.iter().enumerate() {
        let refused = |field: &str, message: &str| {
            ApiError::unprocessable("invalid_request", "Invalid request.").with_details(json!({
                "fields": [{"path": format!("/automations/{i}/{field}"), "message": message}]
            }))
        };
        if c.kind != "schedule" {
            return Err(refused("kind", "must be schedule"));
        }
        let known = rendered.automations.iter().any(|a| {
            matches!(a, aurora_template::RenderedAutomation::Schedule { name, .. } if *name == c.name)
        });
        if !known {
            return Err(refused("name", "the template has no schedule of this name"));
        }
    }
    Ok(())
}

/// What a create has made outside its transaction: the sealed webhook secrets and the avatar
/// blob. Each is recorded as soon as it may exist, so a failure at any later step removes it
/// (SEC-146 D).
#[derive(Default)]
struct Outside {
    /// The webhooks whose secrets are sealed, in the template's order.
    hooks: Vec<(webhooks::PreparedWebhook, Option<Value>)>,
    avatar_key: Option<String>,
}

impl Outside {
    async fn discard(&self, state: &AppState, org: Uuid, ws: Uuid) {
        for (hook, verification) in &self.hooks {
            if let Some(r) = webhooks::stored_ref(verification) {
                webhooks::discard_for(state, org, ws, hook.id, &r).await;
            }
        }
        if let Some(key) = &self.avatar_key {
            let (_, store) = blob::current(&state.pool);
            let _ = store.delete(key).await;
        }
    }
}

/// Everything the transaction needs, checked and fetched before it.
struct Prepared<'a> {
    req: &'a CreateFromTemplateRequest,
    /// The agent's name, cleaned as any agent create cleans it.
    name: &'a str,
    plan: &'a dto::TemplatePlan,
    content: &'a aurora_template::TemplateContent,
    rendered: &'a aurora_template::RenderedTemplate,
    /// The stored, checked input values (C-1112's `template_inputs`).
    inputs_json: Value,
    accounts: Vec<ChosenAccount>,
    /// `(skill_refs index, fetched content)` of the chosen library skills.
    refs: &'a [(usize, library::PreparedAdd)],
    /// The sealed webhooks, in the template's order.
    hooks: &'a [(webhooks::PreparedWebhook, Option<Value>)],
    /// The avatar blob already in the store, with its fresh public key.
    avatar: Option<(String, String)>,
}

/// (P6-TPL-3, C-1111) The whole create: checks, then the one transaction.
pub async fn create_from_template(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    req: CreateFromTemplateRequest,
) -> Result<CreatedFromTemplate, ApiError> {
    org_switch::ensure_enabled(state)?;
    let auth = caller.workspace(state, ws).await?;
    // The agent is a hosted one on the default harness: the rules of any hosted create.
    agents::service::hosted_enabled(state)?;
    agents::service::harness_ok(state, &state.ops().hosted.default_harness)?;
    // The plan decides (C-1110): it is also the 409's reload answer, so build it first.
    let plan = super::workspace::plan(state, caller, ws, &req.template_id).await?;
    if req.version != plan.version
        || plan.content_sha256.as_deref() != Some(req.content_sha256.as_str())
    {
        return Err(version_changed(Some(&plan)));
    }
    // The stored version row: the bytes creation copies are exactly the approved content.
    let t = trepo::template_by_template_id(&state.pool, &req.template_id)
        .await?
        .ok_or_else(not_found)?;
    let v = admin::offered_version(&state.pool, t.id)
        .await?
        .ok_or_else(not_found)?;
    let content_v = v.content.as_ref().ok_or_else(not_found)?;
    let content = dto::content_from_json(content_v).ok_or_else(not_found)?;
    // The stored content itself is what the request answers to (SEC-146 B): the version row is
    // read separately from the plan, so its version, the content's own version and the
    // content's own canonical hash are checked against the request here.
    if v.version != req.version
        || content.meta.version != v.version
        || aurora_template::content_sha256(&content) != req.content_sha256
    {
        return Err(current_version_changed(state, caller, ws, &req.template_id).await);
    }

    // The inputs (C-1104's rules, all problems at once).
    let checked =
        aurora_template::check_inputs(&content.meta.inputs, &req.inputs).map_err(|problems| {
            let fields: Vec<_> = problems
                .iter()
                .map(|p| json!({"path": format!("/inputs/{}", p.input), "message": p.message}))
                .collect();
            ApiError::unprocessable(
                "template_inputs_invalid",
                "The template inputs are invalid.",
            )
            .with_details(json!({ "fields": fields }))
        })?;
    let inputs_json = json!(
        content
            .meta
            .inputs
            .iter()
            .map(|i| (
                i.id.clone(),
                checked.get(&i.id).unwrap_or_default().to_owned()
            ))
            .collect::<BTreeMap<_, _>>()
    );

    // The person's name for `{person.name}` (the audit label is the fallback).
    let person = aurora_db::repo::users::find_by_id(&state.pool, caller.user_id)
        .await?
        .map(|u| u.display_name)
        .unwrap_or_else(|| caller.email.clone());
    // The name is cleaned before anything renders it, as any agent create cleans it.
    let name = agents::service::name_ok(&req.name)?;
    let rendered = aurora_template::render(&content, &checked, &name, &person);
    if rendered.instructions.chars().count() > agents::service::BRIEF_MAX {
        return Err(ApiError::unprocessable(
            "template_too_large",
            "The rendered instructions are too large for an agent brief.",
        ));
    }
    for s in &rendered.skills {
        rendered_skill_ok(s)?;
    }
    automations_ok(&req.automations, &rendered)?;
    // The name, handle and max_agents rules of agent creation apply (C-1111); the agent-limit
    // and handle-uniqueness rules live in `agents::service::insert_in_tx`.
    if let Some(h) = &req.handle
        && !agents::service::valid_handle(h)
    {
        return Err(bad("/handle", "must match [a-z][a-z0-9-]{0,39}"));
    }

    // The accounts: per entry at most one, required entries exactly one, every app available,
    // every connection one the plan's C-504 filter listed.
    let mut notes: Vec<CreateNote> = Vec::new();
    let accounts =
        choose_accounts(state, &plan, &req.accounts, auth.org_id, ws, &mut notes).await?;

    // The skill references: resolve the library dir of each chosen reference and fetch its
    // content now (the library add path's fetch step; the write joins the transaction).
    let mut prepared_refs: Vec<(usize, library::PreparedAdd)> = Vec::new();
    for (index, r) in content.meta.skill_refs.iter().enumerate() {
        let plan_ref = plan.skill_refs.get(index).ok_or_else(not_found)?;
        match r {
            aurora_template::SkillRef::Organization { required, .. } => {
                // Nothing is ever written for an organization reference (C-1142); a required
                // one must still be present in the workspace.
                if *required && !plan_ref.available {
                    return Err(at(
                        "template_skill_missing",
                        "This organization skill is not in the workspace.",
                        "/skill_refs",
                        index,
                    ));
                }
            }
            aurora_template::SkillRef::Library { .. } => {
                let item = req.skill_refs.iter().find(|s| s.index == index);
                let Some(item) = item else {
                    if r.required() {
                        return Err(at(
                            "template_skill_missing",
                            "This required skill reference has no library skill to add.",
                            "/skill_refs",
                            index,
                        ));
                    }
                    notes.push(CreateNote {
                        code: "skill_skipped".into(),
                        entry: None,
                        index: Some(index),
                        detail: "Optional skill reference left out.".into(),
                    });
                    continue;
                };
                // The reference must be the one the plan resolved, at the plan's commit
                // (SEC-146 C): the fetch commits to what the plan showed, never a
                // body-chosen commit.
                if !plan_ref.available {
                    return Err(at(
                        "template_skill_unavailable",
                        "This skill reference is not available in this workspace.",
                        "/skill_refs",
                        index,
                    ));
                }
                // The library must be the one the C-1142 resolution accepted (C-1111).
                let Some(lib_view) = &plan_ref.library else {
                    return Err(at(
                        "template_skill_unavailable",
                        "This skill reference is not available in this workspace.",
                        "/skill_refs",
                        index,
                    ));
                };
                if lib_view.id != item.library_id {
                    return Err(at(
                        "template_skill_unavailable",
                        "This library is not the one the plan resolved for this reference.",
                        "/skill_refs",
                        index,
                    ));
                }
                if lib_view.commit_sha != item.commit_sha {
                    return Err(at(
                        "template_commit_changed",
                        "This library skill's commit is not the one the plan resolved for this reference.",
                        "/skill_refs",
                        index,
                    ));
                }
                let dir = skill_dir(state, item.library_id, r.name(), index).await?;
                let from = FromLibrary {
                    library_id: item.library_id,
                    path: dir,
                    commit_sha: Some(item.commit_sha.clone()),
                };
                let prepared = library::prepare_add(state, caller, auth.org_id, &from).await?;
                prepared_refs.push((index, prepared));
            }
        }
    }

    // ---- everything from the first seal to the commit ------------------------------------------
    // One block, so that every way out of it but success passes the cleanup below (SEC-146 D).
    // `Ok(None)`: the offer moved on before the transaction.
    let mut outside = Outside::default();
    let outcome: Result<Option<CreatedFromTemplate>, ApiError> = async {
        // The webhooks' secrets are sealed before the transaction (vault IO); the agent that
        // does not exist yet is named by its org and workspace.
        for a in &rendered.automations {
            if let aurora_template::RenderedAutomation::Webhook {
                name,
                instructions,
                verification,
            } = a
            {
                let input = match verification {
                    aurora_template::Verification::None => Some(webhooks::dto::VerificationInput {
                        kind: "none".into(),
                        header: None,
                        secret: None,
                        generate_secret: false,
                    }),
                    // Signed and password hooks get the create default (or bearer): a new
                    // secret Aurora generates, shown once in the answer (C-1111).
                    aurora_template::Verification::Password => {
                        Some(webhooks::dto::VerificationInput {
                            kind: "bearer".into(),
                            header: None,
                            secret: None,
                            generate_secret: true,
                        })
                    }
                    aurora_template::Verification::Signed => None,
                };
                let p = webhooks::prepare(
                    webhooks::Create {
                        name: name.clone(),
                        instructions: instructions.clone(),
                        verification: input,
                        anchor: "creator",
                    },
                    Some(caller.user_id),
                )?;
                let verification = p.seal(state, auth.org_id, ws).await?;
                outside.hooks.push((p, verification));
            }
        }

        // The avatar blob goes to the store before the transaction; the row joins it.
        let avatar = match &rendered.avatar {
            Some((_, bytes)) => {
                let (_, store) = blob::current(&state.pool);
                let key = blob::new_key();
                // Recorded first: a put that fails may have written something.
                outside.avatar_key = Some(key.clone());
                store.put(&key, bytes).await?;
                let public_key = agents::avatar::new_public_key()?;
                Some((key, public_key))
            }
            None => None,
        };

        let p = Prepared {
            req: &req,
            name: &name,
            plan: &plan,
            content: &content,
            rendered: &rendered,
            inputs_json,
            accounts,
            refs: &prepared_refs,
            hooks: &outside.hooks,
            avatar,
        };

        // ---- the one transaction ---------------------------------------------------------------
        let mut tx = audit::begin(state).await?;
        // Re-read the offer and re-verify (SEC-146 B): it may have moved between the plan and
        // this transaction, and the agent is stamped with the version verified here. FOR SHARE
        // holds the template row and the approved version rows until the commit, so a
        // withdrawal or a change to those rows waits. It does not hold back the approval of a
        // pending version (that row is not locked): this create then still makes the version
        // it verified. The organization's switch is a plain read.
        let t = trepo::template_by_template_id_share(&mut *tx, &req.template_id)
            .await?
            .ok_or_else(not_found)?;
        if trepo::org_disabled(&mut *tx, auth.org_id)
            .await?
            .contains(&t.template_id)
        {
            return Ok(None);
        }
        let Some(v) = admin::offered_version_share(&mut *tx, t.id).await? else {
            return Ok(None);
        };
        if v.version != req.version
            || v.content_sha256.as_deref() != Some(req.content_sha256.as_str())
        {
            return Ok(None);
        }
        let (created, events) = create_in_tx(&mut tx, state, caller, &auth, ws, &p, &notes).await?;
        // The critical events (the grants, the library adds) commit with the transaction;
        // without any, a plain commit (C-1111: all or nothing).
        if events.is_empty() {
            tx.commit().await.map_err(aurora_db::DbError::from)?;
        } else {
            audit::commit_critical_many(tx, &events).await?;
        }
        Ok(Some(created))
    }
    .await;
    if let Ok(Some(created)) = outcome {
        return Ok(created);
    }
    // The transaction is rolled back (it was dropped with the block); remove what was made
    // outside it.
    outside.discard(state, auth.org_id, ws).await;
    match outcome {
        Err(e) => Err(e),
        Ok(_) => Err(current_version_changed(state, caller, ws, &req.template_id).await),
    }
}

/// The library directory of the referenced skill (the fetch pins to it), from the listing.
/// Shared with the C-1112 apply (the same resolution the create used).
pub(crate) async fn skill_dir(
    state: &AppState,
    library_id: Uuid,
    skill: &str,
    index: usize,
) -> Result<String, ApiError> {
    let row = skill_libraries::service::get(state, library_id).await?;
    let listing = skill_libraries::service::listing(state, &row).await?;
    match listing.items.iter().find(|i| i.name == skill && i.valid) {
        Some(i) => Ok(i.path.clone()),
        None => Err(at(
            "template_skill_unavailable",
            "This skill is not in this library.",
            "/skill_refs",
            index,
        )),
    }
}

/// Validate the account choices against the plan's entries (C-1111) and build the grants. The
/// plan is the source of truth: an app it did not offer and a connection it did not list (the
/// C-504 filter) are refused here.
async fn choose_accounts(
    state: &AppState,
    plan: &dto::TemplatePlan,
    given: &[AccountChoice],
    org: Uuid,
    ws: Uuid,
    notes: &mut Vec<CreateNote>,
) -> Result<Vec<ChosenAccount>, ApiError> {
    let mut by_entry: BTreeMap<usize, &AccountChoice> = BTreeMap::new();
    for g in given {
        if by_entry.insert(g.entry, g).is_some() {
            return Err(at(
                "template_account_invalid",
                "An entry takes at most one account.",
                "/accounts",
                g.entry,
            ));
        }
    }
    // One catalogue read for the custom tool splits.
    let mut snap: Option<aurora_registry::index::Snapshot> = None;
    let mut out = Vec::new();
    for (index, entry) in plan.entries.iter().enumerate() {
        let Some(g) = by_entry.get(&index) else {
            if entry.required {
                return Err(at(
                    "template_account_missing",
                    "This required app entry has no account.",
                    "/accounts",
                    index,
                ));
            }
            continue;
        };
        let Some(choice) = entry.choices.iter().find(|c| c.app_id == g.app_id) else {
            return Err(at(
                "template_app_unavailable",
                "This app is not one of the entry's choices.",
                "/accounts",
                index,
            ));
        };
        if !choice.available {
            return Err(at(
                "template_app_unavailable",
                "This app is not available in this workspace.",
                "/accounts",
                index,
            ));
        }
        // (C-504) The plan only lists the accounts the caller may give to an agent of this
        // workspace; anything else is the same 403.
        let giveable = choice.accounts.iter().any(|a| {
            a.reference.kind == AccountRefType::Connection
                && a.reference.id == g.connection_id
                && a.app.as_ref().is_some_and(|x| x.app_id == g.app_id)
        });
        if !giveable {
            return Err(forbid(
                "Workspace owner role required.",
                WhoCanRole::WorkspaceOwner,
                Some(org),
                Some(ws),
            ));
        }
        // (C-1111) The connection itself must be `active` (the plan lists accounts by whom
        // they may be given, not by the row's status); anything else is the same 403.
        let active = connections::get(&state.pool, g.connection_id)
            .await?
            .is_some_and(|c| c.org_id == org && c.status == "active");
        if !active {
            return Err(forbid(
                "Workspace owner role required.",
                WhoCanRole::WorkspaceOwner,
                Some(org),
                Some(ws),
            ));
        }
        let grant = match &g.custom {
            Some(custom) => {
                if snap.is_none() {
                    snap = apps::snapshot(state).await?;
                }
                let cat: BTreeSet<String> =
                    catalogue(snap.as_ref(), &g.app_id).into_iter().collect();
                let cut = |v: &[String]| {
                    v.iter()
                        .filter(|t| cat.contains(*t))
                        .cloned()
                        .collect::<Vec<_>>()
                };
                let (allow, ask) = (cut(&custom.allow_tools), cut(&custom.ask_tools));
                let dropped: Vec<String> = custom
                    .allow_tools
                    .iter()
                    .chain(custom.ask_tools.iter())
                    .filter(|t| !cat.contains(*t))
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                if !dropped.is_empty() {
                    notes.push(CreateNote {
                        code: "tools_dropped".into(),
                        entry: Some(index),
                        index: None,
                        detail: format!(
                            "Not in the app's current catalogue: {}.",
                            dropped.join(", ")
                        ),
                    });
                }
                if allow.is_empty() && ask.is_empty() {
                    // A custom choice left with no tool gives no access (C-1111).
                    notes.push(CreateNote {
                        code: "access_not_given".into(),
                        entry: Some(index),
                        index: None,
                        detail: "No tool left to give access to.".into(),
                    });
                    continue;
                }
                PutConnectionGrant {
                    mode: GrantMode::Custom,
                    allow_tools: allow,
                    ask_tools: ask,
                    ask_timeout_secs: None,
                }
            }
            None => {
                let level = level_of(g.level.as_deref().unwrap_or(&choice.level), index)?;
                if snap.is_none() {
                    snap = apps::snapshot(state).await?;
                }
                let tools = snap
                    .as_ref()
                    .and_then(|s| s.app(&g.app_id))
                    .map(apps::tools)
                    .unwrap_or_default();
                levels::expand(level, &tools)
            }
        };
        out.push(ChosenAccount {
            connection_id: g.connection_id,
            grant,
        });
    }
    Ok(out)
}

/// The tool ids of an app's current catalogue (the custom access is cut to it, C-1111).
fn catalogue(snap: Option<&aurora_registry::index::Snapshot>, app_id: &str) -> Vec<String> {
    snap.and_then(|s| s.app(app_id))
        .map(apps::tools)
        .map(|ts| ts.into_iter().map(|t| t.id).collect())
        .unwrap_or_default()
}

/// `read` | `ask` | `act` (C-1111's body level).
fn level_of(s: &str, entry: usize) -> Result<levels::Level, ApiError> {
    match s {
        "read" => Ok(levels::Level::Read),
        "ask" => Ok(levels::Level::Ask),
        "act" => Ok(levels::Level::Act),
        _ => Err(bad("/accounts", "level must be read, ask or act")).map_err(|e| {
            e.with_details(json!({
                "fields": [{"path": "/accounts", "message": format!("entry {entry}")}]
            }))
        }),
    }
}

// ---- the transaction ---------------------------------------------------------------------------

/// The one transaction of [`create_from_template`] (C-1111): every write goes through the
/// existing writer of its kind. Commits nothing: the answer and the critical events (the grants'
/// `grant.create`, the library skills' `skill.library.add`) come back; the caller commits —
/// through [`audit::commit_critical_many`] when there are critical events, else a plain commit —
/// or lets the transaction drop (rollback) on any error.
#[allow(clippy::too_many_arguments)]
async fn create_in_tx(
    tx: &mut audit::Tx,
    state: &AppState,
    caller: &Caller,
    auth: &aurora_db::authz::WorkspaceAuth,
    ws: Uuid,
    p: &Prepared<'_>,
    notes: &[CreateNote],
) -> Result<(CreatedFromTemplate, Vec<AuditEvent>), ApiError> {
    let org = auth.org_id;
    let content = p.content;
    let rendered = p.rendered;

    // The agent (C-1111): hosted, the default harness, no model, the template's effort, the
    // rendered instructions as the brief, proactive from `may_message_first`.
    let harness = state.ops().hosted.default_harness.clone();
    let effort = match content.meta.effort {
        Some(aurora_template::Effort::Low) => "low",
        Some(aurora_template::Effort::High) => "high",
        Some(aurora_template::Effort::Max) => "max",
        _ => "medium",
    };
    let agent = agents::service::insert_in_tx(
        tx,
        state,
        caller,
        org,
        ws,
        "hosted",
        p.name,
        p.req.handle.clone(),
        Some(harness),
        Some(effort),
        if rendered.may_message_first {
            "allow"
        } else {
            "ask"
        },
        &rendered.instructions,
        None,
        None,
        None,
    )
    .await?;
    // The avatar row (the blob is already in the store).
    if let Some((key, public_key)) = &p.avatar {
        aurora_db::repo::agents::set_avatar(
            &mut **tx,
            agent.id,
            Some(key.as_str()),
            Some(public_key.as_str()),
        )
        .await?;
    }

    // The provenance (C-1112): the template's id, name and version as text, the checked inputs.
    // The version is the request's, which the transaction re-verified against the version row
    // it read under the row lock (SEC-146 B) — never the plan's possibly older one.
    aurora_db::repo::agents::set_template(
        &mut **tx,
        agent.id,
        &content.meta.id,
        &p.plan.name,
        &p.req.version,
        &p.inputs_json,
    )
    .await?;

    let mut events: Vec<AuditEvent> = Vec::new();
    let mut skills_out: Vec<Skill> = Vec::new();
    let mut parts: Vec<trepo::PartRow> = Vec::new();

    // The instructions part (C-1112): the rendered brief's hash is what an update offer diffs
    // against (the brief itself was written by `insert_in_tx` above).
    parts.push(trepo::PartRow {
        agent_id: agent.id,
        kind: "instructions".into(),
        key: String::new(),
        content_sha256: instructions_hash(&rendered.instructions),
    });

    // The agent's own skills from the template: ordinary editable skills (C-1111).
    for s in &rendered.skills {
        let view = skills_domain::service::create_in_tx(
            tx,
            org,
            Owner {
                agent: Some(agent.id),
                workspace: None,
            },
            &NewSkill {
                name: &s.name,
                title: s.title.as_deref(),
                description: &s.use_it_when,
                body: &s.body,
                enabled: true,
            },
            "user",
        )
        .await?;
        for f in &s.files {
            aurora_db::repo::skills::upsert_file(
                tx,
                view.row.id,
                &f.path,
                &f.bytes,
                Some(f.executable),
            )
            .await?;
        }
        parts.push(trepo::PartRow {
            agent_id: agent.id,
            kind: "skill".into(),
            key: s.name.clone(),
            content_sha256: skill_hash(
                &s.use_it_when,
                &s.body,
                &s.files
                    .iter()
                    .map(|f| (f.path.clone(), f.bytes.clone(), f.executable))
                    .collect::<Vec<_>>(),
            ),
        });
        skills_out.push(skills_domain::service::view(tx, view.row).await?.into());
    }

    // The chosen library skills, through the library add path at the agent tier (C-1111,
    // C-1142): pinned, read-only, with their budgets — `library_budget_exceeded` rolls back.
    for (index, prepared) in p.refs {
        let (row, ev) = library::write_add_in_tx(
            tx,
            state,
            caller,
            org,
            Owner {
                agent: Some(agent.id),
                workspace: None,
            },
            None,
            prepared,
        )
        .await?;
        events.push(ev);
        let key = skill_ref_key(content.meta.skill_refs.get(*index).ok_or_else(not_found)?);
        parts.push(trepo::PartRow {
            agent_id: agent.id,
            kind: "library_skill".into(),
            key: key.clone(),
            content_sha256: dto::sha256_hex(key.as_bytes()),
        });
        skills_out.push(skills_domain::service::view(tx, row).await?.into());
    }

    // The helper files (C-1143): the one writer replaces the agent's set.
    if !rendered.helpers.is_empty() {
        let files: Vec<agent_helpers::service::HelperFile<'_>> = rendered
            .helpers
            .iter()
            .map(|f| agent_helpers::service::HelperFile {
                path: &f.path,
                content: &f.bytes,
                executable: f.executable,
            })
            .collect();
        agent_helpers::service::replace_all(tx, agent.id, &files).await?;
        parts.push(helpers_part(agent.id, &rendered.helpers));
    }

    // The memories (C-1151): author the person, provenance "from template <name> <version>";
    // a locked one is created locked, `locked_by` the caller.
    let author = memory::Author {
        kind: "user",
        user_id: Some(caller.user_id),
        api_key_id: match caller.principal {
            crate::http::auth::Principal::ApiKey { id, .. } => Some(id),
            _ => None,
        },
        label: String::new(),
    };
    let source = memory::Source::Template {
        name: p.plan.name.clone(),
        version: p.plan.version.clone(),
    };
    for m in &rendered.memories {
        let w = memory::Write {
            key: &m.key,
            title: None,
            description: None,
            content: &m.content,
            max_content: limits::MEMORY_CONTENT_MAX,
            create_only: false,
            skip_unchanged: false,
            locked: m.locked,
        };
        memory::service::check(&w)?;
        memory::service::save_in_tx(tx, agent.id, &w, &author, &source).await?;
    }

    // The schedules: creator = the caller, so the fire-time authority is theirs (C-1111).
    for a in &rendered.automations {
        if let aurora_template::RenderedAutomation::Schedule { name, prompt, cron } = a {
            let choice = p
                .req
                .automations
                .iter()
                .find(|c| c.kind == "schedule" && c.name == *name);
            let row = schedules::service::create_in_tx(
                tx,
                &agent,
                schedules::service::Create {
                    name: name.clone(),
                    prompt: prompt.clone(),
                    cron: Some(cron.clone()),
                    run_at: None,
                    timezone: choice.and_then(|c| c.timezone.clone()),
                    gate_command: None,
                    gate_description: None,
                    anchor: "creator",
                    ends_at: None,
                },
                Some(caller.user_id),
                None,
            )
            .await?;
            // A schedule the person switched off is created paused, through the same write
            // `patch` does.
            if choice.is_some_and(|c| !c.enabled) {
                schedules::service::pause_in_tx(tx, row, caller.user_id).await?;
            }
        }
    }

    // The webhooks, always paused (C-1111), each with its new secret (sealed above).
    let mut hooks_out: Vec<WebhookWithUrl> = Vec::new();
    for (hook, verification) in p.hooks {
        let row = webhooks::insert_in_tx(
            tx,
            &agent,
            hook,
            verification.clone(),
            None,
            Some(caller.user_id),
        )
        .await?;
        hooks_out.push(WebhookWithUrl {
            webhook: row.into(),
            url: hook.url(state),
            secret: hook.generated_secret.clone(),
        });
    }

    // Each account's access, through the one grant writer with `via: "template"` (C-1111).
    let mut grants_out: Vec<grants::dto::ConnectionGrant> = Vec::new();
    for a in &p.accounts {
        let written = grants::service::grant_connection_in_tx_for_agent(
            tx,
            state,
            &agent,
            Some(auth),
            caller,
            a.connection_id,
            a.grant.clone(),
            grants::service::Origin::Template,
            false,
        )
        .await?;
        grants_out.push(written.grant);
        if let Some(ev) = written.event {
            events.push(ev);
        }
    }

    // The parts the agent took (C-1112), and the `agent.create` audit row with the template
    // metadata (ordinary; inside the transaction so a rollback leaves no row).
    for part in &parts {
        trepo::insert_part(tx, part).await?;
    }
    let ev = AuditEvent::by(caller, "agent.create")
        .org(org)
        .workspace(ws)
        .target("agent", agent.id)
        .metadata(json!({
            "handle": agent.handle, "kind": agent.kind,
            "template": format!("{}@{}", content.meta.id, p.plan.version),
        }));
    audit::insert_tx(tx, &ev).await.map_err(|e| {
        tracing::error!(error = %e, "agent.create audit row not written");
        audit::unavailable()
    })?;

    // The answer's agent shows the provenance (C-1112): the row this transaction just stamped.
    let mut agent_row = agent.clone();
    agent_row.template_id = Some(content.meta.id.clone());
    agent_row.template_name = Some(p.plan.name.clone());
    agent_row.template_version = Some(p.plan.version.clone());
    agent_row.template_inputs = Some(p.inputs_json.clone());
    Ok((
        CreatedFromTemplate {
            agent: agent_row.into(),
            grants: grants_out,
            skills: skills_out,
            webhooks: hooks_out,
            notes: notes.to_vec(),
        },
        events,
    ))
}

/// The helpers part row of a rendered template's files (shared with the C-1112 apply).
pub(crate) fn helpers_part(agent: Uuid, files: &[aurora_template::TemplateFile]) -> trepo::PartRow {
    trepo::PartRow {
        agent_id: agent,
        kind: "helpers".into(),
        key: String::new(),
        content_sha256: helpers_hash(
            &files
                .iter()
                .map(|f| (f.path.clone(), f.bytes.clone(), f.executable))
                .collect::<Vec<_>>(),
        ),
    }
}

/// `POST /v1/workspaces/{ws}/agents/from-template` (C-1111).
#[utoipa::path(post, path = "/workspaces/{ws}/agents/from-template", operation_id = "createAgentFromTemplate", tag = "agent-templates",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = CreateFromTemplateRequest,
    responses((status = 201, body = CreatedFromTemplate), (status = 403, body = crate::http::error::ErrorEnvelope),
        (status = 404, body = crate::http::error::ErrorEnvelope), (status = 409, body = crate::http::error::ErrorEnvelope),
        (status = 422, body = crate::http::error::ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_from_template_route(
    caller: Caller,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(ws): axum::extract::Path<Uuid>,
    crate::http::json::Json(req): crate::http::json::Json<CreateFromTemplateRequest>,
) -> Result<
    (
        axum::http::StatusCode,
        crate::http::json::Json<CreatedFromTemplate>,
    ),
    ApiError,
> {
    let created = create_from_template(&state, &caller, ws, req).await?;
    Ok((
        axum::http::StatusCode::CREATED,
        crate::http::json::Json(created),
    ))
}

pub(super) fn router() -> utoipa_axum::router::OpenApiRouter<AppState> {
    utoipa_axum::router::OpenApiRouter::new()
        .routes(utoipa_axum::routes!(create_from_template_route))
}
