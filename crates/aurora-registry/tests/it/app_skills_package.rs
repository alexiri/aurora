//! (P5-APPSKILL-1, C-1131, SEC-150) The executable bit of a skill file is part of the signed digest:
//! the digest of a package without `skills/` is what it always was, flipping the bit changes the
//! digest and breaks the signature, the bit is read from the archive entry that was hashed and
//! only under `skills/`, and a link under `skills/` is refused.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::path::PathBuf;

use aurora_crypto::generate_signing_key;
use aurora_registry::builder::PackageBuilder;
use aurora_registry::digest::digest;
use aurora_registry::{Limits, Package, TrustStore};
use sha2::{Digest, Sha256};
use tar::EntryType;

use super::agent_files::{AGENT, TOOLS, skill_md, yaml};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The digest of `plugins/builtin/caldav` (no skills, no agent files) as recorded on master before
/// P5-APPSKILL-1, computed there by an independent implementation of 09 §2 (sha256 over
/// `<path>\0<sha256 hex of content>\n`, bytewise path order).
const CALDAV_DIGEST_ON_MASTER: &str =
    "ccaa1428bb2ec78be4d1d5f693ab93f4cf845e0b03a13d27687cfda53d014b36";

#[test]
fn a_package_without_skills_keeps_its_digest_byte_for_byte() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/builtin/caldav");
    let mut pkg = Package::default();
    fn walk(dir: &std::path::Path, base: &std::path::Path, pkg: &mut Package) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, base, pkg);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                pkg.files.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    walk(&root, &root, &mut pkg);
    assert_eq!(pkg.files.len(), 5);
    assert_eq!(hex(&digest(&pkg)), CALDAV_DIGEST_ON_MASTER);
    // A mode bit outside `skills/` is not part of the digest.
    pkg.executable.insert("docs/setup.md".into());
    assert_eq!(hex(&digest(&pkg)), CALDAV_DIGEST_ON_MASTER);
}

#[test]
fn an_executable_skill_file_adds_x_to_its_digest_line() {
    let mut pkg = Package::default();
    pkg.files.insert("plugin.yaml".into(), b"p".to_vec());
    pkg.files
        .insert("skills/a/a-x/scripts/run.sh".into(), b"echo\n".to_vec());
    pkg.files
        .insert("skills/a/a-x/SKILL.md".into(), b"md".to_vec());
    pkg.executable.insert("skills/a/a-x/scripts/run.sh".into());
    let line = |path: &str, content: &[u8], x: bool| {
        format!(
            "{path}\0{}{}\n",
            hex(&Sha256::digest(content)),
            if x { "\0x" } else { "" }
        )
    };
    let want = [
        line("plugin.yaml", b"p", false),
        line("skills/a/a-x/SKILL.md", b"md", false),
        line("skills/a/a-x/scripts/run.sh", b"echo\n", true),
    ]
    .concat();
    assert_eq!(
        digest(&pkg).to_vec(),
        Sha256::digest(want.as_bytes()).to_vec()
    );
    let mut plain = pkg.clone();
    plain.executable.clear();
    assert_ne!(digest(&plain), digest(&pkg));
}

fn signed_with_script(exec: bool) -> (PackageBuilder, aurora_crypto::SigningKey) {
    let k = generate_signing_key().unwrap();
    let mut b = PackageBuilder::new(&yaml(TOOLS, AGENT, ""));
    b.set_file(
        "skills/demo-app/demo-app-x/SKILL.md",
        skill_md("demo-app-x", "d", "b").as_bytes(),
    );
    b.set_file_exec(
        "skills/demo-app/demo-app-x/scripts/run.sh",
        b"echo hi\n",
        exec,
    );
    b.sign(&k, "builtin");
    (b, k)
}

fn trust(k: &aurora_crypto::SigningKey) -> TrustStore {
    TrustStore {
        project_key: Some(k.verifying_key()),
        keys: vec![],
    }
}

#[test]
fn flipping_the_executable_bit_of_a_skill_file_breaks_the_signature() {
    let (mut b, k) = signed_with_script(false);
    let p = Package::read(&b.build().unwrap(), &Limits::default()).unwrap();
    assert!(aurora_registry::signature::verify(&p, &trust(&k), false).is_ok());
    assert!(p.executable.is_empty());
    // Same bytes, same signature, the bit flipped: a different digest, so `verify` fails.
    b.set_file_exec(
        "skills/demo-app/demo-app-x/scripts/run.sh",
        b"echo hi\n",
        true,
    );
    let flipped = Package::read(&b.build().unwrap(), &Limits::default()).unwrap();
    assert!(
        flipped
            .executable
            .contains("skills/demo-app/demo-app-x/scripts/run.sh")
    );
    assert_ne!(digest(&p), digest(&flipped));
    let e = aurora_registry::signature::verify(&flipped, &trust(&k), false).unwrap_err();
    assert_eq!(e.code, "digest_mismatch");
    // And the other way: signed executable, shipped without the bit.
    let (mut b, k) = signed_with_script(true);
    let ok = Package::read(&b.build().unwrap(), &Limits::default()).unwrap();
    assert!(aurora_registry::signature::verify(&ok, &trust(&k), false).is_ok());
    b.set_file_exec(
        "skills/demo-app/demo-app-x/scripts/run.sh",
        b"echo hi\n",
        false,
    );
    let stripped = Package::read(&b.build().unwrap(), &Limits::default()).unwrap();
    assert_eq!(
        aurora_registry::signature::verify(&stripped, &trust(&k), false)
            .unwrap_err()
            .code,
        "digest_mismatch"
    );
}

#[test]
fn the_reader_records_the_bit_of_skill_files_only() {
    let (mut b, _) = signed_with_script(true);
    // A mode bit on an agent file or a doc is not recorded.
    b.set_file_exec("agent/demo-app/operations.json", b"{}", true);
    b.set_file_exec("docs/setup.md", b"# Setup\n", true);
    let p = Package::read(&b.build().unwrap(), &Limits::default()).unwrap();
    let got: Vec<&str> = p.executable.iter().map(String::as_str).collect();
    assert_eq!(got, ["skills/demo-app/demo-app-x/scripts/run.sh"]);
}

#[test]
fn a_symlink_or_hard_link_under_skills_is_refused() {
    let (mut b, _) = signed_with_script(false);
    b.push_symlink("skills/demo-app/demo-app-x/link.txt", "/etc/passwd");
    let r = Package::read(&b.build().unwrap(), &Limits::default());
    assert!(r.is_err_and(|v| v.iter().any(|x| x.code == "archive_link")));
    let (mut b, _) = signed_with_script(false);
    b.push_raw("skills/demo-app/demo-app-x/hard.txt", EntryType::Link, b"");
    let r = Package::read(&b.build().unwrap(), &Limits::default());
    assert!(r.is_err_and(|v| v.iter().any(|x| x.code == "archive_link")));
}

/// A package with skills is not valid without the shared validator's blessing end to end: the
/// linter (`validate::package`) accepts a good one and rejects one that breaks a rule.
#[test]
fn the_package_validator_runs_the_skill_rules() {
    use std::collections::HashMap;
    let ctx = aurora_registry::validate::ValidationContext {
        running_version: "99.0.0".into(),
        reserved_hosts: vec![],
        active_app_ids: HashMap::new(),
        active_families: HashMap::new(),
    };
    let (b, k) = signed_with_script(true);
    let p = Package::read(&b.build().unwrap(), &Limits::default()).unwrap();
    let codes: Vec<_> = aurora_registry::validate::package(&p, &ctx, &trust(&k), false)
        .iter()
        .map(|v| v.code)
        .collect();
    assert!(codes.is_empty(), "{codes:?}");
    let (mut b, k) = signed_with_script(true);
    b.set_file(
        "skills/demo-app/other-x/SKILL.md",
        skill_md("other-x", "d", "b").as_bytes(),
    );
    b.sign(&k, "builtin");
    let p = Package::read(&b.build().unwrap(), &Limits::default()).unwrap();
    let codes: Vec<_> = aurora_registry::validate::package(&p, &ctx, &trust(&k), false)
        .iter()
        .map(|v| v.code)
        .collect();
    assert_eq!(codes, ["app_skill_name"]);
}

/// (P5-APPSKILL-1, acceptance 6) Every built-in package passes the agent and skill rules with no
/// warning, ships no `guide:` key and nothing but `operations.json` under `agent/`, and every
/// skill it ships is named after its app.
#[test]
fn every_builtin_package_passes_the_skill_rules_without_a_warning() {
    use aurora_registry::Manifest;
    use aurora_registry::validate::agent_rules::archive_rules;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/builtin");
    let mut skills = 0;
    // app id -> built-in package (directory) id, for every built-in.
    let mut all_apps = std::collections::HashMap::new();
    for e in std::fs::read_dir(&dir).unwrap() {
        let root = e.unwrap().path();
        let Ok(text) = std::fs::read_to_string(root.join("plugin.yaml")) else {
            continue;
        };
        let pid = root.file_name().unwrap().to_string_lossy().into_owned();
        for a in Manifest::parse(&text, "9.9.9").unwrap().apps {
            all_apps.insert(a.about.id, pid.clone());
        }
    }
    let others_of = |_pid: &str| all_apps.clone();
    for e in std::fs::read_dir(&dir).unwrap() {
        let root = e.unwrap().path();
        let manifest = root.join("plugin.yaml");
        if !manifest.is_file() {
            continue;
        }
        let id = root.file_name().unwrap().to_string_lossy().into_owned();
        let m = Manifest::parse(&std::fs::read_to_string(&manifest).unwrap(), "9.9.9").unwrap();
        let mut pkg = Package::default();
        for sub in ["agent", "skills"] {
            let mut stack = vec![root.join(sub)];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else {
                    continue;
                };
                for e in rd {
                    let p = e.unwrap().path();
                    if p.is_dir() {
                        stack.push(p);
                    } else {
                        let rel = p
                            .strip_prefix(&root)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned();
                        pkg.files.insert(rel.clone(), std::fs::read(&p).unwrap());
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if rel.starts_with("skills/")
                                && p.metadata().unwrap().permissions().mode() & 0o100 != 0
                            {
                                pkg.executable.insert(rel);
                            }
                        }
                    }
                }
            }
        }
        let mut v = Vec::new();
        // (FIX-221) Built-ins are linted against every other built-in's app ids.
        archive_rules(&m, &pkg, &others_of(&id), &mut v);
        assert!(v.is_empty(), "{id}: {v:?}");
        let w = aurora_registry::validate::package_warnings(&m, &pkg);
        assert!(w.is_empty(), "{id}: {w:?}");
        for a in &m.apps {
            assert!(
                a.agent.as_ref().is_none_or(|g| g.guide.is_empty()),
                "{id}: guide:"
            );
        }
        for k in pkg.files.keys().filter(|k| k.starts_with("agent/")) {
            assert!(k.ends_with("/operations.json"), "{id}: {k}");
        }
        skills += pkg
            .files
            .keys()
            .filter(|k| k.ends_with("/SKILL.md"))
            .count();
    }
    assert!(
        skills >= 20,
        "the converted guides are skills now: {skills}"
    );
}
