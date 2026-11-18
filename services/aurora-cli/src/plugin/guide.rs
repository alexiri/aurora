//! `aurora plugin guide` (docs/24 §10, C-649): render what an agent would get for a sample account,
//! with the same renderer the API uses for the home sync.

use super::{cmds, pack};
use anyhow::{Context, Result, anyhow, bail};
use aurora_registry::agent_guide::{GrantAccess, GuideAccount, project_file, render};
use aurora_registry::snapshot::{
    AppEntry, ClaimStatus, HostEntry, Snapshot, compile_with_guides, guide_digests,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Sample connection id printed in the preview.
const SAMPLE_ID: &str = "c_sample";

fn sample_account(app: &AppEntry, snap: &Snapshot, access: &str) -> GuideAccount {
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
    GuideAccount {
        id: SAMPLE_ID.to_owned(),
        label: "Sample account".to_owned(),
        host,
        service: app.database.as_ref().map(|_| "sample".to_owned()),
        access,
    }
}

/// The files an agent would find in `.aurora/apps/<app_id>/`, as `(file name, text)`.
fn files_of(
    app: &AppEntry,
    snap: &Snapshot,
    pkg: &aurora_registry::Package,
    version: &str,
    access: &str,
) -> Result<Vec<(String, String)>> {
    let lost: Vec<&HostEntry> = snap
        .hosts
        .iter()
        .filter(|h| h.app_id == app.app_id && h.claim_status == ClaimStatus::Lost)
        .collect();
    let account = sample_account(app, snap, access);
    let r = render(app, &lost, &[account], version, true);
    let mut out = vec![
        ("guide.md".to_owned(), r.guide_md),
        ("tools.md".to_owned(), r.tools_md),
    ];
    if let Some(ag) = &app.agent {
        for g in &ag.guide {
            let path = format!("agent/{}/{}", app.app_id, g.file);
            let bytes = pkg
                .file(&path)
                .ok_or_else(|| anyhow!("{path} is listed but missing"))?;
            let text = String::from_utf8_lossy(bytes);
            out.push((
                g.file.clone(),
                project_file(&app.about.name, version, g, &text),
            ));
        }
    }
    Ok(out)
}

pub fn guide(path: &Path, app: Option<&str>, access: &str, out: Option<&Path>) -> Result<i32> {
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
    let guides = BTreeMap::from([(manifest.package.id.clone(), guide_digests(&manifest, &pkg))]);
    let version = manifest.package.version.clone();
    let active = cmds::active(&pkg, cmds::CORE_VERSION)?;
    let c = compile_with_guides(&[active], &[], &[], cmds::CONTRACT, &guides)
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
        for (name, text) in files_of(a, &snap, &pkg, &version, access)? {
            match out {
                Some(dir) => {
                    let d = dir.join(&a.app_id);
                    std::fs::create_dir_all(&d).with_context(|| d.display().to_string())?;
                    std::fs::write(d.join(&name), text).with_context(|| name.clone())?;
                }
                None => println!("==> {}/{name} <==\n{text}", a.app_id),
            }
        }
    }
    if let Some(dir) = out {
        println!("wrote {}", dir.display());
    }
    Ok(0)
}
