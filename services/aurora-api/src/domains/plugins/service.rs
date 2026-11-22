use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::repo::plugins::{self, PackageRow};
use aurora_registry::diff::{Widening, widens};
use aurora_registry::manifest::Agent;
use aurora_registry::snapshot::{AgentFiles, SkillEntry, agent_files};
use aurora_registry::{Limits, Manifest, Package};
use sqlx::PgConnection;
use uuid::Uuid;

use super::dto::{
    PluginAgent, PluginAgentChanged, PluginAgentFileChange, PluginAgentOperations,
    PluginAgentSkill, PluginAgentSkillChange, PluginAgentSkillFile, PluginPackage,
};

const STATUSES: [&str; 6] = [
    "pending_approval",
    "active",
    "superseded",
    "rejected",
    "disabled",
    "invalid",
];

pub fn check_status(s: &str) -> Result<(), ApiError> {
    if STATUSES.contains(&s) {
        Ok(())
    } else {
        Err(ApiError::invalid_request(
            "Invalid status filter.",
            vec![aurora_core::FieldError::new("/status", "unknown status")],
        ))
    }
}

pub async fn get_row(state: &AppState, id: Uuid) -> Result<PackageRow, ApiError> {
    plugins::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Plugin package not found."))
}

/// Widenings versus the previously approved version (pending packages only).
async fn widenings(state: &AppState, r: &PackageRow) -> Result<Vec<Widening>, ApiError> {
    if r.status != "pending_approval" {
        return Ok(Vec::new());
    }
    let Some(prev) = plugins::previous_approved(&state.pool, &r.package_id).await? else {
        return Ok(Vec::new());
    };
    let (Ok(old), Ok(new)) = (
        serde_json::from_value::<Manifest>(prev.manifest),
        serde_json::from_value::<Manifest>(r.manifest.clone()),
    ) else {
        return Ok(Vec::new());
    };
    Ok(widens(&old, &new))
}

/// The detail view: like [`to_dto`], plus `impact` for a `pending_approval` version (C-340).
pub async fn to_detail_dto(state: &AppState, r: PackageRow) -> Result<PluginPackage, ApiError> {
    let pending = r.status == "pending_approval";
    let agent = {
        let mut conn = state
            .pool
            .acquire()
            .await
            .map_err(aurora_db::DbError::from)?;
        agent_of(&mut conn, &r).await?
    };
    let impact = if pending {
        let mut conn = state
            .pool
            .acquire()
            .await
            .map_err(aurora_db::DbError::from)?;
        Some(super::upgrade::compute(&mut conn, &r).await?.into_dto())
    } else {
        None
    };
    let mut dto = to_dto(state, r).await?;
    dto.impact = impact;
    dto.agent = agent;
    Ok(dto)
}

/// A package version's manifest with what its archive gives agents (the operations file and the
/// skills with their digests and executable bits, from the stored archive as the snapshot's are,
/// C-1132).
pub struct AgentView {
    pub manifest: Manifest,
    pub files: AgentFiles,
}

/// Reads the archive only when an app has an `agent` block (skills need its summary).
pub async fn agent_view(conn: &mut PgConnection, r: &PackageRow) -> Result<AgentView, ApiError> {
    let manifest: Manifest = serde_json::from_value(r.manifest.clone())
        .map_err(|e| ApiError::internal(format!("stored manifest unreadable: {e}")))?;
    let has_agent = manifest.apps.iter().any(|a| a.agent.is_some());
    let mut files = AgentFiles::default();
    // An `invalid` package stores no archive (ingest keeps none): the detail still answers, with
    // the manifest and no agent files, so the admin can read why it was refused.
    let archive = if has_agent && r.status != "invalid" {
        plugins::get_blob(&mut *conn, &r.digest, "archive").await?
    } else {
        None
    };
    if let Some(bytes) = archive {
        let pkg = Package::read(&bytes, &Limits::default())
            .map_err(|_| ApiError::internal("Stored package archive is unreadable."))?;
        files = agent_files(&manifest, &pkg);
    }
    Ok(AgentView { manifest, files })
}

/// Every file of a skill as `(path, sha256, executable)`, `SKILL.md` first (never executable).
fn skill_files(s: &SkillEntry) -> Vec<(&str, &str, bool)> {
    std::iter::once(("SKILL.md", s.sha256.as_str(), false))
        .chain(
            s.files
                .iter()
                .map(|f| (f.path.as_str(), f.sha256.as_str(), f.executable)),
        )
        .collect()
}

/// How one skill changed (`None` = the same). A file whose executable bit differs has a `mode`
/// entry, next to a `changed` one when its content changed too (C-1136).
fn skill_change(
    old: Option<&SkillEntry>,
    new: Option<&SkillEntry>,
) -> Option<PluginAgentSkillChange> {
    let all = |s: &SkillEntry, change: &str| -> Vec<PluginAgentFileChange> {
        skill_files(s)
            .into_iter()
            .map(|(p, _, _)| PluginAgentFileChange {
                path: p.to_owned(),
                change: change.to_owned(),
            })
            .collect()
    };
    match (old, new) {
        (None, Some(n)) => Some(PluginAgentSkillChange {
            name: n.name.clone(),
            change: "added".into(),
            files: all(n, "added"),
        }),
        (Some(o), None) => Some(PluginAgentSkillChange {
            name: o.name.clone(),
            change: "removed".into(),
            files: all(o, "removed"),
        }),
        (Some(o), Some(n)) => {
            let of = skill_files(o);
            let nf = skill_files(n);
            let mut files = Vec::new();
            for (p, h, x) in &nf {
                match of.iter().find(|(op, _, _)| op == p) {
                    None => files.push(PluginAgentFileChange {
                        path: (*p).to_owned(),
                        change: "added".into(),
                    }),
                    Some((_, oh, ox)) => {
                        // A file whose content and executable bit both changed is listed twice
                        // (`changed` and `mode`), so the review says it became (or stopped being) a
                        // script even when its text changed too.
                        if oh != h {
                            files.push(PluginAgentFileChange {
                                path: (*p).to_owned(),
                                change: "changed".into(),
                            });
                        }
                        if ox != x {
                            files.push(PluginAgentFileChange {
                                path: (*p).to_owned(),
                                change: "mode".into(),
                            });
                        }
                    }
                }
            }
            for (p, _, _) in &of {
                if !nf.iter().any(|(np, _, _)| np == p) {
                    files.push(PluginAgentFileChange {
                        path: (*p).to_owned(),
                        change: "removed".into(),
                    });
                }
            }
            (!files.is_empty() || o.description != n.description).then(|| PluginAgentSkillChange {
                name: n.name.clone(),
                change: "changed".into(),
                files,
            })
        }
        (None, None) => None,
    }
}

/// What changed for one app against the previous version (`old` = `None`: the app is new, all of it
/// is "added").
fn app_changed(
    old: Option<(&Agent, &AgentFiles)>,
    new: (&Agent, &AgentFiles),
    app_id: &str,
) -> PluginAgentChanged {
    let (nag, nfiles) = new;
    let empty = Agent::default();
    let (oag, ofiles) = match old {
        Some((a, f)) => (a, Some(f)),
        None => (&empty, None),
    };
    let none: Vec<SkillEntry> = Vec::new();
    let oskills = ofiles.and_then(|f| f.skills.get(app_id)).unwrap_or(&none);
    let nskills = nfiles.skills.get(app_id).unwrap_or(&none);
    let mut skills = Vec::new();
    for n in nskills {
        if let Some(c) = skill_change(oskills.iter().find(|o| o.name == n.name), Some(n)) {
            skills.push(c);
        }
    }
    for o in oskills {
        if !nskills.iter().any(|n| n.name == o.name)
            && let Some(c) = skill_change(Some(o), None)
        {
            skills.push(c);
        }
    }
    PluginAgentChanged {
        summary: oag.summary != nag.summary,
        instructions: oag.instructions != nag.instructions,
        operations: ofiles.and_then(|f| f.operations.get(app_id)) != nfiles.operations.get(app_id),
        skills,
    }
}

/// `changed` of every app of `new` against `old` (`24 §9`).
fn changes(old: &AgentView, new: &AgentView) -> Vec<(String, PluginAgentChanged)> {
    new.manifest
        .apps
        .iter()
        .map(|a| {
            let id = a.about.id.as_str();
            let empty = Agent::default();
            let nag = a.agent.as_ref().unwrap_or(&empty);
            let oag = old
                .manifest
                .apps
                .iter()
                .find(|o| o.about.id == a.about.id)
                .map(|o| o.agent.as_ref().unwrap_or(&empty));
            (
                id.to_owned(),
                app_changed(oag.map(|o| (o, &old.files)), (nag, &new.files), id),
            )
        })
        .collect()
}

/// `agent_changed` of the audit metadata (C-1137): the apps whose summary, essentials, operations
/// file or any app skill (added, removed, `SKILL.md` or a file changed, an executable bit flipped)
/// differs from the previously active version of the same package. Empty when there is no previous
/// version.
pub fn agent_changed(old: Option<&AgentView>, new: &AgentView) -> Vec<String> {
    let Some(old) = old else { return Vec::new() };
    changes(old, new)
        .into_iter()
        .filter(|(_, c)| c.summary || c.instructions || c.operations || !c.skills.is_empty())
        .map(|(id, _)| id)
        .collect()
}

/// The agent files of the active version of `row`'s package, when there is another one.
pub async fn active_agent_view(
    conn: &mut PgConnection,
    row: &PackageRow,
) -> Result<Option<AgentView>, ApiError> {
    match plugins::active_of_package(&mut *conn, &row.package_id).await? {
        Some(a) if a.id != row.id => Ok(Some(agent_view(conn, &a).await?)),
        _ => Ok(None),
    }
}

/// `PluginPackage.agent` for the detail view (C-1136).
pub async fn agent_of(
    conn: &mut PgConnection,
    r: &PackageRow,
) -> Result<Vec<PluginAgent>, ApiError> {
    let new = agent_view(conn, r).await?;
    let old = active_agent_view(conn, r).await?;
    let mut changed: std::collections::BTreeMap<String, PluginAgentChanged> = old
        .as_ref()
        .map(|o| changes(o, &new).into_iter().collect())
        .unwrap_or_default();
    Ok(new
        .manifest
        .apps
        .iter()
        .map(|a| {
            let id = a.about.id.clone();
            let ag = a.agent.as_ref();
            PluginAgent {
                summary: ag.and_then(|g| g.summary.clone()),
                instructions: ag.map(|g| g.instructions.clone()).unwrap_or_default(),
                operations: new
                    .files
                    .operations
                    .get(&id)
                    .map(|o| PluginAgentOperations {
                        file: o.file.clone(),
                        size: o.size,
                        sha256: o.sha256.clone(),
                        count: u32::try_from(o.count).unwrap_or(u32::MAX),
                    }),
                skills: new
                    .files
                    .skills
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .map(|s| PluginAgentSkill {
                        name: s.name.clone(),
                        description: s.description.clone(),
                        size: s.size + s.files.iter().map(|f| f.size).sum::<u64>(),
                        files: std::iter::once(PluginAgentSkillFile {
                            path: "SKILL.md".into(),
                            size: s.size,
                            sha256: s.sha256.clone(),
                            executable: false,
                        })
                        .chain(s.files.iter().map(|f| PluginAgentSkillFile {
                            path: f.path.clone(),
                            size: f.size,
                            sha256: f.sha256.clone(),
                            executable: f.executable,
                        }))
                        .collect(),
                    })
                    .collect(),
                changed: changed.remove(&id),
                app_id: id,
            }
        })
        .collect())
}

/// The bytes of one skill file of a package (`GET …/apps/{app_id}/skills/{name}/file`): the file
/// must be one the archive carried for that skill (`path` `None` = `SKILL.md`); the content is
/// read by its digest.
pub async fn skill_file(
    conn: &mut PgConnection,
    r: &PackageRow,
    app_id: &str,
    name: &str,
    path: Option<&str>,
) -> Result<Vec<u8>, ApiError> {
    let not_found = || ApiError::not_found("Skill file not found.");
    let view = agent_view(conn, r).await?;
    let skill = view
        .files
        .skills
        .get(app_id)
        .and_then(|l| l.iter().find(|s| s.name == name))
        .ok_or_else(not_found)?;
    let digest = match path {
        None | Some("SKILL.md") => &skill.sha256,
        Some(p) => {
            &skill
                .files
                .iter()
                .find(|f| f.path == p)
                .ok_or_else(not_found)?
                .sha256
        }
    };
    plugins::get_agent_blob(&mut *conn, digest)
        .await?
        .ok_or_else(not_found)
}

/// The operations file of an app of a package (`GET …/apps/{app_id}/operations`).
pub async fn operations_file(
    conn: &mut PgConnection,
    r: &PackageRow,
    app_id: &str,
) -> Result<Vec<u8>, ApiError> {
    let not_found = || ApiError::not_found("Operations file not found.");
    let view = agent_view(conn, r).await?;
    let o = view.files.operations.get(app_id).ok_or_else(not_found)?;
    plugins::get_agent_blob(&mut *conn, &o.sha256)
        .await?
        .ok_or_else(not_found)
}

pub async fn to_dto(state: &AppState, r: PackageRow) -> Result<PluginPackage, ApiError> {
    let w = widenings(state, &r).await?;
    Ok(PluginPackage::from_row(r, &w))
}
