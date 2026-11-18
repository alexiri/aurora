use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::repo::plugins::{self, PackageRow};
use aurora_registry::diff::{Widening, widens};
use aurora_registry::manifest::{Agent, GuideEntry};
use aurora_registry::snapshot::{GuideDigests, guide_digests};
use aurora_registry::{Limits, Manifest, Package};
use sqlx::PgConnection;
use uuid::Uuid;

use super::dto::{
    PluginAgentGuide, PluginGuideChanged, PluginGuideFile, PluginGuideFileChange, PluginPackage,
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
    let guides = {
        let mut conn = state
            .pool
            .acquire()
            .await
            .map_err(aurora_db::DbError::from)?;
        agent_guides(&mut conn, &r).await?
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
    dto.agent_guides = guides;
    Ok(dto)
}

/// A package version's manifest with the digests of its guide files (from the stored archive, as
/// the snapshot's are, C-641).
pub struct GuideView {
    pub manifest: Manifest,
    pub digests: GuideDigests,
}

/// Reads the archive only when the manifest declares a guide file.
pub async fn guide_view(conn: &mut PgConnection, r: &PackageRow) -> Result<GuideView, ApiError> {
    let manifest: Manifest = serde_json::from_value(r.manifest.clone())
        .map_err(|e| ApiError::internal(format!("stored manifest unreadable: {e}")))?;
    let has_guide = manifest
        .apps
        .iter()
        .any(|a| a.agent.as_ref().is_some_and(|g| !g.guide.is_empty()));
    let mut digests = GuideDigests::new();
    if has_guide {
        let bytes = plugins::get_blob(&mut *conn, &r.digest, "archive")
            .await?
            .ok_or_else(|| ApiError::internal("Package archive blob is missing."))?;
        let pkg = Package::read(&bytes, &Limits::default())
            .map_err(|_| ApiError::internal("Stored package archive is unreadable."))?;
        digests = guide_digests(&manifest, &pkg);
    }
    Ok(GuideView { manifest, digests })
}

fn kind_of(g: &GuideEntry) -> &'static str {
    if g.is_recipe() { "recipe" } else { "reference" }
}

/// What changed in one app's guide against the previous version (`None` = the app is new: all of
/// it is "added").
fn app_changed(
    old: Option<(&Agent, &GuideDigests, &str)>,
    new: (&Agent, &GuideDigests, &str),
) -> PluginGuideChanged {
    let (nag, ndig, app_id) = new;
    let empty = Agent::default();
    let (oag, odig) = match old {
        Some((a, d, _)) => (a, Some(d)),
        None => (&empty, None),
    };
    let sha = |d: &GuideDigests, f: &str| {
        d.get(&(app_id.to_owned(), f.to_owned()))
            .map(|(h, _)| h.clone())
    };
    let mut files = Vec::new();
    for g in &nag.guide {
        match oag.guide.iter().find(|o| o.file == g.file) {
            None => files.push(PluginGuideFileChange {
                file: g.file.clone(),
                change: "added".into(),
            }),
            Some(o) => {
                let same = o.title == g.title
                    && kind_of(o) == kind_of(g)
                    && odig.is_some_and(|d| sha(d, &o.file) == sha(ndig, &g.file));
                if !same {
                    files.push(PluginGuideFileChange {
                        file: g.file.clone(),
                        change: "changed".into(),
                    });
                }
            }
        }
    }
    for o in &oag.guide {
        if !nag.guide.iter().any(|g| g.file == o.file) {
            files.push(PluginGuideFileChange {
                file: o.file.clone(),
                change: "removed".into(),
            });
        }
    }
    PluginGuideChanged {
        summary: oag.summary != nag.summary,
        instructions: oag.instructions != nag.instructions,
        operations: false,
        files,
    }
}

/// `changed` of every app of `new` against `old` (`24 §9`).
fn changes(old: &GuideView, new: &GuideView) -> Vec<(String, PluginGuideChanged)> {
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
                app_changed(oag.map(|o| (o, &old.digests, id)), (nag, &new.digests, id)),
            )
        })
        .collect()
}

/// `agent_guide_changed` of the audit metadata: the apps whose summary, essentials, any guide
/// file (recipes included) or operations file differs from the previously active version of the
/// same package. Empty when there is no previous version.
pub fn agent_guide_changed(old: Option<&GuideView>, new: &GuideView) -> Vec<String> {
    let Some(old) = old else { return Vec::new() };
    changes(old, new)
        .into_iter()
        .filter(|(_, c)| c.summary || c.instructions || c.operations || !c.files.is_empty())
        .map(|(id, _)| id)
        .collect()
}

/// The guide of the active version of `row`'s package, when there is another one.
pub async fn active_guide_view(
    conn: &mut PgConnection,
    row: &PackageRow,
) -> Result<Option<GuideView>, ApiError> {
    match plugins::active_of_package(&mut *conn, &row.package_id).await? {
        Some(a) if a.id != row.id => Ok(Some(guide_view(conn, &a).await?)),
        _ => Ok(None),
    }
}

/// `PluginPackage.agent_guides` for the detail view (C-648).
pub async fn agent_guides(
    conn: &mut PgConnection,
    r: &PackageRow,
) -> Result<Vec<PluginAgentGuide>, ApiError> {
    let new = guide_view(conn, r).await?;
    let old = active_guide_view(conn, r).await?;
    let mut changed: std::collections::BTreeMap<String, PluginGuideChanged> = old
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
            PluginAgentGuide {
                summary: ag.and_then(|g| g.summary.clone()),
                instructions: ag.map(|g| g.instructions.clone()).unwrap_or_default(),
                files: ag
                    .into_iter()
                    .flat_map(|g| &g.guide)
                    .map(|g| {
                        let (sha, size) = new
                            .digests
                            .get(&(id.clone(), g.file.clone()))
                            .cloned()
                            .unwrap_or_default();
                        PluginGuideFile {
                            file: g.file.clone(),
                            title: g.title.clone(),
                            kind: kind_of(g).to_owned(),
                            size,
                            sha256: sha,
                        }
                    })
                    .collect(),
                operations: None,
                changed: changed.remove(&id),
                app_id: id,
            }
        })
        .collect())
}

/// The bytes of one guide file of a package (`GET …/agent-guides/{app_id}/{file}`): the file must
/// be listed in the app's guide; the content is read by its digest.
pub async fn guide_file(
    conn: &mut PgConnection,
    r: &PackageRow,
    app_id: &str,
    file: &str,
) -> Result<Vec<u8>, ApiError> {
    let not_found = || ApiError::not_found("Agent guide file not found.");
    let view = guide_view(conn, r).await?;
    let listed = view
        .manifest
        .apps
        .iter()
        .find(|a| a.about.id == app_id)
        .and_then(|a| a.agent.as_ref())
        .is_some_and(|g| g.guide.iter().any(|e| e.file == file));
    if !listed {
        return Err(not_found());
    }
    let (digest, _) = view
        .digests
        .get(&(app_id.to_owned(), file.to_owned()))
        .ok_or_else(not_found)?;
    plugins::get_guide_blob(&mut *conn, digest)
        .await?
        .ok_or_else(not_found)
}

pub async fn to_dto(state: &AppState, r: PackageRow) -> Result<PluginPackage, ApiError> {
    let w = widenings(state, &r).await?;
    Ok(PluginPackage::from_row(r, &w))
}
