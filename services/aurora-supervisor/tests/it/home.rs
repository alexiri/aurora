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
