//! The one validation both read forms share (C-1141): `read_tree` is the public tree form; the
//! archive form inflates first and lands here with the same `TreeFile`s. Names, modes and the
//! caps are checked before any file is parsed; every problem is collected and at most 20 come
//! back, sorted deterministically.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Duration, TimeZone, Utc};
use croner::parser::CronParser;
use serde::Deserialize;

use aurora_core::limits::{
    self, AVATAR_MAX, LIBRARY_FILE_MAX, MEMORY_CONTENT_MAX, SKILL_BODY_MAX, SKILL_DESCRIPTION_MAX,
};

use crate::meta::{self, parse_meta_str};
use crate::model::{
    Automation, MemoryFile, TemplateContent, TemplateFile, TemplateMeta, TemplateSkill, TreeFile,
    Verification,
};
use crate::problem::{ProblemCode, TemplateProblem, finish_problems};
use crate::render::placeholder_inputs;
use crate::skill_rules::{self, path_ok};
use crate::yaml::{clean_text, parse_strict_yaml, split_front_matter_owned, typed_error_path};

/// `instructions.md` is capped like the brief it becomes (C-1141); `BRIEF_MAX` lives with the
/// API's agents service, so the number is repeated here and pinned by a test.
pub(crate) const BRIEF_MAX: usize = 20_000;
// `BRIEF_MAX` mirrors the API's 20 000; if the API's constant moves, this assertion and the
// docs move with it.
const _: () = assert!(BRIEF_MAX == 20_000);

/// The minimum minutes between two schedule fires (C-1141, as `10 §Schedules`).
const MIN_INTERVAL_MINUTES: i64 = 5;
/// How far the minimum-interval scan looks (a full week of occurrences; `schedules/cron.rs`).
const SCAN_SPAN_DAYS: i64 = 8;
const SCAN_MAX: usize = 2_100;

/// `read_tree` (C-1104): the tree form. `root_name` is the template directory the files came
/// from; every path starts with `<root_name>/`. The same files as in the packed form produce the
/// same [`TemplateContent`].
pub fn read_tree(
    root_name: &str,
    files: &[TreeFile],
) -> Result<TemplateContent, Vec<TemplateProblem>> {
    read_files(root_name, files)
}

/// A problem about a file's name or mode in either form (C-1141 has no separate code for the
/// path grammar, so both readers use `template_archive`).
fn name_problem(path: &str, message: impl Into<String>) -> TemplateProblem {
    TemplateProblem::new(ProblemCode::TemplateArchive, path, message)
}

/// The path grammar below the root: relative, `/`-separated, `[A-Za-z0-9._/-]`, at most 6
/// segments, no `..`, no hidden segment, no double slash, no trailing slash (C-1141).
pub(crate) fn rel_path_problem(rel: &str) -> Option<TemplateProblem> {
    if rel.is_empty() {
        return Some(name_problem("", "the path is empty"));
    }
    if rel.len() > 255 {
        return Some(name_problem(rel, "the path is longer than 255 characters"));
    }
    if !rel
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
    {
        return Some(name_problem(
            rel,
            "the path may only use letters, digits, . _ / -",
        ));
    }
    let segs: Vec<&str> = rel.split('/').collect();
    if segs.len() > 6 {
        return Some(name_problem(
            rel,
            "the path is more than 6 segments below the root",
        ));
    }
    for seg in segs {
        if seg.is_empty() {
            return Some(name_problem(rel, "the path has an empty segment"));
        }
        if seg == ".." {
            return Some(name_problem(rel, "the path climbs out with .."));
        }
        if seg.starts_with('.') {
            return Some(name_problem(rel, "the path has a hidden segment"));
        }
    }
    None
}

/// The only git/zip modes accepted: a regular file, executable or not (C-1141).
pub(crate) fn mode_problem(path: &str, mode: u32) -> Option<TemplateProblem> {
    match mode {
        0o100755 | 0o100644 => None,
        0o120000 => Some(name_problem(path, "symlinks are refused")),
        0o160000 => Some(name_problem(path, "submodules are refused")),
        _ => Some(name_problem(
            path,
            "only regular files (100644 or 100755) are accepted",
        )),
    }
}

/// The caps and the required files, shared by both forms; everything after this is parsing.
pub(crate) fn read_files(
    root_name: &str,
    files: &[TreeFile],
) -> Result<TemplateContent, Vec<TemplateProblem>> {
    let mut problems: Vec<TemplateProblem> = Vec::new();
    if !meta::id_ok(root_name) {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateDirName,
            root_name,
            "the template directory must be named by its id: lowercase words joined by dashes",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0usize;
    let mut structural = Vec::new();
    for f in files {
        let rest = f
            .path
            .strip_prefix(root_name)
            .and_then(|r| r.strip_prefix('/'));
        let Some(rel) = rest else {
            structural.push(TemplateProblem::new(
                ProblemCode::TemplateDirName,
                &f.path,
                "every file must be under the template's own directory",
            ));
            continue;
        };
        if let Some(p) = mode_problem(&f.path, f.mode) {
            structural.push(p);
            continue;
        }
        if let Some(p) = rel_path_problem(rel) {
            structural.push(p);
            continue;
        }
        // No `.github` skip here: a `.github` segment in `rel` is a hidden segment, refused just
        // above (a `.github` folder inside the template is refused, not silently dropped), and a
        // template folder that sits under the repository's own `.github` carries that prefix in
        // the root name, never in `rel` — the files of such a folder are checked as any other.
        if !seen.insert(f.path.clone()) {
            structural.push(name_problem(&f.path, "the same path appears twice"));
            continue;
        }
        total += f.bytes.len();
        if total > limits::TEMPLATE_UNPACKED_MAX {
            structural.push(TemplateProblem::new(
                ProblemCode::TemplateTooLarge,
                &f.path,
                format!(
                    "the unpacked template is larger than {} bytes",
                    limits::TEMPLATE_UNPACKED_MAX
                ),
            ));
            break;
        }
    }
    // Files whose path the grammar refused were pushed to `structural` and are not content, so
    // they are not counted toward the file cap either.
    let content_files = files.len();
    if content_files > limits::TEMPLATE_FILES_MAX {
        structural.push(TemplateProblem::new(
            ProblemCode::TemplateTooLarge,
            root_name,
            format!(
                "the template has more than {} files",
                limits::TEMPLATE_FILES_MAX
            ),
        ));
    }
    // Names and modes first: with those broken, the file set is not trustworthy.
    if !structural.is_empty() {
        structural.extend(problems);
        return Err(finish_problems(structural));
    }

    let find = |rel: &str| {
        files
            .iter()
            .find(|f| f.path == format!("{root_name}/{rel}"))
    };
    for (rel, code) in [
        ("template.yaml", ProblemCode::TemplateYaml),
        ("agent.yaml", ProblemCode::TemplateYaml),
        ("instructions.md", ProblemCode::TemplateYaml),
    ] {
        if find(rel).is_none() {
            problems.push(TemplateProblem::new(
                code,
                rel,
                format!("{rel} is required"),
            ));
        }
    }
    let readme = find("README.md");
    if readme.is_none() {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateReadmeMissing,
            "README.md",
            "README.md is required: what this agent is for, for the people deciding whether to \
             deploy it",
        ));
    }

    // template.yaml: the strict parse, then the id.
    let mut meta_opt: Option<TemplateMeta> = None;
    if let Some(f) = find("template.yaml") {
        match parse_meta_str(&text_of(f, ProblemCode::TemplateYaml, &mut problems)) {
            Ok(m) => {
                if m.id != root_name {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateDirName,
                        "template.yaml#/id",
                        format!("the id must equal the directory name ({root_name})"),
                    ));
                }
                meta_opt = Some(m);
            }
            Err(mut ps) => problems.append(&mut ps),
        }
    }

    // agent.yaml: the export's shape, strictly.
    let mut agent_name = String::new();
    let mut may_message_first = false;
    if let Some(f) = find("agent.yaml") {
        let parsed = parse_agent_yaml(f, &mut problems);
        agent_name = parsed.0;
        may_message_first = parsed.1;
    }

    // instructions.md: cleaned, at most BRIEF_MAX characters.
    let mut instructions = String::new();
    if let Some(f) = find("instructions.md") {
        let raw = text_of(f, ProblemCode::TemplateYaml, &mut problems);
        let cleaned = clean_text(&raw);
        if cleaned.chars().count() > BRIEF_MAX {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateTooLarge,
                "instructions.md",
                format!("instructions.md is longer than {BRIEF_MAX} characters"),
            ));
        }
        instructions = cleaned;
    }

    // README.md: required, 1..TEMPLATE_README_MAX, part of the content (D-458).
    let mut readme_text = String::new();
    if let Some(f) = readme {
        let raw = text_of(f, ProblemCode::TemplateReadmeMissing, &mut problems);
        let cleaned = clean_text(&raw);
        let n = cleaned.chars().count();
        if n == 0 {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateReadmeMissing,
                "README.md",
                "README.md must not be empty",
            ));
        } else if n > limits::TEMPLATE_README_MAX {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateTooLarge,
                "README.md",
                format!(
                    "README.md is longer than {} characters",
                    limits::TEMPLATE_README_MAX
                ),
            ));
        }
        readme_text = cleaned;
    }

    // Classify every other file.
    let mut memories_raw: Vec<(String, &TreeFile)> = Vec::new();
    let mut skills_raw: BTreeMap<String, Vec<(String, &TreeFile)>> = BTreeMap::new();
    let mut helpers_raw: Vec<(String, &TreeFile)> = Vec::new();
    let mut automations_raw: Vec<(String, &TreeFile)> = Vec::new();
    let mut avatars: Vec<(String, &TreeFile)> = Vec::new();
    for f in files {
        let rel = &f.path[root_name.len() + 1..];
        match rel {
            "template.yaml" | "agent.yaml" | "instructions.md" | "README.md" | "CHANGELOG.md" => {}
            _ if rel.starts_with("memory/") => memories_raw.push((rel.to_owned(), f)),
            _ if rel.starts_with("skills/") => {
                let rest = &rel["skills/".len()..];
                match rest.split_once('/') {
                    Some((name, file)) => {
                        skills_raw
                            .entry(name.to_owned())
                            .or_default()
                            .push((file.to_owned(), f));
                    }
                    None => problems.push(TemplateProblem::new(
                        ProblemCode::TemplateFileUnexpected,
                        &f.path,
                        "a file directly under skills/ belongs in a skill directory",
                    )),
                }
            }
            _ if rel.starts_with("helpers/") => helpers_raw.push((rel.to_owned(), f)),
            _ if rel.starts_with("automations/") => automations_raw.push((rel.to_owned(), f)),
            "avatar.png" | "avatar.jpg" | "avatar.webp" => avatars.push((rel.to_owned(), f)),
            _ => problems.push(TemplateProblem::new(
                ProblemCode::TemplateFileUnexpected,
                &f.path,
                "this file is not part of the template format",
            )),
        }
    }

    let memories = parse_memories(root_name, memories_raw, &mut problems);
    let skills = parse_skills(skills_raw, &mut problems);
    let helpers = parse_helpers(helpers_raw, &mut problems);
    let automations = parse_automations(automations_raw, &mut problems);
    let avatar = parse_avatar(avatars, &mut problems);

    // Placeholders: only the fillable places are scanned (C-1141).
    if let Some(meta) = &meta_opt {
        let declared: BTreeSet<&str> = meta.inputs.iter().map(|i| i.id.as_str()).collect();
        let check = |text: &str, path: &str, problems: &mut Vec<TemplateProblem>| {
            for id in placeholder_inputs(text) {
                if !declared.contains(id.as_str()) {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplatePlaceholderUnknown,
                        path,
                        format!("{{input.{id}}} names no input declared in template.yaml"),
                    ));
                }
            }
        };
        check(&instructions, "instructions.md", &mut problems);
        for m in &memories {
            check(&m.content, &format!("memory/{}.md", m.key), &mut problems);
        }
        for s in &skills {
            check(
                &s.use_it_when,
                &format!("skills/{}/SKILL.md", s.name),
                &mut problems,
            );
            check(
                &s.body,
                &format!("skills/{}/SKILL.md", s.name),
                &mut problems,
            );
        }
        for a in &automations {
            match a {
                Automation::Schedule { name, prompt, .. } => {
                    check(prompt, &format!("automations/{name}.yaml"), &mut problems);
                }
                Automation::Webhook {
                    name, instructions, ..
                } => {
                    check(
                        instructions,
                        &format!("automations/{name}.yaml"),
                        &mut problems,
                    );
                }
            }
        }
        // A name may appear only once across own skills and references (C-1142).
        let mut taken: BTreeSet<&str> = BTreeSet::new();
        for s in &skills {
            if !taken.insert(s.name.as_str()) {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateSkillRefInvalid,
                    format!("skills/{}/", s.name),
                    "this skill name is used twice in the template",
                ));
            }
        }
        for (i, r) in meta.skill_refs.iter().enumerate() {
            if !taken.insert(r.name()) {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateSkillRefInvalid,
                    format!("template.yaml#/skills/{i}"),
                    "this skill name is already used by the template's own skills or another \
                     reference",
                ));
            }
        }
    }

    if !problems.is_empty() {
        return Err(finish_problems(problems));
    }
    let Some(meta) = meta_opt else {
        // Unreachable: a missing template.yaml was reported above.
        return Err(finish_problems(vec![TemplateProblem::new(
            ProblemCode::TemplateYaml,
            "template.yaml",
            "template.yaml is required",
        )]));
    };
    Ok(TemplateContent {
        meta,
        agent_name,
        may_message_first,
        instructions,
        readme: readme_text,
        memories,
        skills,
        helpers,
        automations,
        avatar,
    })
}

/// The bytes as text; a file that is not UTF-8 is refused with one fixed sentence (the code
/// depends on which file it is).
fn text_of(f: &TreeFile, code: ProblemCode, problems: &mut Vec<TemplateProblem>) -> String {
    match std::str::from_utf8(&f.bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            problems.push(TemplateProblem::new(
                code,
                &f.path,
                "this file must be valid UTF-8 text",
            ));
            String::new()
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentYamlRaw {
    #[serde(default)]
    format: Option<serde_yaml::Value>,
    name: String,
    /// `purpose` is accepted and ignored (C-1141).
    #[serde(default)]
    #[allow(dead_code)]
    purpose: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    may_message_first: Option<bool>,
}

fn parse_agent_yaml(f: &TreeFile, problems: &mut Vec<TemplateProblem>) -> (String, bool) {
    let mut name = String::new();
    let mut may_message_first = false;
    let raw = match std::str::from_utf8(&f.bytes) {
        Ok(s) => s,
        Err(_) => {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateYaml,
                "agent.yaml",
                "agent.yaml must be valid UTF-8 text",
            ));
            return (name, may_message_first);
        }
    };
    let value = match parse_strict_yaml(raw) {
        Ok(v) => v,
        Err(m) => {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateYaml,
                "agent.yaml",
                format!("agent.yaml: {m}"),
            ));
            return (name, may_message_first);
        }
    };
    let a: AgentYamlRaw = match serde_path_to_error::deserialize(value) {
        Ok(a) => a,
        Err(e) => {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateYaml,
                typed_error_path("agent.yaml", &e),
                "a key is missing, unknown, or has the wrong type here",
            ));
            return (name, may_message_first);
        }
    };
    match &a.format {
        Some(v) if v.as_u64() != Some(1) => problems.push(TemplateProblem::new(
            ProblemCode::TemplateFormatUnsupported,
            "agent.yaml#/format",
            "format must be 1",
        )),
        _ => {}
    }
    let cleaned = aurora_core::sanitize::one_line(&a.name, usize::MAX);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 80 {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateField,
            "agent.yaml#/name",
            "the name must be 1 to 80 characters",
        ));
    } else {
        name = trimmed.to_owned();
    }
    if let Some(model) = &a.model
        && model.trim() != "default"
    {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateModelNamed,
            "agent.yaml#/model",
            "the model must be default: model names differ between installations",
        ));
    }
    // `purpose` is accepted and ignored. `may_message_first` defaults to false: true means the
    // new agent may message first (proactive_messaging = allow), false means ask.
    may_message_first = a.may_message_first.unwrap_or(false);
    (name, may_message_first)
}

/// `memory/<key>.md`: front matter `key` (the file name), `updated` ignored, optional `locked`
/// boolean (C-1151); the text after it is cleaned and capped.
fn parse_memories(
    root_name: &str,
    raw: Vec<(String, &TreeFile)>,
    problems: &mut Vec<TemplateProblem>,
) -> Vec<MemoryFile> {
    if raw.len() > limits::TEMPLATE_MEMORIES_MAX {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateMemoryInvalid,
            format!("{root_name}/memory/"),
            format!(
                "a template carries at most {} memories",
                limits::TEMPLATE_MEMORIES_MAX
            ),
        ));
    }
    let mut out = Vec::new();
    for (rel, f) in raw {
        let Some(key) = rel
            .strip_prefix("memory/")
            .and_then(|r| r.strip_suffix(".md"))
        else {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateMemoryInvalid,
                &f.path,
                "a memory file is memory/<key>.md",
            ));
            continue;
        };
        if !meta::id_ok(key) {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateMemoryInvalid,
                &f.path,
                "the memory key must be lowercase words joined by dashes, at most 64 characters",
            ));
            continue;
        }
        let text = text_of(f, ProblemCode::TemplateMemoryInvalid, problems);
        let Some((front, body)) = split_front_matter_owned(text) else {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateMemoryInvalid,
                &f.path,
                "a memory file needs front matter with its key",
            ));
            continue;
        };
        let value = match parse_strict_yaml(&front) {
            Ok(v) => v,
            Err(m) => {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateMemoryInvalid,
                    &f.path,
                    format!("the front matter is not valid: {m}"),
                ));
                continue;
            }
        };
        if value.get("key").and_then(serde_yaml::Value::as_str) != Some(key) {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateMemoryInvalid,
                format!("{}#/key", f.path),
                "the front-matter key must equal the file name",
            ));
            continue;
        }
        // `updated` is accepted and ignored. `locked` must be a boolean when present (C-1151).
        let mut locked = false;
        match value.get("locked") {
            Some(serde_yaml::Value::Bool(b)) => locked = *b,
            Some(_) => problems.push(TemplateProblem::new(
                ProblemCode::TemplateField,
                format!("{}#/locked", f.path),
                "locked is true or false",
            )),
            None => {}
        }
        let content = clean_text(&body);
        if content.chars().count() > MEMORY_CONTENT_MAX {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateMemoryInvalid,
                &f.path,
                format!("the memory is longer than {MEMORY_CONTENT_MAX} characters"),
            ));
            continue;
        }
        out.push(MemoryFile {
            key: key.to_owned(),
            content,
            locked,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// `skills/<name>/SKILL.md` plus the skill's other files, with the library's file rules.
fn parse_skills(
    raw: BTreeMap<String, Vec<(String, &TreeFile)>>,
    problems: &mut Vec<TemplateProblem>,
) -> Vec<TemplateSkill> {
    if raw.len() > limits::TEMPLATE_SKILLS_MAX {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateSkillInvalid,
            "skills/",
            format!(
                "a template carries at most {} skills",
                limits::TEMPLATE_SKILLS_MAX
            ),
        ));
    }
    let mut out = Vec::new();
    for (name, files) in raw {
        let at = format!("skills/{name}/");
        if let Err(e) = skill_rules::check_name(&name) {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateSkillInvalid,
                &at,
                match e {
                    skill_rules::NameError::Grammar => {
                        "the skill name must be lowercase words joined by dashes, at most 64 \
                         characters"
                    }
                    skill_rules::NameError::Reserved => {
                        "the name aurora-* is reserved for the platform"
                    }
                },
            ));
        }
        if files.len() > skill_rules::FILES_PER_SKILL_MAX {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateSkillInvalid,
                &at,
                format!(
                    "a skill carries at most {} files",
                    skill_rules::FILES_PER_SKILL_MAX
                ),
            ));
        }
        let mut skill = TemplateSkill {
            name: name.clone(),
            use_it_when: String::new(),
            body: String::new(),
            files: Vec::new(),
        };
        if let Some((file, below)) = file_folder_clash(files.iter().map(|(rel, _)| rel.as_str())) {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateSkillInvalid,
                &at,
                clash_message(file, below),
            ));
        }
        let mut has_skill_md = false;
        for (file_rel, f) in files {
            if file_rel == "SKILL.md" {
                has_skill_md = true;
                parse_skill_md(f, &name, &mut skill, problems);
                continue;
            }
            if !path_ok(file_rel.as_str()) {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateSkillInvalid,
                    &f.path,
                    "a skill file path is at most 4 segments of letters, digits, . _ - and \
                     carries no other SKILL.md",
                ));
                continue;
            }
            if f.bytes.len() > LIBRARY_FILE_MAX {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateSkillInvalid,
                    &f.path,
                    format!("a skill file is larger than {} bytes", LIBRARY_FILE_MAX),
                ));
                continue;
            }
            if f.mode == 0o100755 && !skill_rules::is_text(&f.bytes) {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateExecutableBinary,
                    &f.path,
                    "an executable file must be text: the admin reads everything that will run",
                ));
            }
            skill.files.push(TemplateFile {
                path: file_rel,
                bytes: f.bytes.clone(),
                executable: f.mode == 0o100755,
            });
        }
        if !has_skill_md {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateSkillInvalid,
                &at,
                "every skill needs a SKILL.md with its name and use-it-when",
            ));
        }
        skill.files.sort_by(|a, b| a.path.cmp(&b.path));
        out.push(skill);
    }
    out
}

/// The SKILL.md front matter: `name` equal to the directory, `use-it-when` one line.
fn parse_skill_md(
    f: &TreeFile,
    dir: &str,
    skill: &mut TemplateSkill,
    problems: &mut Vec<TemplateProblem>,
) {
    let text = text_of(f, ProblemCode::TemplateSkillInvalid, problems);
    let Some((front, body)) = split_front_matter_owned(text) else {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateSkillInvalid,
            &f.path,
            "SKILL.md needs front matter with name and use-it-when",
        ));
        return;
    };
    let value = match parse_strict_yaml(&front) {
        Ok(v) => v,
        Err(m) => {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateSkillInvalid,
                &f.path,
                format!("the front matter is not valid: {m}"),
            ));
            return;
        }
    };
    match value.get("name").and_then(serde_yaml::Value::as_str) {
        Some(n) if n == dir => {}
        _ => problems.push(TemplateProblem::new(
            ProblemCode::TemplateSkillInvalid,
            format!("{}#/name", f.path),
            "the front-matter name must equal the directory name",
        )),
    }
    match value.get("use-it-when").and_then(serde_yaml::Value::as_str) {
        Some(w) => {
            let w = clean_text(w);
            // One line, like the skill service's description rule: `\r` survives
            // `clean_text` (a control character that is whitespace is kept), so it is
            // refused explicitly — the stored description must never differ from the
            // rendered text (it is hashed at create).
            if w.contains(['\n', '\r']) || w.trim().is_empty() {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateSkillInvalid,
                    format!("{}#/use-it-when", f.path),
                    "use-it-when is one line saying when to use the skill",
                ));
            } else if w.chars().count() > SKILL_DESCRIPTION_MAX {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateSkillInvalid,
                    format!("{}#/use-it-when", f.path),
                    format!("use-it-when is longer than {SKILL_DESCRIPTION_MAX} characters"),
                ));
            } else {
                skill.use_it_when = w.trim().to_owned();
            }
        }
        None => problems.push(TemplateProblem::new(
            ProblemCode::TemplateSkillInvalid,
            format!("{}#/use-it-when", f.path),
            "use-it-when is one line saying when to use the skill",
        )),
    }
    let body = clean_text(&body);
    if body.chars().count() > SKILL_BODY_MAX {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateSkillInvalid,
            &f.path,
            format!("the skill body is longer than {SKILL_BODY_MAX} characters"),
        ));
    }
    skill.body = body;
}

/// `helpers/<below>`: the C-1141 helper path rules a stored helper file's path must satisfy
/// (C-1143 stores the path below `helpers/` and projects it at `.aurora/helpers/<below>`).
/// The shared path grammar below the root (`rel_path_problem`: no `..`, no hidden segment, no
/// empty segment, `[A-Za-z0-9._/-]`, at most 6 segments) plus the helpers' own depth cap of 4
/// segments. The API's helper writer and the supervisor's managed root both call this (P6-TPL-6);
/// it is never re-implemented elsewhere.
pub fn helper_path_ok(below_helpers: &str) -> bool {
    below_helpers.split('/').count() <= 4 && rel_path_problem(below_helpers).is_none()
}

/// The first pair among `paths` where one is a file and also a folder of the other (`run` and
/// `run/x.sh`): `(file, the path below it)`. No filesystem can hold both. The validator and the
/// API's helper writer both call this.
pub fn file_folder_clash<'a>(
    paths: impl IntoIterator<Item = &'a str>,
) -> Option<(&'a str, &'a str)> {
    let all: BTreeSet<&str> = paths.into_iter().collect();
    all.iter().find_map(|below| {
        below
            .match_indices('/')
            .find_map(|(i, _)| all.get(&below[..i]).map(|file| (*file, *below)))
    })
}

fn clash_message(file: &str, below: &str) -> String {
    format!("`{file}` and `{below}` cannot both exist: one path is a file and a folder")
}

/// `helpers/<path>`: text-only files (owner decision 2026-11-24: the home sync carries UTF-8
/// text only, so anything else is `template_helper_invalid`), capped and shallow (C-1141;
/// projected by C-1143).
fn parse_helpers(
    raw: Vec<(String, &TreeFile)>,
    problems: &mut Vec<TemplateProblem>,
) -> Vec<TemplateFile> {
    if raw.len() > limits::TEMPLATE_HELPERS_MAX {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateHelperInvalid,
            "helpers/",
            format!(
                "a template carries at most {} helper files",
                limits::TEMPLATE_HELPERS_MAX
            ),
        ));
    }
    if let Some((file, below)) =
        file_folder_clash(raw.iter().map(|(rel, _)| &rel["helpers/".len()..]))
    {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateHelperInvalid,
            "helpers/",
            clash_message(file, below),
        ));
    }
    let mut total = 0usize;
    let mut out = Vec::new();
    for (rel, f) in raw {
        let below = &rel["helpers/".len()..];
        if !helper_path_ok(below) {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateHelperInvalid,
                &f.path,
                "a helper path is at most 4 segments below helpers/",
            ));
            continue;
        }
        if f.bytes.len() > limits::TEMPLATE_HELPER_FILE_MAX {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateHelperInvalid,
                &f.path,
                format!(
                    "a helper file is larger than {} bytes",
                    limits::TEMPLATE_HELPER_FILE_MAX
                ),
            ));
            continue;
        }
        total += f.bytes.len();
        if total > limits::TEMPLATE_HELPERS_TOTAL_MAX {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateHelperInvalid,
                "helpers/",
                format!(
                    "the helper files together are larger than {} bytes",
                    limits::TEMPLATE_HELPERS_TOTAL_MAX
                ),
            ));
            break;
        }
        if !skill_rules::is_text(&f.bytes) {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateHelperInvalid,
                &f.path,
                "a helper file must be UTF-8 text without NUL: the home sync carries text only",
            ));
        }
        out.push(TemplateFile {
            path: below.to_owned(),
            bytes: f.bytes.clone(),
            executable: f.mode == 0o100755,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// `automations/<file>.yaml`: the export's shapes (C-1141).
fn parse_automations(
    raw: Vec<(String, &TreeFile)>,
    problems: &mut Vec<TemplateProblem>,
) -> Vec<Automation> {
    if raw.len() > limits::TEMPLATE_AUTOMATIONS_MAX {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateAutomationInvalid,
            "automations/",
            format!(
                "a template carries at most {} automations",
                limits::TEMPLATE_AUTOMATIONS_MAX
            ),
        ));
    }
    let mut out = Vec::new();
    let mut schedule_names: BTreeSet<String> = BTreeSet::new();
    let mut webhook_names: BTreeSet<String> = BTreeSet::new();
    for (rel, f) in raw {
        if rel
            .strip_prefix("automations/")
            .and_then(|r| r.strip_suffix(".yaml"))
            .is_none()
        {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateFileUnexpected,
                &f.path,
                "an automation file is automations/<name>.yaml",
            ));
            continue;
        }
        let text = match std::str::from_utf8(&f.bytes) {
            Ok(s) => s,
            Err(_) => {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateAutomationInvalid,
                    &f.path,
                    "an automation file must be valid UTF-8 text",
                ));
                continue;
            }
        };
        let value = match parse_strict_yaml(text) {
            Ok(v) => v,
            Err(m) => {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateAutomationInvalid,
                    &f.path,
                    format!("the automation is not valid: {m}"),
                ));
                continue;
            }
        };
        if value.as_mapping().is_none() {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateAutomationInvalid,
                &f.path,
                "an automation is a mapping",
            ));
            continue;
        }
        let at = |key: &str| format!("{}#{key}", f.path);
        let name = match value.get("name").and_then(serde_yaml::Value::as_str) {
            Some(n) => {
                let n = aurora_core::sanitize::one_line(n, usize::MAX);
                let t = n.trim().to_owned();
                if t.is_empty() || t.chars().count() > 80 {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateAutomationInvalid,
                        at("name"),
                        "the name must be 1 to 80 characters",
                    ));
                    continue;
                }
                t
            }
            None => {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateAutomationInvalid,
                    at("name"),
                    "an automation needs a name",
                ));
                continue;
            }
        };
        // `time_zone` (accepted; the schedule runs in the workspace's zone) and `enabled`
        // (accepted) are optional.
        for key in ["time_zone", "enabled"] {
            if let Some(v) = value.get(key) {
                let ok = match key {
                    "time_zone" => v.is_string(),
                    _ => v.is_bool(),
                };
                if !ok {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateAutomationInvalid,
                        at(key),
                        format!("{key} has the wrong type"),
                    ));
                }
            }
        }
        match value.get("kind").and_then(serde_yaml::Value::as_str) {
            Some("schedule") => {
                if !schedule_names.insert(name.clone()) {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateAutomationInvalid,
                        at("name"),
                        "two schedules carry the same name",
                    ));
                }
                let prompt = value.get("prompt").and_then(serde_yaml::Value::as_str);
                let Some(prompt) = prompt else {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateAutomationInvalid,
                        at("prompt"),
                        "a schedule needs its prompt",
                    ));
                    continue;
                };
                let Some(cadence) = value.get("cadence").and_then(serde_yaml::Value::as_mapping)
                else {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateAutomationInvalid,
                        at("cadence"),
                        "a schedule needs a cadence",
                    ));
                    continue;
                };
                if cadence.get("preset").and_then(serde_yaml::Value::as_str) == Some("once") {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateScheduleOnce,
                        at("cadence/preset"),
                        "a one-time schedule cannot come from a template",
                    ));
                    continue;
                }
                let Some(cron) = cadence.get("cron").and_then(serde_yaml::Value::as_str) else {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateAutomationInvalid,
                        at("cadence"),
                        "a schedule's cadence is a cron",
                    ));
                    continue;
                };
                if let Some(m) = cron_problem(cron) {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateAutomationInvalid,
                        at("cadence/cron"),
                        m,
                    ));
                    continue;
                }
                out.push(Automation::Schedule {
                    name,
                    prompt: clean_text(prompt),
                    cron: cron.to_owned(),
                });
            }
            Some("webhook") => {
                if !webhook_names.insert(name.clone()) {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateAutomationInvalid,
                        at("name"),
                        "two webhooks carry the same name",
                    ));
                }
                let Some(instructions) = value
                    .get("instructions")
                    .and_then(serde_yaml::Value::as_str)
                else {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateAutomationInvalid,
                        at("instructions"),
                        "a webhook needs its instructions",
                    ));
                    continue;
                };
                let verification = match value
                    .get("verification")
                    .and_then(serde_yaml::Value::as_str)
                {
                    Some("signed") => Verification::Signed,
                    Some("password") => Verification::Password,
                    Some("none") => Verification::None,
                    _ => {
                        problems.push(TemplateProblem::new(
                            ProblemCode::TemplateAutomationInvalid,
                            at("verification"),
                            "verification is signed, password or none",
                        ));
                        continue;
                    }
                };
                out.push(Automation::Webhook {
                    name,
                    instructions: clean_text(instructions),
                    verification,
                });
            }
            _ => problems.push(TemplateProblem::new(
                ProblemCode::TemplateAutomationInvalid,
                at("kind"),
                "an automation is a schedule or a webhook",
            )),
        }
    }
    out.sort_by(|a, b| {
        let name = |x: &Automation| match x {
            Automation::Schedule { name, .. } | Automation::Webhook { name, .. } => name.clone(),
        };
        name(a).cmp(&name(b))
    });
    out
}

/// The cron rules of `10 §Schedules`: 5 fields, `croner` with sloppy ranges, at least 5 minutes
/// between fires. The scan starts at a fixed instant, so validation is pure.
fn cron_problem(expr: &str) -> Option<String> {
    if expr.split_whitespace().count() != 5 {
        return Some(
            "the cron must have exactly 5 fields: minute hour day-of-month month day-of-week"
                .into(),
        );
    }
    let cron = CronParser::builder()
        .sloppy_ranges(true)
        .build()
        .parse(expr)
        .ok()?;
    let start = Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).single()?;
    let Ok(first) = cron.find_next_occurrence(&start, false) else {
        return Some("the cron never fires".into());
    };
    let mut prev = first;
    for _ in 0..SCAN_MAX {
        let Ok(next) = cron.find_next_occurrence(&prev, false) else {
            break;
        };
        if next - first > Duration::days(SCAN_SPAN_DAYS) {
            break;
        }
        if next - prev < Duration::minutes(MIN_INTERVAL_MINUTES) {
            return Some(format!(
                "the schedule must not fire more often than every {MIN_INTERVAL_MINUTES} minutes"
            ));
        }
        prev = next;
    }
    None
}

/// The optional avatar: one of `avatar.png`, `avatar.jpg`, `avatar.webp`, at most `AVATAR_MAX`.
fn parse_avatar(
    raw: Vec<(String, &TreeFile)>,
    problems: &mut Vec<TemplateProblem>,
) -> Option<(String, Vec<u8>)> {
    if raw.len() > 1 {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateField,
            raw[1].0.clone(),
            "a template carries at most one avatar",
        ));
    }
    let (name, f) = raw.into_iter().next()?;
    if f.bytes.len() > AVATAR_MAX {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateField,
            &name,
            format!("the avatar is larger than {AVATAR_MAX} bytes"),
        ));
        return None;
    }
    Some((name, f.bytes.clone()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;
    use crate::problem::ProblemCode;
    use crate::test_fixtures::{TEMPLATE_YAML, exec, file, sample_files};

    #[test]
    fn sample_is_valid() {
        let c = read_tree("service-monitor", &sample_files()).unwrap();
        assert_eq!(c.meta.id, "service-monitor");
        assert_eq!(c.meta.version, "1.0.0");
        assert_eq!(c.agent_name, "Monitor");
        assert!(!c.may_message_first);
        assert_eq!(c.memories.len(), 1);
        assert!(c.memories[0].locked);
        assert_eq!(c.skills.len(), 1);
        assert_eq!(c.skills[0].files.len(), 1);
        assert!(c.skills[0].files[0].executable);
        assert_eq!(c.helpers.len(), 1);
        assert_eq!(c.automations.len(), 2);
        assert!(c.avatar.is_none());
    }

    #[test]
    fn the_github_skip_tests_the_path_relative_to_the_template_folder() {
        // A template folder that lives under the repository's own `.github` has that prefix in
        // the root name, not in the paths below it: the template's own files are all seen (no
        // "required" refusals), and the root name itself is refused by its own check.
        let files: Vec<TreeFile> = sample_files()
            .into_iter()
            .map(|f| TreeFile {
                path: format!(".github/templates/{}", f.path),
                bytes: f.bytes,
                mode: f.mode,
            })
            .collect();
        let err = read_tree(".github/templates/service-monitor", &files).unwrap_err();
        assert!(
            !err.iter().any(|p| p.message.contains("is required")),
            "{err:?}"
        );
        assert!(
            err.iter().any(|p| p.code == ProblemCode::TemplateDirName),
            "{err:?}"
        );
        // A `.github` folder inside the template is a hidden segment: the grammar refuses it, it
        // is not silently skipped.
        let mut files = sample_files();
        files.push(file("service-monitor/.github/workflows/ci.yml", "x"));
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter().any(|p| p.message.contains("hidden segment")),
            "{err:?}"
        );
    }

    #[test]
    fn bad_names_and_modes_in_both_forms() {
        // The tree form and (below, via read_archive) the zip form refuse the same names; the
        // agreement test covers the equality, this one the grammar.
        let mut files = sample_files();
        files.push(file("service-monitor/../escape.txt", "x"));
        files.push(file("service-monitor/.hidden", "x"));
        files.push(TreeFile {
            path: "service-monitor/link".to_owned(),
            bytes: vec![],
            mode: 0o120000,
        });
        files.push(TreeFile {
            path: "service-monitor/sub".to_owned(),
            bytes: vec![],
            mode: 0o160000,
        });
        files.push(file("other/top.txt", "x"));
        files.push(file("service-monitor/a/b/c/d/e/f.txt", "x"));
        let err = read_tree("service-monitor", &files).unwrap_err();
        let codes: Vec<_> = err.iter().map(|p| p.code).collect();
        assert!(codes.contains(&ProblemCode::TemplateArchive), "{err:?}");
        assert!(codes.contains(&ProblemCode::TemplateDirName), "{err:?}");
    }

    #[test]
    fn duplicate_paths_refused() {
        let mut files = sample_files();
        files.push(file("service-monitor/README.md", "dup"));
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(err.iter().any(|p| p.message.contains("appears twice")));
    }

    #[test]
    fn required_files() {
        let files: Vec<TreeFile> = sample_files()
            .into_iter()
            .filter(|f| !f.path.ends_with("README.md"))
            .collect();
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateReadmeMissing)
        );
        let empty_readme = sample_files()
            .into_iter()
            .map(|f| {
                if f.path.ends_with("README.md") {
                    file("service-monitor/README.md", "")
                } else {
                    f
                }
            })
            .collect::<Vec<_>>();
        let err = read_tree("service-monitor", &empty_readme).unwrap_err();
        assert!(
            err.iter().any(
                |p| p.code == ProblemCode::TemplateReadmeMissing && p.message.contains("empty")
            )
        );
    }

    #[test]
    fn root_must_be_the_id() {
        let err = read_tree("wrong-name", &sample_files()).unwrap_err();
        assert!(err.iter().any(|p| p.code == ProblemCode::TemplateDirName));
        let mut files = sample_files();
        files[0] = file(
            "service-monitor/template.yaml",
            &TEMPLATE_YAML.replace("id: service-monitor", "id: other-name"),
        );
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateDirName && p.path.contains("#/id"))
        );
    }

    #[test]
    fn agent_yaml_rules() {
        let mut files = sample_files();
        files[1] = file(
            "service-monitor/agent.yaml",
            "format: 1\nname: Monitor\nmodel: gpt-9\nmay_message_first: true\n",
        );
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateModelNamed)
        );
        files[1] = file("service-monitor/agent.yaml", "name: Monitor\nsurprise: 1\n");
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(err.iter().any(|p| p.code == ProblemCode::TemplateYaml));
    }

    #[test]
    fn unexpected_files_and_memory_rules() {
        let mut files = sample_files();
        files.push(file("service-monitor/extra.txt", "x"));
        files.push(file(
            "service-monitor/memory/Bad_Key.md",
            "---\nkey: x\n---\ntext\n",
        ));
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateFileUnexpected)
        );
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateMemoryInvalid)
        );
        // locked must be a boolean (C-1151).
        let mut files = sample_files();
        files[4] = file(
            "service-monitor/memory/service-notes.md",
            "---\nkey: service-notes\nlocked: yes\n---\ntext\n",
        );
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateField && p.path.contains("#/locked"))
        );
    }

    #[test]
    fn skill_rules_enforced() {
        let mut files = sample_files();
        files.push(file(
            "service-monitor/skills/aurora-taken/SKILL.md",
            "---\nname: aurora-taken\nuse-it-when: x\n---\nbody\n",
        ));
        files.push(file(
            "service-monitor/skills/what-is-on/deep/SKILL.md",
            "---\nname: x\n---\n",
        ));
        files.push(exec(
            "service-monitor/skills/what-is-on/tool.bin",
            "\u{0}\u{1}\u{2}",
        ));
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateSkillInvalid
                    && p.message.contains("reserved"))
        );
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateSkillInvalid
                    && p.message.contains("SKILL.md"))
        );
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateExecutableBinary)
        );
        // A non-executable binary skill file is fine.
        let mut files = sample_files();
        files.push(TreeFile {
            path: "service-monitor/skills/what-is-on/data.bin".to_owned(),
            bytes: vec![0, 1, 2, 255],
            mode: 0o100644,
        });
        assert!(read_tree("service-monitor", &files).is_ok());
    }

    /// The skill service stores a description only as a single line (it rejects `\r` and
    /// `\n`); the validator refuses the same at template time, so the stored use-it-when
    /// never differs from the rendered text (it is hashed at create). A bare `\r` survives
    /// `clean_text` (a control character that is whitespace is kept) and is refused too.
    #[test]
    fn skill_description_is_a_single_line() {
        for bad in ["\"one\\rtwo\"", "\"one\\r\\ntwo\"", "\"one\\r\""] {
            let mut files = sample_files();
            files[5] = file(
                "service-monitor/skills/what-is-on/SKILL.md",
                &format!("---\nname: what-is-on\nuse-it-when: {bad}\n---\nbody\n"),
            );
            let err = read_tree("service-monitor", &files).unwrap_err();
            assert!(
                err.iter()
                    .any(|p| p.code == ProblemCode::TemplateSkillInvalid
                        && p.path.contains("#/use-it-when")),
                "{bad}: {err:?}"
            );
        }
    }

    #[test]
    fn helper_rules() {
        let mut files = sample_files();
        files.push(exec(
            "service-monitor/helpers/a/b/c/d/tool.sh",
            "#!/bin/sh\n",
        ));
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateHelperInvalid
                    && p.message.contains("4 segments"))
        );
        let big = vec![b'x'; limits::TEMPLATE_HELPER_FILE_MAX + 1];
        files.push(TreeFile {
            path: "service-monitor/helpers/big.sh".to_owned(),
            bytes: big,
            mode: 0o100755,
        });
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateHelperInvalid
                    && p.message.contains("larger"))
        );
        // Owner decision 2026-11-24: every helper file is text, executable or not — the home
        // sync carries UTF-8 text only. A binary helper is refused with the file's path.
        for (path, mode) in [
            ("service-monitor/helpers/blob.bin", 0o100644),
            ("service-monitor/helpers/nul.sh", 0o100755),
        ] {
            let mut files = sample_files();
            files.push(TreeFile {
                path: path.to_owned(),
                bytes: if path.ends_with("nul.sh") {
                    b"#!/bin/sh\necho \0x\n".to_vec()
                } else {
                    vec![0xff, 0x00, 0xfe]
                },
                mode,
            });
            let err = read_tree("service-monitor", &files).unwrap_err();
            assert!(
                err.iter()
                    .any(|p| p.code == ProblemCode::TemplateHelperInvalid
                        && p.path == path
                        && p.message.contains("UTF-8")),
                "{path}: {err:?}"
            );
        }
        // A non-executable binary SKILL file is still fine (the rule changed for helpers only).
        let mut files = sample_files();
        files.push(TreeFile {
            path: "service-monitor/skills/what-is-on/data.bin".to_owned(),
            bytes: vec![0xff, 0x00, 0xfe],
            mode: 0o100644,
        });
        assert!(read_tree("service-monitor", &files).is_ok());
    }

    /// A path that is a file and also a folder of another path is refused, for helpers and for
    /// a skill's files.
    #[test]
    fn a_file_that_is_also_a_folder_is_refused() {
        for (dir, code) in [
            ("helpers", ProblemCode::TemplateHelperInvalid),
            ("skills/what-is-on", ProblemCode::TemplateSkillInvalid),
        ] {
            let mut files = sample_files();
            // `run.md` sorts between `run` and `run/x.sh`: neighbours alone are not enough.
            for below in ["run", "run.md", "run/x.sh"] {
                files.push(file(&format!("service-monitor/{dir}/{below}"), "x\n"));
            }
            let err = read_tree("service-monitor", &files).unwrap_err();
            assert!(
                err.iter().any(|p| p.code == code
                    && p.message.contains("`run`")
                    && p.message.contains("`run/x.sh`")),
                "{dir}: {err:?}"
            );
        }
    }

    #[test]
    fn helper_path_rules_below_root() {
        // The path check the API writer and the supervisor's managed root both call.
        assert!(helper_path_ok("alert-summary.sh"));
        assert!(helper_path_ok("bin/tools/render.sh"));
        assert!(helper_path_ok("a/b/c/d"));
        assert!(!helper_path_ok("a/b/c/d/e"));
        assert!(!helper_path_ok(""));
        assert!(!helper_path_ok("../escape"));
        assert!(!helper_path_ok(".hidden"));
        assert!(!helper_path_ok("sp ace.sh"));
        assert!(!helper_path_ok("a//b"));
    }

    #[test]
    fn automation_rules() {
        let mut files = sample_files();
        files[8] = file(
            "service-monitor/automations/morning-check.yaml",
            "kind: schedule\nname: Morning check\nprompt: x\ncadence:\n  preset: once\n  run_at: 2026-01-01T09:00\n",
        );
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateScheduleOnce)
        );
        files[8] = file(
            "service-monitor/automations/morning-check.yaml",
            "kind: schedule\nname: Morning check\nprompt: x\ncadence:\n  cron: \"* * * * *\"\n",
        );
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateAutomationInvalid
                    && p.message.contains("every 5 minutes"))
        );
        files[8] = file(
            "service-monitor/automations/morning-check.yaml",
            "kind: schedule\nname: Morning check\nprompt: x\ncadence:\n  cron: \"0 8 * *\"\n",
        );
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateAutomationInvalid
                    && p.message.contains("5 fields"))
        );
        files[8] = file(
            "service-monitor/automations/morning-check.yaml",
            "kind: webhook\nname: Escalate\ninstructions: x\nverification: signed\n",
        );
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(err.iter().any(|p| p.message.contains("same name")));
    }

    #[test]
    fn placeholders_and_clashes() {
        let mut files = sample_files();
        files[2] = file(
            "service-monitor/instructions.md",
            "Watch {input.unknown_input} please.\n",
        );
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplatePlaceholderUnknown)
        );
        // A literal brace that is not a placeholder stays fine.
        files[2] = file(
            "service-monitor/instructions.md",
            "Use {curly braces} freely.\n",
        );
        assert!(read_tree("service-monitor", &files).is_ok());
        // A skill name taken by a reference is refused (C-1142).
        files[0] = file(
            "service-monitor/template.yaml",
            &format!("{TEMPLATE_YAML}skills:\n  - organization: what-is-on\n    why: clash\n"),
        );
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateSkillRefInvalid)
        );
    }

    #[test]
    fn avatar_rules() {
        let mut files = sample_files();
        files.push(TreeFile {
            path: "service-monitor/avatar.png".to_owned(),
            bytes: vec![0x89, b'P'],
            mode: 0o100644,
        });
        files.push(TreeFile {
            path: "service-monitor/avatar.jpg".to_owned(),
            bytes: vec![0xff, 0xd8],
            mode: 0o100644,
        });
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(err.iter().any(|p| p.message.contains("at most one avatar")));
        let mut files = sample_files();
        files.push(TreeFile {
            path: "service-monitor/avatar.png".to_owned(),
            bytes: vec![b'x'; AVATAR_MAX + 1],
            mode: 0o100644,
        });
        let err = read_tree("service-monitor", &files).unwrap_err();
        assert!(err.iter().any(|p| p.message.contains("avatar is larger")));
        let mut files = sample_files();
        files.push(TreeFile {
            path: "service-monitor/avatar.png".to_owned(),
            bytes: vec![0x89, b'P'],
            mode: 0o100644,
        });
        let c = read_tree("service-monitor", &files).unwrap();
        assert_eq!(c.avatar.unwrap().0, "avatar.png");
    }
}
