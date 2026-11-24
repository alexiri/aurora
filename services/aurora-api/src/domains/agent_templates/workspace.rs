//! The workspace catalogue (C-1110): the cards and the plan of the offered templates, built
//! from the stored rows and the resolved references — the apps against the instance's active
//! packages and the organization's app availability (07 §App availability), the skill
//! references resolved per C-1142. `can_use` on the workspace; the kill switch answers 404
//! `templates_disabled`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aurora_core::ApiError;
use aurora_db::repo::agent_templates::{self as repo};
use aurora_db::repo::skill_libraries::LibraryRow;
use aurora_registry::snapshot::{AppEntry, Snapshot};
use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::app::AppState;
use crate::domains::accounts::dto::{Account, AccountsQuery};
use crate::domains::accounts::service as accounts;
use crate::domains::agent_templates::admin;
use crate::domains::agent_templates::dto::{
    self, AutomationBrief, CardAppEntry, CardSkillRef, CustomToolsView, FileView, PlanChoice,
    PlanEntry, ResolvedLibraryView, SkillContentBrief, SkillRefPlan, TemplateCard,
    TemplateCardList, TemplatePlan,
};
use crate::domains::apps::service as apps;
use crate::domains::skill_libraries;
use crate::domains::skills;
use crate::http::auth::Caller;

use super::org::ensure_enabled;

fn not_found() -> ApiError {
    ApiError::not_found("Template not found.")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Everything the catalogue reads once per request: the workspace's organization, the active
/// packages, the organization's app availability and the organization's template switches.
pub(crate) struct Ctx {
    org: Uuid,
    ws: Uuid,
    snap: Option<Snapshot>,
    /// `restricted` | `open` (the org exists, so this is never null in practice).
    posture: Option<String>,
    /// The allow-list rows `(app_id, workspace_id)`; a NULL workspace allows the whole org.
    allow: Vec<(String, Option<Uuid>)>,
    disabled: BTreeSet<String>,
}

impl Ctx {
    async fn build(state: &AppState, caller: &Caller, ws: Uuid) -> Result<Self, ApiError> {
        ensure_enabled(state)?;
        let auth = caller.workspace(state, ws).await?;
        let org = auth.org_id;
        let snap = apps::snapshot(state).await?;
        let posture = aurora_db::repo::availability::posture(&state.pool, org).await?;
        let allow = aurora_db::repo::availability::list(&state.pool, org).await?;
        let disabled = repo::org_disabled(&state.pool, org)
            .await?
            .into_iter()
            .collect();
        Ok(Self {
            org,
            ws,
            snap,
            posture,
            allow,
            disabled,
        })
    }

    /// The app's catalogue entry, when it is active on the instance.
    fn app(&self, app_id: &str) -> Option<&AppEntry> {
        self.snap.as_ref().and_then(|s| s.app(app_id))
    }

    /// Available to this workspace under 07 §App availability: active on the instance, and —
    /// under a `restricted` posture — on the organization's allow-list (org-wide or this
    /// workspace).
    fn available(&self, app_id: &str) -> bool {
        if self.app(app_id).is_none() {
            return false;
        }
        if self.posture.as_deref() != Some("restricted") {
            return true;
        }
        self.allow
            .iter()
            .any(|(id, w)| id == app_id && (w.is_none() || *w == Some(self.ws)))
    }

    /// Why a choice's app is unavailable (C-1110), null when it is available.
    fn unavailable_reason(&self, app_id: &str) -> Option<&'static str> {
        if self.app(app_id).is_none() {
            Some("not_installed")
        } else if !self.available(app_id) {
            Some("not_available_here")
        } else {
            None
        }
    }
}

/// A template's stored content, with the parsed parts the catalogue reads (shared with the
/// C-1112 update offer).
pub(crate) struct Parsed<'a> {
    pub(crate) content: &'a Value,
    pub(crate) readme: String,
    pub(crate) apps: Vec<EntryJson>,
    pub(crate) refs: Vec<RefJson>,
    pub(crate) has_helpers: bool,
    pub(crate) automations: Vec<AutomationBrief>,
}

/// One `apps:` entry of the stored content, as the catalogue reads it.
pub(crate) struct EntryJson {
    pub(crate) app_ids: Vec<String>,
    pub(crate) required: bool,
    pub(crate) why: String,
    pub(crate) level: Option<String>,
    /// The template's `tools:` split, when the entry has one.
    pub(crate) tools: Option<(Vec<String>, Vec<String>)>,
}

impl EntryJson {
    fn parse(v: &Value) -> Option<Self> {
        let app_ids = match (v.get("app"), v.get("any_of")) {
            (Some(Value::String(a)), _) => vec![a.clone()],
            (_, Some(Value::Array(ids))) => ids
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => return None,
        };
        let tools = v.get("tools").filter(|t| !t.is_null()).map(|t| {
            let arr = |k: &str| {
                t.get(k)
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            };
            (arr("allow"), arr("ask"))
        });
        Some(Self {
            app_ids,
            required: v.get("required").and_then(Value::as_bool).unwrap_or(true),
            why: v
                .get("why")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            level: v.get("level").and_then(Value::as_str).map(str::to_owned),
            tools,
        })
    }
}

/// One skill reference of the stored content, as the resolver reads it (C-1142).
pub(crate) struct RefJson {
    pub(crate) kind: &'static str,
    pub(crate) name: String,
    pub(crate) required: bool,
    pub(crate) why: String,
    /// The normalized address of a `library` reference.
    pub(crate) url: Option<String>,
    pub(crate) reference: Option<String>,
    /// The folder in the repository (the root is `""`).
    pub(crate) path: String,
}

impl RefJson {
    fn parse(v: &Value) -> Option<Self> {
        let kind = match v.get("kind").and_then(Value::as_str) {
            Some("library") => "library",
            Some("organization") => "organization",
            _ => return None,
        };
        let name = match kind {
            "library" => v.get("skill").and_then(Value::as_str)?,
            _ => v.get("name").and_then(Value::as_str)?,
        };
        Some(Self {
            kind,
            name: name.to_owned(),
            required: v.get("required").and_then(Value::as_bool).unwrap_or(true),
            why: v
                .get("why")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            url: v.get("address").and_then(Value::as_str).map(str::to_owned),
            reference: v
                .get("ref")
                .and_then(Value::as_str)
                .filter(|s: &&str| !s.is_empty())
                .map(str::to_owned),
            path: v
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }
}

impl Parsed<'_> {
    pub(crate) fn parse(content: &Value) -> Option<Parsed<'_>> {
        let meta = content.get("meta")?;
        let apps = meta
            .get("apps")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(EntryJson::parse).collect::<Vec<_>>())
            .unwrap_or_default();
        let refs = meta
            .get("skill_refs")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(RefJson::parse).collect::<Vec<_>>())
            .unwrap_or_default();
        Some(Parsed {
            readme: content.get("readme")?.as_str()?.to_owned(),
            has_helpers: content
                .get("helpers")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty()),
            automations: content
                .get("automations")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| {
                            Some(AutomationBrief {
                                kind: x.get("kind")?.as_str()?.to_owned(),
                                name: x.get("name")?.as_str()?.to_owned(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            apps,
            refs,
            content,
        })
    }
}

/// The C-1142 resolution of one reference.
pub(crate) struct Resolved {
    pub(crate) available: bool,
    pub(crate) unavailable_reason: Option<&'static str>,
    /// The matched library and the listing commit the content comes from.
    pub(crate) matched: Option<(LibraryRow, String)>,
    /// The matched skill's directory in the library (the `skill_content` dir).
    pub(crate) dir: Option<String>,
}

/// The C-1142 resolver: the candidate libraries of the organization (its own, then the
/// instance ones, by label), the listing cache's single read per library and the
/// organization-tier names, queried once per name.
pub(crate) struct Resolver<'a> {
    state: &'a AppState,
    org: Uuid,
    ws: Uuid,
    /// Whether the skill-library kill switch is off.
    libs_disabled: bool,
    libs: Vec<LibraryRow>,
    listings: HashMap<Uuid, Option<std::sync::Arc<skill_libraries::service::Listing>>>,
    org_names: BTreeMap<String, bool>,
}

impl<'a> Resolver<'a> {
    /// The candidates (C-1142): the organization's own libraries, then the instance ones,
    /// each group by label. The `active` filter happens at match time (a paused or failed
    /// library is `library_unreadable`, not `library_not_added`).
    pub(crate) async fn new(state: &'a AppState, org: Uuid, ws: Uuid) -> Result<Self, ApiError> {
        let rows = skill_libraries::service::list_for_org(state, org).await?;
        let mut own: Vec<LibraryRow> = Vec::new();
        let mut instance: Vec<LibraryRow> = Vec::new();
        for l in rows {
            if l.org_id.is_some() {
                own.push(l);
            } else {
                instance.push(l);
            }
        }
        own.sort_by(|a, b| a.label.cmp(&b.label));
        instance.sort_by(|a, b| a.label.cmp(&b.label));
        own.extend(instance);
        Ok(Self {
            state,
            org,
            ws,
            libs_disabled: !state.ops().skills_library.enabled,
            libs: own,
            listings: HashMap::new(),
            org_names: BTreeMap::new(),
        })
    }

    /// The listing of one library from the C-571 cache; `None` when the read failed (an
    /// unreadable library). At most one read per library per request.
    async fn listing(
        &mut self,
        row: &LibraryRow,
    ) -> Option<std::sync::Arc<skill_libraries::service::Listing>> {
        if let Some(l) = self.listings.get(&row.id) {
            return l.clone();
        }
        let out = skill_libraries::service::listing(self.state, row)
            .await
            .ok();
        self.listings.insert(row.id, out.clone());
        out
    }

    /// Whether an `organization` reference's name exists as an enabled organization-tier skill
    /// of the org or an enabled workspace-tier skill of the workspace (queried once per name).
    async fn org_name(&mut self, name: &str) -> Result<bool, ApiError> {
        if let Some(y) = self.org_names.get(name) {
            return Ok(*y);
        }
        let found = aurora_db::repo::skills::enabled_org_or_workspace_names(
            &self.state.pool,
            self.org,
            self.ws,
            std::slice::from_ref(&name.to_owned()),
        )
        .await?;
        let y = !found.is_empty();
        self.org_names.insert(name.to_owned(), y);
        Ok(y)
    }

    pub(crate) async fn resolve(&mut self, r: &RefJson) -> Result<Resolved, ApiError> {
        if r.kind == "organization" {
            let ok = self.org_name(&r.name).await?;
            return Ok(Resolved {
                available: ok,
                unavailable_reason: (!ok).then_some("not_in_organization"),
                matched: None,
                dir: None,
            });
        }
        if self.libs_disabled {
            return Ok(Resolved {
                available: false,
                unavailable_reason: Some("libraries_disabled"),
                matched: None,
                dir: None,
            });
        }
        let Some(url) = r.url.as_deref() else {
            return Ok(Resolved {
                available: false,
                unavailable_reason: Some("library_not_added"),
                matched: None,
                dir: None,
            });
        };
        // The candidate rule: same url, same folder, and — when the reference gives a `ref` —
        // the same one. The first match wins (org's own before instance ones, then by label).
        let row = self
            .libs
            .iter()
            .find(|l| {
                l.url.as_deref() == Some(url)
                    && l.path == r.path
                    && (r.reference.is_none() || l.reference.as_deref() == r.reference.as_deref())
            })
            .cloned();
        let Some(row) = row else {
            return Ok(Resolved {
                available: false,
                unavailable_reason: Some("library_not_added"),
                matched: None,
                dir: None,
            });
        };
        if row.status != "active" {
            return Ok(Resolved {
                available: false,
                unavailable_reason: Some("library_unreadable"),
                matched: None,
                dir: None,
            });
        }
        let Some(listing) = self.listing(&row).await else {
            return Ok(Resolved {
                available: false,
                unavailable_reason: Some("library_unreadable"),
                matched: None,
                dir: None,
            });
        };
        match listing.items.iter().find(|i| i.name == r.name) {
            Some(i) if i.valid => {
                // The agent-tier library budget (C-1142's `over_budget`): the gallery answers
                // from the listing's byte size; the plan re-checks the loaded content's exact
                // budget unit.
                if !skills::library::agent_tier_fits(self.state, i.size as usize) {
                    return Ok(Resolved {
                        available: false,
                        unavailable_reason: Some("over_budget"),
                        matched: None,
                        dir: None,
                    });
                }
                Ok(Resolved {
                    available: true,
                    unavailable_reason: None,
                    matched: Some((row, listing.commit_sha.clone())),
                    dir: Some(i.path.clone()),
                })
            }
            _ => Ok(Resolved {
                available: false,
                unavailable_reason: Some("skill_not_in_library"),
                matched: None,
                dir: None,
            }),
        }
    }
}

/// The card fields of one available template, plus (for the plan) the choices and the
/// resolved references with content.
async fn build_card(
    ctx: &Ctx,
    t: &repo::TemplateRow,
    source_name: String,
    version: String,
    parsed: &Parsed<'_>,
    refs: &[SkillRefPlan],
) -> TemplateCard {
    let apps = parsed
        .apps
        .iter()
        .map(|e| {
            let mut skills = BTreeSet::new();
            for id in &e.app_ids {
                if !ctx.available(id) {
                    continue;
                }
                let Some(agent) = ctx.app(id).and_then(|a| a.agent.as_ref()) else {
                    continue;
                };
                skills.extend(agent.skills.iter().map(|s| s.name.clone()));
            }
            CardAppEntry {
                app_ids: e.app_ids.clone(),
                required: e.required,
                available: e.app_ids.iter().any(|id| ctx.available(id)),
                skills: skills.into_iter().collect(),
            }
        })
        .collect();
    let card_refs = refs
        .iter()
        .map(|r| CardSkillRef {
            index: r.index,
            kind: r.kind.clone(),
            name: r.name.clone(),
            required: r.required,
            available: r.available,
            repo: r.repo.clone(),
        })
        .collect();
    TemplateCard {
        template_id: t.template_id.clone(),
        name: t.name.clone(),
        summary: t.summary.clone(),
        readme: parsed.readme.clone(),
        version,
        source_name,
        has_avatar: dto::TemplateContentView::avatar_bytes(parsed.content).is_some(),
        apps,
        skill_refs: card_refs,
        has_helpers: parsed.has_helpers,
        automations: parsed.automations.clone(),
        ready: refs.iter().filter(|r| r.required).all(|r| r.available)
            && parsed
                .apps
                .iter()
                .filter(|e| e.required)
                .all(|e| e.app_ids.iter().any(|id| ctx.available(id))),
    }
}

/// One entry's choices (C-1110): per app, what creating would preselect.
async fn choices(
    ctx: &Ctx,
    e: &EntryJson,
    accounts: &[Account],
) -> Result<Vec<PlanChoice>, ApiError> {
    let mut out = Vec::new();
    for app_id in &e.app_ids {
        let entry = ctx.app(app_id);
        let installed = entry.is_some();
        let available = ctx.available(app_id);
        let app_name = entry
            .map(|a| a.about.name.clone())
            .unwrap_or_else(|| app_id.clone());
        let mut level = e.level.clone().unwrap_or_else(|| "read".to_owned());
        let mut level_note = None;
        // `act` on a `custom_only` app becomes `ask` (C-1110).
        if level == "act" && installed && entry.is_some_and(|a| a.grants.custom_only) {
            level = "ask".to_owned();
            level_note = Some("custom_only".to_owned());
        }
        let custom = e.tools.as_ref().map(|(allow, ask)| {
            let catalogue: BTreeSet<String> = entry
                .map(|a| {
                    apps::tools(a)
                        .into_iter()
                        .map(|t| t.id)
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            let cut = |v: &[String]| {
                v.iter()
                    .filter(|t| catalogue.contains(*t))
                    .cloned()
                    .collect()
            };
            let mut dropped: Vec<String> = allow
                .iter()
                .chain(ask.iter())
                .filter(|t| !catalogue.contains(*t))
                .cloned()
                .collect();
            dropped.sort();
            dropped.dedup();
            CustomToolsView {
                allow_tools: cut(allow),
                ask_tools: cut(ask),
                dropped_tools: dropped,
            }
        });
        // The app's accounts the caller may give to an agent of this workspace (C-504's
        // `give_access`), each with its `limit` where it has one.
        let for_app: Vec<Account> = accounts
            .iter()
            .filter(|a| a.can.give_access && a.app.as_ref().is_some_and(|x| x.app_id == *app_id))
            .cloned()
            .collect();
        out.push(PlanChoice {
            app_id: app_id.clone(),
            app_name,
            available,
            unavailable_reason: ctx.unavailable_reason(app_id).map(str::to_owned),
            level,
            level_note,
            custom,
            accounts: for_app,
        });
    }
    Ok(out)
}

/// The offered version row of an available template, or 404 `not_found` (C-1110).
pub(crate) async fn offered(
    state: &AppState,
    ctx: &Ctx,
    template_id: &str,
) -> Result<(repo::TemplateRow, repo::VersionRow), ApiError> {
    let t = repo::template_by_template_id(&state.pool, template_id)
        .await?
        .ok_or_else(not_found)?;
    if !t.offered || ctx.disabled.contains(&t.template_id) {
        return Err(not_found());
    }
    let v = admin::offered_version(&state.pool, t.id)
        .await?
        .ok_or_else(not_found)?;
    let v = v.content.is_some().then_some(v).ok_or_else(not_found)?;
    Ok((t, v))
}

/// (C-1142) The resolved library view of one matched library at one commit (shared by the plan
/// and the C-1112 offer).
pub(crate) fn resolved_view(row: &LibraryRow, commit: &str) -> ResolvedLibraryView {
    ResolvedLibraryView {
        id: row.id,
        label: row.label.clone(),
        scope: if row.org_id.is_some() {
            "organization"
        } else {
            "instance"
        }
        .to_owned(),
        repo: row.repo.clone().unwrap_or_default(),
        reference: row.reference.clone(),
        path: row.path.clone(),
        commit_sha: commit.to_owned(),
    }
}

/// (C-1142, amended) The reference's own library address in display form: the C-1123 parser's
/// `host/path` (no scheme, no trailing `.git`) — the same display normalization
/// [`ResolvedLibraryView.repo`] uses. `None` for an `organization` reference.
pub(crate) fn ref_repo(r: &RefJson) -> Option<String> {
    if r.kind != "library" {
        return None;
    }
    r.url
        .as_deref()
        .and_then(|u| aurora_core::git_url::parse(u).ok())
        .map(|a| a.repo_display)
}

/// (C-1142) The resolved skill references of a stored content, shared by the plan (C-1110)
/// and the update offer (C-1112).
pub(crate) async fn skill_ref_plans(
    state: &AppState,
    resolver: &mut Resolver<'_>,
    refs: &[RefJson],
) -> Result<Vec<SkillRefPlan>, ApiError> {
    let mut out = Vec::with_capacity(refs.len());
    for (index, r) in refs.iter().enumerate() {
        let mut x = resolver.resolve(r).await?;
        let (library, content_brief) = match (&x.matched, x.available) {
            (Some((row, commit)), true) => {
                let dir = x.dir.clone().unwrap_or_default();
                let c =
                    skill_libraries::service::skill_content(state, row, &dir, Some(commit)).await?;
                if !c.valid {
                    (None, None)
                } else if !skills::library::agent_tier_fits(state, c.size) {
                    // The loaded content's exact budget unit does not fit the agent tier
                    // (C-1142's `over_budget`); the listing's estimate had let it through.
                    x.available = false;
                    x.unavailable_reason = Some("over_budget");
                    (None, None)
                } else {
                    let view = resolved_view(row, commit);
                    let brief = SkillContentBrief {
                        use_it_when: c.description.unwrap_or_default(),
                        body: c.body.unwrap_or_default(),
                        files: c.files.iter().map(file_view).collect(),
                    };
                    (Some(view), Some(brief))
                }
            }
            _ => (None, None),
        };
        out.push(SkillRefPlan {
            index,
            kind: r.kind.to_owned(),
            name: r.name.clone(),
            required: r.required,
            why: r.why.clone(),
            available: x.available,
            unavailable_reason: x.unavailable_reason.map(str::to_owned),
            repo: ref_repo(r),
            library,
            content: content_brief,
        });
    }
    Ok(out)
}

/// `GET /v1/workspaces/{ws}/agent-templates` (C-1110): the available templates, by name.
pub async fn list_cards(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
) -> Result<TemplateCardList, ApiError> {
    let ctx = Ctx::build(state, caller, ws).await?;
    let rows = repo::all_templates_with_source(&state.pool).await?;
    let mut resolver = Resolver::new(state, ctx.org, ctx.ws).await?;
    let mut items = Vec::new();
    for t in &rows {
        if !t.template.offered || ctx.disabled.contains(&t.template.template_id) {
            continue;
        }
        let Some(v) = admin::offered_version(&state.pool, t.template.id).await? else {
            continue;
        };
        let Some(content) = v.content.as_ref() else {
            continue;
        };
        let Some(parsed) = Parsed::parse(content) else {
            continue;
        };
        let mut refs = Vec::with_capacity(parsed.refs.len());
        for (index, r) in parsed.refs.iter().enumerate() {
            let x = resolver.resolve(r).await?;
            refs.push(SkillRefPlan {
                index,
                kind: r.kind.to_owned(),
                name: r.name.clone(),
                required: r.required,
                why: r.why.clone(),
                available: x.available,
                unavailable_reason: x.unavailable_reason.map(str::to_owned),
                repo: ref_repo(r),
                library: None,
                content: None,
            });
        }
        items.push(
            build_card(
                &ctx,
                &t.template,
                t.source_name.clone(),
                v.version,
                &parsed,
                &refs,
            )
            .await,
        );
    }
    items.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.template_id.cmp(&b.template_id))
    });
    Ok(TemplateCardList { items })
}

/// `GET /v1/workspaces/{ws}/agent-templates/{template_id}` (C-1110): the full plan.
pub async fn plan(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    template_id: &str,
) -> Result<TemplatePlan, ApiError> {
    let ctx = Ctx::build(state, caller, ws).await?;
    let (t, v) = offered(state, &ctx, template_id).await?;
    let content = v.content.as_ref().ok_or_else(not_found)?;
    let Some(parsed) = Parsed::parse(content) else {
        return Err(not_found());
    };
    let source_name = repo::get_source(&state.pool, t.source_id)
        .await?
        .map(|s| s.name)
        .unwrap_or_default();
    let mut resolver = Resolver::new(state, ctx.org, ctx.ws).await?;
    let refs = skill_ref_plans(state, &mut resolver, &parsed.refs).await?;
    let card = build_card(&ctx, &t, source_name, v.version.clone(), &parsed, &refs).await;
    // One accounts read for the whole plan (C-504's `give_access` flags included); the
    // choices filter it per app.
    let all_accounts = if parsed.apps.is_empty() {
        Vec::new()
    } else {
        accounts::list_workspace(
            state,
            caller,
            ws,
            &AccountsQuery {
                agent_id: None,
                kind: Some("app".to_owned()),
                app_id: None,
            },
        )
        .await?
        .items
    };
    let mut entries = Vec::with_capacity(parsed.apps.len());
    for (index, e) in parsed.apps.iter().enumerate() {
        entries.push(PlanEntry {
            index,
            required: e.required,
            why: e.why.clone(),
            choices: choices(&ctx, e, &all_accounts).await?,
        });
    }
    let view = dto::TemplateContentView::from_json(content).ok_or_else(not_found)?;
    Ok(TemplatePlan {
        template_id: card.template_id,
        name: card.name,
        summary: card.summary,
        readme: card.readme,
        version: card.version,
        source_name: card.source_name,
        has_avatar: card.has_avatar,
        apps: card.apps,
        has_helpers: card.has_helpers,
        automations: card.automations,
        ready: card.ready,
        content_sha256: v.content_sha256.clone(),
        content: view,
        entries,
        skill_refs: refs,
    })
}

/// `GET …/{template_id}/avatar` (C-1110): the offered version's avatar PNG.
pub async fn avatar(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    template_id: &str,
) -> Result<Vec<u8>, ApiError> {
    let ctx = Ctx::build(state, caller, ws).await?;
    let (_, v) = offered(state, &ctx, template_id).await?;
    let content = v.content.as_ref().ok_or_else(not_found)?;
    dto::TemplateContentView::avatar_bytes(content).ok_or_else(not_found)
}

/// [`FileView`] of a library file (C-1142's `SkillRefPlan.content.files`): the sha256 comes
/// from the decoded bytes, the text by the review's cap.
fn file_view(f: &skill_libraries::dto::LibraryFile) -> FileView {
    let bytes = match (&f.content, &f.content_b64) {
        (Some(t), _) => t.as_bytes().to_vec(),
        (None, Some(b64)) => dto::B64.decode(b64).unwrap_or_default(),
        _ => Vec::new(),
    };
    let text = (!f.binary)
        .then(|| std::str::from_utf8(&bytes).ok())
        .flatten()
        .filter(|_| bytes.len() <= dto::FILE_TEXT_MAX)
        .map(str::to_owned);
    FileView {
        path: f.path.clone(),
        size: f.size as i64,
        executable: f.executable,
        sha256: sha256_hex(&bytes),
        text,
    }
}

// ---- the routes ---------------------------------------------------------------------------------

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_templates))
        .routes(routes!(get_plan))
        .routes(routes!(get_avatar))
}

/// The workspace's available templates (C-1110), ordered by name.
#[utoipa::path(get, path = "/workspaces/{ws}/agent-templates", operation_id = "listWorkspaceTemplates", tag = "agent-templates",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, body = TemplateCardList), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_templates(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
) -> Result<Json<TemplateCardList>, ApiError> {
    Ok(Json(list_cards(&s, &c, ws).await?))
}

/// One template's full plan (C-1110). A template that is not available is 404 `not_found`.
#[utoipa::path(get, path = "/workspaces/{ws}/agent-templates/{template_id}", operation_id = "getWorkspaceTemplatePlan", tag = "agent-templates",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("template_id" = String, Path, description = "Template id")),
    responses((status = 200, body = TemplatePlan), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_plan(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, template_id)): Path<(Uuid, String)>,
) -> Result<Json<TemplatePlan>, ApiError> {
    Ok(Json(plan(&s, &c, ws, &template_id).await?))
}

/// The offered version's avatar (C-1110), `image/png`. A template that is not available is
/// 404 `not_found`.
#[utoipa::path(get, path = "/workspaces/{ws}/agent-templates/{template_id}/avatar", operation_id = "getWorkspaceTemplateAvatar", tag = "agent-templates",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("template_id" = String, Path, description = "Template id")),
    responses((status = 200, body = Vec<u8>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_avatar(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, template_id)): Path<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let bytes = avatar(&s, &c, ws, &template_id).await?;
    Ok(([(header::CONTENT_TYPE, "image/png")], bytes).into_response())
}
