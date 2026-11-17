//! The configuration fingerprint (D-291, C-866, C-883): a canonical JSON object of everything an
//! agent's answers depend on, and its SHA-256. Pure: the caller gathers the inputs. The object
//! holds hashes and ids only: no brief or skill content, no question text, no secret value.

use aurora_registry::snapshot::canonical_bytes;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The keys of the fingerprint object, exactly (C-866, C-883).
pub const FIELDS: [&str; 10] = [
    "instructions_sha256",
    "harness",
    "model",
    "effort",
    "model_roles",
    "subagent_model",
    "skills",
    "granted_rule_ids",
    "judge",
    "question_set_version",
];

/// One skill of the agent's effective set.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    /// `agent` | `workspace` | `org`.
    pub tier: String,
    pub name: String,
    pub content_sha256: String,
}

/// The gathered inputs.
#[derive(Debug, Clone)]
pub struct Input {
    pub brief: String,
    pub harness: Option<String>,
    pub provider: Option<String>,
    pub endpoint_id: Option<String>,
    pub model_id: Option<String>,
    pub effort: Option<String>,
    /// The resolved `large`, `medium`, `small` models (C-883).
    pub model_roles: [Option<String>; 3],
    pub subagent_model: String,
    pub skills: Vec<SkillEntry>,
    pub granted_rule_ids: Vec<Uuid>,
    pub judge: Option<(Uuid, String)>,
    pub question_set_version: String,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// SHA-256 of a skill's content: its description, body and files (path, executable bit, bytes),
/// each part length-prefixed so no two contents share a hash by moving a boundary.
pub fn skill_content_sha256(
    description: &str,
    body: &str,
    files: &[(String, bool, Vec<u8>)],
) -> String {
    let mut h = Sha256::new();
    let mut part = |b: &[u8]| {
        h.update((b.len() as u64).to_be_bytes());
        h.update(b);
    };
    part(description.as_bytes());
    part(body.as_bytes());
    let mut sorted: Vec<_> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, exec, content) in sorted {
        part(path.as_bytes());
        part(&[u8::from(*exec)]);
        part(content);
    }
    hex::encode(h.finalize())
}

/// SHA-256 over the enabled questions' `id` and `version`, sorted by id.
pub fn question_set_version(enabled: &[(Uuid, i32)]) -> String {
    let mut v: Vec<_> = enabled.to_vec();
    v.sort();
    let mut text = String::new();
    for (id, version) in v {
        text.push_str(&format!("{id}:{version}\n"));
    }
    sha256_hex(text.as_bytes())
}

/// The fingerprint object.
pub fn build(i: &Input) -> Value {
    let mut skills: Vec<&SkillEntry> = i.skills.iter().collect();
    skills.sort_by(|a, b| (&a.tier, &a.name).cmp(&(&b.tier, &b.name)));
    let mut rules = i.granted_rule_ids.clone();
    rules.sort();
    rules.dedup();
    json!({
        "instructions_sha256": sha256_hex(i.brief.as_bytes()),
        "harness": i.harness,
        "model": {
            "provider": i.provider,
            "endpoint_id": i.endpoint_id,
            "model_id": i.model_id,
        },
        "effort": i.effort,
        "model_roles": {
            "large": i.model_roles[0],
            "medium": i.model_roles[1],
            "small": i.model_roles[2],
        },
        "subagent_model": i.subagent_model,
        "skills": skills.iter().map(|s| json!({
            "tier": s.tier, "name": s.name, "content_sha256": s.content_sha256,
        })).collect::<Vec<_>>(),
        "granted_rule_ids": rules,
        "judge": i.judge.as_ref().map(|(id, model)| json!({"secret_id": id, "model_id": model})),
        "question_set_version": i.question_set_version,
    })
}

/// SHA-256 of the canonical bytes (compact JSON, sorted keys).
pub fn hash(fingerprint: &Value) -> String {
    sha256_hex(&canonical_bytes(fingerprint))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Input {
        Input {
            brief: "Be brief.".into(),
            harness: Some("claude-code".into()),
            provider: Some("anthropic".into()),
            endpoint_id: Some("anthropic".into()),
            model_id: Some("m1".into()),
            effort: Some("medium".into()),
            model_roles: [Some("big".into()), None, Some("tiny".into())],
            subagent_model: "inherit".into(),
            skills: vec![
                SkillEntry {
                    tier: "org".into(),
                    name: "b".into(),
                    content_sha256: "11".into(),
                },
                SkillEntry {
                    tier: "agent".into(),
                    name: "a".into(),
                    content_sha256: "22".into(),
                },
            ],
            granted_rule_ids: vec![Uuid::from_u128(2), Uuid::from_u128(1)],
            judge: Some((Uuid::from_u128(9), "judge-1".into())),
            question_set_version: question_set_version(&[(Uuid::from_u128(5), 1)]),
        }
    }

    fn h(i: &Input) -> String {
        hash(&build(i))
    }

    #[test]
    fn identical_configuration_gives_identical_hash_whatever_the_input_order() {
        let a = base();
        let mut b = base();
        b.skills.reverse();
        b.granted_rule_ids.reverse();
        assert_eq!(h(&a), h(&b));
        assert_eq!(h(&a).len(), 64);
    }

    #[test]
    fn the_field_list_is_the_contract_exactly() {
        let v = build(&base());
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        let mut want: Vec<_> = FIELDS.iter().map(|s| (*s).to_owned()).collect();
        want.sort();
        assert_eq!(keys, want);
        let mut model: Vec<_> = v["model"].as_object().unwrap().keys().cloned().collect();
        model.sort();
        assert_eq!(model, ["endpoint_id", "model_id", "provider"]);
        let mut roles: Vec<_> = v["model_roles"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        roles.sort();
        assert_eq!(roles, ["large", "medium", "small"]);
    }

    #[test]
    fn every_input_that_matters_changes_the_hash() {
        let a = h(&base());
        let mut c = base();
        c.brief = "Be long.".into();
        assert_ne!(h(&c), a, "brief");
        let mut c = base();
        c.model_id = Some("m2".into());
        assert_ne!(h(&c), a, "model");
        let mut c = base();
        c.effort = Some("high".into());
        assert_ne!(h(&c), a, "effort");
        let mut c = base();
        c.harness = Some("codex".into());
        assert_ne!(h(&c), a, "harness");
        let mut c = base();
        c.skills[0].content_sha256 = "33".into();
        assert_ne!(h(&c), a, "skill content");
        let mut c = base();
        c.granted_rule_ids.push(Uuid::from_u128(3));
        assert_ne!(h(&c), a, "granted rules");
        let mut c = base();
        c.model_roles[1] = Some("mid".into());
        assert_ne!(h(&c), a, "a model role (C-883)");
        let mut c = base();
        c.subagent_model = "small".into();
        assert_ne!(h(&c), a, "subagent model (C-883)");
        let mut c = base();
        c.judge = None;
        assert_ne!(h(&c), a, "judge");
        let mut c = base();
        c.question_set_version = question_set_version(&[(Uuid::from_u128(5), 2)]);
        assert_ne!(h(&c), a, "question set version");
    }

    #[test]
    fn the_object_holds_no_content_no_question_text_no_secret_value() {
        let mut i = base();
        i.brief = "canary-brief-text".into();
        let text = build(&i).to_string();
        assert!(!text.contains("canary-brief-text"));
        // Only hashes and ids: the judge is a secret *id* and a model id.
        assert!(text.contains(&Uuid::from_u128(9).to_string()));
        let sk = skill_content_sha256("d", "canary-skill-body", &[]);
        assert!(!sk.contains("canary") && sk.len() == 64);
    }

    #[test]
    fn skill_content_hash_moves_with_description_body_and_files() {
        let f = |c: &[u8]| vec![("a.sh".to_owned(), false, c.to_vec())];
        let base = skill_content_sha256("d", "b", &f(b"x"));
        assert_ne!(base, skill_content_sha256("d2", "b", &f(b"x")));
        assert_ne!(base, skill_content_sha256("d", "b2", &f(b"x")));
        assert_ne!(base, skill_content_sha256("d", "b", &f(b"y")));
        // A boundary moved between description and body is another content.
        assert_ne!(
            skill_content_sha256("ab", "c", &[]),
            skill_content_sha256("a", "bc", &[])
        );
    }

    #[test]
    fn question_set_version_is_order_free_and_sensitive_to_id_and_version() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(
            question_set_version(&[(a, 1), (b, 1)]),
            question_set_version(&[(b, 1), (a, 1)])
        );
        assert_ne!(
            question_set_version(&[(a, 1)]),
            question_set_version(&[(a, 2)])
        );
        assert_ne!(
            question_set_version(&[(a, 1)]),
            question_set_version(&[(b, 1)])
        );
    }
}
