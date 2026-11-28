//! (P5-APPSKILL-1, C-1130, D-453; replaces P5-GUIDE-1's guide rules) What an app's `agent` block
//! and the archive files under `agent/<app_id>/` and `skills/<app_id>/<name>/` may contain.
//!
//! The skills go through the one shared validator (`crate::skill_format`), the same rules the API
//! applies to a skill-library skill. `agent.guide` and every file under `agent/<app_id>/` other
//! than the operations file are *ignored* (a warning, [`ignored_warnings`], never an error): an
//! old package still ingests and is served, and nothing of them reaches an agent.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use aurora_core::limits::{
    AGENT_SUMMARY_MAX, APP_SKILLS_APP_MAX, APP_SKILLS_MAX, LIBRARY_FILE_MAX,
};

use crate::Violation;
use crate::manifest::{Agent, Manifest};
use crate::package::{AGENT_DIR, Package, SKILLS_DIR};
use crate::skill_format::{self, Kind, NameError};

/// Names the core writes into the app's folder; the operations file cannot take them.
pub const RESERVED_NAMES: [&str; 4] = ["guide.md", "app.md", "tools.md", "api.json"];
/// (P5-GUIDE-6, C-640) The app id of the platform target of `aurora-agent`.
pub const RESERVED_APP_ID: &str = "aurora";

/// `[a-z0-9][a-z0-9_-]{0,47}.json` (the operations file name, C-640).
#[must_use]
pub fn operations_name_ok(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    !stem.is_empty()
        && stem.len() <= 48
        && stem.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && (b == b'_' || b == b'-'))
        })
}

/// UTF-8 text the operations file may hold: no NUL, no control character other than LF and TAB (CR
/// included), no byte-order mark.
#[must_use]
pub fn is_operations_text(bytes: &[u8]) -> bool {
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
    if ag.operations.is_some() && ag.summary.as_deref().is_none_or(|s| s.trim().is_empty()) {
        out.push(Violation::new(
            "agent_summary_required",
            p("/summary"),
            "agent.summary is required when agent.operations or skills are present",
        ));
    }
    if let Some(f) = &ag.operations {
        if RESERVED_NAMES.contains(&f.as_str()) {
            out.push(Violation::new(
                "agent_operations_reserved_name",
                p("/operations"),
                format!("`{f}` is written by Aurora"),
            ));
        } else if !operations_name_ok(f) {
            out.push(Violation::new(
                "agent_operations_file_name",
                p("/operations"),
                format!("`{f}` must match [a-z0-9][a-z0-9_-]{{0,47}}.json"),
            ));
        }
    }
}

/// `skills/<app>/<name>/<rel>` split into its parts.
fn split_skill(path: &str) -> Option<(&str, &str, &str)> {
    let rest = path.strip_prefix(SKILLS_DIR)?;
    let (app, rest) = rest.split_once('/')?;
    let (name, rel) = rest.split_once('/')?;
    Some((app, name, rel))
}

/// The app of the package whose id is the longest prefix of `name` (the name equals the id or
/// continues it with `-`): `acme-eu-x` belongs to `acme-eu`, not to `acme`.
fn owner_app<'a>(app_ids: &BTreeSet<&'a str>, name: &str) -> Option<&'a str> {
    app_ids
        .iter()
        .copied()
        .filter(|id| {
            name == *id
                || name
                    .strip_prefix(*id)
                    .is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|id| id.len())
}

#[derive(Default)]
struct SkillFiles<'a> {
    skill_md: Option<&'a [u8]>,
    files: Vec<(&'a str, &'a [u8])>,
}

/// Archive rules: the operations file of every app exists and passes its own linter; every file
/// under `agent/<app_id>/` belongs to an app of the package; every skill under
/// `skills/<app_id>/<name>/` passes the shared skill validator plus the package rules (name
/// prefix, text only, per-app counts and sizes).
///
/// (FIX-221, D-465) `others` maps the app ids of the other active packages to their package id;
/// a skill whose longest-prefix owner is one of those is `app_skill_name_claimed`. Ids this same
/// package owns (its own old version, on an upgrade) and ids of its own apps are not "other".
pub fn archive_rules(
    m: &Manifest,
    pkg: &Package,
    others: &HashMap<String, String>,
    out: &mut Vec<Violation>,
) {
    let app_ids: BTreeSet<&str> = m.apps.iter().map(|a| a.about.id.as_str()).collect();
    let foreign: BTreeMap<&str, &str> = others
        .iter()
        .filter(|(id, owner)| **owner != m.package.id && !app_ids.contains(id.as_str()))
        .map(|(id, owner)| (id.as_str(), owner.as_str()))
        .collect();
    let all_ids: BTreeSet<&str> = app_ids
        .iter()
        .copied()
        .chain(foreign.keys().copied())
        .collect();
    let operations_of = |app: &str, file: &str| {
        m.apps
            .iter()
            .find(|a| a.about.id == app)
            .filter(|a| a.agent.as_ref().and_then(|g| g.operations.as_deref()) == Some(file))
    };
    // ---- agent/<app_id>/<file>: the operations file only; the rest is ignored (a warning).
    for (path, bytes) in &pkg.files {
        let Some(rest) = path.strip_prefix(AGENT_DIR) else {
            continue;
        };
        let Some((app, file)) = rest.split_once('/') else {
            continue;
        };
        if !app_ids.contains(app) {
            out.push(Violation::new(
                "agent_unknown_app",
                path.clone(),
                format!("`{app}` is not an app of this package"),
            ));
            continue;
        }
        if let Some(owner) = operations_of(app, file) {
            // The operations file has its own size limit and linter (C-651).
            if is_operations_text(bytes) {
                super::agent_operations_rules::rules(owner, path, bytes, out);
            } else {
                out.push(Violation::new(
                    "agent_operations_not_text",
                    path.clone(),
                    "the operations file is UTF-8 text without NUL, CR or other control characters and without a byte-order mark",
                ));
            }
        }
    }
    for (i, a) in m.apps.iter().enumerate() {
        let Some(ag) = &a.agent else { continue };
        if let Some(f) = &ag.operations {
            let path = format!("{AGENT_DIR}{}/{f}", a.about.id);
            if !pkg.files.contains_key(&path) {
                out.push(Violation::new(
                    "agent_operations_missing_file",
                    format!("/apps/{i}/agent/operations"),
                    format!("`{path}` is named by agent.operations but absent from the archive"),
                ));
            }
        }
    }
    // ---- skills/<app_id>/<name>/<path>
    let mut skills: BTreeMap<(&str, &str), SkillFiles<'_>> = BTreeMap::new();
    for (path, bytes) in &pkg.files {
        if !path.starts_with(SKILLS_DIR) {
            continue;
        }
        let Some((app, name, rel)) = split_skill(path) else {
            out.push(Violation::new(
                "app_skill_path",
                path.clone(),
                "a skill file is `skills/<app_id>/<name>/<path>`",
            ));
            continue;
        };
        if !app_ids.contains(app) {
            out.push(Violation::new(
                "app_skill_unknown_app",
                path.clone(),
                format!("`{app}` is not an app of this package"),
            ));
            continue;
        }
        let entry = skills.entry((app, name)).or_default();
        if rel == "SKILL.md" {
            entry.skill_md = Some(bytes);
        } else {
            entry.files.push((rel, bytes));
        }
        if !skill_format::is_text(bytes) {
            out.push(Violation::new(
                "app_skill_not_text",
                path.clone(),
                "skill files are UTF-8 text without NUL and without a byte-order mark",
            ));
        }
    }
    let mut per_app: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for ((app, name), sk) in &skills {
        let at = format!("{SKILLS_DIR}{app}/{name}");
        match skill_format::check_name(name) {
            Err(NameError::Grammar) => out.push(Violation::new(
                "app_skill_name",
                at.clone(),
                "a skill name is lowercase letters and digits joined by single hyphens, at most 64 characters",
            )),
            Err(NameError::Reserved) => out.push(Violation::new(
                "app_skill_name",
                at.clone(),
                "the aurora- prefix is reserved",
            )),
            Ok(()) => {
                let owner = owner_app(&all_ids, name);
                if let Some(o) = owner.filter(|o| foreign.contains_key(o)) {
                    out.push(Violation::new(
                        "app_skill_name_claimed",
                        at.clone(),
                        format!(
                            "`{name}` starts with the id of the app `{o}` (package `{}`); a skill of `{app}` must be named `{app}` or `{app}-…` and not start with another app's id",
                            foreign[o]
                        ),
                    ));
                } else if owner != Some(*app) {
                    out.push(Violation::new(
                        "app_skill_name",
                        at.clone(),
                        format!(
                            "a skill of `{app}` is named `{app}` or starts with `{app}-` (and belongs to the longest matching app id)"
                        ),
                    ));
                }
            }
        }
        let Some(md) = sk.skill_md else {
            out.push(Violation::new(
                "app_skill_frontmatter",
                format!("{at}/SKILL.md"),
                "SKILL.md is missing",
            ));
            continue;
        };
        let counts = per_app.entry(app).or_default();
        counts.0 += 1;
        counts.1 += md.len() + sk.files.iter().map(|(_, b)| b.len()).sum::<usize>();
        // Per-file size and path rules for the extra files, then the skill's own.
        let mut ok_files: Vec<(&str, &[u8])> = Vec::new();
        for (rel, bytes) in &sk.files {
            let file_at = format!("{at}/{rel}");
            if !skill_format::path_ok(rel) {
                out.push(Violation::new(
                    "app_skill_path",
                    file_at,
                    "a skill file path is [A-Za-z0-9._/-], at most 4 segments and 200 characters, no `.` or `..`, and no segment named skill.md",
                ));
            } else if bytes.len() > LIBRARY_FILE_MAX {
                out.push(Violation::new(
                    "app_skill_file_too_large",
                    file_at,
                    format!("a skill file is at most {LIBRARY_FILE_MAX} bytes"),
                ));
            } else {
                ok_files.push((rel, bytes));
            }
        }
        if md.len() > LIBRARY_FILE_MAX {
            out.push(Violation::new(
                "app_skill_file_too_large",
                format!("{at}/SKILL.md"),
                format!("SKILL.md is at most {LIBRARY_FILE_MAX} bytes"),
            ));
            continue;
        }
        if !skill_format::is_text(md) {
            continue; // already reported as app_skill_not_text
        }
        let (description, body) = match skill_format::parse_skill_md_typed(md, name) {
            Ok(x) => x,
            Err(e) => {
                out.push(Violation::new(
                    code_of(e.kind),
                    format!("{at}/SKILL.md"),
                    e.message,
                ));
                continue;
            }
        };
        if let Err(e) =
            skill_format::check_content(name, &description, &body, &ok_files, LIBRARY_FILE_MAX)
        {
            out.push(Violation::new(code_of(e.kind), at.clone(), e.message));
        }
    }
    for (app, (count, bytes)) in per_app {
        if count > APP_SKILLS_MAX {
            out.push(Violation::new(
                "app_skills_too_many",
                format!("{SKILLS_DIR}{app}"),
                format!("`{app}` has more than {APP_SKILLS_MAX} skills"),
            ));
        }
        if bytes > APP_SKILLS_APP_MAX {
            out.push(Violation::new(
                "app_skills_app_too_large",
                format!("{SKILLS_DIR}{app}"),
                format!("the skills of `{app}` exceed {APP_SKILLS_APP_MAX} bytes in total"),
            ));
        }
    }
    // An app with skills needs its one-line summary (C-1130).
    for (i, a) in m.apps.iter().enumerate() {
        let has_skills = skills.keys().any(|(app, _)| *app == a.about.id);
        let summary_ok = a
            .agent
            .as_ref()
            .and_then(|g| g.summary.as_deref())
            .is_some_and(|s| !s.trim().is_empty());
        if has_skills && !summary_ok {
            out.push(Violation::new(
                "agent_summary_required",
                format!("/apps/{i}/agent/summary"),
                "agent.summary is required when the app ships skills",
            ));
        }
    }
}

const fn code_of(k: Kind) -> &'static str {
    match k {
        Kind::Frontmatter | Kind::DescriptionTooLong => "app_skill_frontmatter",
        Kind::BodyTooLarge => "app_skill_body_too_large",
        Kind::Path => "app_skill_path",
        Kind::TooManyFiles => "app_skill_too_many_files",
        Kind::FileTooLarge => "app_skill_file_too_large",
        Kind::TooLarge => "app_skill_too_large",
    }
}

/// (C-1130) `agent_guide_ignored`, once per app: the manifest still lists `agent.guide` entries,
/// or the archive still carries a file under `agent/<app_id>/` other than the operations file.
/// Warnings never reject a package. `pkg` is `None` for a manifest-only check.
#[must_use]
pub fn ignored_warnings(m: &Manifest, pkg: Option<&Package>) -> Vec<Violation> {
    let mut out = Vec::new();
    for (i, a) in m.apps.iter().enumerate() {
        let listed = a.agent.as_ref().is_some_and(|g| !g.guide.is_empty());
        let ops = a.agent.as_ref().and_then(|g| g.operations.as_deref());
        let prefix = format!("{AGENT_DIR}{}/", a.about.id);
        let stray = pkg.is_some_and(|p| {
            p.files.keys().any(|k| {
                k.strip_prefix(&prefix)
                    .is_some_and(|file| Some(file) != ops)
            })
        });
        if listed || stray {
            out.push(Violation::new(
                "agent_guide_ignored",
                format!("/apps/{i}/agent/guide"),
                "agent.guide and guide files are ignored: ship skills under skills/<app_id>/ instead (D-453)",
            ));
        }
    }
    out
}
