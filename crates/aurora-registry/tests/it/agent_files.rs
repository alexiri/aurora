//! (P5-GUIDE-1, P5-APPSKILL-1, `docs/24`) What a package gives agents: compatibility of existing snapshots, the linter matrix,
//! snapshot shape and the renderer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::fmt::Write as _;
use std::path::PathBuf;

use aurora_registry::Manifest;
use aurora_registry::snapshot::{ActivePackage, compile};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A one-app manifest: `tools` and `agent` are YAML fragments at app level (4-space indent), `extra`
/// more app keys.
pub fn yaml(tools: &str, agent: &str, extra: &str) -> String {
    format!(
        "package: {{ id: demo, version: 1.2.3, contract: 1, min_core: 0.1.0, publisher: aurora, name: Demo }}\napps:\n  - about: {{ id: demo-app, name: Demo app, api_docs: \"https://docs.example.test/api\" }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]\n    hosts:\n      - match: {{ host: api.example.test }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n{extra}{tools}{agent}"
    )
}

pub const TOOLS: &str = "    tools:
      read:
        items:
          - { id: list_things, name: List things, description: All things, host: api.example.test, path: /v1/things, methods: [GET] }
          - { id: get_thing, name: Get a thing, host: api.example.test, path: \"/v1/things/*\", methods: [GET] }
      write:
        items:
          - { id: make_thing, name: Make a thing, host: api.example.test, path: /v1/things, methods: [POST] }
";

pub const AGENT: &str = "    agent:
      summary: \"Things, listed and made.\"
      instructions: |
        Things are addressed by id.
";

/// `AGENT` in the format before P5-APPSKILL (D-453): `agent.guide` entries.
pub const LEGACY_AGENT: &str = "    agent:
      summary: \"Things, listed and made.\"
      instructions: |
        Things are addressed by id.
      guide:
        - { file: make-and-check.md, title: \"Make a thing and check it\", kind: reference }
        - { file: api.md, title: \"API conventions\" }
        - { file: codes.tsv, title: \"Codes\" }
";

/// A `SKILL.md` as an author writes it.
pub fn skill_md(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}")
}

/// Compile `y` with the agent files of `files` (`archive path -> bytes`, `exec` = executable
/// paths) and return the snapshot.
pub fn snapshot_with(
    y: &str,
    files: &[(&str, &str)],
    exec: &[&str],
) -> aurora_registry::snapshot::Snapshot {
    use aurora_registry::snapshot::{agent_files, compile_with_agent_files};
    let m = Manifest::parse(y, "99.0.0").unwrap();
    let mut pkg = aurora_registry::Package::default();
    for (f, c) in files {
        pkg.files.insert((*f).to_owned(), c.as_bytes().to_vec());
    }
    for e in exec {
        pkg.executable.insert((*e).to_owned());
    }
    let p = ActivePackage {
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
        manifest: m.clone(),
    };
    let g = std::collections::BTreeMap::from([("demo".to_owned(), agent_files(&m, &pkg))]);
    let c = compile_with_agent_files(&[p], &[], &[], 1, &g).unwrap();
    aurora_registry::snapshot::Snapshot::from_slice(&c.json).unwrap()
}

/// [`snapshot_with`] for files under `agent/demo-app/` (the operations file).
pub fn snapshot_of(y: &str, files: &[(&str, &str)]) -> aurora_registry::snapshot::Snapshot {
    let full: Vec<(String, &str)> = files
        .iter()
        .map(|(f, c)| (format!("agent/demo-app/{f}"), *c))
        .collect();
    let refs: Vec<(&str, &str)> = full.iter().map(|(f, c)| (f.as_str(), *c)).collect();
    snapshot_with(y, &refs, &[])
}

/// The snapshot carries `summary`, the skills with their digests (SKILL.md digest, file digests,
/// executable bits, sorted) and omits `skills` when the app has none (the fragment is then the
/// pre-P5-APPSKILL one).
#[test]
fn snapshot_carries_summary_and_skills_with_digests_and_omits_them_when_absent() {
    use sha2::{Digest, Sha256};
    let md_b = skill_md("demo-app-b", "Second skill.", "Steps.\n");
    let md_a = skill_md("demo-app-a", "First skill.", "Steps.\n");
    let snap = snapshot_with(
        &yaml(TOOLS, AGENT, ""),
        &[
            ("skills/demo-app/demo-app-b/SKILL.md", &md_b),
            ("skills/demo-app/demo-app-a/SKILL.md", &md_a),
            (
                "skills/demo-app/demo-app-a/scripts/run.sh",
                "#!/bin/sh\necho hi\n",
            ),
            ("skills/demo-app/demo-app-a/reference/a.txt", "a\n"),
        ],
        &["skills/demo-app/demo-app-a/scripts/run.sh"],
    );
    let ag = snap.apps[0].agent.as_ref().unwrap();
    assert_eq!(ag.summary.as_deref(), Some("Things, listed and made."));
    let names: Vec<_> = ag.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["demo-app-a", "demo-app-b"], "sorted by name");
    let a = &ag.skills[0];
    assert_eq!(a.description, "First skill.");
    let want: String = Sha256::digest(md_a.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(a.sha256, want, "SKILL.md as shipped");
    assert_eq!(a.size, md_a.len() as u64);
    let files: Vec<_> = a
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.executable))
        .collect();
    assert_eq!(
        files,
        [("reference/a.txt", false), ("scripts/run.sh", true)],
        "sorted by path, with the executable bit"
    );
    assert_eq!(a.files[0].size, 2);
    // Nothing of the content is in the snapshot, and no `guide` key.
    let json = serde_json::to_string(&snap.apps[0]).unwrap();
    assert!(
        !json.contains("echo hi") && !json.contains("\"guide\""),
        "{json}"
    );
    let v: serde_json::Value = serde_json::from_slice(
        &aurora_registry::snapshot::compile(&[], &[], &[], 1)
            .unwrap()
            .json,
    )
    .unwrap();
    assert!(v["apps"].as_array().unwrap().is_empty());
    // No skills: the key is not in the agent fragment.
    let plain = yaml(TOOLS, "    agent:\n      instructions: hi\n", "");
    let snap = snapshot_of(&plain, &[]);
    let v = serde_json::to_value(&snap.apps[0]).unwrap();
    let keys: Vec<_> = v["agent"].as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        ["default_blocklist", "env", "instructions", "stubs"],
        "{keys:?}"
    );
}

/// (C-1132) A stored version in the old format (`agent.guide`) still compiles: the entries are
/// ignored, and the snapshot has no `guide` key whatever the files in the archive were.
#[test]
fn a_stored_version_with_agent_guide_still_compiles_without_a_guide_key() {
    let snap = snapshot_of(
        &yaml(TOOLS, LEGACY_AGENT, ""),
        &[
            ("make-and-check.md", "# Make\n1. POST\n"),
            ("api.md", "# API\n"),
            ("codes.tsv", "a\tb\n"),
        ],
    );
    let ag = snap.apps[0].agent.as_ref().unwrap();
    assert_eq!(ag.summary.as_deref(), Some("Things, listed and made."));
    assert!(ag.skills.is_empty());
    let json = serde_json::to_string(&snap.apps[0]).unwrap();
    assert!(
        !json.contains("guide") && !json.contains("api.md"),
        "{json}"
    );
    // And the compiled snapshot of an older version, which carries `guide[]`, still parses.
    let old = r#"{"summary":"s","instructions":"","guide":[{"file":"a.md","title":"A","kind":"reference","sha256":"00","size":1}],"env":[],"stubs":[],"default_blocklist":[]}"#;
    let e: aurora_registry::snapshot::AgentEntry = serde_json::from_str(old).unwrap();
    assert!(e.skills.is_empty());
    assert!(!serde_json::to_string(&e).unwrap().contains("guide"));
}

/// An operations file whose digest was not supplied cannot be compiled (a validated package
/// always has it).
#[test]
fn compile_refuses_an_operations_file_without_a_digest() {
    use aurora_registry::snapshot::{CompileError, compile};
    let m = Manifest::parse(
        &yaml(
            TOOLS,
            "    agent:\n      summary: \"S\"\n      operations: operations.json\n",
            "",
        ),
        "99.0.0",
    )
    .unwrap();
    let p = ActivePackage {
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
        manifest: m,
    };
    assert!(matches!(
        compile(&[p], &[], &[], 1),
        Err(CompileError::AgentFileMissing { .. })
    ));
}

fn hash_of(yaml: &str) -> Option<String> {
    let m = Manifest::parse(yaml, "99.0.0").ok()?;
    let p = ActivePackage {
        manifest: m,
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    Some(compile(&[p], &[], &[], 1).ok()?.hash)
}

/// The compiled snapshot of every built-in and fixture package that does not use the agent
/// keys hashes exactly as before P5-GUIDE-1 (golden file `agent_files_hashes.txt`;
/// `UPDATE_HASHES=1` rewrites it).
#[test]
fn snapshots_of_packages_without_a_guide_keep_their_hash() {
    let mut lines = Vec::new();
    for dir in ["plugins/builtin", "plugins/fixtures"] {
        let mut names: Vec<_> = std::fs::read_dir(root().join(dir))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.join("plugin.yaml").is_file())
            .collect();
        names.sort();
        for p in names {
            let y = std::fs::read_to_string(p.join("plugin.yaml")).unwrap();
            if y.contains("summary:") || y.contains("guide:") || y.contains("operations:") {
                continue;
            }
            if let Some(h) = hash_of(&y) {
                lines.push(format!(
                    "{dir}/{} {h}",
                    p.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    let mut got = String::new();
    for l in &lines {
        let _ = writeln!(got, "{l}");
    }
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/it/agent_files_hashes.txt");
    if std::env::var_os("UPDATE_HASHES").is_some() {
        std::fs::write(&golden, &got).unwrap();
    }
    let want = std::fs::read_to_string(&golden).unwrap();
    let want_lines: std::collections::BTreeSet<_> = want.lines().collect();
    // Every golden line must still be produced (a new package may add lines; they are checked in
    // with the next update).
    for l in want.lines() {
        assert!(
            lines.iter().any(|x| x == l),
            "snapshot hash changed or package missing: {l}"
        );
    }
    assert!(!want_lines.is_empty());
}
