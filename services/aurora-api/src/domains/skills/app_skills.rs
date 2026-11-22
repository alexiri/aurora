//! (P5-APPSKILL-2, C-1133, D-453, SEC-150) The skills an agent gets from the apps it can use: the
//! lowest tier (agent > workspace > organization > app).
//!
//! **One merge function** ([`merge`]) is called by both the effective-skills route / agent skill
//! tools and the home projection, so what the agent is told and what it can read cannot drift. The
//! candidates come from the active registry snapshot for exactly the apps the agent's home lists
//! (an active connection with a grant, the app available to the organization and workspace); the
//! content is read by digest from the package blobs and never interpreted beyond `SKILL.md`'s
//! frontmatter (the shared parser).

use std::collections::BTreeMap;

use aurora_core::ApiError;
use aurora_registry::snapshot::{AppEntry, SkillEntry, Snapshot};
use uuid::Uuid;

/// A skill of one app, with what the agent-facing text needs about the app.
#[derive(Debug, Clone)]
pub struct AppSkill {
    pub app_id: String,
    pub app_name: String,
    /// The package version the skill comes from.
    pub version: String,
    pub skill: SkillEntry,
}

/// Why an app skill is not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum By {
    /// An enabled skill of another tier has the name (`agent` | `workspace` | `org`); `id` is that skill.
    Tier {
        tier: &'static str,
        id: Option<Uuid>,
    },
    /// Another app's skill of the name wins (the app whose id is the longest prefix of the name).
    App(String),
}

/// The outcome of [`merge`].
#[derive(Debug, Default)]
pub struct Merged {
    /// The skills the agent gets, in name order.
    pub winners: Vec<AppSkill>,
    /// Every other candidate with the reason, in name order.
    pub shadowed: Vec<(AppSkill, By)>,
}

/// The skills of the apps in `app_ids` from the snapshot, in `(name, app_id)` order.
#[must_use]
pub fn candidates(snap: &Snapshot, app_ids: &[String]) -> Vec<AppSkill> {
    let mut out = Vec::new();
    for app in snap.apps.iter().filter(|a| app_ids.contains(&a.app_id)) {
        let version = version_of(snap, app);
        for s in app.agent.iter().flat_map(|a| &a.skills) {
            out.push(AppSkill {
                app_id: app.app_id.clone(),
                app_name: app.about.name.clone(),
                version: version.clone(),
                skill: s.clone(),
            });
        }
    }
    out.sort_by(|a, b| (&a.skill.name, &a.app_id).cmp(&(&b.skill.name, &b.app_id)));
    out
}

fn version_of(snap: &Snapshot, app: &AppEntry) -> String {
    snap.packages
        .iter()
        .find(|p| p.package_id == app.package_id)
        .map(|p| p.version.clone())
        .unwrap_or_default()
}

/// How well `app_id` owns `name`: its length when the name equals it or continues it with `-`,
/// else 0 (C-1133: the longest prefix wins between apps).
fn prefix_score(app_id: &str, name: &str) -> usize {
    if name == app_id
        || name
            .strip_prefix(app_id)
            .is_some_and(|rest| rest.starts_with('-'))
    {
        app_id.len()
    } else {
        0
    }
}

/// The one merge (C-1133). `taken` maps a name to the enabled skill of another tier that has it
/// (`tier`, `id`): such a name's app skills are all shadowed by it (a disabled skill is not in
/// `taken`, so it never vetoes). Two app skills of one name: the app whose id is the longest
/// prefix of the name wins and the other is shadowed by it.
#[must_use]
pub fn merge(
    candidates: Vec<AppSkill>,
    taken: &BTreeMap<String, (&'static str, Option<Uuid>)>,
) -> Merged {
    let mut out = Merged::default();
    // Grouping below takes adjacent candidates of one name, and the winners are documented in name
    // order: sort here with `candidates()`'s key (a no-op for its output) so any input is grouped and
    // ordered right; unsorted, one name could split into two groups with two winners.
    let mut candidates = candidates;
    candidates.sort_by(|a, b| (&a.skill.name, &a.app_id).cmp(&(&b.skill.name, &b.app_id)));
    let mut it = candidates.into_iter().peekable();
    while let Some(first) = it.next() {
        let mut group = vec![first];
        while it
            .peek()
            .is_some_and(|n| n.skill.name == group[0].skill.name)
        {
            group.extend(it.next());
        }
        if let Some((tier, id)) = taken.get(&group[0].skill.name) {
            for g in group {
                out.shadowed.push((g, By::Tier { tier, id: *id }));
            }
            continue;
        }
        let name = group[0].skill.name.clone();
        let win = group
            .iter()
            .enumerate()
            .max_by_key(|(_, g)| {
                (
                    prefix_score(&g.app_id, &name),
                    std::cmp::Reverse(g.app_id.clone()),
                )
            })
            .map_or(0, |(i, _)| i);
        let winner = group.remove(win);
        for g in group {
            out.shadowed.push((g, By::App(winner.app_id.clone())));
        }
        out.winners.push(winner);
    }
    out
}

/// The apps the agent's home lists: an active connection with a grant, the app available to the
/// agent's organization and workspace (the same set the home's `apps` part uses).
pub async fn granted_app_ids(
    state: &crate::app::AppState,
    conn: &mut sqlx::PgConnection,
    agent: &aurora_db::repo::agents::AgentRow,
) -> Result<Vec<String>, ApiError> {
    let conns = crate::domains::hosted::home::granted_connections(
        state,
        conn,
        agent.id,
        agent.org_id,
        agent.workspace_id,
    )
    .await?;
    let mut ids: Vec<String> = conns.into_iter().map(|c| c.app_id).collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// An agent's app skills next to its database ones (C-1133).
#[derive(Debug, Default)]
pub struct AppEffective {
    pub items: Vec<AppSkill>,
    pub shadowed: Vec<(AppSkill, By)>,
}

/// The app skills of `agent` merged under the database tiers of `eff` (the one [`merge`]).
pub async fn effective_of(
    state: &crate::app::AppState,
    agent: &aurora_db::repo::agents::AgentRow,
    eff: &super::service::Effective,
) -> Result<AppEffective, ApiError> {
    let Some(snap) = crate::domains::plugins::registry::active_typed(state).await? else {
        return Ok(AppEffective::default());
    };
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let ids = granted_app_ids(state, &mut conn, agent).await?;
    let taken: BTreeMap<String, (&'static str, Option<Uuid>)> = eff
        .items
        .iter()
        .map(|i| {
            (
                i.view.row.name.clone(),
                (super::service::tier_of(&i.view.row), Some(i.view.row.id)),
            )
        })
        .collect();
    let m = merge(candidates(&snap, &ids), &taken);
    Ok(AppEffective {
        items: m.winners,
        shadowed: m.shadowed,
    })
}

/// `SKILL.md` as shipped, read by digest; `None` when the blob is missing.
pub async fn skill_md(
    conn: &mut sqlx::PgConnection,
    s: &AppSkill,
) -> Result<Option<Vec<u8>>, ApiError> {
    Ok(aurora_db::repo::plugins::get_agent_blob(&mut *conn, &s.skill.sha256).await?)
}

/// The body of the skill as the agent reads it (the shared parser's output), `None` when the
/// blob is missing or not a valid `SKILL.md`.
pub async fn body(conn: &mut sqlx::PgConnection, s: &AppSkill) -> Result<Option<String>, ApiError> {
    Ok(skill_md(conn, s)
        .await?
        .and_then(|raw| aurora_registry::skill_format::parse_skill_md(&raw, &s.skill.name).ok())
        .map(|(_, body)| body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sk(app: &str, name: &str) -> AppSkill {
        AppSkill {
            app_id: app.into(),
            app_name: app.into(),
            version: "1".into(),
            skill: SkillEntry {
                name: name.into(),
                description: "d".into(),
                sha256: "0".into(),
                size: 1,
                files: Vec::new(),
            },
        }
    }

    #[test]
    fn another_tier_wins_and_the_longest_prefix_app_wins_between_apps() {
        let cands = vec![
            sk("demo", "demo-x-thing"),
            sk("demo-x", "demo-x-thing"),
            sk("demo", "demo-own"),
            sk("demo", "demo-taken"),
        ];
        let mut taken = BTreeMap::new();
        taken.insert("demo-taken".to_owned(), ("workspace", None));
        let m = merge(cands, &taken);
        let w: Vec<(&str, &str)> = m
            .winners
            .iter()
            .map(|s| (s.app_id.as_str(), s.skill.name.as_str()))
            .collect();
        assert_eq!(w, [("demo", "demo-own"), ("demo-x", "demo-x-thing")]);
        assert!(m.shadowed.iter().any(|(s, b)| s.app_id == "demo"
            && s.skill.name == "demo-x-thing"
            && *b == By::App("demo-x".into())));
        assert!(m.shadowed.iter().any(|(s, b)| s.skill.name == "demo-taken"
            && *b
                == (By::Tier {
                    tier: "workspace",
                    id: None
                })));
    }
}
