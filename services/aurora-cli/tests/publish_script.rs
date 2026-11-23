//! The publish job's shell logic (C-1144) run against a local fixture: the
//! script `templates/examples/.github/aurora-publish.sh` runs with a fake `gh`
//! on PATH and the real `aurora` CLI, over a repository folder with the three
//! cases: a template already released and unchanged (nothing happens), one
//! released and changed (the job fails with the C-1144 sentence), and one new
//! (`gh release create` gets the right tag, asset and notes).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runtime path first (nextest), compile-time path as the fallback.
fn aurora_bin() -> PathBuf {
    std::env::var_os("NEXTEST_BIN_EXE_aurora")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_aurora")))
}

const TEMPLATE_YAML: &str = "\
format: 1
id: {id}
version: 1.0.0
name: {name}
summary: A fixture template
";

const README: &str = "# Fixture\n\nWhat this agent is for.\n";
const AGENT_YAML: &str = "name: Fixture agent\n";
const INSTRUCTIONS: &str = "Do the fixture things.\n";
const MANIFEST: &str = "\
api: 1
repo:
  name: Fixture templates
  description: For the publish-script test
packages:
  url: https://github.com/example/agent-templates/releases/download/{id}-v{version}/{id}-{version}.zip
";

struct Fixture {
    dir: PathBuf,
    log: PathBuf,
    releases: PathBuf,
    dist: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn write_template(root: &Path, id: &str, name: &str, changelog: bool) {
    let d = root.join(id);
    fs::create_dir_all(&d).expect("template dir");
    fs::write(
        d.join("template.yaml"),
        TEMPLATE_YAML.replace("{id}", id).replace("{name}", name),
    )
    .unwrap();
    fs::write(d.join("README.md"), README).unwrap();
    fs::write(d.join("agent.yaml"), AGENT_YAML).unwrap();
    fs::write(d.join("instructions.md"), INSTRUCTIONS).unwrap();
    if changelog {
        fs::write(d.join("CHANGELOG.md"), "# 1.0.0\n\nFirst release.\n").unwrap();
    }
}

fn fixture(name: &str, templates: &[(&str, &str, bool)]) -> Fixture {
    let dir = std::env::temp_dir().join(format!("aurora-publish-{name}-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("aurora-templates.yaml"), MANIFEST).unwrap();
    for (id, template_name, changelog) in templates {
        write_template(&dir, id, template_name, *changelog);
    }
    Fixture {
        releases: dir.join("released"),
        log: dir.join("gh.log"),
        dist: dir.join("dist"),
        dir,
    }
}

/// The fake `gh`: logs every call, answers `release view` from the fixture's
/// `released/` directory, `download` copies the recorded asset, `create`
/// records the asset into it (so a later `view` finds it).
const FAKE_GH: &str = r#"#!/bin/sh
printf 'gh %s\n' "$*" >> "$FAKE_GH_LOG"
cmd=$1; shift
[ "$cmd" = release ] || exit 0
sub=$1; shift
case "$sub" in
  view)
    tag=$1
    [ -d "$FAKE_RELEASES/$tag" ]
    ;;
  download)
    tag=""; pattern=""; out=""
    while [ $# -gt 0 ]; do
      case $1 in
        -p) pattern=$2; shift 2;;
        -O) out=$2; shift 2;;
        *) [ -z "$tag" ] && tag=$1; shift;;
      esac
    done
    cat "$FAKE_RELEASES/$tag/$pattern" > "$out"
    ;;
  create)
    tag=$1; shift
    asset=$1; shift
    mkdir -p "$FAKE_RELEASES/$tag"
    cp "$asset" "$FAKE_RELEASES/$tag/$(basename "$asset")"
    ;;
esac
"#;

fn bin_dir() -> PathBuf {
    let mut p = aurora_bin();
    p.pop();
    p
}

fn write_fake_gh(f: &Fixture) -> PathBuf {
    let bin = f.dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, FAKE_GH).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn run_publish(f: &Fixture) -> (bool, String, String) {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/examples/.github/aurora-publish.sh");
    // The new PATH wins for finding `sh` too: keep the system directories at
    // the end.
    let fake_path = std::env::join_paths([write_fake_gh(f), bin_dir()]).unwrap();
    let path = std::env::var("PATH")
        .map(|rest| format!("{}:{rest}", fake_path.to_string_lossy()))
        .unwrap_or_else(|_| fake_path.to_string_lossy().into_owned());
    let out = Command::new("sh")
        .arg(script)
        .arg(".")
        .current_dir(&f.dir)
        .env("GITHUB_SHA", "0000000000000000000000000000000000000000")
        .env("FAKE_GH_LOG", &f.log)
        .env("FAKE_RELEASES", &f.releases)
        .env("PATH", &path)
        .output()
        .expect("runs the publish script");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Packs a fixture template with the real CLI and records the bytes as the
/// release asset for `tag`.
fn seed_release(f: &Fixture, id: &str, tag: &str) {
    let asset = format!("{id}-1.0.0.zip");
    let packed = f.dist.join(&asset);
    fs::create_dir_all(&f.dist).unwrap();
    let out = Command::new(aurora_bin())
        .args(["template", "pack"])
        .arg(f.dir.join(id))
        .arg("-o")
        .arg(&packed)
        .output()
        .expect("packs the fixture template");
    assert!(
        out.status.success(),
        "pack failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let dst = f.releases.join(tag);
    fs::create_dir_all(&dst).unwrap();
    fs::rename(&packed, dst.join(&asset)).unwrap();
}

fn log_of(f: &Fixture) -> String {
    fs::read_to_string(&f.log).unwrap_or_default()
}

#[test]
fn unchanged_release_publishes_nothing() {
    let f = fixture("unchanged", &[("stable-agent", "Stable", false)]);
    seed_release(&f, "stable-agent", "stable-agent-v1.0.0");
    let (ok, stdout, stderr) = run_publish(&f);
    assert!(ok, "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        !log_of(&f).contains(" release create"),
        "nothing should be created:\n{:?}",
        log_of(&f)
    );
    assert!(stdout.contains("already exists and matches"), "{stdout}");
}

#[test]
fn changed_release_fails_with_the_c1144_sentence() {
    let f = fixture("changed", &[("drifting-agent", "Drifter", false)]);
    seed_release(&f, "drifting-agent", "drifting-agent-v1.0.0");
    // The template changes without a version bump.
    fs::write(
        f.dir.join("drifting-agent/instructions.md"),
        "Now do different things.\n",
    )
    .unwrap();
    let (ok, stdout, stderr) = run_publish(&f);
    assert!(!ok, "the publish job must fail; stdout:\n{stdout}");
    assert!(
        stderr.contains(
            "drifting-agent changed without a new version: raise version in \
             drifting-agent/template.yaml"
        ),
        "{stderr}"
    );
    assert!(
        !log_of(&f).contains(" release create"),
        "a changed version is never overwritten:\n{:?}",
        log_of(&f)
    );
}

#[test]
fn new_template_creates_the_release_with_tag_asset_and_notes() {
    let f = fixture("new", &[("fresh-agent", "Fresh", true)]);
    let (ok, stdout, stderr) = run_publish(&f);
    assert!(ok, "stdout:\n{stdout}\nstderr:\n{stderr}");
    let log = log_of(&f);
    assert!(log.contains("release create fresh-agent-v1.0.0"), "{log}");
    assert!(log.contains("dist/fresh-agent-1.0.0.zip"), "{log}");
    assert!(log.contains("--notes-file"), "{log}");
    assert!(log.contains("fresh-agent/CHANGELOG.md"), "{log}");
    // The asset recorded by the fake gh is exactly a fresh pack.
    let recorded = f.releases.join("fresh-agent-v1.0.0/fresh-agent-1.0.0.zip");
    let repacked = f.dist.join("repack.zip");
    let out = Command::new(aurora_bin())
        .args(["template", "pack"])
        .arg(f.dir.join("fresh-agent"))
        .arg("-o")
        .arg(&repacked)
        .output()
        .expect("packs again");
    assert!(
        out.status.success(),
        "repack failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read(&recorded).unwrap(), fs::read(&repacked).unwrap());
    assert!(stdout.contains("published"));
}
