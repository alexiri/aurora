//! `aurora plugin preview` (docs/24 §10, C-1138; replaces `aurora plugin guide`): render what an
//! agent would get for a sample account, with the same renderers the API uses for the home sync:
//! the files Aurora writes into `apps/<app_id>/` and the app's skills as projected into the
//! skills folder, provenance line and file modes included.

use super::{cmds, pack};
use anyhow::{Context, Result, anyhow, bail};
use aurora_registry::app_folder::{
    AppAccount, AppSkills, GrantAccess, SkillLine, project_skill_md, render_with_operations,
};
use aurora_registry::snapshot::{
    AppEntry, ClaimStatus, HostEntry, Snapshot, agent_files, compile_with_agent_files,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The skills folder the preview shows (the claude-code one; a real home uses the harness's).
const SAMPLE_SKILLS_DIR: &str = ".claude/skills";

/// Sample connection id printed in the preview.
const SAMPLE_ID: &str = "c_sample";

fn sample_account(app: &AppEntry, snap: &Snapshot, access: &str) -> AppAccount {
    let access = if access == "full" {
        GrantAccess::Full
    } else {
        // `custom`: read tools allowed, write tools need approval, nothing else (C-649).
        let mut allow = BTreeSet::new();
        let mut ask = BTreeSet::new();
        if let Some(t) = &app.tools {
            for i in t.read.iter().flat_map(|g| &g.items) {
                allow.insert(i.id.clone());
            }
            for i in t.write.iter().flat_map(|g| &g.items) {
                ask.insert(i.id.clone());
            }
        }
        GrantAccess::Custom { allow, ask }
    };
    let host = app
        .instances
        .as_ref()
        .and_then(|i| i.default_host.clone())
        .or_else(|| {
            snap.hosts
                .iter()
                .find(|h| h.app_id == app.app_id)
                .map(|h| h.host.clone())
        });
    AppAccount {
        id: SAMPLE_ID.to_owned(),
        label: "Sample account".to_owned(),
        host,
        service: app.database.as_ref().map(|_| "sample".to_owned()),
        access,
        webhook: None,
    }
}

/// One file of the preview: its path relative to the preview root, the text, and whether it is
/// projected executable.
struct Out {
    path: String,
    text: String,
    exec: bool,
}

/// The files an agent would find for `app`: `apps/<app_id>/…` and `skills/<name>/…`.
fn files_of(
    app: &AppEntry,
    snap: &Snapshot,
    pkg: &aurora_registry::Package,
    version: &str,
    access: &str,
) -> Result<Vec<Out>> {
    let lost: Vec<&HostEntry> = snap
        .hosts
        .iter()
        .filter(|h| h.app_id == app.app_id && h.claim_status == ClaimStatus::Lost)
        .collect();
    let account = sample_account(app, snap, access);
    // (P5-GUIDE-6, C-649) The operations file feeds `api.json`, as in the home sync.
    let operations = match app.agent.as_ref().and_then(|a| a.operations.as_ref()) {
        Some(r) => {
            let path = format!("agent/{}/{}", app.app_id, r.file);
            let bytes = pkg
                .file(&path)
                .ok_or_else(|| anyhow!("{path} is named by agent.operations but missing"))?;
            Some(
                aurora_registry::operations::parse(bytes)
                    .map_err(|e| anyhow!("{path}: {}", e.message()))?,
            )
        }
        None => None,
    };
    // The sample agent has no skills of its own, so every app skill is projected (the claude-code
    // folder stands for the harness).
    let skills = AppSkills {
        lines: app
            .agent
            .iter()
            .flat_map(|a| &a.skills)
            .map(|s| SkillLine {
                name: s.name.clone(),
                description: s.description.clone(),
                shadowed_by: None,
            })
            .collect(),
        left_out: false,
        skills_dir: SAMPLE_SKILLS_DIR.to_owned(),
    };
    let r = render_with_operations(
        app,
        &lost,
        &[account],
        version,
        &skills,
        operations.as_ref(),
    );
    let apps_dir = format!("apps/{}", app.app_id);
    let mut out = vec![
        Out {
            path: format!("{apps_dir}/app.md"),
            text: r.app_md,
            exec: false,
        },
        Out {
            path: format!("{apps_dir}/tools.md"),
            text: r.tools_md,
            exec: false,
        },
    ];
    if let Some(api) = r.api_json {
        out.push(Out {
            path: format!("{apps_dir}/api.json"),
            text: api,
            exec: false,
        });
    }
    for s in app.agent.iter().flat_map(|a| &a.skills) {
        let md_path = format!("skills/{}/{}/SKILL.md", app.app_id, s.name);
        let md = pkg
            .file(&md_path)
            .ok_or_else(|| anyhow!("{md_path} is missing"))?;
        let text = project_skill_md(&app.about.name, version, &s.name, md)
            .ok_or_else(|| anyhow!("{md_path}: not a valid SKILL.md"))?;
        out.push(Out {
            path: format!("skills/{}/SKILL.md", s.name),
            text,
            exec: false,
        });
        for f in &s.files {
            let p = format!("skills/{}/{}/{}", app.app_id, s.name, f.path);
            let bytes = pkg.file(&p).ok_or_else(|| anyhow!("{p} is missing"))?;
            out.push(Out {
                path: format!("skills/{}/{}", s.name, f.path),
                text: String::from_utf8_lossy(bytes).into_owned(),
                exec: f.executable,
            });
        }
    }
    Ok(out)
}

#[cfg(unix)]
fn set_mode(p: &Path, exec: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // The supervisor projects 0555 with the executable bit and 0444 without (C-1134).
    let mode = if exec { 0o555 } else { 0o444 };
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
        .with_context(|| p.display().to_string())
}

#[cfg(not(unix))]
fn set_mode(_p: &Path, _exec: bool) -> Result<()> {
    Ok(())
}

pub fn preview(path: &Path, app: Option<&str>, access: &str, out: Option<&Path>) -> Result<i32> {
    let pkg = pack::load(path)?;
    let manifest = pkg.manifest(cmds::CORE_VERSION).map_err(|v| {
        anyhow!(
            "manifest invalid: {}",
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    let files = BTreeMap::from([(manifest.package.id.clone(), agent_files(&manifest, &pkg))]);
    let version = manifest.package.version.clone();
    let active = cmds::active(&pkg, cmds::CORE_VERSION)?;
    let c = compile_with_agent_files(&[active], &[], &[], cmds::CONTRACT, &files)
        .map_err(|e| anyhow!("compile: {e}"))?;
    let snap = Snapshot::from_slice(&c.json)?;
    let apps: Vec<&AppEntry> = snap
        .apps
        .iter()
        .filter(|a| app.is_none_or(|id| a.app_id == id))
        .collect();
    if apps.is_empty() {
        bail!(
            "no app{} in this package",
            app.map(|a| format!(" `{a}`")).unwrap_or_default()
        );
    }
    for a in apps {
        for f in files_of(a, &snap, &pkg, &version, access)? {
            match out {
                Some(dir) => {
                    let target = dir.join(&f.path);
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)
                            .with_context(|| parent.display().to_string())?;
                    }
                    // A read-only file from an earlier preview is replaced, not appended to.
                    let _ = std::fs::remove_file(&target);
                    std::fs::write(&target, &f.text).with_context(|| f.path.clone())?;
                    set_mode(&target, f.exec)?;
                }
                None => println!(
                    "==> {}{} <==\n{}",
                    f.path,
                    if f.exec { " (executable)" } else { "" },
                    f.text
                ),
            }
        }
    }
    if let Some(dir) = out {
        println!("wrote {}", dir.display());
    }
    Ok(0)
}
