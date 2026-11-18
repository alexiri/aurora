//! (P5-GUIDE-1, `docs/24`) Agent guides: compatibility of existing snapshots, the linter matrix,
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
      guide:
        - { file: make-and-check.md, title: \"Make a thing and check it\", kind: recipe }
        - { file: api.md, title: \"API conventions\" }
        - { file: codes.tsv, title: \"Codes\" }
";

/// Compile `y` with the guide digests of `files` (`app file -> bytes`) and return the snapshot.
pub fn snapshot_of(y: &str, files: &[(&str, &str)]) -> aurora_registry::snapshot::Snapshot {
    use aurora_registry::snapshot::{compile_with_guides, guide_digests};
    let m = Manifest::parse(y, "99.0.0").unwrap();
    let mut pkg = aurora_registry::Package {
        files: Default::default(),
        signature: None,
    };
    for (f, c) in files {
        pkg.files
            .insert(format!("agent/demo-app/{f}"), c.as_bytes().to_vec());
    }
    let p = ActivePackage {
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
        manifest: m.clone(),
    };
    let g = std::collections::BTreeMap::from([("demo".to_owned(), guide_digests(&m, &pkg))]);
    let c = compile_with_guides(&[p], &[], &[], 1, &g).unwrap();
    aurora_registry::snapshot::Snapshot::from_slice(&c.json).unwrap()
}

pub const FILES: [(&str, &str); 3] = [
    ("make-and-check.md", "# Make\n1. POST\n"),
    ("api.md", "# API\n"),
    ("codes.tsv", "a\tb\n"),
];

/// The snapshot carries `summary` and `guide[]` with `kind`, `sha256` and `size`, and omits both
/// when the app has no guide (the fragment is then the pre-P5-GUIDE-1 one).
#[test]
fn snapshot_carries_summary_and_guide_with_digests_and_omits_them_when_absent() {
    use sha2::{Digest, Sha256};
    let snap = snapshot_of(&yaml(TOOLS, AGENT, ""), &FILES);
    let ag = snap.apps[0].agent.as_ref().unwrap();
    assert_eq!(ag.summary.as_deref(), Some("Things, listed and made."));
    assert_eq!(ag.guide.len(), 3);
    assert_eq!(ag.guide[0].file, "make-and-check.md");
    assert_eq!(
        ag.guide[0].kind,
        aurora_registry::snapshot::GuideKind::Recipe
    );
    assert_eq!(
        ag.guide[1].kind,
        aurora_registry::snapshot::GuideKind::Reference
    );
    let want: String = Sha256::digest(b"# API\n")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(ag.guide[1].sha256, want);
    assert_eq!(ag.guide[1].size, 6);
    let v: serde_json::Value = serde_json::from_slice(
        &aurora_registry::snapshot::compile(&[], &[], &[], 1)
            .unwrap()
            .json,
    )
    .unwrap();
    assert!(v["apps"].as_array().unwrap().is_empty());
    // No guide keys: neither key is in the agent fragment.
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

/// A listed file whose digest is not supplied cannot be compiled (a validated package always has it).
#[test]
fn compile_refuses_a_guide_entry_without_a_digest() {
    use aurora_registry::snapshot::{CompileError, compile};
    let m = Manifest::parse(&yaml(TOOLS, AGENT, ""), "99.0.0").unwrap();
    let p = ActivePackage {
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
        manifest: m,
    };
    assert!(matches!(
        compile(&[p], &[], &[], 1),
        Err(CompileError::GuideFileMissing { .. })
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

/// The compiled snapshot of every built-in and fixture package that does not use the agent guide
/// keys hashes exactly as before P5-GUIDE-1 (golden file `agent_guide_hashes.txt`;
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
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/it/agent_guide_hashes.txt");
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
