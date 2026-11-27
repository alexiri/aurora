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
use crate::domains::agent_templates::merge::{
    self, Budget, MergeError, RESOLUTION_TEXT_MAX, ResolveError,
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

/// The offered version of each of the given templates, by template id: the semver-newest
/// approved one while the template is offered (C-1108), whatever order the versions were
/// stored in. A withdrawn or unknown template has no entry.
pub(crate) async fn offered_version_strings(
    pool: &sqlx::PgPool,
    template_ids: &[String],
) -> Result<BTreeMap<String, String>, ApiError> {
    let strings = trepo::approved_version_strings(pool, template_ids).await?;
    let mut newest: BTreeMap<String, String> = BTreeMap::new();
    // The rows come newest stored first, so a tie keeps the newest stored one.
    for (template_id, version) in strings {
        let key = aurora_registry::manifest::parse_semver(&version).unwrap_or_default();
        let best = newest.entry(template_id).or_insert_with(|| version.clone());
        if key > aurora_registry::manifest::parse_semver(best).unwrap_or_default() {
            *best = version;
        }
    }
    Ok(newest)
}

/// (FIX-251, D-488(a)) Which of the listed agents are behind their template: the template's
/// offered version (the semver-newest approved one, C-1108) is semver-newer than the agent's
/// stored one — the same rule [`status`] applies to decide `update`, computed for a list from
/// one read of the approved version strings. An agent without a template, or whose template is
/// withdrawn or turned off for the organization, is never behind. Keyed by agent id; absent
/// means not behind.
pub(crate) async fn behind_map(
    pool: &sqlx::PgPool,
    rows: &[agents_repo::AgentRow],
) -> Result<BTreeMap<Uuid, bool>, ApiError> {
    let mut ids: Vec<String> = Vec::new();
    for id in rows.iter().filter_map(|r| r.template_id.as_deref()) {
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_owned());
        }
    }
    let newest = offered_version_strings(pool, &ids).await?;
    Ok(rows
        .iter()
        .map(|r| {
            let behind = r
                .template_id
                .as_deref()
                .and_then(|t| {
                    newest
                        .get(t)
                        .map(|v| newer_than_stored(v, r.template_version.as_deref()))
                })
                .unwrap_or(false);
            (r.id, behind)
        })
        .collect())
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

// ---- the three-way merge of a part (FIX-251, D-488(b)) ------------------------------------------
//
// A part the agent edited since the recorded version is no longer offered as an
// all-or-nothing overwrite: where the agent's starting text is stored (the version content
// the recorded hashes were written from), the part is offered as a three-way merge per text
// unit — the starting text (base), the agent's current text (ours) and the new version's text
// rendered for this agent (theirs). The merge, its hunks and the merged text are computed
// here, on the server, from the stored rows: the apply recomputes them under the per-agent
// lock and accepts nothing but the person's per-hunk choices. Memories, schedules, webhooks
// and grants are never parts, so no merge can ever touch them.

/// Why a part has no merge (the offer says so in words).
const MERGE_NOTE: &str = "The text this agent started from is not stored any more, so your \
     changes cannot be merged: taking the new version replaces this part wholesale.";

/// Why a part's merge was capped (FIX-251): the offer says so in words.
const MERGE_TOO_LARGE_NOTE: &str = "This file is too large to merge automatically: taking the new version replaces \
     this part wholesale.";

/// One text unit of a part: which text it is (`text` for instructions, `description` and
/// `body` for a skill, `file:<path>` for a skill or helper file), and the three texts the
/// merge works on. `None` is a file that does not exist at that layer. A file whose bytes
/// are not text on a side it exists is a **byte unit**: its texts are the digests of the
/// bytes (so the comparisons and the hash stay byte-exact) and the bytes themselves ride
/// along — such a unit is never line-merged, only kept or taken wholesale.
struct UnitTexts {
    unit: String,
    base: Option<String>,
    ours: Option<String>,
    theirs: Option<String>,
    /// The executable bit the merged file gets (a file unit: the new version's bit).
    executable: bool,
    /// The agent's own bit (what Keep mine keeps).
    ours_executable: bool,
    /// The real bytes behind `ours`/`theirs` on a byte unit.
    ours_bytes: Option<Vec<u8>>,
    theirs_bytes: Option<Vec<u8>>,
}

/// The offer's line for a file unit whose bytes are not text.
const BINARY_UNIT_TEXT: &str = "(not a text file)";

/// A whole-unit choice: the unit's file is gone on one side, its texts are over the merge's
/// input cap, or it is a byte unit — there is nothing to line-merge, and the person's choice
/// replaces the unit's content wholesale. `None` is a file that side does not have.
struct WholeChoice {
    /// The offer's texts of the two sides (a byte unit shows a placeholder line).
    ours: Option<String>,
    theirs: Option<String>,
    /// What Keep mine / Take the new one write on a byte unit (a text unit's content is its
    /// texts, so these stay empty).
    ours_bytes: Option<Vec<u8>>,
    theirs_bytes: Option<Vec<u8>>,
}

/// The server's own merge of one unit, computed once for the offer and recomputed
/// identically for the apply.
struct UnitPlan {
    unit: String,
    /// The unit's text when the merge decided on its own and the content is text; `None`
    /// when the unit is not in the merged set, is a byte unit (its `bytes` carry the
    /// content), or still has conflicts to resolve.
    text: Option<String>,
    /// The unit's content when it is not its text (a byte unit's winner).
    bytes: Option<Vec<u8>>,
    /// The unit's hunks to resolve, when the merge is not clean.
    merge: Option<merge::Merge>,
    /// The unit's whole choice, when there is nothing to line-merge.
    whole: Option<WholeChoice>,
    executable: bool,
    ours_executable: bool,
}

/// The server's own merge of one part: what the offer shows and what the apply writes.
struct PartPlan {
    view: dto::PartMerge,
    /// The units in the offer's conflict order: the apply resolves them unit by unit.
    units: Vec<UnitPlan>,
    /// The agent's own files of the part that neither the starting text nor the new version
    /// has: the person added them, and an apply keeps them.
    own_files: Vec<aurora_template::TemplateFile>,
}

/// The merge of one part, or the fallback when there is nothing to merge against.
struct PlannedPart {
    state: &'static str,
    view: Option<dto::PartMerge>,
    note: Option<String>,
    plan: Option<PartPlan>,
}

/// The three texts of one unit into a [`UnitPlan`]: one side untouched takes the other, the
/// same edit on both sides once, a clean three-way merge as it is, and everything else a
/// conflict the person resolves hunk by hunk. A file one side no longer has — or one whose
/// bytes are not text — is one whole-unit choice, never a line merge.
fn plan_unit(u: &UnitTexts, budget: &Budget) -> Result<UnitPlan, MergeError> {
    let byte_unit = u.ours_bytes.is_some() || u.theirs_bytes.is_some();
    // The winning side's content: a text unit's text, a byte unit's bytes.
    let content = |t: &Option<String>, b: &Option<Vec<u8>>| {
        if byte_unit {
            (None, b.clone())
        } else {
            (t.clone(), None)
        }
    };
    let decided = |text: Option<String>, bytes: Option<Vec<u8>>, executable: bool| UnitPlan {
        unit: u.unit.clone(),
        text,
        bytes,
        merge: None,
        whole: None,
        executable,
        ours_executable: u.ours_executable,
    };
    if u.ours == u.theirs {
        // The agent already has the new version's content (or both removed the file).
        let (text, bytes) = content(&u.ours, &u.ours_bytes);
        return Ok(decided(text, bytes, u.ours_executable));
    }
    if u.base == u.ours {
        // The agent never touched this unit: the new version's content.
        let (text, bytes) = content(&u.theirs, &u.theirs_bytes);
        return Ok(decided(text, bytes, u.executable));
    }
    if u.base == u.theirs {
        // The new version did not touch this unit: the person's edit stays.
        let (text, bytes) = content(&u.ours, &u.ours_bytes);
        return Ok(decided(text, bytes, u.ours_executable));
    }
    if byte_unit || u.theirs.is_none() || u.ours.is_none() {
        // Nothing to line-merge: Keep mine or Take the new one replaces the unit wholesale
        // (a byte unit shows a placeholder line; its real bytes ride along).
        let show = |t: &Option<String>, b: &Option<Vec<u8>>| {
            if byte_unit {
                b.as_ref().map(|_| BINARY_UNIT_TEXT.to_owned())
            } else {
                t.clone()
            }
        };
        return Ok(UnitPlan {
            unit: u.unit.clone(),
            text: None,
            bytes: None,
            merge: None,
            whole: Some(WholeChoice {
                ours: show(&u.ours, &u.ours_bytes),
                theirs: show(&u.theirs, &u.theirs_bytes),
                ours_bytes: u.ours_bytes.clone(),
                theirs_bytes: u.theirs_bytes.clone(),
            }),
            executable: u.executable,
            ours_executable: u.ours_executable,
        });
    }
    let b = u.base.clone().unwrap_or_default();
    let o = u.ours.clone().unwrap_or_default();
    let t = u.theirs.clone().unwrap_or_default();
    // A merge over the line, edit or step caps (FIX-251) is refused; the caller falls back.
    let m = merge::merge(&b, &o, &t, budget)?;
    Ok(match m.text() {
        Some(text) => decided(Some(text), None, u.executable),
        None => UnitPlan {
            unit: u.unit.clone(),
            text: None,
            bytes: None,
            merge: Some(m),
            whole: None,
            executable: u.executable,
            ours_executable: u.ours_executable,
        },
    })
}

/// The merge's hash over the three texts of every unit: what the review showed, echoed by the
/// apply and re-checked server-side (a stale review is refused with
/// `template_version_changed`).
fn merge_sha256(units: &[UnitTexts]) -> String {
    let mut buf = Vec::new();
    for u in units {
        buf.extend_from_slice(u.unit.as_bytes());
        for t in [&u.base, &u.ours, &u.theirs] {
            buf.push(0x1f);
            match t {
                Some(s) => {
                    buf.push(b'1');
                    buf.extend_from_slice(s.as_bytes());
                }
                None => buf.push(b'0'),
            }
        }
        buf.push(0x1e);
    }
    dto::sha256_hex(&buf)
}

/// The offer's view of one part's merge, and the plan behind it. CPU work: called through
/// [`plan_off_runtime`] (FIX-251), with the request's shared pool of merge steps.
fn plan_part(
    units: Vec<UnitTexts>,
    own_files: Vec<aurora_template::TemplateFile>,
    compose: fn(&[UnitPlan], &[aurora_template::TemplateFile]) -> String,
    base_present: bool,
    budget: Budget,
) -> PlannedPart {
    if !base_present {
        return PlannedPart {
            state: "edited_here",
            view: None,
            note: Some(MERGE_NOTE.to_owned()),
            plan: None,
        };
    }
    // One unit over the merge's caps (FIX-251) makes the whole part today's all-or-nothing
    // choice, and the remaining units are not merged.
    let plans: Result<Vec<UnitPlan>, MergeError> =
        units.iter().map(|u| plan_unit(u, &budget)).collect();
    let plans = match plans {
        Ok(plans) => plans,
        Err(MergeError::TooLarge) => {
            return PlannedPart {
                state: "edited_here",
                view: None,
                note: Some(MERGE_TOO_LARGE_NOTE.to_owned()),
                plan: None,
            };
        }
    };
    let mut conflicts: Vec<dto::MergeConflict> = Vec::new();
    for u in &plans {
        if let Some(w) = &u.whole {
            conflicts.push(dto::MergeConflict {
                unit: u.unit.clone(),
                ours: w.ours.clone().unwrap_or_default(),
                theirs: w.theirs.clone().unwrap_or_default(),
            });
        } else if let Some(m) = &u.merge {
            conflicts.extend(m.conflicts().into_iter().map(|c| dto::MergeConflict {
                unit: u.unit.clone(),
                ours: c.ours,
                theirs: c.theirs,
            }));
        }
    }
    let state = if conflicts.is_empty() {
        "merged"
    } else {
        "conflict"
    };
    let view = dto::PartMerge {
        state: state.to_owned(),
        result: conflicts.is_empty().then(|| compose(&plans, &own_files)),
        conflicts,
        merge_sha256: merge_sha256(&units),
    };
    PlannedPart {
        state,
        view: Some(view.clone()),
        note: None,
        plan: Some(PartPlan {
            view,
            units: plans,
            own_files,
        }),
    }
}

/// [`plan_part`] off the async runtime (FIX-251): a merge never stalls the worker, and the
/// part's merges spend from the request's shared step budget.
async fn plan_off_runtime(
    units: Vec<UnitTexts>,
    own_files: Vec<aurora_template::TemplateFile>,
    compose: fn(&[UnitPlan], &[aurora_template::TemplateFile]) -> String,
    base_present: bool,
    budget: &Budget,
) -> Result<PlannedPart, ApiError> {
    let budget = budget.clone();
    tokio::task::spawn_blocking(move || plan_part(units, own_files, compose, base_present, budget))
        .await
        .map_err(|e| ApiError::internal(format!("the merge task failed: {e}")))
}

/// The content of a planned unit: its text when it is text, its bytes when it is a file
/// whose bytes are not text.
fn unit_content(u: &UnitPlan) -> Option<Vec<u8>> {
    u.bytes
        .clone()
        .or_else(|| u.text.clone().map(String::into_bytes))
}

/// The offer's display text of a merged instructions part: its one unit.
fn compose_instructions(plans: &[UnitPlan], _: &[aurora_template::TemplateFile]) -> String {
    plans
        .iter()
        .find(|u| u.unit == "text")
        .and_then(unit_content)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// The offer's display text of a merged skill part: the skill text (C-1112) of the merged
/// description, body and files.
fn compose_skill(plans: &[UnitPlan], own: &[aurora_template::TemplateFile]) -> String {
    let text = |unit: &str| {
        plans
            .iter()
            .find(|u| u.unit == unit)
            .and_then(unit_content)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    };
    let mut files: Vec<(String, usize, bool, String)> = own
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
    for u in plans {
        if let Some(path) = u.unit.strip_prefix("file:")
            && let Some(bytes) = unit_content(u)
        {
            files.push((
                path.to_owned(),
                bytes.len(),
                u.executable,
                dto::sha256_hex(&bytes),
            ));
        }
    }
    files.sort();
    skill_text(&text("description"), &text("body"), &files)
}

/// The offer's display text of a merged helpers part: the helper text (C-1112) of the merged
/// files.
fn compose_helpers(plans: &[UnitPlan], own: &[aurora_template::TemplateFile]) -> String {
    let mut files: Vec<(String, usize, bool, String, Option<String>)> = own
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
        .collect();
    for u in plans {
        if let Some(path) = u.unit.strip_prefix("file:")
            && let Some(bytes) = unit_content(u)
        {
            files.push((
                path.to_owned(),
                bytes.len(),
                u.executable,
                dto::sha256_hex(&bytes),
                file_text(&bytes),
            ));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    helpers_text(&files)
}

/// One file unit: a text unit when every side that has the file has it as text, otherwise a
/// byte unit — the texts are the digests of the bytes (byte-exact for the comparisons and
/// the hash) and the bytes ride along for Keep mine / Take the new one.
fn file_unit(
    unit: String,
    base: Option<&[u8]>,
    ours: Option<(&[u8], bool)>,
    theirs: Option<(&[u8], bool)>,
) -> UnitTexts {
    let as_text = |b: Option<&[u8]>| -> Option<String> {
        b.and_then(|b| std::str::from_utf8(b).ok().map(str::to_owned))
    };
    fn bytes_of(o: Option<(&[u8], bool)>) -> Option<&[u8]> {
        o.map(|(b, _)| b)
    }
    let (tb, to, tt) = (
        as_text(base),
        as_text(bytes_of(ours)),
        as_text(bytes_of(theirs)),
    );
    let binary = (base.is_some() && tb.is_none())
        || (ours.is_some() && to.is_none())
        || (theirs.is_some() && tt.is_none());
    let ours_exec = ours.map(|(_, e)| e).unwrap_or(false);
    let theirs_exec = theirs.map(|(_, e)| e).unwrap_or(false);
    if binary {
        UnitTexts {
            unit,
            base: base.map(dto::sha256_hex),
            ours: bytes_of(ours).map(dto::sha256_hex),
            theirs: bytes_of(theirs).map(dto::sha256_hex),
            executable: theirs_exec,
            ours_executable: ours_exec,
            ours_bytes: bytes_of(ours).map(<[u8]>::to_vec),
            theirs_bytes: bytes_of(theirs).map(<[u8]>::to_vec),
        }
    } else {
        UnitTexts {
            unit,
            base: tb,
            ours: to,
            theirs: tt,
            executable: theirs_exec,
            ours_executable: ours_exec,
            ours_bytes: None,
            theirs_bytes: None,
        }
    }
}

/// The file units of a part: one per path any of the three sides has (sorted by path), plus
/// the files only the agent has — the person's own, which an apply keeps.
fn file_units(
    base: impl IntoIterator<Item = (String, Vec<u8>, bool)>,
    ours: impl IntoIterator<Item = (String, Vec<u8>, bool)>,
    theirs: impl IntoIterator<Item = (String, Vec<u8>, bool)>,
) -> (Vec<UnitTexts>, Vec<aurora_template::TemplateFile>) {
    type Side = (
        Option<(Vec<u8>, bool)>,
        Option<(Vec<u8>, bool)>,
        Option<(Vec<u8>, bool)>,
    );
    let mut sides: BTreeMap<String, Side> = BTreeMap::new();
    for (path, bytes, exec) in base {
        sides.entry(path).or_default().0 = Some((bytes, exec));
    }
    for (path, bytes, exec) in ours {
        sides.entry(path).or_default().1 = Some((bytes, exec));
    }
    for (path, bytes, exec) in theirs {
        sides.entry(path).or_default().2 = Some((bytes, exec));
    }
    let mut units = Vec::new();
    let mut own = Vec::new();
    for (path, (base, ours, theirs)) in sides {
        if base.is_none()
            && theirs.is_none()
            && let Some((bytes, exec)) = ours
        {
            // The person's own file, on no template side: an apply keeps it.
            own.push(aurora_template::TemplateFile {
                path,
                bytes,
                executable: exec,
            });
            continue;
        }
        units.push(file_unit(
            format!("file:{path}"),
            base.as_ref().map(|(b, _)| b.as_slice()),
            ours.as_ref().map(|(b, e)| (b.as_slice(), *e)),
            theirs.as_ref().map(|(b, e)| (b.as_slice(), *e)),
        ));
    }
    (units, own)
}

/// The units of a skill part: the description and the body, then one unit per file path any
/// of the three sides has, plus the agent's own files (a skill's files are part of its hash,
/// so they are parts of the merge too).
fn skill_units(
    base: &aurora_template::RenderedSkill,
    row: &skills_repo::SkillRow,
    row_files: &[skills_repo::FileRow],
    theirs: &aurora_template::RenderedSkill,
) -> (Vec<UnitTexts>, Vec<aurora_template::TemplateFile>) {
    let mut units = vec![
        UnitTexts {
            unit: "description".to_owned(),
            base: Some(base.use_it_when.clone()),
            ours: Some(row.description.clone()),
            theirs: Some(theirs.use_it_when.clone()),
            executable: false,
            ours_executable: false,
            ours_bytes: None,
            theirs_bytes: None,
        },
        UnitTexts {
            unit: "body".to_owned(),
            base: Some(base.body.clone()),
            ours: Some(row.body.clone()),
            theirs: Some(theirs.body.clone()),
            executable: false,
            ours_executable: false,
            ours_bytes: None,
            theirs_bytes: None,
        },
    ];
    let (files, own) = file_units(
        base.files
            .iter()
            .map(|f| (f.path.clone(), f.bytes.clone(), f.executable)),
        row_files
            .iter()
            .map(|f| (f.path.clone(), f.content.clone(), f.executable)),
        theirs
            .files
            .iter()
            .map(|f| (f.path.clone(), f.bytes.clone(), f.executable)),
    );
    units.extend(files);
    (units, own)
}

/// The units of the helpers part: one per file path any of the three sides has, plus the
/// agent's own helper files.
fn helper_units(
    base: &[aurora_template::TemplateFile],
    ours: &[helpers_repo::HelperRow],
    theirs: &[aurora_template::TemplateFile],
) -> (Vec<UnitTexts>, Vec<aurora_template::TemplateFile>) {
    file_units(
        base.iter()
            .map(|f| (f.path.clone(), f.bytes.clone(), f.executable)),
        ours.iter()
            .map(|h| (h.path.clone(), h.content.clone(), h.executable)),
        theirs
            .iter()
            .map(|f| (f.path.clone(), f.bytes.clone(), f.executable)),
    )
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
    /// `(kind, key)` → the server's own merge of every `merged`/`conflict` part (FIX-251):
    /// the offer's view and the plan the apply resolves against.
    plans: BTreeMap<(String, String), PartPlan>,
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
    // The new version's texts take its inputs' defaults (D-500), also for an input the agent
    // has no value for; the base keeps what the agent started from.
    let new_inputs = &ctx.new_content.meta.inputs;
    let checked = aurora_template::CheckedInputs::from_values(aurora_template::with_defaults(
        new_inputs,
        render_inputs,
    ));
    let rendered = aurora_template::render(&ctx.new_content, &checked, &agent.name, person);
    // The merge's own renders (FIX-251), both with the **stored** inputs: the base is the
    // text the agent started from, and theirs is what the review showed. The merge's hash
    // pins these texts, so the inputs an apply gives never move them — the offer the person
    // answered to is the one the apply verifies against.
    let stored_checked = aurora_template::CheckedInputs::from_values(stored.clone());
    let merge_base = ctx
        .old_content
        .as_ref()
        .map(|old| aurora_template::render(old, &stored_checked, &agent.name, person));
    let theirs_checked = aurora_template::CheckedInputs::from_values(
        aurora_template::with_defaults(new_inputs, stored.clone()),
    );
    let merge_theirs =
        aurora_template::render(&ctx.new_content, &theirs_checked, &agent.name, person);

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

    // One pool of merge steps for the whole request (FIX-251): every part's merge spends
    // from it, wherever it runs.
    let budget = merge::Budget::new(merge::MERGE_STEPS_MAX);

    let mut parts_out: Vec<OfferPart> = Vec::new();
    let mut states: BTreeMap<(String, String), &'static str> = BTreeMap::new();
    let mut plans: BTreeMap<(String, String), PartPlan> = BTreeMap::new();

    // ---- instructions -------------------------------------------------------------------------
    let proposed_hash = instructions_hash(&rendered.instructions);
    let current_hash = instructions_hash(&agent.brief);
    let rec = recorded.get(&("instructions".to_owned(), String::new()));
    if let Some(mut st) = listed(rec, Some(&proposed_hash), Some(&current_hash)) {
        let mut merge = None;
        let mut merge_note = None;
        if st == "edited_here" {
            // The agent's own edit meets the new version (FIX-251): the three-way merge of
            // the starting text (the stored version's), the agent's brief and the new
            // version's text — or, when the starting text is not stored, the
            // all-or-nothing edit with a note saying why.
            let units = vec![UnitTexts {
                unit: "text".to_owned(),
                base: merge_base.as_ref().map(|r| r.instructions.clone()),
                ours: Some(agent.brief.clone()),
                theirs: Some(merge_theirs.instructions.clone()),
                executable: false,
                ours_executable: false,
                ours_bytes: None,
                theirs_bytes: None,
            }];
            let planned = plan_off_runtime(
                units,
                Vec::new(),
                compose_instructions,
                merge_base.is_some(),
                &budget,
            )
            .await?;
            st = planned.state;
            merge = planned.view;
            merge_note = planned.note;
            if let Some(plan) = planned.plan {
                plans.insert(("instructions".to_owned(), String::new()), plan);
            }
        }
        states.insert(("instructions".to_owned(), String::new()), st);
        parts_out.push(OfferPart {
            kind: "instructions".to_owned(),
            key: String::new(),
            state: st.to_owned(),
            current: Some(agent.brief.clone()),
            proposed: Some(rendered.instructions.clone()),
            merge,
            merge_note,
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
        if let Some(mut st) = listed(rec, Some(&ph), ch.as_deref()) {
            let mut merge = None;
            let mut merge_note = None;
            if st == "edited_here" {
                // The skill is merged per text, not per skill (FIX-251, D-488(b)): the
                // description, the body and every file path any side has are units of their
                // own. Without the stored version's skill of this name (or the agent's own
                // row) there is nothing to merge against: the all-or-nothing edit.
                let base = merge_base
                    .as_ref()
                    .and_then(|r| r.skills.iter().find(|x| x.name == s.name));
                let theirs = merge_theirs.skills.iter().find(|x| x.name == s.name);
                match (base, theirs, own.as_ref()) {
                    (Some(base), Some(theirs), Some(row)) => {
                        let row_files: &[skills_repo::FileRow] = live_files
                            .get(&row.id)
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        let (units, own_files) = skill_units(base, row, row_files, theirs);
                        let planned =
                            plan_off_runtime(units, own_files, compose_skill, true, &budget)
                                .await?;
                        st = planned.state;
                        merge = planned.view;
                        merge_note = planned.note;
                        if let Some(plan) = planned.plan {
                            plans.insert(("skill".to_owned(), s.name.clone()), plan);
                        }
                    }
                    _ => merge_note = Some(MERGE_NOTE.to_owned()),
                }
            }
            states.insert(("skill".to_owned(), s.name.clone()), st);
            parts_out.push(OfferPart {
                kind: "skill".to_owned(),
                key: s.name.clone(),
                state: st.to_owned(),
                current: cur,
                proposed: Some(proposed_text(s)),
                merge,
                merge_note,
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
                    merge: None,
                    merge_note: None,
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
    if let Some(mut st) = listed(rec, proposed_opt, current_opt) {
        let mut merge = None;
        let mut merge_note = None;
        if st == "edited_here" {
            // The helper files are merged per file (FIX-251, D-488(b)); the person's own
            // files — on no template side — an apply keeps. Without the stored version's
            // content there is nothing to merge against.
            match &merge_base {
                Some(base) => {
                    let (units, own_files) =
                        helper_units(&base.helpers, &live_helpers, &merge_theirs.helpers);
                    let planned =
                        plan_off_runtime(units, own_files, compose_helpers, true, &budget).await?;
                    st = planned.state;
                    merge = planned.view;
                    merge_note = planned.note;
                    if let Some(plan) = planned.plan {
                        plans.insert(("helpers".to_owned(), String::new()), plan);
                    }
                }
                None => merge_note = Some(MERGE_NOTE.to_owned()),
            }
        }
        states.insert(("helpers".to_owned(), String::new()), st);
        parts_out.push(OfferPart {
            kind: "helpers".to_owned(),
            key: String::new(),
            state: st.to_owned(),
            current: (!live_helpers.is_empty()).then_some(current_helpers_text),
            proposed: (!rendered.helpers.is_empty()).then_some(proposed_helpers_text),
            merge,
            merge_note,
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
                    merge: None,
                    merge_note: None,
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
                        merge: None,
                        merge_note: None,
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
        plans,
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

// ---- the resolutions ----------------------------------------------------------------------------

/// The written content of one taken part with a merge: the server's own composition of the
/// person's choices against the server's own merge (FIX-251).
enum ResolvedPart {
    Instructions(String),
    Skill {
        description: String,
        body: String,
        files: Vec<aurora_template::TemplateFile>,
    },
    Helpers(Vec<aurora_template::TemplateFile>),
}

/// The 422 for resolutions that do not match the offer's conflict hunks (a stale or forged
/// review).
fn merge_unresolved() -> ApiError {
    ApiError::unprocessable(
        "template_merge_unresolved",
        "The resolutions do not match this part's conflicts.",
    )
}

/// The 422 for a free edit over the resolution size cap.
fn resolution_too_large() -> ApiError {
    ApiError::unprocessable("template_resolution_too_large", "This edit is too large.")
}

/// The 422 for a merged part whose composed text is over the part's own limit (the same
/// code the all-or-nothing parts are refused with).
fn part_too_large() -> ApiError {
    ApiError::unprocessable(
        "template_too_large",
        "The merged text is too large for this part.",
    )
}

/// The person's choice for one conflict hunk, as the request states it.
enum Choice {
    KeepMine,
    TakeTheirs,
    Text(String),
}

fn choice_of(r: &dto::ApplyResolution) -> Result<Choice, ApiError> {
    match r.choice.as_str() {
        "keep_mine" => Ok(Choice::KeepMine),
        "take_theirs" => Ok(Choice::TakeTheirs),
        "text" => match &r.text {
            Some(t) if t.chars().count() <= RESOLUTION_TEXT_MAX => Ok(Choice::Text(t.clone())),
            Some(_) => Err(resolution_too_large()),
            None => Err(merge_unresolved()),
        },
        _ => Err(merge_unresolved()),
    }
}

/// How many resolutions one planned unit takes: one for a whole choice, one per conflict
/// hunk of its merge, none for a unit the merge decided on its own.
fn unit_resolutions(u: &UnitPlan) -> usize {
    match (&u.whole, &u.merge) {
        (Some(_), _) => 1,
        (None, Some(m)) => m.conflict_count(),
        (None, None) => 0,
    }
}

/// The content and executable bit one planned unit resolves to: `None` is a file the
/// resolution removes (or one that was never in the merged set).
fn resolve_unit(
    u: &UnitPlan,
    res: &[&dto::ApplyResolution],
) -> Result<(Option<Vec<u8>>, bool), ApiError> {
    if let Some(w) = &u.whole {
        let Some(r) = res.first() else {
            return Err(merge_unresolved());
        };
        return Ok(match choice_of(r)? {
            Choice::KeepMine => (
                w.ours_bytes
                    .clone()
                    .or_else(|| w.ours.clone().map(String::into_bytes)),
                u.ours_executable,
            ),
            Choice::TakeTheirs => (
                w.theirs_bytes
                    .clone()
                    .or_else(|| w.theirs.clone().map(String::into_bytes)),
                u.executable,
            ),
            Choice::Text(t) => (Some(t.into_bytes()), u.executable),
        });
    }
    if let Some(m) = &u.merge {
        let rs: Vec<merge::Resolution> = res
            .iter()
            .map(|r| match choice_of(r)? {
                Choice::KeepMine => Ok(merge::Resolution::KeepMine),
                Choice::TakeTheirs => Ok(merge::Resolution::TakeTheirs),
                Choice::Text(t) => Ok(merge::Resolution::Text(t)),
            })
            .collect::<Result<_, ApiError>>()?;
        let text = m.resolve(&rs, RESOLUTION_TEXT_MAX).map_err(|e| match e {
            ResolveError::Count => merge_unresolved(),
            ResolveError::TooLarge => resolution_too_large(),
        })?;
        return Ok((Some(text.into_bytes()), u.executable));
    }
    Ok((
        u.bytes
            .clone()
            .or_else(|| u.text.clone().map(String::into_bytes)),
        u.executable,
    ))
}

/// The resolved text of a text unit (the instructions' `text`, a skill's `description` or
/// `body`): these units are never files, so their resolution is always text.
fn resolved_text(u: &UnitPlan, res: &[&dto::ApplyResolution]) -> Result<String, ApiError> {
    match resolve_unit(u, res)?
        .0
        .and_then(|b| String::from_utf8(b).ok())
    {
        Some(t) => Ok(t),
        None => Err(merge_unresolved()),
    }
}

/// The resolved files of a part: every file unit's content (or its removal) plus the
/// person's own files, which no merge touches. The description and body units come first in
/// a skill's units, so the file units start at 2.
fn resolved_files(
    units: &[UnitPlan],
    res: &[Vec<&dto::ApplyResolution>],
    own: &[aurora_template::TemplateFile],
) -> Result<Vec<aurora_template::TemplateFile>, ApiError> {
    let mut files: Vec<aurora_template::TemplateFile> = own.to_vec();
    for (u, r) in units.iter().zip(res) {
        let Some(path) = u.unit.strip_prefix("file:") else {
            return Err(ApiError::internal("merge unit is not a file"));
        };
        let (bytes, executable) = resolve_unit(u, r)?;
        let Some(bytes) = bytes else { continue };
        files.push(aurora_template::TemplateFile {
            path: path.to_owned(),
            bytes,
            executable,
        });
    }
    Ok(files)
}

/// A helper set as the one helper writer takes it.
fn helper_files(
    files: &[aurora_template::TemplateFile],
) -> Vec<agent_helpers::service::HelperFile<'_>> {
    files
        .iter()
        .map(|f| agent_helpers::service::HelperFile {
            path: &f.path,
            content: &f.bytes,
            executable: f.executable,
        })
        .collect()
}

/// The written content of every taken part that carries a merge (FIX-251): the offer's merge
/// hash echoed back, one resolution per conflict hunk in the offer's order, and the part's
/// own limits on what the choices compose. Anything else is refused before anything is
/// written; the parts without a merge apply exactly as before.
fn resolve_parts(
    diff: &Diff,
    taken: &[((String, String), &'static str)],
    req_parts: &[dto::ApplyPart],
) -> Result<BTreeMap<(String, String), ResolvedPart>, ApiError> {
    let mut out: BTreeMap<(String, String), ResolvedPart> = BTreeMap::new();
    for (k, st) in taken {
        let Some(p) = req_parts.iter().find(|p| p.kind == k.0 && p.key == k.1) else {
            return Err(ApiError::internal("the request's merge part is missing"));
        };
        if !matches!(*st, "merged" | "conflict") {
            // A reviewed merge that is no longer one (the part grew past the merge's caps
            // while the review was open, FIX-251) is the stale review: never the wholesale
            // overwrite in its place.
            if p.merge_sha256.is_some() || !p.resolutions.is_empty() {
                return Err(version_changed());
            }
            continue;
        }
        let plan = diff
            .plans
            .get(k)
            .ok_or_else(|| ApiError::internal("the merge plan of a listed part is missing"))?;
        // The review's merge is the server's own: a hash the offer did not show is the plain
        // 409 (C-1112's stale review), never a silent re-merge.
        if p.merge_sha256.as_deref() != Some(plan.view.merge_sha256.as_str()) {
            return Err(version_changed());
        }
        // The resolutions follow the offer's hunks: same units, same order, none missing,
        // none extra.
        let mut it = p.resolutions.iter();
        let mut per_unit: Vec<Vec<&dto::ApplyResolution>> = Vec::new();
        for u in &plan.units {
            let mut mine = Vec::new();
            for _ in 0..unit_resolutions(u) {
                let r = it.next().ok_or_else(merge_unresolved)?;
                if r.unit != u.unit {
                    return Err(merge_unresolved());
                }
                choice_of(r)?;
                mine.push(r);
            }
            per_unit.push(mine);
        }
        if it.next().is_some() {
            return Err(merge_unresolved());
        }
        let at = |name: &str| -> Result<usize, ApiError> {
            plan.units
                .iter()
                .position(|u| u.unit == name)
                .ok_or_else(|| ApiError::internal("merge unit missing"))
        };
        let resolved = match k.0.as_str() {
            "instructions" => {
                let i = at("text")?;
                let text = resolved_text(&plan.units[i], &per_unit[i])?;
                if text.chars().count() > agents::service::BRIEF_MAX {
                    return Err(part_too_large());
                }
                ResolvedPart::Instructions(text)
            }
            "skill" => {
                let di = at("description")?;
                let bi = at("body")?;
                let description = resolved_text(&plan.units[di], &per_unit[di])?;
                let body = resolved_text(&plan.units[bi], &per_unit[bi])?;
                let files = resolved_files(&plan.units[2..], &per_unit[2..], &plan.own_files)?;
                // The merged skill keeps the limits of an edit, with the same errors.
                let contents: Vec<&[u8]> = files.iter().map(|f| f.bytes.as_slice()).collect();
                skills_domain::service::check_composed(&description, &body, &contents)?;
                ResolvedPart::Skill {
                    description,
                    body,
                    files,
                }
            }
            "helpers" => {
                let files = resolved_files(&plan.units, &per_unit, &plan.own_files)?;
                // The merged set keeps the caps of any helper write, with the same errors.
                agent_helpers::service::check_set(&helper_files(&files))?;
                ResolvedPart::Helpers(files)
            }
            _ => return Err(ApiError::internal("merge part of an unknown kind")),
        };
        out.insert(k.clone(), resolved);
    }
    Ok(out)
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
/// kept only when it still passes the input's (possibly changed) rules on its own. An input
/// left without a value takes its `default` (D-500).
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
    aurora_template::with_defaults(&ctx.new_content.meta.inputs, out)
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
    // The merge parts resolve before anything is fetched (FIX-251): the hash echo, the
    // resolutions and the size limits are checked against the offer's own merge.
    resolve_parts(&diff, &taken, &req.parts)?;

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
    // have moved between the pre-check above and this transaction. The agent's row is locked
    // first: the manual writers of the brief, the skills and the helpers take that lock too, so
    // an edit is either committed before the re-reads (and seen) or waits for the commit.
    let agent = agents_repo::lock_for_write(&mut *tx, agent_id)
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
    // The merges are re-resolved on the transaction's own diff (SEC-146 A): the person's
    // choices apply to the server's own hunks, recomputed under the lock.
    let resolved = resolve_parts(&diff, &taken, &req.parts)?;
    let result = apply_in_tx(
        &mut tx,
        state,
        caller,
        &agent,
        &diff,
        &taken,
        &resolved,
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
/// on any error. A part with a merge writes [`ResolvedPart`] — the server's own composition of
/// the person's choices; every other part writes the rendered version as before.
#[allow(clippy::too_many_arguments)]
async fn apply_in_tx(
    tx: &mut audit::Tx,
    state: &AppState,
    caller: &Caller,
    agent: &agents_repo::AgentRow,
    diff: &Diff,
    taken: &[((String, String), &'static str)],
    resolved: &BTreeMap<(String, String), ResolvedPart>,
    adds: &[(String, library::PreparedAdd)],
    inputs_json: &Value,
) -> Result<(Vec<dto::ApplyNote>, Vec<AuditEvent>), ApiError> {
    let mut notes: Vec<dto::ApplyNote> = Vec::new();
    let mut events: Vec<AuditEvent> = Vec::new();
    let mut recorded: Vec<trepo::PartRow> = Vec::new();

    for ((kind, key), st) in taken {
        let st: &str = st;
        match (kind.as_str(), st) {
            // ---- instructions: the merged brief, or the rendered one, replaces the agent's --
            ("instructions", _) => {
                let brief = match resolved.get(&("instructions".to_owned(), String::new())) {
                    Some(ResolvedPart::Instructions(t)) => t.clone(),
                    _ => diff.rendered.instructions.clone(),
                };
                agents_repo::update(
                    &mut **tx,
                    agent.id,
                    &agents_repo::AgentPatch {
                        brief: Some(&brief),
                        ..Default::default()
                    },
                )
                .await?;
                recorded.push(trepo::PartRow {
                    agent_id: agent.id,
                    kind: "instructions".into(),
                    key: String::new(),
                    content_sha256: instructions_hash(&brief),
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
            // Any other state: the merged skill (the person's resolutions composed with the
            // server's own merge, the person's own files kept), or the version's skill
            // replacing the agent's own, or creating it (`new`, `removed_here`) — an
            // ordinary editable skill (C-1112, FIX-251).
            ("skill", _) => {
                let (use_it_when, body, files) = match resolved
                    .get(&("skill".to_owned(), key.clone()))
                {
                    Some(ResolvedPart::Skill {
                        description,
                        body,
                        files,
                    }) => (description.clone(), body.clone(), files.clone()),
                    _ => {
                        let Some(s) = diff.rendered.skills.iter().find(|s| &s.name == key) else {
                            continue; // cannot happen: every other state has proposed content
                        };
                        // Filling in the inputs can push a text over a skill's limits.
                        create::rendered_skill_ok(s)?;
                        (s.use_it_when.clone(), s.body.clone(), s.files.clone())
                    }
                };
                let existing = skills_repo::get_agent_named(tx, agent.id, key).await?;
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
                        // The apply replaces the skill's text, not the person's on/off
                        // choice (SEC-146 E): the existing row's `enabled` stays.
                        skills_repo::update(tx, row.id, &use_it_when, &body, row.enabled).await?;
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
                                name: key,
                                description: &use_it_when,
                                body: &body,
                                enabled: true,
                            },
                            "user",
                        )
                        .await?
                        .row
                    }
                };
                for f in &files {
                    skills_repo::upsert_file(tx, row.id, &f.path, &f.bytes, Some(f.executable))
                        .await?;
                }
                skills_repo::bump_agent(tx, agent.id).await?;
                // The recorded hash is the written text's, so the next offer diffs against
                // what the apply really left behind.
                recorded.push(trepo::PartRow {
                    agent_id: agent.id,
                    kind: "skill".into(),
                    key: key.clone(),
                    content_sha256: skill_hash(
                        &use_it_when,
                        &body,
                        &files
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
                if !removed.is_empty() {
                    skills_repo::bump_agent(tx, agent.id).await?;
                }
            }
            // Any other state: the merged helper files (the person's resolutions composed
            // with the server's own merge, the person's own files kept), or the version's
            // helper files replacing the agent's set (C-1143's one writer, which bumps the
            // home sync itself).
            ("helpers", _) => {
                let written: &[aurora_template::TemplateFile] =
                    match resolved.get(&("helpers".to_owned(), String::new())) {
                        Some(ResolvedPart::Helpers(fs)) => fs.as_slice(),
                        _ => &diff.rendered.helpers,
                    };
                agent_helpers::service::replace_all(tx, agent.id, &helper_files(written)).await?;
                recorded.push(create::helpers_part(agent.id, written));
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
    let agent = agents_repo::lock_for_write(&mut *tx, agent_id)
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
