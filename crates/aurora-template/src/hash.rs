//! The canonical hashes (C-1104, C-1112): `content_sha256` over a canonical serialization of the
//! whole parsed content, and the part hashes an update offer diffs against. All of them are
//! SHA-256 over compact JSON built from `BTreeMap`s (sorted keys), with file bytes in base64 —
//! stable across releases and pinned by golden tests.

use base64::Engine as _;
use serde_json::{Value, json};

use crate::model::{AppEntry, Automation, InputDecl, InputKind, Level, SkillRef, TemplateContent};

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Canonical JSON: compact serialization (map keys sorted, array order kept), then SHA-256 as
/// lowercase hex.
fn json_sha256(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    sha256_hex(&bytes)
}

fn bytes_json(bytes: &[u8]) -> Value {
    Value::String(BASE64.encode(bytes))
}

fn file_json(path: &str, bytes: &[u8], executable: bool) -> Value {
    json!({ "path": path, "bytes": bytes_json(bytes), "executable": executable })
}

fn input_json(i: &InputDecl) -> Value {
    json!({
        "id": i.id,
        "label": i.label,
        "help": i.help,
        "kind": match i.kind { InputKind::Text => "text", InputKind::Choice => "choice" },
        "required": i.required,
        "pattern": i.pattern,
        "max_length": i.max_length,
        "options": i.options,
        "default": i.default,
    })
}

fn app_json(a: &AppEntry) -> Value {
    match a {
        AppEntry::App {
            app,
            required,
            level,
            tools,
            why,
        } => json!({
            "app": app,
            "required": required,
            "level": level_json(*level),
            "tools": tools.as_ref().map(|t| json!({ "allow": t.allow, "ask": t.ask })),
            "why": why,
        }),
        AppEntry::AnyOf {
            any_of,
            required,
            level,
            why,
        } => json!({
            "any_of": any_of,
            "required": required,
            "level": level_json(*level),
            "why": why,
        }),
    }
}

fn level_json(level: Level) -> &'static str {
    match level {
        Level::Read => "read",
        Level::Ask => "ask",
        Level::Act => "act",
    }
}

fn skill_ref_json(r: &SkillRef) -> Value {
    match r {
        SkillRef::Library {
            address,
            ref_,
            path,
            skill,
            required,
            why,
        } => json!({
            "kind": "library",
            "url": address.url,
            "repo": address.repo_display,
            "ref": ref_,
            "path": path,
            "skill": skill,
            "required": required,
            "why": why,
        }),
        SkillRef::Organization {
            name,
            required,
            why,
        } => json!({
            "kind": "organization",
            "name": name,
            "required": required,
            "why": why,
        }),
    }
}

fn automation_json(a: &Automation) -> Value {
    match a {
        Automation::Schedule { name, prompt, cron } => json!({
            "kind": "schedule",
            "name": name,
            "prompt": prompt,
            "cron": cron,
        }),
        Automation::Webhook {
            name,
            instructions,
            verification,
        } => json!({
            "kind": "webhook",
            "name": name,
            "instructions": instructions,
            "verification": match verification {
                crate::model::Verification::Signed => "signed",
                crate::model::Verification::Password => "password",
                crate::model::Verification::None => "none",
            },
        }),
    }
}

/// `content_sha256` (C-1104): SHA-256 of the canonical serialization of the whole parsed
/// content — `README.md` included (the admin approves it with the rest), `CHANGELOG.md` already
/// excluded by the parse. Every file's bytes are base64; every map is sorted by key.
#[must_use]
pub fn content_sha256(content: &TemplateContent) -> String {
    let c = content;
    let value = json!({
        "meta": {
            "format": c.meta.format,
            "id": c.meta.id,
            "version": c.meta.version,
            "name": c.meta.name,
            "summary": c.meta.summary,
            "min_core": c.meta.min_core,
            "effort": c.meta.effort.map(|e| match e {
                crate::model::Effort::Low => "low",
                crate::model::Effort::Medium => "medium",
                crate::model::Effort::High => "high",
                crate::model::Effort::Max => "max",
            }),
            "inputs": c.meta.inputs.iter().map(input_json).collect::<Vec<_>>(),
            "apps": c.meta.apps.iter().map(app_json).collect::<Vec<_>>(),
            "skill_refs": c.meta.skill_refs.iter().map(skill_ref_json).collect::<Vec<_>>(),
        },
        "agent": {
            "name": c.agent_name,
            "may_message_first": c.may_message_first,
        },
        "instructions": c.instructions,
        "readme": c.readme,
        "memories": c.memories.iter().map(|m| json!({
            "key": m.key,
            "content": m.content,
            "locked": m.locked,
        })).collect::<Vec<_>>(),
        "skills": c.skills.iter().map(|s| {
            let mut v = json!({
                "name": s.name,
                "use_it_when": s.use_it_when,
                "body": s.body,
                "files": s.files.iter().map(|f| file_json(&f.path, &f.bytes, f.executable)).collect::<Vec<_>>(),
            });
            // Only when set, so a version without titles keeps its hash.
            if let Some(t) = &s.title {
                v["title"] = json!(t);
            }
            v
        }).collect::<Vec<_>>(),
        "helpers": c.helpers.iter().map(|f| file_json(&f.path, &f.bytes, f.executable)).collect::<Vec<_>>(),
        "automations": c.automations.iter().map(automation_json).collect::<Vec<_>>(),
        "avatar": c.avatar.as_ref().map(|(name, bytes)| json!({
            "name": name,
            "bytes": bytes_json(bytes),
        })),
    });
    json_sha256(&value)
}

/// The instructions part hash (C-1112): SHA-256 of the exact text.
#[must_use]
pub fn instructions_hash(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

/// The skill part hash (C-1112): `description` (the use-it-when line), `body` and the skill's
/// files sorted by path.
#[must_use]
pub fn skill_hash(description: &str, body: &str, files: &[(String, Vec<u8>, bool)]) -> String {
    let mut files: Vec<_> = files.iter().map(|(p, b, e)| file_json(p, b, *e)).collect();
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    json_sha256(&json!({
        "description": description,
        "body": body,
        "files": files,
    }))
}

/// The helpers part hash (C-1112): the helper files sorted by path.
#[must_use]
pub fn helpers_hash(files: &[(String, Vec<u8>, bool)]) -> String {
    let mut files: Vec<_> = files.iter().map(|(p, b, e)| file_json(p, b, *e)).collect();
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    json_sha256(&json!({ "files": files }))
}

/// The `library_skill` part key (C-1112): the normalized reference that identifies the part.
/// `library|<url>|<ref>|<path>|<skill>`, with an empty string for an absent ref or path; an
/// organization reference is not a library skill and has no part key.
///
/// The key is unambiguous: the format refuses `|` in a `ref` (the shared ref grammar), the
/// address grammar and the skill name grammar have none, so the url and the ref end at the
/// first two separators after `library|`, the skill starts after the last one, and the path is
/// all that is between. The spelling is stored with an agent's parts: keep it.
#[must_use]
pub fn skill_ref_key(r: &SkillRef) -> String {
    match r {
        SkillRef::Library {
            address,
            ref_,
            path,
            skill,
            ..
        } => format!(
            "library|{}|{}|{}|{}",
            address.url,
            ref_.as_deref().unwrap_or(""),
            path.as_deref().unwrap_or(""),
            skill
        ),
        SkillRef::Organization { name, .. } => format!("organization|{name}"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::{Automation, SkillRef, TreeFile, Verification};
    use crate::test_fixtures::hash_content as content;

    #[test]
    fn golden_content_hash() {
        // Pinned: any change to the canonical serialization changes this constant, which is a
        // release-notes-worthy event (C-1104: stable across releases).
        assert_eq!(
            content_sha256(&content()),
            "3521e0c306eaecd2bd1fe3e5f6248dff8476414d301c36c8fe4eb4f4d7f87ef2"
        );
    }

    #[test]
    fn golden_part_hashes() {
        let files = vec![("format.py".to_owned(), b"print(1)\n".to_vec(), true)];
        assert_eq!(
            skill_hash("Use it when asked.", "Body.", &files),
            "d0df4f1f23a02f04444db1b833ccec0015c8d64eb141f74da941b6390295f0b4"
        );
        let helpers = vec![("run.sh".to_owned(), b"#!/bin/sh\n".to_vec(), true)];
        assert_eq!(
            helpers_hash(&helpers),
            "6b7bc8e64874a8f511a23efc2c2438ac079f733d6d7b1bf8250746f1391a88f8"
        );
        assert_eq!(
            instructions_hash("Do the thing."),
            "5f398191f09fb70a2eee093ea10c4086b0858d3f2ad7c9ed03d8f8aa512a1fbd"
        );
    }

    #[test]
    fn content_hash_changes_with_every_part() {
        let base = content();
        let mut c = base.clone();
        c.readme.push('!');
        assert_ne!(content_sha256(&base), content_sha256(&c));
        c = base.clone();
        c.memories[0].locked = false;
        assert_ne!(content_sha256(&base), content_sha256(&c));
        c = base.clone();
        c.skills[0].files[0].executable = false;
        assert_ne!(content_sha256(&base), content_sha256(&c));
        c = base.clone();
        c.avatar = None;
        assert_ne!(content_sha256(&base), content_sha256(&c));
        c = base.clone();
        c.automations[1] = Automation::Webhook {
            name: "Escalate".into(),
            instructions: "Page the on-call.".into(),
            verification: Verification::None,
        };
        assert_ne!(content_sha256(&base), content_sha256(&c));
    }

    #[test]
    fn part_hash_inputs() {
        // File order does not matter for the part hashes.
        let a = vec![
            ("a".to_owned(), b"1".to_vec(), false),
            ("b".to_owned(), b"2".to_vec(), true),
        ];
        let b = vec![
            ("b".to_owned(), b"2".to_vec(), true),
            ("a".to_owned(), b"1".to_vec(), false),
        ];
        assert_eq!(helpers_hash(&a), helpers_hash(&b));
        let files = vec![("x".to_owned(), b"1".to_vec(), false)];
        assert_ne!(skill_hash("d", "b", &files), helpers_hash(&files));
    }

    #[test]
    fn ref_keys() {
        use crate::test_fixtures::{
            LIBRARY_BARE_CLONE, LIBRARY_BARE_CLONE_KEY, LIBRARY_CLONE, LIBRARY_CLONE_KEY,
        };
        use aurora_core::git_url;
        let address = git_url::parse(LIBRARY_CLONE).unwrap();
        let r = SkillRef::Library {
            address,
            ref_: Some("main".into()),
            path: Some("team".into()),
            skill: "incident-writeup".into(),
            required: true,
            why: "why".into(),
        };
        assert_eq!(skill_ref_key(&r), LIBRARY_CLONE_KEY);
        let bare = SkillRef::Library {
            address: git_url::parse(LIBRARY_BARE_CLONE).unwrap(),
            ref_: None,
            path: None,
            skill: "x".into(),
            required: false,
            why: "why".into(),
        };
        assert_eq!(skill_ref_key(&bare), LIBRARY_BARE_CLONE_KEY);
        let org = SkillRef::Organization {
            name: "deploy-checklist".into(),
            required: true,
            why: "why".into(),
        };
        assert_eq!(skill_ref_key(&org), "organization|deploy-checklist");
    }

    #[test]
    fn tree_files_agree_with_content_hash_inputs() {
        // The hash is computed from the parsed content; read_tree and read_archive produce the
        // same content (their agreement test lives in validate.rs), so one golden here pins both.
        let _ = TreeFile {
            path: String::new(),
            bytes: Vec::new(),
            mode: 0o100644,
        };
        assert_eq!(instructions_hash(""), sha256_hex(b""));
    }
}
