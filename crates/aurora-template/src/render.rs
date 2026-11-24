//! Filling inputs (C-1104) and the one-pass placeholder substitution (C-1141):
//! `{input.<id>}`, `{agent.name}` and `{person.name}` are replaced in the fillable texts, an
//! inserted value is never scanned again, and any other text in braces stays as typed.

use std::collections::BTreeMap;

use crate::meta::{compile_pattern, input_id_ok};
use crate::model::{Automation, InputDecl, InputKind, TemplateContent};

/// Why an input value was refused (C-1104).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputProblemCode {
    Missing,
    Unknown,
    TooLong,
    Pattern,
    NotAnOption,
    ControlCharacters,
}

impl InputProblemCode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unknown => "unknown",
            Self::TooLong => "too_long",
            Self::Pattern => "pattern",
            Self::NotAnOption => "not_an_option",
            Self::ControlCharacters => "control_characters",
        }
    }
}

impl std::fmt::Display for InputProblemCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One input refusal: the code, the input id it is about (for `unknown`, the key that was not
/// declared) and one sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputProblem {
    pub code: InputProblemCode,
    pub input: String,
    pub message: String,
}

impl std::fmt::Display for InputProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}: {}", self.input, self.code, self.message)
    }
}

/// The checked values: every declared input that was given (an optional input left out becomes
/// the empty string at render time).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckedInputs {
    values: BTreeMap<String, String>,
}

impl CheckedInputs {
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&str> {
        self.values.get(id).map(String::as_str)
    }

    /// (P6-TPL-3, C-1112) The stored input values of an agent, for rendering a new version's
    /// parts in an update offer: only the values the agent still has (a needed input renders
    /// as `{input.<id>}`).
    #[must_use]
    pub fn from_values(values: BTreeMap<String, String>) -> Self {
        Self { values }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// `check_inputs`: every rule at once, all problems back (C-1104).
pub fn check_inputs(
    inputs: &[InputDecl],
    values: &BTreeMap<String, String>,
) -> Result<CheckedInputs, Vec<InputProblem>> {
    let mut problems = Vec::new();
    let mut checked = BTreeMap::new();
    for key in values.keys() {
        if !inputs.iter().any(|i| i.id == *key) {
            problems.push(InputProblem {
                code: InputProblemCode::Unknown,
                input: (*key).clone(),
                message: "no input with this id is declared in template.yaml".into(),
            });
        }
    }
    for input in inputs {
        let given = values.get(&input.id).map(String::as_str);
        let value = match given {
            None | Some("") if input.required => {
                problems.push(InputProblem {
                    code: InputProblemCode::Missing,
                    input: input.id.clone(),
                    message: "this input is required".into(),
                });
                continue;
            }
            // An optional input left empty is the empty string (C-1141); no other rules run.
            None | Some("") => {
                checked.insert(input.id.clone(), String::new());
                continue;
            }
            Some(v) => v,
        };
        // Control characters first: a value with a newline never reaches the pattern or length
        // checks (C-1104: a value may not contain newlines or control characters).
        if value.chars().any(char::is_control) {
            problems.push(InputProblem {
                code: InputProblemCode::ControlCharacters,
                input: input.id.clone(),
                message: "the value must not contain newlines or control characters".into(),
            });
            continue;
        }
        if input.kind == InputKind::Choice
            && !input.options.is_empty()
            && !input.options.iter().any(|o| o == value)
        {
            problems.push(InputProblem {
                code: InputProblemCode::NotAnOption,
                input: input.id.clone(),
                message: "the value must be one of the offered options".into(),
            });
            continue;
        }
        if input.kind == InputKind::Text {
            let limit = input.max_length.unwrap_or(100);
            if value.chars().count() > limit as usize {
                problems.push(InputProblem {
                    code: InputProblemCode::TooLong,
                    input: input.id.clone(),
                    message: format!("the value is longer than {limit} characters"),
                });
                continue;
            }
            if let Some(pattern) = &input.pattern {
                let whole = compile_pattern(pattern)
                    .is_some_and(|r| r.find(value).is_some_and(|m| m.as_str() == value));
                if !whole {
                    problems.push(InputProblem {
                        code: InputProblemCode::Pattern,
                        input: input.id.clone(),
                        message: "the value does not match the expected shape".into(),
                    });
                    continue;
                }
            }
        }
        checked.insert(input.id.clone(), value.to_owned());
    }
    if problems.is_empty() {
        Ok(CheckedInputs { values: checked })
    } else {
        Err(problems)
    }
}

/// One rendered text of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedMemory {
    pub key: String,
    pub content: String,
    pub locked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSkill {
    pub name: String,
    pub use_it_when: String,
    pub body: String,
    /// The skill's other files, byte for byte (never placeholder-filled).
    pub files: Vec<crate::model::TemplateFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderedAutomation {
    Schedule {
        name: String,
        prompt: String,
        cron: String,
    },
    Webhook {
        name: String,
        instructions: String,
        verification: crate::model::Verification,
    },
}

/// The template as created (C-1104): everything copied into the new agent, placeholders filled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedTemplate {
    pub agent_name: String,
    pub may_message_first: bool,
    pub instructions: String,
    /// The README is shown at creation, never applied, and is not placeholder-filled.
    pub readme: String,
    pub memories: Vec<RenderedMemory>,
    pub skills: Vec<RenderedSkill>,
    pub helpers: Vec<crate::model::TemplateFile>,
    pub automations: Vec<RenderedAutomation>,
    pub avatar: Option<(String, Vec<u8>)>,
}

/// `render`: one pass over every fillable text (C-1141). Inserted values are never scanned
/// again, so a value containing `{input.x}` stays as typed. An optional input left empty is the
/// empty string.
#[must_use]
pub fn render(
    content: &TemplateContent,
    inputs: &CheckedInputs,
    agent_name: &str,
    person_name: &str,
) -> RenderedTemplate {
    let c = content;
    let fill = |text: &str| substitute(text, inputs, agent_name, person_name);
    RenderedTemplate {
        agent_name: agent_name.to_owned(),
        may_message_first: c.may_message_first,
        instructions: fill(&c.instructions),
        readme: c.readme.clone(),
        memories: c
            .memories
            .iter()
            .map(|m| RenderedMemory {
                key: m.key.clone(),
                content: fill(&m.content),
                locked: m.locked,
            })
            .collect(),
        skills: c
            .skills
            .iter()
            .map(|s| RenderedSkill {
                name: s.name.clone(),
                use_it_when: fill(&s.use_it_when),
                body: fill(&s.body),
                files: s.files.clone(),
            })
            .collect(),
        helpers: c.helpers.clone(),
        automations: c
            .automations
            .iter()
            .map(|a| match a {
                Automation::Schedule { name, prompt, cron } => RenderedAutomation::Schedule {
                    name: name.clone(),
                    prompt: fill(prompt),
                    cron: cron.clone(),
                },
                Automation::Webhook {
                    name,
                    instructions,
                    verification,
                } => RenderedAutomation::Webhook {
                    name: name.clone(),
                    instructions: fill(instructions),
                    verification: *verification,
                },
            })
            .collect(),
        avatar: c.avatar.clone(),
    }
}

/// The one-pass substitution. A `{…}` token is substituted only when it is exactly
/// `{agent.name}`, `{person.name}` or `{input.<declared id>}`; anything else — including an
/// undeclared `{input.…}`, which validation refuses earlier — stays as typed.
fn substitute(text: &str, inputs: &CheckedInputs, agent_name: &str, person_name: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && let Some(end) = text[i + 1..].find('}')
        {
            let token = &text[i + 1..i + 1 + end];
            let replacement = if token == "agent.name" {
                Some(agent_name.to_owned())
            } else if token == "person.name" {
                Some(person_name.to_owned())
            } else if let Some(id) = token.strip_prefix("input.") {
                if input_id_ok(id) {
                    inputs.get(id).map(str::to_owned)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(value) = replacement {
                out.push_str(&value);
                i += end + 2;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The `{input.<id>}` placeholders a text names (used by validation to refuse unknown ones);
/// non-input tokens and undeclared ids are the caller's concern.
pub(crate) fn placeholder_inputs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && let Some(end) = text[i + 1..].find('}')
            && let Some(id) = text[i + 1..i + 1 + end].strip_prefix("input.")
        {
            out.push(id.to_owned());
            i += end + 2;
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or('\u{fffd}');
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn decl(id: &str, required: bool) -> InputDecl {
        InputDecl {
            id: id.to_owned(),
            label: "L".into(),
            help: None,
            kind: InputKind::Text,
            required,
            pattern: None,
            max_length: None,
            options: vec![],
            default: None,
        }
    }

    fn checked(inputs: &[InputDecl], values: &[(&str, &str)]) -> CheckedInputs {
        let map: BTreeMap<String, String> = values
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        check_inputs(inputs, &map).unwrap()
    }

    #[test]
    fn every_input_problem_code() {
        let inputs = vec![
            decl("jira_project", true),
            InputDecl {
                kind: InputKind::Choice,
                options: vec!["Low".into(), "High".into()],
                ..decl("level", false)
            },
            InputDecl {
                pattern: Some("^[A-Z]+$".into()),
                ..decl("key", false)
            },
            InputDecl {
                max_length: Some(5),
                ..decl("short", false)
            },
            decl("extra", false),
        ];
        let mut values = BTreeMap::new();
        values.insert("jira_project".to_owned(), String::new());
        values.insert("level".to_owned(), "Medium".into());
        values.insert("key".to_owned(), "ops1".into());
        values.insert("short".to_owned(), "123456".into());
        values.insert("extra".to_owned(), "a\nb".into());
        values.insert("undeclared".to_owned(), "x".into());
        let err = check_inputs(&inputs, &values).unwrap_err();
        let codes: Vec<_> = err.iter().map(|p| p.code).collect();
        assert!(codes.contains(&InputProblemCode::Missing));
        assert!(codes.contains(&InputProblemCode::Unknown));
        assert!(codes.contains(&InputProblemCode::NotAnOption));
        assert!(codes.contains(&InputProblemCode::Pattern));
        assert!(codes.contains(&InputProblemCode::TooLong));
        assert!(codes.contains(&InputProblemCode::ControlCharacters));
        // All at once, each naming its input.
        assert_eq!(err.len(), 6);
        assert!(err.iter().any(|p| p.input == "undeclared"));
        assert!(err.iter().any(|p| p.input == "jira_project"));
        let ok = checked(&inputs, &[("jira_project", "OPS"), ("key", "OPS")]);
        assert_eq!(ok.get("key"), Some("OPS"));
        // An optional input left out renders as the empty string.
        assert_eq!(ok.get("level"), Some(""));
    }

    #[test]
    fn optional_empty_and_max_length_default() {
        // An optional input left empty is fine and renders as the empty string; a text input's
        // default max_length is 100 (TEMPLATE_INPUT_VALUE_MAX is the hard cap).
        let inputs = vec![decl("optional", false)];
        let ok = checked(&inputs, &[("optional", "")]);
        assert_eq!(ok.get("optional"), Some(""));
        let mut values = BTreeMap::new();
        values.insert("optional".to_owned(), "x".repeat(101));
        let err = check_inputs(&inputs, &values).unwrap_err();
        assert_eq!(err[0].code, InputProblemCode::TooLong);
    }

    #[test]
    fn pattern_matches_the_whole_value() {
        let inputs = vec![InputDecl {
            pattern: Some("^[A-Z][A-Z0-9]+$".into()),
            ..decl("key", false)
        }];
        let mut values = BTreeMap::new();
        values.insert("key".to_owned(), "OPS1x".into());
        assert_eq!(
            check_inputs(&inputs, &values).unwrap_err()[0].code,
            InputProblemCode::Pattern
        );
        values.insert("key".to_owned(), "OPS1".into());
        assert!(check_inputs(&inputs, &values).is_ok());
    }

    #[test]
    fn one_pass_substitution_hostile_values() {
        let inputs = vec![decl("jira_project", true)];
        let values = checked(&inputs, &[("jira_project", "{input.key} {agent.name}")]);
        let out = substitute(
            "Hello {person.name}, {input.jira_project} and {input.key} stay {agent.name}.",
            &values,
            "Monitor",
            "Ada",
        );
        // The inserted value is never scanned again (its `{agent.name}` stays as typed), and an
        // undeclared input stays literal.
        assert_eq!(
            out,
            "Hello Ada, {input.key} {agent.name} and {input.key} stay Monitor."
        );
        // A declared input whose value itself carries a token: also never rescanned.
        let map2: BTreeMap<String, String> = [("key".to_owned(), "inner".to_owned())]
            .into_iter()
            .collect();
        let with_inner = CheckedInputs { values: map2 };
        let out = substitute("{input.key}", &with_inner, "A", "P");
        assert_eq!(out, "inner");
    }

    #[test]
    fn braces_kept_literally() {
        let values = CheckedInputs::default();
        for (text, want) in [
            ("{}", "{}"),
            ("{input.}", "{input.}"),
            ("{Input.x}", "{Input.x}"),
            ("{input.1bad}", "{input.1bad}"),
            ("a { b } c", "a { b } c"),
            ("{unclosed {input.x}", "{unclosed {input.x}"),
            ("{agent.name}-v{person.name}", "A-vP"),
        ] {
            let got = substitute(text, &values, "A", "P");
            assert_eq!(got, want, "{text}");
        }
    }

    #[test]
    fn render_fills_only_the_fillable_places() {
        let c = crate::validate::tests::sample_content();
        let inputs = vec![decl("jira_project", true)];
        let values = checked(&inputs, &[("jira_project", "OPS")]);
        let r = render(&c, &values, "Monitor", "Ada");
        assert_eq!(r.instructions, "Watch OPS tickets and the morning board.\n");
        assert_eq!(r.readme, "# Service monitor\nFor the service desk.\n");
        assert_eq!(r.memories[0].content, "OPS is the desk's project.\n");
        assert_eq!(r.skills[0].use_it_when, "The person asks about OPS.");
        assert_eq!(r.skills[0].body, "Look at OPS.\n");
        assert_eq!(r.skills[0].files[0].bytes, b"#!/bin/sh\necho run\n");
        assert!(r
            .automations
            .iter()
            .any(|a| matches!(a, RenderedAutomation::Schedule { prompt, .. } if prompt == "Check the OPS dashboards.")));
        assert_eq!(r.helpers[0].path, "alert-summary.sh");
        assert_eq!(r.agent_name, "Monitor");
    }

    #[test]
    fn placeholders_found() {
        assert_eq!(
            placeholder_inputs("a {input.x} b {agent.name} {input.jira_project}"),
            vec!["x", "jira_project"]
        );
        assert!(placeholder_inputs("no tokens {here}").is_empty());
    }
}
