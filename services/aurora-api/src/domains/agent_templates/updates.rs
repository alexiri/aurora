//! The provenance and updates of an agent made from a template (C-1112, P6-TPL-3): the update
//! offer (the diff against the offered version), the apply and the detach. Until the person
//! applies an update the agent does not change (D-457(f)): everything here reads the stored
//! versions — never the repository — and an apply changes only the parts the person listed.
//!
//! Hash comparability: every part hash was recorded at creation from content rendered with the
//! **creator's** display name and the agent's name. The offer and the apply therefore render
//! with the creator's name again (never the viewer's) — the recorded hashes stay comparable
//! whatever the person reading the offer is called, and stay comparable across applies.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use uuid::Uuid;

use aurora_template::{helpers_hash, instructions_hash, skill_hash, skill_ref_key};

use crate::app::AppState;
use crate::audit::{self, AuditEvent};
use crate::domains::agent_helpers;
use crate::domains::agent_templates::admin;
use crate::domains::agent_templates::create;
use crate::domains::agent_templates::dto::{
    self, AgentTemplateRefView, AgentTemplateStatus, ApplyTemplateRequest, OfferPart,
};
use crate::domains::agent_templates::workspace::{RefJson, Resolver, skill_ref_plans};
use crate::domains::agents;
use crate::domains::skills::dto::FromLibrary;
use crate::domains::skills::{self as skills_domain, NewSkill, library};
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::repo::agent_templates as trepo;
use aurora_db::repo::skills::Owner;
use aurora_db::repo::{
    agent_helpers as helpers_repo, agents as agents_repo, grants as grants_repo,
    memory as memory_repo, skills as skills_repo,
};

/// The agent row behind these routes, with `can_use` of its workspace (404/403 as for any
/// agent route).
async fn agent_row(
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
) -> Result<agents_repo::AgentRow, ApiError> {
    let agent = agents_repo::get(&state.pool, agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    caller.workspace(state, agent.workspace_id).await?;
    Ok(agent)
}

/// The display name an agent's parts were rendered with at creation: the **creator's**
/// (C-1112). Rendering with the viewer's name would make every recorded hash differ for every
/// viewer, so the offer and the apply always render with the creator's name again.
async fn creator_person(
    state: &AppState,
    agent: &agents_repo::AgentRow,
) -> Result<String, ApiError> {
    Ok(
        match agents_repo::created_by(&state.pool, agent.id).await? {
            Some(user) => aurora_db::repo::users::find_by_id(&state.pool, user)
                .await?
                .map(|u| {
                    if u.display_name.trim().is_empty() {
                        u.email
                    } else {
                        u.display_name
                    }
                })
                .unwrap_or_default(),
            // No creator recorded (pre-provenance rows): render without a person name.
            None => String::new(),
        },
    )
}

/// The stored input values of the agent (C-1112's `template_inputs`), as given.
fn stored_inputs(agent: &agents_repo::AgentRow) -> BTreeMap<String, String> {
    agent
        .template_inputs
        .as_ref()
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

// ---- the offered version and the stored one -----------------------------------------------------

/// Everything the offer reads once: the template row, the offered version and both contents.
/// `None` when the template is no longer available (deleted, withdrawn, turned off for the
/// organization, or the kill switch is off): the agent keeps its provenance and runs its own
/// content, but no offer is built (C-1112).
struct OfferCtx {
    new_content: aurora_template::TemplateContent,
    /// The agent's stored version's content (the creation-time or last-applied one), when
    /// still stored — the `removed_upstream` match base and the old references' resolution.
    old_content: Option<aurora_template::TemplateContent>,
    offered: trepo::VersionRow,
}

async fn offer_ctx(
    state: &AppState,
    agent: &agents_repo::AgentRow,
) -> Result<Option<OfferCtx>, ApiError> {
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    offer_ctx_on(state, &mut conn, agent).await
}

/// [`offer_ctx`] on an existing connection (SEC-146 A): the apply's transaction re-runs it
/// under the per-agent lock, so the offer it verifies against is the one it writes with.
async fn offer_ctx_on(
    state: &AppState,
    conn: &mut sqlx::PgConnection,
    agent: &agents_repo::AgentRow,
) -> Result<Option<OfferCtx>, ApiError> {
    if !state.ops().agents.templates.enabled {
        return Ok(None);
    }
    let Some(template_id) = &agent.template_id else {
        return Ok(None);
    };
    let Some(t) = trepo::template_by_template_id(&mut *conn, template_id).await? else {
        return Ok(None);
    };
    if !t.offered {
        return Ok(None);
    }
    if trepo::org_disabled(&mut *conn, agent.org_id)
        .await?
        .iter()
        .any(|x| x == template_id)
    {
        return Ok(None);
    }
    let Some(v) = admin::offered_version(&mut *conn, t.id).await? else {
        return Ok(None);
    };
    let Some(content_v) = v.content.as_ref() else {
        return Ok(None);
    };
    let Some(new_content) = dto::content_from_json(content_v) else {
        return Ok(None);
    };
    // Older approved versions stay stored (C-1108): the agent's own version's content is the
    // base the removed references are resolved against.
    let old_content = match agent.template_version.as_deref() {
        Some(ver) => trepo::get_version(&mut *conn, t.id, ver)
            .await?
            .and_then(|ov| ov.content)
            .and_then(|c| dto::content_from_json(&c)),
        None => None,
    };
    Ok(Some(OfferCtx {
        new_content,
        old_content,
        offered: v,
    }))
}

/// Whether the offered version is semver-newer than the agent's stored one (C-1112).
fn newer_than_stored(offered: &str, stored: Option<&str>) -> bool {
    let Some(o) = aurora_registry::manifest::parse_semver(offered) else {
        return false;
    };
    match stored.and_then(aurora_registry::manifest::parse_semver) {
        Some(s) => o > s,
        // No stored semver to compare against: the offer stands.
        None => true,
    }
}

// ---- the part states ----------------------------------------------------------------------------

/// Which of the offer's states a part gets, from its recorded hash, the offered content's hash
/// and the agent's live hash (C-1112). Only parts that differ are listed:
///
/// - not recorded and the template has it → `new`;
/// - recorded and the template no longer has it → `removed_upstream` (the template's move wins:
///   there is nothing of this part left to apply);
/// - recorded, still offered, but the agent no longer has it → `removed_here`;
/// - the offered content is what was recorded, or the agent already has it → not listed;
/// - the agent's copy still has the recorded hash → `unchanged_here` (the template changed it);
/// - otherwise the agent edited it → `edited_here`.
fn listed(
    recorded: Option<&String>,
    proposed_hash: Option<&str>,
    current_hash: Option<&str>,
) -> Option<&'static str> {
    let (rec, proposed) = match (recorded, proposed_hash) {
        (None, None) => return None,
        (None, Some(_)) => return Some("new"),
        (Some(rec), proposed) => (rec, proposed),
    };
    match proposed {
        None => Some("removed_upstream"),
        Some(ph) => match current_hash {
            None => Some("removed_here"),
            Some(ch) => {
                if ph == rec.as_str() || ch == ph {
                    None
                } else if ch == rec.as_str() {
                    Some("unchanged_here")
                } else {
                    Some("edited_here")
                }
            }
        },
    }
}

/// The `RefJson` of a stored reference (the resolver's input shape).
fn ref_json(r: &aurora_template::SkillRef) -> RefJson {
    match r {
        aurora_template::SkillRef::Library {
            address,
            ref_,
            path,
            skill,
            required,
            why,
        } => RefJson {
            kind: "library",
            name: skill.clone(),
            required: *required,
            why: why.clone(),
            url: Some(address.url.clone()),
            reference: ref_.clone(),
            path: path.clone().unwrap_or_default(),
        },
        aurora_template::SkillRef::Organization {
            name,
            required,
            why,
        } => RefJson {
            kind: "organization",
            name: name.clone(),
            required: *required,
            why: why.clone(),
            url: None,
            reference: None,
            path: String::new(),
        },
    }
}

fn level_str(l: aurora_template::Level) -> &'static str {
    match l {
        aurora_template::Level::Read => "read",
        aurora_template::Level::Ask => "ask",
        aurora_template::Level::Act => "act",
    }
}

/// The offer's text of a library reference.
fn ref_text(r: &aurora_template::SkillRef) -> String {
    match r {
        aurora_template::SkillRef::Library {
            address,
            ref_,
            path,
            skill,
            ..
        } => {
            let mut s = format!("library {} skill {skill}", address.url);
            if let Some(r) = ref_ {
                s.push_str(&format!(" ref {r}"));
            }
            if let Some(p) = path {
                s.push_str(&format!(" path {p}"));
            }
            s
        }
        aurora_template::SkillRef::Organization { name, .. } => {
            format!("organization skill {name}")
        }
    }
}

/// The offer's text of a skill part: the use-it-when line, the body and the file list with
/// sizes and sha256 (C-1112).
fn skill_text(description: &str, body: &str, files: &[(String, usize, bool, String)]) -> String {
    let mut out = format!("use it when: {description}\n\n{body}\n\nfiles:");
    if files.is_empty() {
        out.push_str(" none");
        return out;
    }
    for (path, size, executable, sha) in files {
        out.push_str(&format!(
            "\n- {path} ({size} bytes{}, sha256 {sha})",
            if *executable { ", executable" } else { "" }
        ));
    }
    out
}

/// The offer's text of the helpers part: each file's path, size, executable bit and sha256,
/// plus the text of text files (C-1112). `(path, size, executable, sha256, text)`.
fn helpers_text(files: &[(String, usize, bool, String, Option<String>)]) -> String {
    if files.is_empty() {
        return "no helper files".to_owned();
    }
    let mut out = String::new();
    for (i, (path, size, executable, sha, text)) in files.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "- {path} ({size} bytes{}, sha256 {sha})",
            if *executable { ", executable" } else { "" }
        ));
        if let Some(t) = text {
            out.push('\n');
            out.push_str(t);
        }
    }
    out
}

fn file_text(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|_| bytes.len() <= dto::FILE_TEXT_MAX)
        .map(str::to_owned)
}

fn input_needed_view(i: &aurora_template::InputDecl) -> dto::InputNeededView {
    dto::InputNeededView {
        id: i.id.clone(),
        label: i.label.clone(),
        help: i.help.clone(),
        kind: match i.kind {
            aurora_template::InputKind::Text => "text",
            aurora_template::InputKind::Choice => "choice",
        }
        .to_owned(),
        required: i.required,
        pattern: i.pattern.clone(),
        max_length: i.max_length,
        options: i.options.clone(),
        default: i.default.clone(),
    }
}

// ---- the diff -----------------------------------------------------------------------------------

/// The built offer plus everything the apply needs to write the parts it takes.
struct Diff {
    offer: dto::TemplateUpdateOffer,
    /// `(kind, key)` → state of every listed part; the apply validates its parts against this.
    states: BTreeMap<(String, String), &'static str>,
    rendered: aurora_template::RenderedTemplate,
    /// The stored version's library references with their resolutions: the
    /// `removed_upstream` match base (`key, reference, plan`).
    old_refs: Vec<(String, aurora_template::SkillRef, dto::SkillRefPlan)>,
}

/// Build the update offer of one agent against its template's offered version (C-1112).
/// `render_inputs` are the values the proposed texts are rendered with: the stored inputs for
/// the offer, the merged ones for the apply.
async fn build_diff(
    state: &AppState,
    agent: &agents_repo::AgentRow,
    ctx: &OfferCtx,
    person: &str,
    render_inputs: BTreeMap<String, String>,
) -> Result<Diff, ApiError> {
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    build_diff_on(state, &mut conn, agent, ctx, person, render_inputs).await
}

/// [`build_diff`] on an existing connection (SEC-146 A): the apply's transaction recomputes
/// the diff under the per-agent lock, so the states it refuses on are the transaction's own.
async fn build_diff_on(
    state: &AppState,
    conn: &mut sqlx::PgConnection,
    agent: &agents_repo::AgentRow,
    ctx: &OfferCtx,
    person: &str,
    render_inputs: BTreeMap<String, String>,
) -> Result<Diff, ApiError> {
    let stored = stored_inputs(agent);
    let checked = aurora_template::CheckedInputs::from_values(render_inputs);
    let rendered = aurora_template::render(&ctx.new_content, &checked, &agent.name, person);

    let recorded: BTreeMap<(String, String), String> = trepo::parts_of(&mut *conn, agent.id)
        .await?
        .into_iter()
        .map(|p| ((p.kind, p.key), p.content_sha256))
        .collect();

    // The agent's live skills of every name the offer may look at: the template's own skills
    // (both versions) and the library references' skill names.
    let mut names: Vec<String> = rendered.skills.iter().map(|s| s.name.clone()).collect();
    if let Some(old) = &ctx.old_content {
        for s in &old.skills {
            if !names.iter().any(|n| n == &s.name) {
                names.push(s.name.clone());
            }
        }
    }
    for r in ctx.new_content.meta.skill_refs.iter().chain(
        ctx.old_content
            .as_ref()
            .map(|o| o.meta.skill_refs.iter())
            .unwrap_or_default(),
    ) {
        let name = r.name().to_owned();
        if !names.iter().any(|n| n == &name) {
            names.push(name);
        }
    }
    let mut live: BTreeMap<String, Option<skills_repo::SkillRow>> = BTreeMap::new();
    for name in &names {
        live.insert(
            name.clone(),
            skills_repo::get_agent_named(conn, agent.id, name).await?,
        );
    }
    // The live files of every skill the agent still has (one read; the hash needs them).
    let skill_ids: Vec<Uuid> = live.values().flatten().map(|r| r.id).collect();
    let mut live_files: BTreeMap<Uuid, Vec<skills_repo::FileRow>> = BTreeMap::new();
    if !skill_ids.is_empty() {
        for f in skills_repo::files(conn, &skill_ids).await? {
            live_files.entry(f.skill_id).or_default().push(f);
        }
    }
    let live_helpers = helpers_repo::list(&mut *conn, agent.id).await?;

    let mut parts_out: Vec<OfferPart> = Vec::new();
    let mut states: BTreeMap<(String, String), &'static str> = BTreeMap::new();

    // ---- instructions -------------------------------------------------------------------------
    let proposed_hash = instructions_hash(&rendered.instructions);
    let current_hash = instructions_hash(&agent.brief);
    let rec = recorded.get(&("instructions".to_owned(), String::new()));
    if let Some(st) = listed(rec, Some(&proposed_hash), Some(&current_hash)) {
        states.insert(("instructions".to_owned(), String::new()), st);
        parts_out.push(OfferPart {
            kind: "instructions".to_owned(),
            key: String::new(),
            state: st.to_owned(),
            current: Some(agent.brief.clone()),
            proposed: Some(rendered.instructions.clone()),
            skill_ref: None,
        });
    }

    // ---- skills (both versions' names, then the stored version's leftovers) -------------------
    let live_hash = |row: &skills_repo::SkillRow| -> String {
        let raw: Vec<(String, Vec<u8>, bool)> = live_files
            .get(&row.id)
            .map(|fs| {
                fs.iter()
                    .map(|f| (f.path.clone(), f.content.clone(), f.executable))
                    .collect()
            })
            .unwrap_or_default();
        skill_hash(&row.description, &row.body, &raw)
    };
    let live_text = |row: &skills_repo::SkillRow| -> String {
        let files: Vec<(String, usize, bool, String)> = live_files
            .get(&row.id)
            .map(|fs| {
                fs.iter()
                    .map(|f| {
                        (
                            f.path.clone(),
                            f.content.len(),
                            f.executable,
                            dto::sha256_hex(&f.content),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        skill_text(&row.description, &row.body, &files)
    };
    let proposed_text = |s: &aurora_template::RenderedSkill| -> String {
        let files: Vec<(String, usize, bool, String)> = s
            .files
            .iter()
            .map(|f| {
                (
                    f.path.clone(),
                    f.bytes.len(),
                    f.executable,
                    dto::sha256_hex(&f.bytes),
                )
            })
            .collect();
        skill_text(&s.use_it_when, &s.body, &files)
    };

    for s in &rendered.skills {
        let rec = recorded.get(&("skill".to_owned(), s.name.clone()));
        let files: Vec<(String, Vec<u8>, bool)> = s
            .files
            .iter()
            .map(|f| (f.path.clone(), f.bytes.clone(), f.executable))
            .collect();
        let ph = skill_hash(&s.use_it_when, &s.body, &files);
        // A library-sourced skill of the same name is not the template's part (C-1112).
        let own = live
            .get(&s.name)
            .cloned()
            .flatten()
            .filter(|row| row.library_id.is_none());
        let (ch, cur) = match &own {
            Some(row) => (Some(live_hash(row)), Some(live_text(row))),
            None => (None, None),
        };
        if let Some(st) = listed(rec, Some(&ph), ch.as_deref()) {
            states.insert(("skill".to_owned(), s.name.clone()), st);
            parts_out.push(OfferPart {
                kind: "skill".to_owned(),
                key: s.name.clone(),
                state: st.to_owned(),
                current: cur,
                proposed: Some(proposed_text(s)),
                skill_ref: None,
            });
        }
    }
    if let Some(old) = &ctx.old_content {
        for s in &old.skills {
            if rendered.skills.iter().any(|x| x.name == s.name) {
                continue;
            }
            let rec = recorded.get(&("skill".to_owned(), s.name.clone()));
            let own = live
                .get(&s.name)
                .cloned()
                .flatten()
                .filter(|row| row.library_id.is_none());
            let (ch, cur) = match &own {
                Some(row) => (Some(live_hash(row)), Some(live_text(row))),
                None => (None, None),
            };
            if let Some(st) = listed(rec, None, ch.as_deref()) {
                states.insert(("skill".to_owned(), s.name.clone()), st);
                parts_out.push(OfferPart {
                    kind: "skill".to_owned(),
                    key: s.name.clone(),
                    state: st.to_owned(),
                    current: cur,
                    proposed: None,
                    skill_ref: None,
                });
            }
        }
    }

    // ---- helpers ------------------------------------------------------------------------------
    let proposed_helpers: Vec<(String, Vec<u8>, bool)> = rendered
        .helpers
        .iter()
        .map(|f| (f.path.clone(), f.bytes.clone(), f.executable))
        .collect();
    let ph = helpers_hash(&proposed_helpers);
    let proposed_helpers_text = helpers_text(
        &rendered
            .helpers
            .iter()
            .map(|f| {
                (
                    f.path.clone(),
                    f.bytes.len(),
                    f.executable,
                    dto::sha256_hex(&f.bytes),
                    file_text(&f.bytes),
                )
            })
            .collect::<Vec<_>>(),
    );
    let live_helper_raw: Vec<(String, Vec<u8>, bool)> = live_helpers
        .iter()
        .map(|h| (h.path.clone(), h.content.clone(), h.executable))
        .collect();
    let ch = helpers_hash(&live_helper_raw);
    let current_helpers_text = helpers_text(
        &live_helpers
            .iter()
            .map(|h| {
                (
                    h.path.clone(),
                    h.content.len(),
                    h.executable,
                    h.sha256.clone(),
                    file_text(&h.content),
                )
            })
            .collect::<Vec<_>>(),
    );
    let rec = recorded.get(&("helpers".to_owned(), String::new()));
    let proposed_opt = if rendered.helpers.is_empty() {
        None
    } else {
        Some(ph.as_str())
    };
    let current_opt = if live_helpers.is_empty() {
        None
    } else {
        Some(ch.as_str())
    };
    if let Some(st) = listed(rec, proposed_opt, current_opt) {
        states.insert(("helpers".to_owned(), String::new()), st);
        parts_out.push(OfferPart {
            kind: "helpers".to_owned(),
            key: String::new(),
            state: st.to_owned(),
            current: (!live_helpers.is_empty()).then_some(current_helpers_text),
            proposed: (!rendered.helpers.is_empty()).then_some(proposed_helpers_text),
            skill_ref: None,
        });
    }

    // ---- library skills -------------------------------------------------------------------------
    // The part hash of a `library_skill` is the constant key hash (C-1112), so the comparison
    // is by the agent's skill presence: the part is offered again only when the agent no longer
    // has a library skill of the reference's name.
    let mut old_refs: Vec<(String, aurora_template::SkillRef, dto::SkillRefPlan)> = Vec::new();
    let new_refs: Vec<(String, usize, aurora_template::SkillRef)> = ctx
        .new_content
        .meta
        .skill_refs
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, aurora_template::SkillRef::Library { .. }))
        .map(|(index, r)| (skill_ref_key(r), index, r.clone()))
        .collect();
    if !new_refs.is_empty() {
        let mut resolver = Resolver::new(state, agent.org_id, agent.workspace_id).await?;
        let jsons: Vec<RefJson> = new_refs.iter().map(|(_, _, r)| ref_json(r)).collect();
        let plans = skill_ref_plans(state, &mut resolver, &jsons).await?;
        for ((key, index, _), mut plan) in new_refs.iter().cloned().zip(plans) {
            plan.index = index;
            let rec = recorded.get(&("library_skill".to_owned(), key.clone()));
            let constant = dto::sha256_hex(key.as_bytes());
            let ch = live
                .get(plan.name.as_str())
                .cloned()
                .flatten()
                .filter(|row| row.library_id.is_some())
                .map(|_| constant.as_str());
            if let Some(st) = listed(rec, Some(&constant), ch) {
                states.insert(("library_skill".to_owned(), key.clone()), st);
                parts_out.push(OfferPart {
                    kind: "library_skill".to_owned(),
                    key,
                    state: st.to_owned(),
                    current: None,
                    proposed: Some(ref_text(plan_ref(&ctx.new_content, index))),
                    skill_ref: Some(plan.clone()),
                });
            }
        }
    }
    // The stored version's references the offered one dropped: `removed_upstream`, with the
    // reference resolved from the *old* stored version (C-1112).
    if let Some(old) = &ctx.old_content {
        let old_lib: Vec<(String, usize, aurora_template::SkillRef)> = old
            .meta
            .skill_refs
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, aurora_template::SkillRef::Library { .. }))
            .map(|(index, r)| (skill_ref_key(r), index, r.clone()))
            .filter(|(k, _, _)| !new_refs.iter().any(|(nk, _, _)| nk == k))
            .collect();
        if !old_lib.is_empty() {
            let mut resolver = Resolver::new(state, agent.org_id, agent.workspace_id).await?;
            let jsons: Vec<RefJson> = old_lib.iter().map(|(_, _, r)| ref_json(r)).collect();
            let plans = skill_ref_plans(state, &mut resolver, &jsons).await?;
            for ((key, index, r), mut plan) in old_lib.into_iter().zip(plans) {
                plan.index = index;
                // Kept for the apply's `removed_upstream` match even when never listed (the
                // recorded hash decides: never recorded → nothing to remove).
                old_refs.push((key.clone(), r.clone(), plan.clone()));
                let rec = recorded.get(&("library_skill".to_owned(), key.clone()));
                if let Some(st) = listed(rec, None, None) {
                    states.insert(("library_skill".to_owned(), key.clone()), st);
                    parts_out.push(OfferPart {
                        kind: "library_skill".to_owned(),
                        key,
                        state: st.to_owned(),
                        current: Some(ref_text(&r)),
                        proposed: None,
                        skill_ref: Some(plan.clone()),
                    });
                }
            }
        }
    }

    // ---- the apps the agent has no access for yet (C-1112's `apps_added`) ---------------------
    let conn_ids: Vec<Uuid> = grants_repo::list_connection_grants(&state.pool, agent.id)
        .await?
        .into_iter()
        .map(|g| g.connection_id)
        .collect();
    let agent_apps: BTreeSet<String> = grants_repo::connection_apps(&state.pool, &conn_ids)
        .await?
        .into_iter()
        .map(|(_, a)| a)
        .collect();
    let snap = crate::domains::apps::service::snapshot(state).await?;
    let mut apps_added = Vec::new();
    for a in &ctx.new_content.meta.apps {
        let (app_ids, required, why, level) = match a {
            aurora_template::AppEntry::App {
                app,
                required,
                level,
                why,
                ..
            } => (
                vec![app.clone()],
                *required,
                why.clone(),
                Some(level_str(*level)),
            ),
            aurora_template::AppEntry::AnyOf {
                any_of,
                required,
                level,
                why,
            } => (
                any_of.clone(),
                *required,
                why.clone(),
                Some(level_str(*level)),
            ),
        };
        if app_ids.iter().any(|id| agent_apps.contains(id)) {
            continue;
        }
        // The plan's cap: `act` becomes `ask` on a `custom_only` app (any installed one of the
        // entry, C-1110's rule).
        let level = level.map(|l| {
            let capped = app_ids.iter().any(|id| {
                snap.as_ref()
                    .and_then(|s| s.app(id))
                    .is_some_and(|e| e.grants.custom_only)
            });
            if l == "act" && capped {
                "ask".to_owned()
            } else {
                l.to_owned()
            }
        });
        apps_added.push(dto::UpdateAppEntry {
            app_ids,
            required,
            why,
            level,
        });
    }

    // The inputs the stored values do not cover (C-1112): the ones with problems.
    let problems = aurora_template::check_inputs(&ctx.new_content.meta.inputs, &stored)
        .err()
        .unwrap_or_default();
    let inputs_needed = ctx
        .new_content
        .meta
        .inputs
        .iter()
        .filter(|i| problems.iter().any(|p| p.input == i.id))
        .map(input_needed_view)
        .collect();

    let offer = dto::TemplateUpdateOffer {
        version: ctx.offered.version.clone(),
        content_sha256: ctx.offered.content_sha256.clone().unwrap_or_default(),
        readme: ctx.new_content.readme.clone(),
        // Format 1 keeps `CHANGELOG.md` out of the content (C-1112).
        changelog: None,
        inputs_needed,
        apps_added,
        parts: parts_out,
    };
    Ok(Diff {
        offer,
        states,
        rendered,
        old_refs,
    })
}

/// The reference of the new content at a `skill_refs:` index.
fn plan_ref(
    content: &aurora_template::TemplateContent,
    index: usize,
) -> &aurora_template::SkillRef {
    &content.meta.skill_refs[index]
}

// ---- the routes' services -----------------------------------------------------------------------

/// `GET /v1/agents/{agent}/template` (C-1112): the provenance, whether the template is still
/// available, and the update offer when a newer version is offered.
pub async fn status(
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
) -> Result<AgentTemplateStatus, ApiError> {
    let agent = agent_row(state, caller, agent_id).await?;
    let Some(template_id) = agent.template_id.clone() else {
        return Ok(AgentTemplateStatus {
            template: None,
            available: false,
            update: None,
            notes: Vec::new(),
        });
    };
    let template = AgentTemplateRefView {
        template_id: template_id.clone(),
        name: agent.template_name.clone().unwrap_or_default(),
        version: agent.template_version.clone().unwrap_or_default(),
    };
    let Some(ctx) = offer_ctx(state, &agent).await? else {
        return Ok(AgentTemplateStatus {
            template: Some(template),
            available: false,
            update: None,
            notes: Vec::new(),
        });
    };
    let update = if newer_than_stored(&ctx.offered.version, agent.template_version.as_deref()) {
        let person = creator_person(state, &agent).await?;
        let diff = build_diff(state, &agent, &ctx, &person, stored_inputs(&agent)).await?;
        Some(diff.offer)
    } else {
        None
    };
    Ok(AgentTemplateStatus {
        template: Some(template),
        available: true,
        update,
        notes: Vec::new(),
    })
}

// ---- the apply ---------------------------------------------------------------------------------

fn not_found() -> ApiError {
    ApiError::not_found("This agent was not made from a template.")
}

/// The plain 409 of an apply against anything but the current offered version (C-1112): the
/// offer is the single source of truth, so there is nothing to answer with beyond the message.
fn version_changed() -> ApiError {
    ApiError::conflict(
        "template_version_changed",
        "This template was updated. Check the changes, then apply.",
    )
}

/// The 422 for a part that is not in the current offer (C-1112).
fn part_unknown(kind: &str, key: &str) -> ApiError {
    ApiError::unprocessable(
        "template_part_unknown",
        "This part is not in the current offer.",
    )
    .with_details(json!({
        "fields": [{
            "path": "/parts",
            "message": if key.is_empty() { kind.to_owned() } else { format!("{kind} {key}") },
        }]
    }))
}

/// A 422 whose `details.fields` carries `/skill_refs` and the reference index (C-1112's taken
/// library skills, checked as C-1111's).
fn ref_unprocessable(code: &str, msg: &str, index: usize) -> ApiError {
    ApiError::unprocessable(code, msg).with_details(json!({
        "fields": [{"path": "/skill_refs", "message": format!("entry {index}")}]
    }))
}

/// The apply's input values (C-1112): the request supplies `inputs_needed`; a stored value is
/// kept only when it still passes the input's (possibly changed) rules on its own.
fn merged_inputs(
    ctx: &OfferCtx,
    stored: &BTreeMap<String, String>,
    given: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for i in &ctx.new_content.meta.inputs {
        if let Some(v) = given.get(&i.id) {
            out.insert(i.id.clone(), v.clone());
            continue;
        }
        if let Some(s) = stored.get(&i.id) {
            let single = BTreeMap::from([(i.id.clone(), s.clone())]);
            if aurora_template::check_inputs(std::slice::from_ref(i), &single).is_ok() {
                out.insert(i.id.clone(), s.clone());
            }
        }
    }
    out
}

/// The stored provenance of a version: the merged values over the version's declared inputs
/// (C-1112).
fn provenance_inputs(ctx: &OfferCtx, merged: &BTreeMap<String, String>) -> serde_json::Value {
    json!(
        ctx.new_content
            .meta
            .inputs
            .iter()
            .map(|i| {
                (
                    i.id.clone(),
                    aurora_template::CheckedInputs::from_values(merged.clone())
                        .get(&i.id)
                        .unwrap_or_default()
                        .to_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    )
}

/// `POST /v1/agents/{agent}/template/apply` (C-1112): take the listed parts of the offered
/// version. Only the listed parts change; `template_version` moves even when none was taken
/// ("skip this version"). The library fetches happen before the transaction (as C-1111);
/// everything else is one transaction through the existing writers.
pub async fn apply(
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
    req: ApplyTemplateRequest,
) -> Result<AgentTemplateStatus, ApiError> {
    let agent = agent_row(state, caller, agent_id).await?;
    if agent.template_id.is_none() {
        return Err(not_found());
    }
    // The offered version decides what an apply may take (C-1112): without one, and with a
    // stale version or hash, it is the same plain 409.
    let Some(ctx) = offer_ctx(state, &agent).await? else {
        return Err(version_changed());
    };
    if req.version != ctx.offered.version
        || ctx.offered.content_sha256.as_deref() != Some(req.content_sha256.as_str())
    {
        return Err(version_changed());
    }
    let person = creator_person(state, &agent).await?;
    let stored = stored_inputs(&agent);
    let merged = merged_inputs(&ctx, &stored, &req.inputs);
    // Full-input validation only when parts are taken: "skip this version" never fails on
    // inputs (C-1112).
    if !req.parts.is_empty() {
        aurora_template::check_inputs(&ctx.new_content.meta.inputs, &merged).map_err(
            |problems| {
                let fields: Vec<_> = problems
                    .iter()
                    .map(|p| json!({"path": format!("/inputs/{}", p.input), "message": p.message}))
                    .collect();
                ApiError::unprocessable(
                    "template_inputs_invalid",
                    "The template inputs are invalid.",
                )
                .with_details(json!({ "fields": fields }))
            },
        )?;
    }
    // The diff, rendered with the merged inputs: the proposed texts are exactly what an apply
    // writes, with the recorded-hash rendering (the creator's name, the agent's name).
    let diff = build_diff(state, &agent, &ctx, &person, merged.clone()).await?;

    // The taken parts, validated against the offer's states (C-1112): a part that is not in
    // the offer is 422, whatever its kind or key. A repeated part is one part.
    let mut taken: Vec<((String, String), &'static str)> = Vec::new();
    for p in &req.parts {
        let k = (p.kind.clone(), p.key.clone());
        if taken.iter().any(|(t, _)| *t == k) {
            continue;
        }
        let Some(st) = diff.states.get(&k) else {
            return Err(part_unknown(&p.kind, &p.key));
        };
        taken.push((k, st));
    }
    // The rendered instructions follow the brief rules (as C-1111's create).
    if taken.iter().any(|((kind, _), _)| kind == "instructions")
        && diff.rendered.instructions.chars().count() > agents::service::BRIEF_MAX
    {
        return Err(ApiError::unprocessable(
            "template_too_large",
            "The rendered instructions are too large for an agent brief.",
        ));
    }

    // The library fetches of the taken library skills happen before the transaction (C-1112,
    // as C-1111): the resolution must be the offer's, the commit the person's to pin.
    let mut adds: Vec<(String, library::PreparedAdd)> = Vec::new();
    for ((kind, key), st) in &taken {
        if kind != "library_skill" || !matches!(*st, "new" | "removed_here") {
            continue;
        }
        let Some(part) = diff
            .offer
            .parts
            .iter()
            .find(|o| o.kind == "library_skill" && &o.key == key)
        else {
            continue; // cannot happen: every listed state has its offer part
        };
        let Some(plan) = &part.skill_ref else {
            continue;
        };
        let Some(lib) = &plan.library else {
            return Err(ref_unprocessable(
                "template_skill_unavailable",
                "This skill reference is not available in this workspace.",
                plan.index,
            ));
        };
        let Some(item) = req.skill_refs.iter().find(|s| s.index == plan.index) else {
            return Err(ref_unprocessable(
                "template_skill_missing",
                "This taken skill reference has no library skill to add.",
                plan.index,
            ));
        };
        if lib.id != item.library_id {
            return Err(ref_unprocessable(
                "template_skill_unavailable",
                "This library is not the one the offer resolved for this reference.",
                plan.index,
            ));
        }
        // The pin must be the offer's resolved commit, and the reference available (SEC-146 C):
        // the fetch commits to what the offer showed, never a body-chosen commit.
        if !plan.available {
            return Err(ref_unprocessable(
                "template_skill_unavailable",
                "This skill reference is not available in this workspace.",
                plan.index,
            ));
        }
        if lib.commit_sha != item.commit_sha {
            return Err(ref_unprocessable(
                "template_commit_changed",
                "This library skill's commit is not the one the offer resolved for this reference.",
                plan.index,
            ));
        }
        let dir = create::skill_dir(state, item.library_id, &plan.name, plan.index).await?;
        let from = FromLibrary {
            library_id: item.library_id,
            path: dir,
            commit_sha: Some(item.commit_sha.clone()),
        };
        let prepared = library::prepare_add(state, caller, agent.org_id, &from).await?;
        adds.push((key.clone(), prepared));
    }

    // ---- the one transaction (serialized per agent, SEC-146 A) ---------------------------------
    let mut tx = audit::begin(state).await?;
    // The per-agent writer lock: concurrent applies and detaches of this agent serialize here,
    // so the re-reads and the writes below see one consistent state.
    memory_repo::lock_agent(&mut tx, agent.id).await?;
    // Re-verify under the lock (SEC-146 A): the agent, the offer and the part states may all
    // have moved between the pre-check above and this transaction.
    let agent = agents_repo::get(&mut *tx, agent_id)
        .await?
        .ok_or_else(not_found)?;
    let Some(ctx) = offer_ctx_on(state, &mut tx, &agent).await? else {
        return Err(version_changed());
    };
    if req.version != ctx.offered.version
        || ctx.offered.content_sha256.as_deref() != Some(req.content_sha256.as_str())
    {
        return Err(version_changed());
    }
    let stored = stored_inputs(&agent);
    let merged = merged_inputs(&ctx, &stored, &req.inputs);
    let inputs_json = provenance_inputs(&ctx, &merged);
    let diff = build_diff_on(state, &mut tx, &agent, &ctx, &person, merged).await?;
    // The listed parts' states must still be the ones the person answered to (SEC-146 A): a
    // part edited (or applied, or removed) while the offer was open is refused with the same
    // 409, never silently overwritten.
    for (k, st) in &taken {
        if diff.states.get(k) != Some(st) {
            return Err(version_changed());
        }
    }
    // The rendered instructions follow the brief rules (as C-1111's create) — re-checked on
    // the fresh render, which the states alone do not pin down.
    if taken.iter().any(|((kind, _), _)| kind == "instructions")
        && diff.rendered.instructions.chars().count() > agents::service::BRIEF_MAX
    {
        return Err(ApiError::unprocessable(
            "template_too_large",
            "The rendered instructions are too large for an agent brief.",
        ));
    }
    let result = apply_in_tx(
        &mut tx,
        state,
        caller,
        &agent,
        &diff,
        &taken,
        &adds,
        &inputs_json,
    )
    .await;
    let (notes, events) = match result {
        Ok(ok) => ok,
        // Roll back (tx drops): nothing is half-applied.
        Err(e) => return Err(e),
    };
    // The critical events (a taken library skill's `skill.library.add`) commit with the
    // transaction; without any, a plain commit (as C-1111).
    if events.is_empty() {
        tx.commit().await.map_err(aurora_db::DbError::from)?;
    } else {
        audit::commit_critical_many(tx, &events).await?;
    }
    // The fresh answer (C-1112), with what the apply left alone.
    let mut answer = status(state, caller, agent_id).await?;
    answer.notes = notes;
    Ok(answer)
}

/// The one transaction of [`apply`] (C-1112): every write goes through the existing writer of
/// its kind. Commits nothing: the notes and the critical events (a taken library skill's
/// `skill.library.add`) come back; the caller commits — or lets the transaction drop (rollback)
/// on any error.
#[allow(clippy::too_many_arguments)]
async fn apply_in_tx(
    tx: &mut audit::Tx,
    state: &AppState,
    caller: &Caller,
    agent: &agents_repo::AgentRow,
    diff: &Diff,
    taken: &[((String, String), &'static str)],
    adds: &[(String, library::PreparedAdd)],
    inputs_json: &Value,
) -> Result<(Vec<dto::ApplyNote>, Vec<AuditEvent>), ApiError> {
    let mut notes: Vec<dto::ApplyNote> = Vec::new();
    let mut events: Vec<AuditEvent> = Vec::new();
    let mut recorded: Vec<trepo::PartRow> = Vec::new();

    for ((kind, key), st) in taken {
        let st: &str = st;
        match (kind.as_str(), st) {
            // ---- instructions: the rendered brief replaces the agent's -----------------------
            ("instructions", _) => {
                agents_repo::update(
                    &mut **tx,
                    agent.id,
                    &agents_repo::AgentPatch {
                        brief: Some(&diff.rendered.instructions),
                        ..Default::default()
                    },
                )
                .await?;
                recorded.push(trepo::PartRow {
                    agent_id: agent.id,
                    kind: "instructions".into(),
                    key: String::new(),
                    content_sha256: instructions_hash(&diff.rendered.instructions),
                });
            }
            // ---- skills ----------------------------------------------------------------------
            // `removed_upstream`: the template no longer has the skill. The agent's own skill
            // of that name goes; a library skill of the same name is not the template's part
            // and stays (C-1112).
            ("skill", "removed_upstream") => {
                let row = skills_repo::get_agent_named(tx, agent.id, key).await?;
                match row {
                    Some(r) if r.library_id.is_none() => {
                        skills_repo::delete(tx, r.id).await?;
                        skills_repo::bump_agent(tx, agent.id).await?;
                    }
                    Some(_) => notes.push(dto::ApplyNote {
                        code: "skill_kept".into(),
                        detail: format!(
                            "The library skill {key} is not the template's part; it stays."
                        ),
                    }),
                    None => {}
                }
            }
            // Any other state: the version's skill replaces the agent's own, or creates it
            // (`new`, `removed_here`) — an ordinary editable skill (C-1112).
            ("skill", _) => {
                let Some(s) = diff.rendered.skills.iter().find(|s| &s.name == key) else {
                    continue; // cannot happen: every other state has proposed content
                };
                let existing = skills_repo::get_agent_named(tx, agent.id, &s.name).await?;
                let row = match existing {
                    // A library-sourced skill of the same name is never written here (C-1112);
                    // re-checked in the transaction so a race cannot write a read-only skill.
                    Some(row) if row.library_id.is_some() => {
                        return Err(ApiError::conflict(
                            "library_skill_readonly",
                            "This skill comes from a skill library and cannot be changed by a template apply.",
                        ));
                    }
                    Some(row) => {
                        skills_repo::update(tx, row.id, &s.use_it_when, &s.body, true).await?;
                        // The files are replaced as a set.
                        for f in skills_repo::files(tx, &[row.id]).await? {
                            skills_repo::delete_file(tx, row.id, &f.path).await?;
                        }
                        row
                    }
                    None => {
                        skills_domain::service::create_in_tx(
                            tx,
                            agent.org_id,
                            Owner {
                                agent: Some(agent.id),
                                workspace: None,
                            },
                            &NewSkill {
                                name: &s.name,
                                description: &s.use_it_when,
                                body: &s.body,
                                enabled: true,
                            },
                            "user",
                        )
                        .await?
                        .row
                    }
                };
                for f in &s.files {
                    skills_repo::upsert_file(tx, row.id, &f.path, &f.bytes, Some(f.executable))
                        .await?;
                }
                skills_repo::bump_agent(tx, agent.id).await?;
                recorded.push(trepo::PartRow {
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
            }
            // ---- helpers ---------------------------------------------------------------------
            // `removed_upstream`: the template no longer has helper files; the agent's go.
            ("helpers", "removed_upstream") => {
                let removed = helpers_repo::delete_all(tx, agent.id).await?;
                if removed > 0 {
                    skills_repo::bump_agent(tx, agent.id).await?;
                }
            }
            // Any other state: the version's helper files replace the agent's set (C-1143's
            // one writer, which bumps the home sync itself).
            ("helpers", _) => {
                let files: Vec<agent_helpers::service::HelperFile<'_>> = diff
                    .rendered
                    .helpers
                    .iter()
                    .map(|f| agent_helpers::service::HelperFile {
                        path: &f.path,
                        content: &f.bytes,
                        executable: f.executable,
                    })
                    .collect();
                agent_helpers::service::replace_all(tx, agent.id, &files).await?;
                recorded.push(create::helpers_part(agent.id, &diff.rendered.helpers));
            }
            // ---- library skills --------------------------------------------------------------
            // `removed_upstream`: the agent's library skill of the reference's name is deleted
            // only while it is still sourced from the library the stored version's reference
            // resolves to (C-1142); otherwise it stays (`notes`: `skill_kept`, C-1112).
            ("library_skill", "removed_upstream") => {
                let Some((_, _, plan)) = diff.old_refs.iter().find(|(k, _, _)| k == key) else {
                    continue; // cannot happen: the state comes from the stored version's refs
                };
                let row = skills_repo::get_agent_named(tx, agent.id, &plan.name).await?;
                match (row, plan.library.as_ref()) {
                    (Some(r), Some(l)) if r.library_id == Some(l.id) => {
                        skills_repo::delete(tx, r.id).await?;
                        skills_repo::bump_agent(tx, agent.id).await?;
                    }
                    (Some(_), _) => notes.push(dto::ApplyNote {
                        code: "skill_kept".into(),
                        detail: format!(
                            "The library skill {} is no longer sourced from the library the template referenced; it stays.",
                            plan.name
                        ),
                    }),
                    (None, _) => {}
                }
            }
            // `new` or `removed_here`: added through the library add path at the agent tier
            // with the content fetched before the transaction — pinned, read-only (C-1142).
            ("library_skill", _) => {
                let Some((_, prepared)) = adds.iter().find(|(k, _)| k == key) else {
                    return Err(ApiError::internal("library skill add not prepared"));
                };
                let (_, ev) = library::write_add_in_tx(
                    tx,
                    state,
                    caller,
                    agent.org_id,
                    Owner {
                        agent: Some(agent.id),
                        workspace: None,
                    },
                    None,
                    prepared,
                )
                .await?;
                events.push(ev);
                recorded.push(trepo::PartRow {
                    agent_id: agent.id,
                    kind: "library_skill".into(),
                    key: key.clone(),
                    content_sha256: dto::sha256_hex(key.as_bytes()),
                });
            }
            _ => {}
        }
    }

    // The taken `removed_upstream` parts lose their recorded hash (C-1112): there is nothing
    // of them left to diff against.
    for ((kind, key), st) in taken {
        if *st == "removed_upstream" {
            trepo::delete_part(tx, agent.id, kind, key).await?;
        }
    }

    // The part rows of everything the apply took (C-1112: a row for every part taken), then
    // the provenance moves to the applied version, with the merged inputs — also when no part
    // was taken ("skip this version").
    for part in &recorded {
        trepo::insert_part(tx, part).await?;
    }
    agents_repo::set_template(
        &mut **tx,
        agent.id,
        agent.template_id.as_deref().unwrap_or_default(),
        agent.template_name.as_deref().unwrap_or_default(),
        &diff.offer.version,
        inputs_json,
    )
    .await?;

    // Audit `agent.template_apply` (ordinary; inside the transaction so a rollback leaves no
    // row, C-1113).
    let parts_meta: Vec<Value> = taken
        .iter()
        .map(|((kind, key), st)| json!({"kind": kind, "key": key, "state": *st}))
        .collect();
    let ev = AuditEvent::by(caller, "agent.template_apply")
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("agent", agent.id)
        .metadata(json!({
            "from": agent.template_version,
            "to": diff.offer.version,
            "parts": parts_meta,
        }));
    audit::insert_tx(tx, &ev).await.map_err(|e| {
        tracing::error!(error = %e, "agent.template_apply audit row not written");
        audit::unavailable()
    })?;
    Ok((notes, events))
}

// ---- the detach ---------------------------------------------------------------------------------

/// `DELETE /v1/agents/{agent}/template` (C-1112): forget the provenance — the four columns go
/// null, the recorded parts go with them, and no further offers are made. Helper files and
/// library skills stay.
pub async fn detach(
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
) -> Result<axum::http::StatusCode, ApiError> {
    let agent = agent_row(state, caller, agent_id).await?;
    if agent.template_id.is_none() {
        return Err(not_found());
    }
    let mut tx = audit::begin(state).await?;
    // The same per-agent writer lock as the apply (SEC-146 A): the re-read below and the
    // writes serialize against a concurrent apply or detach.
    memory_repo::lock_agent(&mut tx, agent.id).await?;
    // Re-read under the lock (SEC-146 A): a concurrent apply or detach may have moved the
    // provenance; a second detach finds it gone and answers 404.
    let agent = agents_repo::get(&mut *tx, agent_id)
        .await?
        .ok_or_else(not_found)?;
    if agent.template_id.is_none() {
        return Err(not_found());
    }
    agents_repo::clear_template(&mut *tx, agent.id).await?;
    trepo::delete_parts(&mut tx, agent.id).await?;
    // Audit `agent.template_detach` (ordinary; inside the transaction, C-1113).
    let ev = AuditEvent::by(caller, "agent.template_detach")
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("agent", agent.id)
        .metadata(json!({
            "template": agent.template_id,
            "version": agent.template_version,
        }));
    audit::insert_tx(&mut tx, &ev).await.map_err(|e| {
        tracing::error!(error = %e, "agent.template_detach audit row not written");
        audit::unavailable()
    })?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---- the routes ---------------------------------------------------------------------------------

use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_template, apply_template, detach_template))
}

/// The agent's template provenance and its update offer (C-1112).
#[utoipa::path(get, path = "/agents/{agent}/template", operation_id = "getAgentTemplate", tag = "agent-templates",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = AgentTemplateStatus), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_template(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<AgentTemplateStatus>, ApiError> {
    Ok(Json(status(&s, &c, agent).await?))
}

/// Apply the listed parts of the offered version (C-1112).
#[utoipa::path(post, path = "/agents/{agent}/template/apply", operation_id = "applyAgentTemplate", tag = "agent-templates",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = ApplyTemplateRequest,
    responses((status = 200, body = AgentTemplateStatus), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn apply_template(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(req): Json<ApplyTemplateRequest>,
) -> Result<Json<AgentTemplateStatus>, ApiError> {
    Ok(Json(apply(&s, &c, agent, req).await?))
}

/// Detach the agent from its template (C-1112): the provenance is forgotten; helper files and
/// library skills stay.
#[utoipa::path(delete, path = "/agents/{agent}/template", operation_id = "detachAgentTemplate", tag = "agent-templates",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn detach_template(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    detach(&s, &c, agent).await
}
