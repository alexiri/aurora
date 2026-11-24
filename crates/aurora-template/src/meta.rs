//! `template.yaml`, strictly parsed (C-1141): unknown keys refused, YAML anchors, aliases, tags
//! and multiple documents refused, every field checked, all field problems reported.

use std::collections::BTreeSet;

use aurora_core::git_url;
use aurora_core::limits;
use regex::RegexBuilder;
use serde::Deserialize;

use crate::model::{
    AppEntry, AppTools, Effort, InputDecl, InputKind, Level, SkillRef, TemplateMeta,
};
use crate::problem::{ProblemCode, TemplateProblem, finish_problems};
use crate::skill_rules::{self, NameError};
use crate::yaml::parse_strict_yaml;

/// The regex size limit a `pattern` compiles with (C-1141); a pattern over it is a
/// `template_field` problem, not a panic.
const PATTERN_SIZE_LIMIT: usize = 64 * 1024;

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaRaw {
    format: serde_yaml::Value,
    id: String,
    version: String,
    name: String,
    summary: String,
    #[serde(default)]
    min_core: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    inputs: Vec<InputRaw>,
    #[serde(default)]
    apps: Vec<AppRaw>,
    #[serde(default)]
    skills: Vec<SkillRefRaw>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum KindRaw {
    #[default]
    Text,
    Choice,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum LevelRaw {
    #[default]
    Read,
    Ask,
    Act,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputRaw {
    id: String,
    label: String,
    #[serde(default)]
    help: Option<String>,
    #[serde(default)]
    kind: KindRaw,
    #[serde(default = "default_true")]
    required: bool,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    max_length: Option<u32>,
    #[serde(default)]
    options: Option<Vec<String>>,
    #[serde(default)]
    default: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolsRaw {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    ask: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppRaw {
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    any_of: Option<Vec<String>>,
    #[serde(default = "default_true")]
    required: bool,
    #[serde(default)]
    level: LevelRaw,
    #[serde(default)]
    tools: Option<ToolsRaw>,
    why: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillRefRaw {
    #[serde(default)]
    library: Option<String>,
    #[serde(default)]
    organization: Option<String>,
    #[serde(default)]
    skill: Option<String>,
    #[serde(rename = "ref", default)]
    ref_: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default = "default_true")]
    required: bool,
    why: String,
}

/// `[a-z0-9]+(-[a-z0-9]+)*`, at most 64 characters — a template id and an app id grammar.
#[must_use]
pub fn id_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.split('-').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Semver without build metadata (`1.2.3` or `1.2.3-rc.1`).
#[must_use]
pub fn semver_ok(s: &str) -> bool {
    let Some((core, pre)) = s.split_once('-') else {
        return core_semver(s);
    };
    core_semver(core)
        && !pre.is_empty()
        && pre.split('.').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        })
}

fn core_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.len() <= 10)
}

/// Compiles a `pattern` with the 64 KiB size limit; used by the field check and by
/// `check_inputs`. A pattern that does not compile (or is over the limit) is `None`.
pub(crate) fn compile_pattern(pattern: &str) -> Option<regex::Regex> {
    RegexBuilder::new(pattern)
        .size_limit(PATTERN_SIZE_LIMIT)
        .build()
        .ok()
}

fn field(path: String, message: impl Into<String>) -> TemplateProblem {
    TemplateProblem::new(ProblemCode::TemplateField, path, message)
}

/// `parse_meta`: `template.yaml` alone, with the same strict rules (the ingest lists a
/// repository with it; C-1104).
pub fn parse_meta(bytes: &[u8]) -> Result<TemplateMeta, Vec<TemplateProblem>> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        vec![TemplateProblem::new(
            ProblemCode::TemplateYaml,
            "template.yaml",
            "template.yaml is not valid UTF-8",
        )]
    })?;
    parse_meta_str(text)
}

/// The strict parse shared by [`parse_meta`] and the full read.
pub(crate) fn parse_meta_str(text: &str) -> Result<TemplateMeta, Vec<TemplateProblem>> {
    let value = parse_strict_yaml(text).map_err(|m| {
        vec![TemplateProblem::new(
            ProblemCode::TemplateYaml,
            "template.yaml",
            m,
        )]
    })?;
    // `serde_path_to_error` names the key of anything the typed parse refuses, so the problem's
    // path can point at `template.yaml#/apps/1/level`.
    let raw: MetaRaw = serde_path_to_error::deserialize(value).map_err(|e| {
        vec![TemplateProblem::new(
            ProblemCode::TemplateYaml,
            crate::yaml::typed_error_path("template.yaml", &e),
            describe_yaml_error(&e),
        )]
    })?;
    let mut problems = Vec::new();
    let meta = validate(raw, &mut problems);
    if problems.is_empty() {
        Ok(meta)
    } else {
        Err(finish_problems(problems))
    }
}

/// One fixed sentence for a typed-parse refusal (the raw error may echo input).
fn describe_yaml_error(e: &serde_path_to_error::Error<serde_yaml::Error>) -> String {
    let raw = e.inner().to_string();
    if raw.contains("unknown field")
        || raw.contains("missing field")
        || raw.contains("invalid type")
    {
        "a key is missing, unknown, or has the wrong type here".into()
    } else {
        "template.yaml is not valid YAML".into()
    }
}

fn validate(raw: MetaRaw, problems: &mut Vec<TemplateProblem>) -> TemplateMeta {
    // format: integer 1; any other value is template_format_unsupported.
    if raw.format.as_u64() != Some(1) {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateFormatUnsupported,
            "template.yaml#/format",
            "format must be 1; this file is not a template format this Aurora reads",
        ));
    }
    if !id_ok(&raw.id) {
        problems.push(field(
            "template.yaml#/id".into(),
            "the id must be lowercase words joined by dashes, at most 64 characters",
        ));
    }
    if !semver_ok(&raw.version) {
        problems.push(field(
            "template.yaml#/version".into(),
            "the version must be semver without build metadata, like 1.0.0",
        ));
    }
    let name = clean_or(&raw.name, 80, "template.yaml#/name", problems);
    let summary = clean_or(&raw.summary, 280, "template.yaml#/summary", problems);
    let min_core = raw.min_core.and_then(|m| {
        if semver_ok(m.trim()) {
            Some(m.trim().to_owned())
        } else {
            problems.push(field(
                "template.yaml#/min_core".into(),
                "min_core must be semver without build metadata, like 6.0.0",
            ));
            None
        }
    });
    let effort = raw.effort.and_then(|e| match e.as_str() {
        "low" => Some(Effort::Low),
        "medium" => Some(Effort::Medium),
        "high" => Some(Effort::High),
        "max" => Some(Effort::Max),
        _ => {
            problems.push(field(
                "template.yaml#/effort".into(),
                "effort must be low, medium, high or max",
            ));
            None
        }
    });
    let inputs = validate_inputs(raw.inputs, problems);
    let apps = validate_apps(raw.apps, problems);
    let skill_refs = validate_skill_refs(raw.skills, problems);
    TemplateMeta {
        format: 1,
        id: raw.id,
        version: raw.version,
        name,
        summary,
        min_core,
        effort,
        inputs,
        apps,
        skill_refs,
    }
}

/// Cleans one text field (text hygiene, then trim) or reports a length problem.
pub(crate) fn clean_or(
    s: &str,
    max: usize,
    path: &str,
    problems: &mut Vec<TemplateProblem>,
) -> String {
    let cleaned = aurora_core::sanitize::one_line(s, usize::MAX);
    let trimmed = cleaned.trim();
    let n = trimmed.chars().count();
    if n == 0 || n > max {
        problems.push(field(
            path.to_owned(),
            format!("must be 1 to {max} characters"),
        ));
    }
    trimmed.to_owned()
}

fn validate_inputs(raw: Vec<InputRaw>, problems: &mut Vec<TemplateProblem>) -> Vec<InputDecl> {
    if raw.len() > limits::TEMPLATE_INPUTS_MAX {
        problems.push(field(
            "template.yaml#/inputs".into(),
            format!(
                "a template declares at most {} inputs",
                limits::TEMPLATE_INPUTS_MAX
            ),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (i, input) in raw.into_iter().enumerate() {
        let at = format!("template.yaml#/inputs/{i}");
        if !input_id_ok(&input.id) || !seen.insert(input.id.clone()) {
            problems.push(field(
                format!("{at}/id"),
                "the id must be a lowercase word with digits and underscores, starting with a \
                 letter, at most 32 characters, and unique",
            ));
        }
        let label = clean_or(&input.label, 80, &format!("{at}/label"), problems);
        let help = input
            .help
            .map(|h| clean_or(&h, 200, &format!("{at}/help"), problems));
        let kind = match input.kind {
            KindRaw::Text => InputKind::Text,
            KindRaw::Choice => InputKind::Choice,
        };
        let pattern = input.pattern.and_then(|p| {
            if kind != InputKind::Text {
                problems.push(field(
                    format!("{at}/pattern"),
                    "a pattern is only allowed on a text input",
                ));
                return None;
            }
            if p.chars().count() > 200 {
                problems.push(field(
                    format!("{at}/pattern"),
                    "the pattern is longer than 200 characters",
                ));
                return None;
            }
            match compile_pattern(&p) {
                Some(_) => Some(p),
                None => {
                    problems.push(field(
                        format!("{at}/pattern"),
                        "the pattern is not a valid regular expression, or it is too large",
                    ));
                    None
                }
            }
        });
        let max_length = input.max_length.and_then(|m| {
            if kind != InputKind::Text {
                problems.push(field(
                    format!("{at}/max_length"),
                    "a max_length is only allowed on a text input",
                ));
                return None;
            }
            if m == 0 || m as usize > limits::TEMPLATE_INPUT_VALUE_MAX {
                problems.push(field(
                    format!("{at}/max_length"),
                    format!(
                        "max_length must be 1 to {}",
                        limits::TEMPLATE_INPUT_VALUE_MAX
                    ),
                ));
                return None;
            }
            Some(m)
        });
        let options = input.options.and_then(|o| {
            if kind != InputKind::Choice {
                problems.push(field(
                    format!("{at}/options"),
                    "options are only allowed on a choice input",
                ));
                return None;
            }
            if o.is_empty() || o.len() > 20 {
                problems.push(field(
                    format!("{at}/options"),
                    "a choice input needs 1 to 20 options",
                ));
                return None;
            }
            let mut cleaned = Vec::new();
            let mut distinct = BTreeSet::new();
            let mut ok = true;
            for opt in o {
                let c = clean_or(&opt, 80, &format!("{at}/options"), problems);
                if !distinct.insert(c.clone()) {
                    problems.push(field(
                        format!("{at}/options"),
                        "the options must be distinct",
                    ));
                    ok = false;
                }
                cleaned.push(c);
            }
            ok.then_some(cleaned)
        });
        let default = input.default.and_then(|d| {
            let cleaned = aurora_core::sanitize::one_line(&d, usize::MAX);
            let trimmed = cleaned.trim().to_owned();
            if trimmed.chars().any(char::is_control) {
                problems.push(field(
                    format!("{at}/default"),
                    "the default must not contain control characters",
                ));
                return None;
            }
            let limit = max_length.unwrap_or(100) as usize;
            if trimmed.chars().count() > limit || trimmed.is_empty() {
                problems.push(field(
                    format!("{at}/default"),
                    "the default must be 1 to max_length characters (100 when max_length is \
                     not set)",
                ));
                return None;
            }
            if let Some(p) = &pattern {
                let matches = compile_pattern(p)
                    .is_some_and(|r| r.find(&trimmed).is_some_and(|m| m.as_str() == trimmed));
                if !matches {
                    problems.push(field(
                        format!("{at}/default"),
                        "the default must match the pattern",
                    ));
                    return None;
                }
            }
            if kind == InputKind::Choice {
                let in_options = options.as_ref().is_some_and(|o| o.contains(&trimmed));
                if !in_options {
                    problems.push(field(
                        format!("{at}/default"),
                        "the default must be one of the options",
                    ));
                    return None;
                }
            }
            Some(trimmed)
        });
        out.push(InputDecl {
            id: input.id,
            label,
            help,
            kind,
            required: input.required,
            pattern,
            max_length,
            options: options.unwrap_or_default(),
            default,
        });
    }
    out
}

/// `[a-z][a-z0-9_]{0,31}` (C-1141).
#[must_use]
pub fn input_id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

fn validate_apps(raw: Vec<AppRaw>, problems: &mut Vec<TemplateProblem>) -> Vec<AppEntry> {
    if raw.len() > limits::TEMPLATE_APPS_MAX {
        problems.push(field(
            "template.yaml#/apps".into(),
            format!(
                "a template lists at most {} apps",
                limits::TEMPLATE_APPS_MAX
            ),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (i, app) in raw.into_iter().enumerate() {
        let at = format!("template.yaml#/apps/{i}");
        let why = clean_or(&app.why, 200, &format!("{at}/why"), problems);
        let level = match app.level {
            LevelRaw::Read => Level::Read,
            LevelRaw::Ask => Level::Ask,
            LevelRaw::Act => Level::Act,
        };
        match (app.app, app.any_of) {
            (Some(app_id), None) => {
                if !id_ok(&app_id) {
                    problems.push(field(
                        format!("{at}/app"),
                        "the app id must be lowercase words joined by dashes",
                    ));
                } else if !seen.insert(app_id.clone()) {
                    problems.push(field(
                        format!("{at}/app"),
                        "an app id may appear in at most one entry",
                    ));
                }
                let tools = app.tools.and_then(|t| {
                    let allow = t.allow;
                    let ask = t.ask;
                    if allow.is_empty() && ask.is_empty() {
                        problems.push(field(
                            format!("{at}/tools"),
                            "tools needs at least one tool in allow or ask",
                        ));
                        return None;
                    }
                    let overlap: BTreeSet<_> =
                        allow.iter().filter(|a| ask.contains(a)).cloned().collect();
                    if !overlap.is_empty() {
                        problems.push(field(
                            format!("{at}/tools"),
                            "a tool may not be in both allow and ask",
                        ));
                        return None;
                    }
                    Some(AppTools { allow, ask })
                });
                out.push(AppEntry::App {
                    app: app_id,
                    required: app.required,
                    level,
                    tools,
                    why,
                });
            }
            (None, Some(any_of)) => {
                if app.tools.is_some() {
                    problems.push(field(
                        format!("{at}/tools"),
                        "tools are only allowed on an entry with one app",
                    ));
                }
                if any_of.len() < 2 || any_of.len() > 5 {
                    problems.push(field(
                        format!("{at}/any_of"),
                        "any_of needs 2 to 5 alternative app ids",
                    ));
                }
                let mut distinct = BTreeSet::new();
                for id in &any_of {
                    if !id_ok(id) {
                        problems.push(field(
                            format!("{at}/any_of"),
                            "every app id must be lowercase words joined by dashes",
                        ));
                        break;
                    }
                    if !distinct.insert(id.clone()) {
                        problems.push(field(
                            format!("{at}/any_of"),
                            "the alternatives must be distinct",
                        ));
                        break;
                    }
                    if !seen.insert(id.clone()) {
                        problems.push(field(
                            format!("{at}/any_of"),
                            "an app id may appear in at most one entry",
                        ));
                        break;
                    }
                }
                out.push(AppEntry::AnyOf {
                    any_of,
                    required: app.required,
                    level,
                    why,
                });
            }
            (Some(_), Some(_)) | (None, None) => {
                problems.push(field(at, "an entry names exactly one of app and any_of"));
            }
        }
    }
    out
}

fn validate_skill_refs(
    raw: Vec<SkillRefRaw>,
    problems: &mut Vec<TemplateProblem>,
) -> Vec<SkillRef> {
    if raw.len() > limits::TEMPLATE_SKILL_REFS_MAX {
        problems.push(TemplateProblem::new(
            ProblemCode::TemplateSkillRefInvalid,
            "template.yaml#/skills",
            format!(
                "a template references at most {} skills",
                limits::TEMPLATE_SKILL_REFS_MAX
            ),
        ));
    }
    let mut out = Vec::new();
    for (i, r) in raw.into_iter().enumerate() {
        let at = format!("template.yaml#/skills/{i}");
        let why = clean_or(&r.why, 200, &format!("{at}/why"), problems);
        match (r.library, r.organization) {
            (Some(library), None) => {
                let skill = match r.skill {
                    Some(s) => match skill_rules::check_name(s.trim()) {
                        Ok(()) => s.trim().to_owned(),
                        Err(NameError::Grammar) => {
                            problems.push(TemplateProblem::new(
                                ProblemCode::TemplateSkillRefInvalid,
                                format!("{at}/skill"),
                                "the skill name must be lowercase words joined by dashes, at \
                                 most 64 characters",
                            ));
                            continue;
                        }
                        Err(NameError::Reserved) => {
                            problems.push(TemplateProblem::new(
                                ProblemCode::TemplateSkillRefInvalid,
                                format!("{at}/skill"),
                                "the name aurora-* is reserved for the platform",
                            ));
                            continue;
                        }
                    },
                    None => {
                        problems.push(TemplateProblem::new(
                            ProblemCode::TemplateSkillRefInvalid,
                            format!("{at}/skill"),
                            "a library reference names the skill it wants",
                        ));
                        continue;
                    }
                };
                if r.path.is_some() && r.ref_.is_none() {
                    // A path without a ref is fine (the library's default branch); nothing to do.
                }
                let path = r.path.and_then(|p| {
                    let p = p.trim().trim_matches('/');
                    let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
                    let ok = !segs.is_empty()
                        && segs.len() <= 4
                        && segs
                            .iter()
                            .all(|s| *s != "." && *s != ".." && !s.starts_with('.'));
                    if !ok {
                        problems.push(TemplateProblem::new(
                            ProblemCode::TemplateSkillRefInvalid,
                            format!("{at}/path"),
                            "the folder is at most 4 segments, without . or .. or hidden names",
                        ));
                        return None;
                    }
                    Some(segs.join("/"))
                });
                let ref_ = r.ref_.and_then(|r| {
                    let t = r.trim();
                    if t.is_empty() || t.len() > 200 || t.starts_with('.') || t.contains("..") {
                        problems.push(TemplateProblem::new(
                            ProblemCode::TemplateSkillRefInvalid,
                            format!("{at}/ref"),
                            "the ref must be a branch, tag or commit name",
                        ));
                        return None;
                    }
                    Some(t.to_owned())
                });
                match git_url::parse(&library) {
                    Ok(address) => {
                        // A page URL fills ref and path when the reference does not give them.
                        let ref_ = ref_.or(address.ref_hint.clone());
                        let path = path.or(address.path_hint.clone());
                        out.push(SkillRef::Library {
                            address,
                            ref_,
                            path,
                            skill,
                            required: r.required,
                            why,
                        });
                    }
                    Err(e) => {
                        problems.push(TemplateProblem::new(
                            ProblemCode::TemplateSkillRefInvalid,
                            format!("{at}/library"),
                            format!(
                                "the library address was refused ({}): {}",
                                e.reason(),
                                e.message()
                            ),
                        ));
                    }
                }
            }
            (None, Some(organization)) => {
                if r.skill.is_some() || r.ref_.is_some() || r.path.is_some() {
                    problems.push(TemplateProblem::new(
                        ProblemCode::TemplateSkillRefInvalid,
                        at,
                        "an organization reference carries only the skill's name, required and \
                         why",
                    ));
                    continue;
                }
                match skill_rules::check_name(organization.trim()) {
                    Ok(()) => out.push(SkillRef::Organization {
                        name: organization.trim().to_owned(),
                        required: r.required,
                        why,
                    }),
                    Err(NameError::Reserved) => problems.push(TemplateProblem::new(
                        ProblemCode::TemplateSkillRefInvalid,
                        format!("{at}/organization"),
                        "the name aurora-* is reserved for the platform",
                    )),
                    Err(NameError::Grammar) => problems.push(TemplateProblem::new(
                        ProblemCode::TemplateSkillRefInvalid,
                        format!("{at}/organization"),
                        "the name must be a skill name: lowercase words joined by dashes, not \
                         starting with aurora-",
                    )),
                }
            }
            (Some(_), Some(_)) | (None, None) => {
                problems.push(TemplateProblem::new(
                    ProblemCode::TemplateSkillRefInvalid,
                    at,
                    "a reference names exactly one of library and organization",
                ));
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const MINIMAL: &str =
        "format: 1\nid: test\ntemplate: 0\nversion: 1.0.0\nname: Test\nsummary: A test.\n";

    #[test]
    fn minimal_ok() {
        let m = parse_meta(MINIMAL.replace("template: 0\n", "").as_bytes()).unwrap();
        assert_eq!(m.id, "test");
        assert_eq!(m.format, 1);
        assert!(m.inputs.is_empty());
        assert!(m.apps.is_empty());
        assert!(m.skill_refs.is_empty());
        assert!(m.effort.is_none());
        assert!(m.min_core.is_none());
    }

    #[test]
    fn grammar_helpers() {
        assert!(id_ok("service-monitor"));
        assert!(!id_ok("Service-monitor"));
        assert!(!id_ok("a--b"));
        assert!(!id_ok(&"a".repeat(65)));
        assert!(semver_ok("1.2.3"));
        assert!(semver_ok("1.2.3-rc.1"));
        assert!(!semver_ok("1.2"));
        assert!(!semver_ok("1.2.3+build"));
        for (id, ok) in crate::test_fixtures::DESK_INPUT_IDS {
            assert_eq!(input_id_ok(id), *ok, "{id}");
        }
    }

    #[test]
    fn unknown_key_refused() {
        let err =
            parse_meta(b"format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\npublisher: x\n")
                .unwrap_err();
        assert_eq!(err[0].code, ProblemCode::TemplateYaml);
        assert!(err[0].path.contains("publisher"), "{}", err[0].path);
    }

    #[test]
    fn anchors_tags_and_multi_documents_refused() {
        for yaml in [
            "format: &a 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\n",
            "format: !!str 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\n",
            "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\n---\nformat: 1\n",
        ] {
            let err = parse_meta(yaml.as_bytes()).unwrap_err();
            assert_eq!(err[0].code, ProblemCode::TemplateYaml, "{yaml}");
        }
    }

    #[test]
    fn format_values() {
        let bad =
            parse_meta(b"format: 2\nid: t\nversion: 1.0.0\nname: N\nsummary: S\n").unwrap_err();
        assert_eq!(bad[0].code, ProblemCode::TemplateFormatUnsupported);
        let bad =
            parse_meta(b"format: one\nid: t\nversion: 1.0.0\nname: N\nsummary: S\n").unwrap_err();
        assert_eq!(bad[0].code, ProblemCode::TemplateFormatUnsupported);
    }

    #[test]
    fn inputs_and_apps() {
        let meta = parse_meta(crate::test_fixtures::INPUTS_AND_APPS_YAML.as_bytes()).unwrap();
        assert_eq!(meta.inputs.len(), 1);
        assert_eq!(meta.inputs[0].default.as_deref(), Some("OPS"));
        assert_eq!(meta.apps.len(), 2);
        assert!(matches!(
            &meta.apps[0],
            AppEntry::App { tools: Some(_), .. }
        ));
        assert!(matches!(&meta.apps[1], AppEntry::AnyOf { .. }));
        assert!(matches!(&meta.skill_refs[0], SkillRef::Organization { .. }));
    }

    #[test]
    fn app_rules() {
        use crate::test_fixtures::{
            APP_BOTH_YAML, APP_DUP_YAML, APP_EMPTY_TOOLS_YAML, APP_OVERLAP_YAML,
        };
        assert!(
            parse_meta(APP_DUP_YAML.as_bytes())
                .unwrap_err()
                .iter()
                .any(|p| p.message.contains("at most one entry"))
        );
        assert!(
            parse_meta(APP_BOTH_YAML.as_bytes())
                .unwrap_err()
                .iter()
                .any(|p| p.path.ends_with("/apps/0"))
        );
        assert!(
            parse_meta(APP_EMPTY_TOOLS_YAML.as_bytes())
                .unwrap_err()
                .iter()
                .any(|p| p.path.contains("tools"))
        );
        assert!(
            parse_meta(APP_OVERLAP_YAML.as_bytes())
                .unwrap_err()
                .iter()
                .any(|p| p.message.contains("both"))
        );
    }

    #[test]
    fn pattern_over_size_limit_is_a_field_problem() {
        // A pattern whose compiled size exceeds the limit fails to build; that must be a
        // problem, never a panic.
        let big = "a{100}bc{100}";
        let yaml = format!(
            "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\ninputs:\n  - id: x\n    label: X\n    pattern: \"({big}){{100}}\"\n"
        );
        let err = parse_meta(yaml.as_bytes()).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.code == ProblemCode::TemplateField && p.path.contains("pattern"))
        );
    }

    #[test]
    fn skill_refs() {
        use crate::test_fixtures::{LIBRARY_NORMALIZED, LIBRARY_PAGE_YAML, LIBRARY_SSH_YAML};
        let meta = parse_meta(LIBRARY_PAGE_YAML.as_bytes()).unwrap();
        match &meta.skill_refs[0] {
            SkillRef::Library {
                address,
                ref_,
                path,
                skill,
                ..
            } => {
                assert_eq!(address.url, LIBRARY_NORMALIZED);
                assert_eq!(ref_.as_deref(), Some("main"));
                assert_eq!(path.as_deref(), Some("team"));
                assert_eq!(skill, "incident-writeup");
            }
            _ => panic!("expected a library reference"),
        }
        let err = parse_meta(LIBRARY_SSH_YAML.as_bytes()).unwrap_err();
        assert!(
            err.iter().any(
                |p| p.code == ProblemCode::TemplateSkillRefInvalid && p.message.contains("ssh")
            )
        );
        let base = "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\n";
        let core = format!("{base}skills:\n  - core: builtin\n    why: y\n");
        assert!(parse_meta(core.as_bytes()).is_err());
        let reserved = format!("{base}skills:\n  - organization: aurora-something\n    why: y\n");
        assert!(
            parse_meta(reserved.as_bytes())
                .unwrap_err()
                .iter()
                .any(|p| p.message.contains("reserved"))
        );
    }
}
