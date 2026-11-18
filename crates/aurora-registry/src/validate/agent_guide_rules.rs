//! (P5-GUIDE-1, `docs/24 §3`, C-640) Agent guide rules: the manifest keys (`agent.summary`,
//! `agent.guide[]`) and the archive files under `agent/<app_id>/`.
use std::collections::{BTreeMap, BTreeSet};

use aurora_core::limits::{
    AGENT_GUIDE_APP_MAX, AGENT_GUIDE_FILE_MAX, AGENT_GUIDE_FILES_MAX, AGENT_SUMMARY_MAX,
};

use crate::Violation;
use crate::manifest::{Agent, Manifest};
use crate::package::{AGENT_DIR, Package};

/// Names the core writes into the app's folder; a package cannot ship them.
pub const RESERVED_NAMES: [&str; 3] = ["guide.md", "tools.md", "api.json"];
const EXTENSIONS: [&str; 6] = ["md", "txt", "json", "yaml", "tsv", "csv"];
const TITLE_MAX: usize = 120;

/// `[a-z0-9][a-z0-9_-]{0,47}\.(md|txt|json|yaml|tsv|csv)` (C-640).
#[must_use]
pub fn file_name_ok(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    EXTENSIONS.contains(&ext)
        && !stem.is_empty()
        && stem.len() <= 48
        && stem.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && (b == b'_' || b == b'-'))
        })
}

/// UTF-8 text a guide file may hold: no NUL, no control character other than LF and TAB (CR
/// included), no byte-order mark (C-640).
#[must_use]
pub fn is_guide_text(bytes: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(bytes) else {
        return false;
    };
    !s.starts_with('\u{feff}') && s.chars().all(|c| c == '\n' || c == '\t' || !c.is_control())
}

/// Manifest-only rules for one app's `agent` block (`base` is `/apps/<i>`).
pub fn manifest_rules(ag: &Agent, base: &str, out: &mut Vec<Violation>) {
    let p = |s: &str| format!("{base}/agent{s}");
    if let Some(s) = &ag.summary {
        if s.chars().any(char::is_control) {
            out.push(Violation::new(
                "agent_summary_multiline",
                p("/summary"),
                "summary must be one line without control characters",
            ));
        }
        if s.chars().count() > AGENT_SUMMARY_MAX {
            out.push(Violation::new(
                "agent_summary_too_long",
                p("/summary"),
                format!("summary is longer than {AGENT_SUMMARY_MAX} characters"),
            ));
        }
    }
    if !ag.guide.is_empty() && ag.summary.as_deref().is_none_or(|s| s.trim().is_empty()) {
        out.push(Violation::new(
            "agent_summary_required",
            p("/summary"),
            "agent.summary is required when agent.guide is present",
        ));
    }
    if ag.guide.len() > AGENT_GUIDE_FILES_MAX {
        out.push(Violation::new(
            "agent_guide_too_many_files",
            p("/guide"),
            format!("more than {AGENT_GUIDE_FILES_MAX} guide files"),
        ));
    }
    let mut seen = BTreeSet::new();
    for (i, g) in ag.guide.iter().enumerate() {
        let at = |s: &str| p(&format!("/guide/{i}{s}"));
        if RESERVED_NAMES.contains(&g.file.as_str()) {
            out.push(Violation::new(
                "agent_guide_reserved_name",
                at("/file"),
                format!("`{}` is written by Aurora", g.file),
            ));
        } else if !file_name_ok(&g.file) {
            out.push(Violation::new(
                "agent_guide_file_name",
                at("/file"),
                format!(
                    "`{}` does not match [a-z0-9][a-z0-9_-]{{0,47}}.(md|txt|json|yaml|tsv|csv)",
                    g.file
                ),
            ));
        }
        if !seen.insert(g.file.as_str()) {
            out.push(Violation::new(
                "agent_guide_duplicate_file",
                at("/file"),
                format!("`{}` is listed twice", g.file),
            ));
        }
        let n = g.title.chars().count();
        if n == 0
            || n > TITLE_MAX
            || g.title.chars().any(char::is_control)
            || g.title.trim().is_empty()
        {
            out.push(Violation::new(
                "agent_guide_title",
                at("/title"),
                format!("title must be one line of 1 to {TITLE_MAX} characters"),
            ));
        }
        match g.kind.as_deref() {
            None | Some("reference") => {}
            Some("recipe") => {
                if !g.file.ends_with(".md") {
                    out.push(Violation::new(
                        "agent_recipe_not_markdown",
                        at("/file"),
                        "a recipe must be a .md file",
                    ));
                }
            }
            Some(other) => out.push(Violation::new(
                "agent_guide_kind",
                at("/kind"),
                format!("`{other}` is not a guide kind (reference | recipe)"),
            )),
        }
        if g.sha256.is_some() || g.size.is_some() {
            out.push(Violation::new(
                "agent_guide_computed_field",
                at(""),
                "sha256 and size are computed by the core and exist only in the snapshot",
            ));
        }
    }
}

/// Archive rules: every file under `agent/<app_id>/` is listed, every listed file exists, the app
/// is one of the package's, content is bounded UTF-8 text.
pub fn archive_rules(m: &Manifest, pkg: &Package, out: &mut Vec<Violation>) {
    let apps: BTreeMap<&str, BTreeSet<&str>> = m
        .apps
        .iter()
        .map(|a| {
            (
                a.about.id.as_str(),
                a.agent
                    .as_ref()
                    .map(|ag| ag.guide.iter().map(|g| g.file.as_str()).collect())
                    .unwrap_or_default(),
            )
        })
        .collect();
    let mut per_app: BTreeMap<&str, usize> = BTreeMap::new();
    for (path, bytes) in &pkg.files {
        let Some(rest) = path.strip_prefix(AGENT_DIR) else {
            continue;
        };
        let Some((app, file)) = rest.split_once('/') else {
            continue;
        };
        let Some(listed) = apps.get(app) else {
            out.push(Violation::new(
                "agent_guide_unknown_app",
                path.clone(),
                format!("`{app}` is not an app of this package"),
            ));
            continue;
        };
        if !listed.contains(file) {
            out.push(Violation::new(
                "agent_guide_unlisted_file",
                path.clone(),
                "file is not listed in agent.guide of its app",
            ));
        }
        if !is_guide_text(bytes) {
            out.push(Violation::new(
                "agent_guide_not_text",
                path.clone(),
                "guide files are UTF-8 text without NUL, CR or other control characters and without a byte-order mark",
            ));
        }
        if bytes.len() > AGENT_GUIDE_FILE_MAX {
            out.push(Violation::new(
                "agent_guide_file_too_large",
                path.clone(),
                format!("file exceeds {AGENT_GUIDE_FILE_MAX} bytes"),
            ));
        }
        *per_app.entry(app).or_default() += bytes.len();
    }
    for (app, n) in per_app {
        if n > AGENT_GUIDE_APP_MAX {
            out.push(Violation::new(
                "agent_guide_app_too_large",
                format!("{AGENT_DIR}{app}"),
                format!("guide files of `{app}` exceed {AGENT_GUIDE_APP_MAX} bytes in total"),
            ));
        }
    }
    for (i, a) in m.apps.iter().enumerate() {
        let Some(ag) = &a.agent else { continue };
        for (j, g) in ag.guide.iter().enumerate() {
            let path = format!("{AGENT_DIR}{}/{}", a.about.id, g.file);
            if !pkg.files.contains_key(&path) {
                out.push(Violation::new(
                    "agent_guide_missing_file",
                    format!("/apps/{i}/agent/guide/{j}/file"),
                    format!("`{path}` is listed but absent from the archive"),
                ));
            }
        }
    }
}
