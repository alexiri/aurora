//! Home projection rendering (docs/12 §Home projection / §Sync protocol). Produces exactly the
//! entry shapes documented in `services/aurora-supervisor/src/home.rs`:
//! `skills {path, content, exec?}`, `memory {path, content}`, `render_inputs` objects keyed by
//! the supervisor's `RenderInputs` fields, `tools` (catalogue entries), `manifest {path}`.
//! No credential is ever rendered (SEC-01).

use crate::app::AppState;
use aurora_core::{ApiError, limits};
use aurora_db::repo::hosted::{self, HomeSyncClaim};
use aurora_protocol::runner::{HomeSync, HomeSyncKind, HomeSyncPart};
use serde_json::{Value, json};
use sqlx::PgConnection;

/// Parts are packed below the wire maximum to leave room for the envelope and escaping drift.
const PACK_MAX: usize = limits::HOME_SYNC_PART_MAX - 2048;

/// Home-relative skills dir declared by the harness adapters (`12 §Harness interface`).
pub fn skills_dir(_harness_id: Option<&str>) -> &'static str {
    ".claude/skills"
}

fn clamp(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn index_line(m: &hosted::MemoryRow) -> String {
    let line = m
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
        .or(m.title.as_deref().filter(|t| !t.trim().is_empty()))
        .or_else(|| m.content.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("");
    let one: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{}: {}", m.key, clamp(&one, 120))
}

/// `key: line` per memory, bounded by `MEMORY_INDEX_MAX` (also the turn-start recall index).
pub fn memory_index(rows: &[hosted::MemoryRow]) -> String {
    let mut out = String::new();
    for r in rows {
        let l = index_line(r);
        if out.chars().count() + l.chars().count() + 1 > limits::MEMORY_INDEX_MAX {
            break;
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}

fn memory_file(m: &hosted::MemoryRow) -> String {
    let mut fm = String::new();
    if let Some(t) = &m.title {
        fm.push_str(&format!("title: {}\n", yaml_str(t)));
    }
    if let Some(d) = &m.description {
        fm.push_str(&format!("description: {}\n", yaml_str(d)));
    }
    if fm.is_empty() {
        m.content.clone()
    } else {
        format!("---\n{fm}---\n{}", m.content)
    }
}

/// Greedy packing of entries into parts of one kind.
fn pack(kind: HomeSyncKind, entries: Vec<Value>, out: &mut Vec<HomeSyncPart>) {
    let mut cur: Vec<Value> = Vec::new();
    let mut size = 0usize;
    for e in entries {
        let n = e.to_string().len() + 1;
        if n > PACK_MAX {
            tracing::warn!(kind = %kind.as_str(), "home sync entry exceeds the part size; skipped");
            continue;
        }
        if size + n > PACK_MAX && !cur.is_empty() {
            out.push(HomeSyncPart {
                kind: kind.clone(),
                entries: std::mem::take(&mut cur),
            });
            size = 0;
        }
        size += n;
        cur.push(e);
    }
    if !cur.is_empty() {
        out.push(HomeSyncPart { kind, entries: cur });
    }
}

/// All `home.sync` parts of generation `claim.generation`, in order.
pub async fn render(
    state: &AppState,
    conn: &mut PgConnection,
    claim: &HomeSyncClaim,
) -> Result<Vec<HomeSync>, ApiError> {
    let inputs = hosted::home_inputs(conn, claim.agent_id).await?;
    let skills_root = skills_dir(claim.harness_id.as_deref());
    let mut manifest: Vec<Value> = Vec::new();

    let mut skill_entries = Vec::new();
    for s in hosted::skills(conn, inputs.org_id, claim.agent_id).await? {
        let path = format!("{skills_root}/{}/SKILL.md", s.name);
        let content = format!(
            "---\nname: {}\ndescription: {}\n---\n\n{}",
            s.name,
            yaml_str(&s.description),
            s.body
        );
        manifest.push(json!({"path": path}));
        skill_entries.push(json!({"path": path, "content": content}));
        for f in hosted::skill_files(conn, s.id).await? {
            let Ok(text) = String::from_utf8(f.content) else {
                tracing::warn!(skill = %s.name, "non-UTF-8 skill file not projected");
                continue;
            };
            let path = format!("{skills_root}/{}/{}", s.name, f.path);
            manifest.push(json!({"path": path}));
            skill_entries.push(json!({"path": path, "content": text, "exec": f.executable}));
        }
    }

    let mems = hosted::memories(conn, claim.agent_id).await?;
    let mut memory_entries = Vec::new();
    for m in &mems {
        let path = format!("memory/{}.md", m.key);
        manifest.push(json!({"path": path}));
        memory_entries.push(json!({"path": path, "content": memory_file(m)}));
    }
    manifest.push(json!({"path": "memory/index.md"}));
    memory_entries.push(json!({"path": "memory/index.md", "content": memory_index(&mems)}));

    let render_inputs = json!({
        "agent": {"name": inputs.name, "handle": inputs.handle, "workspace": inputs.workspace_name},
        "brief": if inputs.brief.trim().is_empty() { Value::Null } else { json!(inputs.brief) },
        "apps": [], "channels": [], "peers": [],
        "memory_language": inputs.memory_language,
        "schedules": null,
        "timezone": inputs.timezone,
    });
    let tools: Vec<Value> = crate::domains::tools::catalogue(&state.pool, claim.agent_id)
        .await?
        .iter()
        .filter_map(|t| serde_json::to_value(t).ok())
        .collect();

    let mut parts = Vec::new();
    pack(HomeSyncKind::Skills, skill_entries, &mut parts);
    pack(HomeSyncKind::Memory, memory_entries, &mut parts);
    parts.push(HomeSyncPart {
        kind: HomeSyncKind::RenderInputs,
        entries: vec![render_inputs],
    });
    pack(HomeSyncKind::Tools, tools, &mut parts);
    pack(HomeSyncKind::Manifest, manifest, &mut parts);
    parts.push(HomeSyncPart {
        kind: HomeSyncKind::Final,
        entries: vec![],
    });
    let count = u32::try_from(parts.len()).unwrap_or(u32::MAX);
    Ok(parts
        .into_iter()
        .enumerate()
        .map(|(i, part)| HomeSync {
            generation: u64::try_from(claim.generation).unwrap_or(0),
            part_index: u32::try_from(i).unwrap_or(u32::MAX),
            part_count: count,
            part,
        })
        .collect())
}
