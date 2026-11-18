#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;

use crate::common::TempDir;
use aurora_protocol::runner::HomeSync;
use aurora_protocol::supervisor::{MAX_FILE_PART_DECODED, SupervisorToRunner};
use aurora_supervisor::channel::ChannelSender;
use aurora_supervisor::harvest::{Harvester, seal, verify};
use aurora_supervisor::home::{HomeConfig, HomeProjector, PartOutcome};
use aurora_supervisor::outbound::{OutboundError, OutboundFile, send_file};
use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn projector(root: &Path) -> HomeProjector {
    HomeProjector::new(HomeConfig {
        root: root.to_path_buf(),
        skills_dir: Some(".claude/skills".into()),
        instruction_files: vec!["AGENTS.md".into()],
        pg: None,
        agent_cli: false,
    })
}

fn part(generation: u64, i: u32, n: u32, kind: &str, entries: Value) -> HomeSync {
    serde_json::from_value(json!({
        "generation": generation, "part_index": i, "part_count": n,
        "part": {"kind": kind, "entries": entries}
    }))
    .unwrap()
}

/// Full generation: skills, memory, render_inputs, tools, manifest, final.
fn generation(g: u64, skills: &[(&str, &str)], memories: &[(&str, &str)]) -> Vec<HomeSync> {
    let mut manifest: Vec<Value> = skills.iter().map(|(p, _)| json!({"path": p})).collect();
    manifest.extend(
        memories
            .iter()
            .map(|(k, _)| json!({"path": format!("memory/{k}.md")})),
    );
    let sk: Vec<Value> = skills
        .iter()
        .map(|(p, c)| json!({"path": p, "content": c}))
        .collect();
    let me: Vec<Value> = memories
        .iter()
        .map(|(k, c)| json!({"path": format!("memory/{k}.md"), "content": c}))
        .collect();
    vec![
        part(g, 0, 6, "skills", json!(sk)),
        part(g, 1, 6, "memory", json!(me)),
        part(
            g,
            2,
            6,
            "render_inputs",
            json!([
                {"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}},
                {"brief": format!("brief gen {g}")}
            ]),
        ),
        part(g, 3, 6, "tools", json!([{"name": "memory_save"}])),
        part(g, 4, 6, "manifest", json!(manifest)),
        part(g, 5, 6, "final", json!([])),
    ]
}

fn apply_all(p: &mut HomeProjector, parts: &[HomeSync]) -> PartOutcome {
    let mut last = PartOutcome::Staged;
    for s in parts {
        last = p.apply_part(s).unwrap();
    }
    last
}

/// (P4-HAR) The projection follows the adapter's declared `instruction_files` and `skills_dir`:
/// a Codex sandbox gets `AGENTS.md` and skills under `.codex/skills/aurora`, no `CLAUDE.md`, and
/// only that subdirectory is made read-only (Codex seeds its own `.system` skills next to it).
#[test]
fn codex_projection_renders_agents_md_and_its_own_skills_dir_only() {
    use aurora_supervisor::adapters::codex::CodexAdapter;
    use aurora_supervisor::harness::HarnessAdapter;
    let caps = CodexAdapter::default().capabilities();
    let d = TempDir::new();
    let mut p = HomeProjector::new(HomeConfig {
        root: d.path().to_path_buf(),
        skills_dir: caps.skills_dir.clone(),
        instruction_files: caps.instruction_files.clone(),
        pg: None,
        agent_cli: false,
    });
    let skill = ".codex/skills/aurora/a/SKILL.md";
    let system = d.path().join(".codex/skills/.system/seeded/SKILL.md");
    fs::create_dir_all(system.parent().unwrap()).unwrap();
    fs::write(&system, "codex's own").unwrap();
    let parts = generation(1, &[(skill, "skill a")], &[]);
    assert!(matches!(
        apply_all(&mut p, &parts),
        PartOutcome::ReadyToCommit(1)
    ));
    p.commit(1).unwrap();
    assert_eq!(fs::read_to_string(d.path().join(skill)).unwrap(), "skill a");
    assert!(d.path().join("AGENTS.md").exists());
    assert!(!d.path().join("CLAUDE.md").exists());
    assert!(!d.path().join(".claude").exists());
    // Codex's own skills are neither removed nor locked.
    assert_eq!(fs::read_to_string(&system).unwrap(), "codex's own");
    assert!(
        !fs::metadata(system.parent().unwrap().parent().unwrap())
            .unwrap()
            .permissions()
            .readonly()
    );
    // The projected skill is read-only.
    assert!(
        fs::metadata(d.path().join(skill))
            .unwrap()
            .permissions()
            .readonly()
    );
}

#[test]
fn multi_part_generation_is_applied_only_at_commit() {
    let d = TempDir::new();
    let mut p = projector(d.path());
    let parts = generation(
        1,
        &[(".claude/skills/a/SKILL.md", "skill a")],
        &[("likes", "tea")],
    );
    for s in &parts[..5] {
        assert_eq!(p.apply_part(s).unwrap(), PartOutcome::Staged);
        assert!(!d.path().join(".claude/skills/a/SKILL.md").exists());
        assert!(!d.path().join("memory/likes.md").exists());
    }
    assert_eq!(
        p.apply_part(&parts[5]).unwrap(),
        PartOutcome::ReadyToCommit(1)
    );
    assert!(!d.path().join("AGENTS.md").exists());
    p.commit(1).unwrap();

    assert_eq!(
        fs::read_to_string(d.path().join(".claude/skills/a/SKILL.md")).unwrap(),
        "skill a"
    );
    let mem = fs::read_to_string(d.path().join("memory/likes.md")).unwrap();
    assert!(mem.starts_with("tea\n") && verify(&mem));
    let agents = fs::read_to_string(d.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("brief gen 1") && agents.contains("Ada"));
    assert!(d.path().join(".aurora/tools.json").exists());
    assert!(!d.path().join(".aurora/sync").exists());
    let mode = |p: &str| {
        use std::os::unix::fs::PermissionsExt as _;
        fs::metadata(d.path().join(p)).unwrap().permissions().mode() & 0o777
    };
    assert_eq!(mode(".claude/skills/a/SKILL.md"), 0o444);
    assert_eq!(mode("AGENTS.md"), 0o444);
    assert_eq!(mode("memory/likes.md"), 0o644);
}

#[test]
fn interrupted_generation_keeps_previous_state_and_is_replaced() {
    let d = TempDir::new();
    let mut p = projector(d.path());
    apply_all(
        &mut p,
        &generation(1, &[(".claude/skills/a/SKILL.md", "v1")], &[]),
    );
    p.commit(1).unwrap();

    // Generation 2 dies after three parts (supervisor restarts: fresh projector).
    let g2 = generation(2, &[(".claude/skills/a/SKILL.md", "v2")], &[]);
    for s in &g2[..3] {
        p.apply_part(s).unwrap();
    }
    drop(p);
    assert_eq!(
        fs::read_to_string(d.path().join(".claude/skills/a/SKILL.md")).unwrap(),
        "v1"
    );
    assert!(
        fs::read_to_string(d.path().join("AGENTS.md"))
            .unwrap()
            .contains("brief gen 1")
    );

    // A later generation wipes the stale staging area and applies cleanly.
    let mut p = projector(d.path());
    let g3 = generation(3, &[(".claude/skills/b/SKILL.md", "v3")], &[]);
    assert_eq!(apply_all(&mut p, &g3), PartOutcome::ReadyToCommit(3));
    p.commit(3).unwrap();
    assert_eq!(
        fs::read_to_string(d.path().join(".claude/skills/b/SKILL.md")).unwrap(),
        "v3"
    );
    assert!(!d.path().join(".claude/skills/a/SKILL.md").exists());
    assert!(
        fs::read_to_string(d.path().join("AGENTS.md"))
            .unwrap()
            .contains("brief gen 3")
    );
}

#[test]
fn replayed_part_is_idempotent_and_missing_manifest_is_not_acked() {
    let d = TempDir::new();
    let mut p = projector(d.path());
    let parts = generation(1, &[(".claude/skills/a/SKILL.md", "x")], &[]);
    p.apply_part(&parts[0]).unwrap();
    p.apply_part(&parts[1]).unwrap();
    p.apply_part(&parts[1]).unwrap(); // replay
    // final without manifest
    let bad = part(1, 5, 6, "final", json!([]));
    assert!(p.apply_part(&bad).is_err());
}

#[test]
fn prune_removes_only_managed_paths() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    apply_all(
        &mut p,
        &generation(
            1,
            &[
                (".claude/skills/a/SKILL.md", "a"),
                (".claude/skills/b/SKILL.md", "b"),
            ],
            &[("keep", "k"), ("drop", "d")],
        ),
    );
    p.commit(1).unwrap();
    // Agent-owned and unmanaged files.
    fs::write(root.join("notes.txt"), "mine").unwrap();
    fs::create_dir_all(root.join(".aurora/inbox/t1")).unwrap();
    fs::write(root.join(".aurora/inbox/t1/f.txt"), "att").unwrap();
    // Agent edit of a memory that is gone from the manifest: stays until harvested/superseded.
    fs::write(root.join("memory/drop.md"), "agent edit").unwrap();
    fs::write(root.join("memory/stray.md"), "not mine either").unwrap();

    apply_all(
        &mut p,
        &generation(2, &[(".claude/skills/a/SKILL.md", "a2")], &[("keep", "k2")]),
    );
    p.commit(2).unwrap();

    assert!(!root.join(".claude/skills/b").exists());
    assert_eq!(
        fs::read_to_string(root.join(".claude/skills/a/SKILL.md")).unwrap(),
        "a2"
    );
    assert!(root.join("memory/keep.md").exists());
    assert!(
        root.join("memory/drop.md").exists(),
        "dirty memory survives prune"
    );
    assert!(root.join("memory/stray.md").exists());
    assert_eq!(fs::read_to_string(root.join("notes.txt")).unwrap(), "mine");
    assert!(root.join(".aurora/inbox/t1/f.txt").exists());
}

#[test]
fn unmodified_projection_pruned_and_unmanaged_paths_refused() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    apply_all(&mut p, &generation(1, &[], &[("gone", "g")]));
    p.commit(1).unwrap();
    assert!(root.join("memory/gone.md").exists());

    let mut g = generation(2, &[], &[]);
    g[0] = part(
        2,
        0,
        6,
        "skills",
        json!([
            {"path": "../escape.txt", "content": "x"},
            {"path": "/etc/passwd", "content": "x"},
            {"path": "notes.txt", "content": "x"},
            {"path": "memory/x.md", "content": "x"}
        ]),
    );
    apply_all(&mut p, &g);
    p.commit(2).unwrap();
    assert!(!root.join("memory/gone.md").exists());
    assert!(!root.join("notes.txt").exists());
    assert!(!root.join("memory/x.md").exists());
    assert!(!root.parent().unwrap().join("escape.txt").exists());
}

/// (FIX-114) A memory file four times the content limit is skipped with one notice and never
/// read whole: `read_bounded` stops at the bound; a file within it is still harvested.
#[test]
fn harvest_skips_an_oversized_memory_file_with_a_notice() {
    use aurora_supervisor::harvest::{MEMORY_FILE_READ_MAX, read_bounded};
    let d = TempDir::new();
    let root = d.path();
    fs::create_dir_all(root.join("memory")).unwrap();
    let big = root.join("memory/huge.md");
    // Sparse: the file claims four times the bound without occupying the disk.
    let f = fs::File::create(&big).unwrap();
    f.set_len(4 * MEMORY_FILE_READ_MAX).unwrap();
    drop(f);
    assert!(read_bounded(&big, MEMORY_FILE_READ_MAX).unwrap().is_none());
    fs::write(
        root.join("memory/ok.md"),
        "---\ntitle: T\ndescription: D\n---\nbody\n",
    )
    .unwrap();
    let mut h = Harvester::new(root);
    let w = h.scan();
    assert_eq!(w.len(), 1, "only the small file is harvested");
    assert_eq!(w[0].key, "ok");
    let notices = h.take_notices();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("huge.md") && notices[0].contains("larger than"));
    // The oversized file is reported once, not on every scan.
    assert!(h.scan().is_empty());
    assert!(h.take_notices().is_empty());
}

#[test]
fn harvest_detects_edits_and_ignores_unchanged() {
    let d = TempDir::new();
    let root = d.path();
    fs::create_dir_all(root.join("memory")).unwrap();
    fs::write(root.join("memory/a.md"), seal("alpha")).unwrap();
    fs::write(root.join("memory/index.md"), "index").unwrap();
    let mut h = Harvester::new(root);
    assert!(h.scan().is_empty(), "pristine projection is ignored");

    // Edit, new file, bad key.
    fs::write(
        root.join("memory/a.md"),
        seal("alpha").replace("alpha", "alpha edited"),
    )
    .unwrap();
    fs::write(
        root.join("memory/new.md"),
        "---\ntitle: T\ndescription: D\n---\nbody\n",
    )
    .unwrap();
    fs::write(root.join("memory/Bad_Key.md"), "x").unwrap();
    let w = h.scan();
    assert_eq!(w.len(), 2);
    assert_eq!(w[0].key, "a");
    assert!(w[0].content.starts_with("alpha edited"));
    assert!(!w[0].content.contains("aurora:checksum"));
    assert_eq!(w[1].key, "new");
    assert_eq!(w[1].title.as_deref(), Some("T"));
    assert_eq!(w[1].description.as_deref(), Some("D"));
    assert_eq!(w[1].content, "body\n");
    let notices = h.take_notices();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("Bad_Key.md"));

    assert!(h.scan().is_empty(), "unchanged edits are not re-uploaded");
    fs::write(root.join("memory/new.md"), "changed again").unwrap();
    assert_eq!(h.scan().len(), 1);

    // Canonical render supersedes the edit; nothing more to upload.
    fs::write(root.join("memory/new.md"), seal("canonical")).unwrap();
    assert!(h.scan().is_empty());
}

#[tokio::test]
async fn outbound_file_is_split_with_correct_sha256() {
    let d = TempDir::new();
    let data: Vec<u8> = (0..(2 * MAX_FILE_PART_DECODED + 1000))
        .map(|i| (i % 251) as u8)
        .collect();
    fs::write(d.path().join("out.bin"), &data).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let out = ChannelSender::from_mpsc(tx);
    let (conv, turn) = (Uuid::now_v7(), Uuid::now_v7());
    let id = send_file(
        &out,
        d.path(),
        OutboundFile {
            path: "out.bin".into(),
            conversation_id: conv,
            turn_id: turn,
            caption: Some("c".into()),
        },
    )
    .await
    .unwrap();

    let mut got = Vec::new();
    let mut indexes = Vec::new();
    let commit = loop {
        match rx.recv().await.unwrap() {
            SupervisorToRunner::FilePart(p) => {
                assert_eq!(p.upload_id, id);
                let b = base64::engine::general_purpose::STANDARD
                    .decode(p.data_b64)
                    .unwrap();
                assert!(b.len() <= 144 * 1024);
                indexes.push(p.index);
                got.extend(b);
            }
            SupervisorToRunner::FileCommit(c) => break c,
            other => panic!("unexpected {}", other.kind()),
        }
    };
    assert_eq!(indexes, vec![0, 1, 2]);
    assert_eq!(got, data);
    assert_eq!(commit.size, data.len() as u64);
    assert_eq!(commit.sha256, hex::encode(Sha256::digest(&data)));
    assert_eq!(commit.filename, "out.bin");
    assert_eq!((commit.conversation_id, commit.turn_id), (conv, turn));
}

#[tokio::test]
async fn outbound_refuses_paths_outside_home() {
    let d = TempDir::new();
    let outside = TempDir::new();
    fs::write(outside.path().join("s.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path().join("s.txt"), d.path().join("link")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    let out = ChannelSender::from_mpsc(tx);
    for path in [
        outside.path().join("s.txt"),
        "../x".into(),
        "link".into(),
        ".".into(),
    ] {
        let r = send_file(
            &out,
            d.path(),
            OutboundFile {
                path,
                conversation_id: Uuid::now_v7(),
                turn_id: Uuid::now_v7(),
                caption: None,
            },
        )
        .await;
        assert!(matches!(r, Err(OutboundError::BadPath)), "{r:?}");
    }
}

#[tokio::test]
async fn outbound_refuses_platform_dir() {
    let d = TempDir::new();
    fs::create_dir_all(d.path().join(".aurora/inbox/t")).unwrap();
    fs::write(d.path().join(".aurora/tools.json"), "[]").unwrap();
    fs::write(d.path().join(".aurora/inbox/t/a.txt"), "x").unwrap();
    fs::create_dir_all(d.path().join("sub")).unwrap();
    std::os::unix::fs::symlink(d.path().join(".aurora/tools.json"), d.path().join("alias"))
        .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    let out = ChannelSender::from_mpsc(tx);
    for path in [
        ".aurora/tools.json".into(),
        ".aurora/inbox/t/a.txt".into(),
        "sub/../.aurora/tools.json".into(),
        "alias".into(),
        d.path().join(".aurora/tools.json"),
    ] {
        let r = send_file(
            &out,
            d.path(),
            OutboundFile {
                path,
                conversation_id: Uuid::now_v7(),
                turn_id: Uuid::now_v7(),
                caption: None,
            },
        )
        .await;
        assert!(matches!(r, Err(OutboundError::Reserved)), "{r:?}");
    }
}

#[test]
fn databases_in_render_inputs_become_libpq_files_and_disappear_with_the_grant() {
    use std::os::unix::fs::PermissionsExt as _;
    let d = TempDir::new();
    let mut p = HomeProjector::new(HomeConfig {
        root: d.path().to_path_buf(),
        skills_dir: Some(".claude/skills".into()),
        instruction_files: vec!["AGENTS.md".into()],
        pg: Some(3130),
        agent_cli: false,
    });
    let conn = Uuid::now_v7();
    let with = |g: u64, dbs: Value| -> Vec<HomeSync> {
        let mut parts = generation(g, &[], &[]);
        parts[2] = part(
            g,
            2,
            6,
            "render_inputs",
            json!([{"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}, "databases": dbs}]),
        );
        parts
    };
    apply_all(
        &mut p,
        &with(1, json!([{"service": "prod", "connection_id": conn}])),
    );
    p.commit(1).unwrap();
    let dir = d.path().join(".aurora/pg");
    let svc = fs::read_to_string(dir.join("pg_service.conf")).unwrap();
    assert!(
        svc.contains("[prod]") && svc.contains(&format!("user={conn}")),
        "{svc}"
    );
    // Loopback relay, no TLS on that leg, no password or credential file at all.
    assert!(svc.contains("host=127.0.0.1\nport=3130\n") && svc.contains("sslmode=disable"));
    assert!(!dir.join("pgpass").exists() && !dir.join("root.crt").exists());
    assert!(!svc.contains("password") && !svc.contains("aub_"));
    assert_eq!(
        fs::metadata(dir.join("pg_service.conf"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    // A pgpass left by an earlier version is removed on the next sync.
    fs::write(dir.join("pgpass"), "h:1:*:u:aub_old\n").unwrap();
    apply_all(
        &mut p,
        &with(3, json!([{"service": "prod", "connection_id": conn}])),
    );
    p.commit(3).unwrap();
    assert!(!dir.join("pgpass").exists());

    apply_all(&mut p, &with(4, json!([])));
    p.commit(4).unwrap();
    assert!(!dir.exists());
}

/// (P5-ICS, D-273, C-843) `links` in the render inputs become `~/.aurora/links.json` (`0600`, the
/// synthetic address and nothing else) and disappear with the last link.
#[test]
fn links_in_render_inputs_become_links_json_and_disappear_with_the_last_link() {
    use std::os::unix::fs::PermissionsExt as _;
    let d = TempDir::new();
    let mut p = HomeProjector::new(HomeConfig {
        root: d.path().to_path_buf(),
        skills_dir: Some(".claude/skills".into()),
        instruction_files: vec!["AGENTS.md".into()],
        // No database relay: the links file does not depend on it.
        pg: None,
        agent_cli: false,
    });
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let with = |g: u64, links: Value| -> Vec<HomeSync> {
        let mut parts = generation(g, &[], &[]);
        parts[2] = part(
            g,
            2,
            6,
            "render_inputs",
            json!([{"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}, "links": links}]),
        );
        parts
    };
    let file = d.path().join(".aurora/links.json");
    apply_all(
        &mut p,
        &with(
            1,
            json!([
                {"name": "Work", "app_id": "ics-calendar", "connection_id": a},
                {"name": "Work", "app_id": "ics-calendar", "connection_id": b}
            ]),
        ),
    );
    p.commit(1).unwrap();
    let v: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        v.as_array().unwrap().len(),
        2,
        "both entries of a duplicated name: {v}"
    );
    assert_eq!(v[0]["url"], format!("https://{a}.link.aurora.internal/"));
    assert_eq!(v[1]["url"], format!("https://{b}.link.aurora.internal/"));
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );

    apply_all(&mut p, &with(2, json!([])));
    p.commit(2).unwrap();
    assert!(!file.exists());
}

// ---- (P5-GUIDE-2, C-643) the app folders

/// A generation with the render inputs, an `apps` part and the manifest naming `paths`.
fn apps_generation(g: u64, apps: &[(&str, &str)], manifest_extra: &[&str]) -> Vec<HomeSync> {
    let entries: Vec<Value> = apps
        .iter()
        .map(|(p, c)| json!({"path": p, "content": c}))
        .collect();
    let mut manifest: Vec<Value> = apps.iter().map(|(p, _)| json!({"path": p})).collect();
    manifest.extend(manifest_extra.iter().map(|p| json!({"path": p})));
    vec![
        part(
            g,
            0,
            4,
            "render_inputs",
            json!([{"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}, "brief": "Be concise."}]),
        ),
        part(g, 1, 4, "apps", json!(entries)),
        part(g, 2, 4, "manifest", json!(manifest)),
        part(g, 3, 4, "final", json!([])),
    ]
}

fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// Accepted paths install 0444 in 0555 directories, refused paths (a deeper path, a bad id, a bad
/// name, traversal, an absolute path, the skills directory) install nothing, and a folder that
/// leaves the manifest is pruned at the next commit.
#[test]
fn app_folders_are_installed_read_only_refused_when_malformed_and_pruned() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let good = [
        (".aurora/apps/gitlab/guide.md", "guide"),
        (".aurora/apps/gitlab/tools.md", "tools"),
        (".aurora/apps/gitlab/api.json", "{}"),
        (".aurora/apps/gitlab/rest-api_v4.md", "ref"),
        (".aurora/apps/indico/guide.md", "indico"),
    ];
    let bad = [
        ".aurora/apps/gitlab/deeper/guide.md",
        ".aurora/apps/Bad/guide.md",
        ".aurora/apps/a/guide.md",
        ".aurora/apps/gitlab/Guide.md",
        ".aurora/apps/gitlab/script.sh",
        ".aurora/apps/gitlab/.hidden.md",
        ".aurora/apps/../escape.md",
        ".aurora/apps/guide.md",
        ".aurora/apps/gitlab",
        "/etc/passwd",
        ".aurora/tools.json",
        ".aurora/apps/gitlab/-x.md",
    ];
    let all: Vec<(&str, &str)> = good
        .iter()
        .copied()
        .chain(bad.iter().map(|p| (*p, "x")))
        .collect();
    let g = apps_generation(1, &all, &[]);
    assert!(matches!(
        apply_all(&mut p, &g),
        PartOutcome::ReadyToCommit(1)
    ));
    p.commit(1).unwrap();
    for (rel, c) in good {
        let f = root.join(rel);
        assert_eq!(fs::read_to_string(&f).unwrap(), c, "{rel}");
        assert_eq!(mode(&f), 0o444, "{rel}");
    }
    assert_eq!(mode(&root.join(".aurora/apps/gitlab")), 0o555);
    assert_eq!(mode(&root.join(".aurora/apps/indico")), 0o555);
    assert!(!root.join(".aurora/apps/gitlab/deeper").exists());
    assert!(!root.join(".aurora/apps/escape.md").exists());
    assert!(!root.join(".aurora/escape.md").exists());
    assert!(!root.join(".aurora/tools.json").exists());
    assert!(!root.join(".aurora/apps/a").exists());
    assert!(!root.join(".aurora/apps/Bad").exists());

    // Generation 2 keeps one app, changes a file: the other folder is gone, the changed file is
    // replaced even though it is read-only.
    let g = apps_generation(2, &[(".aurora/apps/gitlab/guide.md", "guide v2")], &[]);
    apply_all(&mut p, &g);
    p.commit(2).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(".aurora/apps/gitlab/guide.md")).unwrap(),
        "guide v2"
    );
    assert!(!root.join(".aurora/apps/gitlab/tools.md").exists());
    assert!(!root.join(".aurora/apps/gitlab/api.json").exists());
    assert!(!root.join(".aurora/apps/indico").exists());

    // No app at all: the folders are gone.
    apply_all(&mut p, &apps_generation(3, &[], &[]));
    p.commit(3).unwrap();
    assert!(!root.join(".aurora/apps/gitlab").exists());
}

/// A path of a skills or memory part cannot reach `.aurora/apps`, and an `apps` part cannot reach
/// skills or memory.
#[test]
fn app_paths_and_other_roots_do_not_cross_part_kinds() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let mut g = apps_generation(
        1,
        &[
            (".claude/skills/s/SKILL.md", "skill via apps"),
            ("memory/m.md", "memory via apps"),
        ],
        &[],
    );
    g[1] = part(
        1,
        1,
        4,
        "apps",
        json!([
            {"path": ".claude/skills/s/SKILL.md", "content": "skill via apps"},
            {"path": "memory/m.md", "content": "memory via apps"}
        ]),
    );
    apply_all(&mut p, &g);
    p.commit(1).unwrap();
    assert!(!root.join(".claude/skills/s/SKILL.md").exists());
    assert!(!root.join("memory/m.md").exists());

    let mut g = generation(2, &[], &[]);
    g[0] = part(
        2,
        0,
        6,
        "skills",
        json!([{"path": ".aurora/apps/x1/guide.md", "content": "via skills"}]),
    );
    g[1] = part(
        2,
        1,
        6,
        "memory",
        json!([{"path": ".aurora/apps/x1/tools.md", "content": "via memory"}]),
    );
    apply_all(&mut p, &g);
    p.commit(2).unwrap();
    assert!(!root.join(".aurora/apps").exists());
}

/// Files the agent put in `.aurora/apps` (a symlink in place of a folder) are not followed.
#[test]
fn a_symlinked_app_folder_is_replaced_not_followed() {
    let d = TempDir::new();
    let root = d.path();
    let elsewhere = TempDir::new();
    fs::create_dir_all(root.join(".aurora/apps")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), root.join(".aurora/apps/gitlab")).unwrap();
    let mut p = projector(root);
    apply_all(
        &mut p,
        &apps_generation(1, &[(".aurora/apps/gitlab/guide.md", "guide")], &[]),
    );
    p.commit(1).unwrap();
    assert!(!elsewhere.path().join("guide.md").exists());
    assert_eq!(
        fs::read_to_string(root.join(".aurora/apps/gitlab/guide.md")).unwrap(),
        "guide"
    );
    assert!(
        !fs::symlink_metadata(root.join(".aurora/apps/gitlab"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

/// The `.aurora/` content around the folders (tools, render inputs, sync staging, the pg files) is
/// not touched by them and a sync that changes only the folders leaves the instruction files
/// byte-identical (so nothing is released for it).
#[test]
fn a_sync_that_changes_only_the_folders_keeps_the_instruction_files() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let tools = json!([{"name": "memory_save"}]);
    let mk = |g: u64, guide: &str| {
        let mut parts = apps_generation(g, &[(".aurora/apps/gitlab/guide.md", guide)], &[]);
        parts.insert(1, part(g, 1, 5, "tools", tools.clone()));
        let n = u32::try_from(parts.len()).unwrap();
        for (i, s) in parts.iter_mut().enumerate() {
            s.part_index = u32::try_from(i).unwrap();
            s.part_count = n;
        }
        parts
    };
    apply_all(&mut p, &mk(1, "one"));
    p.commit(1).unwrap();
    let before = fs::read(root.join("AGENTS.md")).unwrap();
    let ri_before = fs::read(root.join(".aurora/render_inputs.json")).unwrap();
    apply_all(&mut p, &mk(2, "two"));
    p.commit(2).unwrap();
    assert_eq!(fs::read(root.join("AGENTS.md")).unwrap(), before);
    assert_eq!(
        fs::read(root.join(".aurora/render_inputs.json")).unwrap(),
        ri_before
    );
    assert_eq!(
        fs::read_to_string(root.join(".aurora/apps/gitlab/guide.md")).unwrap(),
        "two"
    );
    assert!(root.join(".aurora/tools.json").exists());
}

/// (written before the feature; passes before and after) A sync with a part kind this supervisor
/// does not know and render-input fields it does not know commits, and the instruction file is
/// exactly what the known fields render.
#[test]
fn unknown_part_kinds_and_render_input_fields_are_skipped() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let known = vec![
        part(
            1,
            0,
            3,
            "render_inputs",
            json!([{"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"},
                    "brief": "Be concise."}]),
        ),
        part(1, 1, 3, "manifest", json!([])),
        part(1, 2, 3, "final", json!([])),
    ];
    apply_all(&mut p, &known);
    p.commit(1).unwrap();
    let want = fs::read(root.join("AGENTS.md")).unwrap();

    let d2 = TempDir::new();
    let mut p2 = projector(d2.path());
    let with_unknown = vec![
        part(
            1,
            0,
            4,
            "render_inputs",
            json!([{"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"},
                    "brief": "Be concise.", "from_the_future": {"a": 1},
                    "apps": []}]),
        ),
        part(1, 1, 4, "from_the_future", json!([{"x": 1}])),
        part(1, 2, 4, "manifest", json!([])),
        part(1, 3, 4, "final", json!([])),
    ];
    apply_all(&mut p2, &with_unknown);
    p2.commit(1).unwrap();
    assert_eq!(fs::read(d2.path().join("AGENTS.md")).unwrap(), want);
}

/// (P5-GUIDE-6, C-653) The `aurora-agent` sentences are rendered only when the supervisor found
/// the wrapper (`agent_cli`); the same render inputs otherwise keep the `tools.md` sentence.
#[test]
fn the_aurora_agent_sentences_follow_the_agent_cli_flag() {
    let render = |agent_cli: bool| -> String {
        let d = TempDir::new();
        let mut p = HomeProjector::new(HomeConfig {
            root: d.path().to_path_buf(),
            skills_dir: Some(".claude/skills".into()),
            instruction_files: vec!["AGENTS.md".into()],
            pg: None,
            agent_cli,
        });
        let mut parts = generation(1, &[], &[]);
        parts[2] = part(
            1,
            2,
            6,
            "render_inputs",
            json!([
                {"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}},
                {"apps": [{"name": "Demo", "accounts": [], "docs_links": [], "app_id": "demo",
                           "summary": "Things.", "guide_path": ".aurora/apps/demo/guide.md"}]}
            ]),
        );
        apply_all(&mut p, &parts);
        p.commit(1).unwrap();
        fs::read_to_string(d.path().join("AGENTS.md")).unwrap()
    };
    let with = render(true);
    assert!(with.contains("`aurora-agent search <words>`"), "{with}");
    assert!(
        with.contains("`aurora-agent call` does both for you"),
        "{with}"
    );
    assert!(!with.contains("`tools.md` lists every operation"));
    let without = render(false);
    assert!(!without.contains("aurora-agent"), "{without}");
    assert!(without.contains("`tools.md` lists every operation"));
}
