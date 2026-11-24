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

use serde_json::Value;
use uuid::Uuid;

use aurora_template::{helpers_hash, instructions_hash, skill_hash, skill_ref_key};

use crate::app::AppState;
use crate::domains::agent_templates::admin;
use crate::domains::agent_templates::dto::{
    self, AgentTemplateRefView, AgentTemplateStatus, OfferPart,
};
use crate::domains::agent_templates::workspace::{RefJson, Resolver, skill_ref_plans};
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::repo::agent_templates as trepo;
use aurora_db::repo::{
    agent_helpers as helpers_repo, agents as agents_repo, grants as grants_repo,
    skills as skills_repo,
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
    if !state.ops().agents.templates.enabled {
        return Ok(None);
    }
    let Some(template_id) = &agent.template_id else {
        return Ok(None);
    };
    let Some(t) = trepo::template_by_template_id(&state.pool, template_id).await? else {
        return Ok(None);
    };
    if !t.offered {
        return Ok(None);
    }
    if trepo::org_disabled(&state.pool, agent.org_id)
        .await?
        .iter()
        .any(|x| x == template_id)
    {
        return Ok(None);
    }
    let Some(v) = admin::offered_version(&state.pool, t.id).await? else {
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
        Some(ver) => trepo::get_version(&state.pool, t.id, ver)
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
    /// The new version's skill-ref resolutions, by the `skill_refs:` index (library ones).
    new_plans: BTreeMap<usize, dto::SkillRefPlan>,
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
    let stored = stored_inputs(agent);
    let checked = aurora_template::CheckedInputs::from_values(render_inputs);
    let rendered = aurora_template::render(&ctx.new_content, &checked, &agent.name, person);

    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
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
            skills_repo::get_agent_named(&mut conn, agent.id, name).await?,
        );
    }
    // The live files of every skill the agent still has (one read; the hash needs them).
    let skill_ids: Vec<Uuid> = live.values().flatten().map(|r| r.id).collect();
    let mut live_files: BTreeMap<Uuid, Vec<skills_repo::FileRow>> = BTreeMap::new();
    if !skill_ids.is_empty() {
        for f in skills_repo::files(&mut conn, &skill_ids).await? {
            live_files.entry(f.skill_id).or_default().push(f);
        }
    }
    let live_helpers = helpers_repo::list(&mut *conn, agent.id).await?;
    drop(conn);

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
    let mut new_plans: BTreeMap<usize, dto::SkillRefPlan> = BTreeMap::new();
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
            new_plans.insert(index, plan);
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
        new_plans,
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

// ---- the routes ---------------------------------------------------------------------------------

use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_template))
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
