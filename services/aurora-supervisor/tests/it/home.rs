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
use rustix::fs::{CWD, FileType, Mode, mknodat};
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
        skills_dir: Some(caps.skills_dir.clone()),
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
    let w = h.scan(&Default::default());
    assert_eq!(w.len(), 1, "only the small file is harvested");
    assert_eq!(w[0].key, "ok");
    let notices = h.take_notices();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("huge.md") && notices[0].contains("larger than"));
    // The oversized file is reported once, not on every scan.
    assert!(h.scan(&Default::default()).is_empty());
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
    assert!(
        h.scan(&Default::default()).is_empty(),
        "pristine projection is ignored"
    );

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
    let w = h.scan(&Default::default());
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

    assert!(
        h.scan(&Default::default()).is_empty(),
        "unchanged edits are not re-uploaded"
    );
    fs::write(root.join("memory/new.md"), "changed again").unwrap();
    assert_eq!(h.scan(&Default::default()).len(), 1);

    // Canonical render supersedes the edit; nothing more to upload.
    fs::write(root.join("memory/new.md"), seal("canonical")).unwrap();
    assert!(h.scan(&Default::default()).is_empty());
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

/// (P5-CALDAV, D-275, C-845) `servers` in the render inputs become `~/.aurora/servers.json`
/// (`0600`, name, app and address only) and disappear with the last server.
#[test]
fn servers_in_render_inputs_become_servers_json_and_disappear_with_the_last_server() {
    use std::os::unix::fs::PermissionsExt as _;
    let d = TempDir::new();
    let mut p = projector(d.path());
    let a = Uuid::now_v7();
    let with = |g: u64, servers: Value| -> Vec<HomeSync> {
        let mut parts = generation(g, &[], &[]);
        parts[2] = part(
            g,
            2,
            6,
            "render_inputs",
            json!([{"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}, "servers": servers}]),
        );
        parts
    };
    let file = d.path().join(".aurora/servers.json");
    apply_all(
        &mut p,
        &with(
            1,
            json!([{"name": "alice at dav.example.test", "app_id": "caldav", "connection_id": a,
                    "url": "https://dav.example.test/dav/calendars/alice/"}]),
        ),
    );
    p.commit(1).unwrap();
    let v: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        v,
        json!([{"name": "alice at dav.example.test", "app": "caldav",
                "url": "https://dav.example.test/dav/calendars/alice/"}])
    );
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
        (".aurora/apps/gitlab/app.md", "guide"),
        (".aurora/apps/gitlab/tools.md", "tools"),
        (".aurora/apps/gitlab/api.json", "{}"),
        (".aurora/apps/indico/app.md", "indico"),
        // The pre-P5-APPSKILL name of `app.md`: an API one release older still sends it.
        (".aurora/apps/legacy/guide.md", "legacy"),
    ];
    let bad = [
        ".aurora/apps/gitlab/deeper/app.md",
        ".aurora/apps/Bad/app.md",
        ".aurora/apps/a/app.md",
        ".aurora/apps/gitlab/Guide.md",
        ".aurora/apps/gitlab/script.sh",
        // (C-1134) Package-shipped reference files are not projected here any more: only the files
        // Aurora writes are accepted.
        ".aurora/apps/gitlab/rest-api_v4.md",
        ".aurora/apps/gitlab/recipe.md",
        ".aurora/apps/gitlab/data.json",
        ".aurora/apps/gitlab/.hidden.md",
        ".aurora/apps/../escape.md",
        ".aurora/apps/app.md",
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
    let g = apps_generation(2, &[(".aurora/apps/gitlab/app.md", "guide v2")], &[]);
    apply_all(&mut p, &g);
    p.commit(2).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(".aurora/apps/gitlab/app.md")).unwrap(),
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
        json!([{"path": ".aurora/apps/x1/app.md", "content": "via skills"}]),
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
        &apps_generation(1, &[(".aurora/apps/gitlab/app.md", "guide")], &[]),
    );
    p.commit(1).unwrap();
    assert!(!elsewhere.path().join("app.md").exists());
    assert_eq!(
        fs::read_to_string(root.join(".aurora/apps/gitlab/app.md")).unwrap(),
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
        let mut parts = apps_generation(g, &[(".aurora/apps/gitlab/app.md", guide)], &[]);
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
        fs::read_to_string(root.join(".aurora/apps/gitlab/app.md")).unwrap(),
        "two"
    );
    assert!(root.join(".aurora/tools.json").exists());
}

// ---- (P6-TPL-6, C-1143) the helper files

/// A generation with the render inputs (optionally naming the helpers root), a `helpers` part and
/// the manifest naming the paths.
fn helpers_generation(g: u64, helpers: &[(&str, &str, bool)], with_inputs: bool) -> Vec<HomeSync> {
    let entries: Vec<Value> = helpers
        .iter()
        .map(|(p, c, e)| json!({"path": p, "content": c, "exec": e}))
        .collect();
    let manifest: Vec<Value> = helpers.iter().map(|(p, _, _)| json!({"path": p})).collect();
    let mut ri = json!([{"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}, "brief": "Be concise."}]);
    if with_inputs {
        ri[0]["helpers"] = json!({"dir": ".aurora/helpers", "count": helpers.len()});
    }
    vec![
        part(g, 0, 4, "render_inputs", ri),
        part(g, 1, 4, "helpers", json!(entries)),
        part(g, 2, 4, "manifest", json!(manifest)),
        part(g, 3, 4, "final", json!([])),
    ]
}

/// Accepted helper paths install 0555 (exec) or 0444 in 0555 directories, refused paths
/// (traversal, a hidden file, too deep, a space, the root itself, an absolute path, another
/// root) install nothing, and a helper that leaves the manifest is pruned at the next commit.
#[test]
fn helper_files_are_installed_read_only_refused_outside_and_pruned() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let good = [
        (".aurora/helpers/run.sh", "#!/bin/sh\n", true),
        (".aurora/helpers/docs/note.md", "note", false),
        // Four segments below the root is the deepest allowed path.
        (".aurora/helpers/a/b/c/d.md", "deep", false),
    ];
    let bad = [
        ".aurora/helpers/../escape.sh",
        ".aurora/helpers/.hidden.sh",
        // Five segments below the root is too deep.
        ".aurora/helpers/a/b/c/d/e.md",
        ".aurora/helpers/sp ace.sh",
        ".aurora/helpers",
        "/etc/passwd",
        ".aurora/tools.json",
        ".aurora/apps/gitlab/app.md",
        ".claude/skills/s/SKILL.md",
        "memory/m.md",
    ];
    let all: Vec<(&str, &str, bool)> = good
        .iter()
        .copied()
        .chain(bad.iter().map(|p| (*p, "x", false)))
        .collect();
    let g = helpers_generation(1, &all, true);
    assert!(matches!(
        apply_all(&mut p, &g),
        PartOutcome::ReadyToCommit(1)
    ));
    p.commit(1).unwrap();
    for (rel, c, exec) in good {
        let f = root.join(rel);
        assert_eq!(fs::read_to_string(&f).unwrap(), c, "{rel}");
        assert_eq!(mode(&f), if exec { 0o555 } else { 0o444 }, "{rel}");
    }
    assert_eq!(mode(&root.join(".aurora/helpers/docs")), 0o555);
    assert_eq!(mode(&root.join(".aurora/helpers/a/b/c")), 0o555);
    assert!(!root.join(".aurora/escape.sh").exists());
    assert!(!root.join(".aurora/helpers/.hidden.sh").exists());
    assert!(!root.join(".aurora/helpers/a/b/c/d/e.md").exists());
    assert!(!root.join(".aurora/helpers/sp ace.sh").exists());
    assert!(!root.join(".aurora/tools.json").exists());
    assert!(!root.join(".aurora/apps").exists());
    assert!(!root.join(".claude/skills/s").exists());
    assert!(!root.join("memory/m.md").exists());

    // Generation 2 keeps two helpers, changes both: the dropped one is pruned even though the
    // tree is read-only, and the changed exec file is replaced.
    let g = helpers_generation(
        2,
        &[
            (".aurora/helpers/run.sh", "#!/bin/sh\necho v2\n", true),
            (".aurora/helpers/docs/note.md", "note v2", false),
        ],
        true,
    );
    apply_all(&mut p, &g);
    p.commit(2).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(".aurora/helpers/run.sh")).unwrap(),
        "#!/bin/sh\necho v2\n"
    );
    assert_eq!(mode(&root.join(".aurora/helpers/run.sh")), 0o555);
    assert_eq!(
        fs::read_to_string(root.join(".aurora/helpers/docs/note.md")).unwrap(),
        "note v2"
    );
    assert!(!root.join(".aurora/helpers/a").exists());

    // No helper at all: everything under the root is gone (the empty root itself stays, like the
    // apps root).
    apply_all(&mut p, &helpers_generation(3, &[], false));
    p.commit(3).unwrap();
    assert!(!root.join(".aurora/helpers/run.sh").exists());
    assert!(!root.join(".aurora/helpers/docs").exists());
    assert!(
        fs::read_dir(root.join(".aurora/helpers"))
            .unwrap()
            .next()
            .is_none()
    );
}

/// A path of a skills or memory part cannot reach `.aurora/helpers`, and a `helpers` part cannot
/// reach skills, memory or the app folders.
#[test]
fn helper_paths_and_other_roots_do_not_cross_part_kinds() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let g = helpers_generation(
        1,
        &[
            (".claude/skills/s/SKILL.md", "skill via helpers", false),
            ("memory/m.md", "memory via helpers", false),
            (".aurora/apps/gitlab/app.md", "app via helpers", false),
        ],
        false,
    );
    apply_all(&mut p, &g);
    p.commit(1).unwrap();
    assert!(!root.join(".claude/skills/s").exists());
    assert!(!root.join("memory/m.md").exists());
    assert!(!root.join(".aurora/apps").exists());
    assert!(!root.join(".aurora/helpers").exists());

    let mut g = generation(2, &[], &[]);
    g[0] = part(
        2,
        0,
        6,
        "skills",
        json!([{"path": ".aurora/helpers/x/h.sh", "content": "via skills"}]),
    );
    g[1] = part(
        2,
        1,
        6,
        "memory",
        json!([{"path": ".aurora/helpers/x/h.sh", "content": "via memory"}]),
    );
    apply_all(&mut p, &g);
    p.commit(2).unwrap();
    assert!(!root.join(".aurora/helpers").exists());
}

/// A sync that changes only the contents of the helper files (same paths, same count) leaves the
/// instruction files byte-identical (so nothing is released for it), and the capability sentence
/// is present exactly when the render inputs name the helpers root.
#[test]
fn a_sync_that_changes_only_helper_contents_keeps_the_instruction_files() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    apply_all(
        &mut p,
        &helpers_generation(
            1,
            &[
                (".aurora/helpers/run.sh", "one", true),
                (".aurora/helpers/docs/note.md", "note", false),
            ],
            true,
        ),
    );
    p.commit(1).unwrap();
    let with_sentence = fs::read(root.join("AGENTS.md")).unwrap();
    assert!(
        String::from_utf8(with_sentence.clone())
            .unwrap()
            .contains("Helper files from your template are in `.aurora/helpers` (read-only).")
    );

    apply_all(
        &mut p,
        &helpers_generation(
            2,
            &[
                (".aurora/helpers/run.sh", "two", true),
                (".aurora/helpers/docs/note.md", "note v2", false),
            ],
            true,
        ),
    );
    p.commit(2).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(".aurora/helpers/run.sh")).unwrap(),
        "two"
    );
    assert_eq!(fs::read(root.join("AGENTS.md")).unwrap(), with_sentence);

    // Without the render input naming the root (and with no helper at all) the sentence is gone.
    apply_all(&mut p, &helpers_generation(3, &[], false));
    p.commit(3).unwrap();
    let without = fs::read(root.join("AGENTS.md")).unwrap();
    assert_ne!(without, with_sentence);
    assert!(!String::from_utf8(without).unwrap().contains("Helper files"));
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

    // (P5-SKILLDIR) The instruction file names the absolute skills folder, so the second run uses
    // the same root path, emptied first, to stay comparable.
    drop(p);
    fs::remove_dir_all(root).unwrap();
    fs::create_dir_all(root).unwrap();
    let mut p2 = projector(root);
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
    assert_eq!(fs::read(root.join("AGENTS.md")).unwrap(), want);
}

/// (P5-GUIDE-7, C-654) The catalogue digest, which releases idle sessions, covers the core
/// descriptors and the rendered instruction files: an on-demand tool's description changing in a
/// core-only sandbox does not move it, a core descriptor does, and in mode `all` any descriptor
/// does.
#[test]
fn the_catalogue_digest_follows_core_descriptors_and_instruction_files_only() {
    // (P5-SKILLDIR) The instruction file names the absolute skills folder, so every call uses the
    // same root path, emptied first, to stay comparable.
    let base = TempDir::new();
    let digest = |mode: &str, core_desc: &str, demand_desc: &str, brief: &str| -> String {
        let root = base.path().join("home");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut p = HomeProjector::new(HomeConfig {
            root,
            skills_dir: Some(".claude/skills".into()),
            instruction_files: vec!["AGENTS.md".into()],
            pg: None,
            agent_cli: true,
        });
        let mut parts = generation(1, &[], &[]);
        parts[2] = part(
            1,
            2,
            6,
            "render_inputs",
            json!([
                {"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}},
                {"brief": brief},
                {"platform_tools": {"mode": mode, "families": [
                    {"name": "memory", "purpose": "Notes.", "tools": ["memory_search"]}]}}
            ]),
        );
        parts[3] = part(
            1,
            3,
            6,
            "tools",
            json!([
                {"name": "send_file", "description": core_desc, "exposure": "core"},
                {"name": "memory_search", "description": demand_desc, "exposure": "on_demand"}
            ]),
        );
        apply_all(&mut p, &parts);
        assert_eq!(
            p.catalogue_digest(),
            None,
            "nothing installed before the commit"
        );
        p.commit(1).unwrap();
        p.catalogue_digest().unwrap()
    };
    let base = digest("on_demand", "c", "d", "b");
    assert_eq!(base, digest("on_demand", "c", "d", "b"), "deterministic");
    assert_eq!(
        base,
        digest("on_demand", "c", "d2", "b"),
        "an on-demand description is not in the digest"
    );
    assert_ne!(
        base,
        digest("on_demand", "c2", "d", "b"),
        "a core descriptor is"
    );
    assert_ne!(
        base,
        digest("on_demand", "c", "d", "b2"),
        "the instruction file is"
    );
    let all = digest("all", "c", "d", "b");
    assert_ne!(
        all,
        digest("all", "c", "d2", "b"),
        "mode all lists every descriptor"
    );
}

/// (P5-GUIDE-7, C-654) The bridge lists only the core tools exactly when the installed render
/// inputs say `on_demand` and the sandbox has `aurora-agent`; an older API (no `platform_tools`),
/// mode `all` and an older image keep listing everything (rolling upgrade, old API + new
/// supervisor). The *Platform tools* fragment is in the instruction file in the same case only.
#[test]
fn the_bridge_is_core_only_exactly_for_on_demand_with_the_command() {
    let run = |platform: Option<serde_json::Value>, agent_cli: bool| -> (bool, String) {
        let d = TempDir::new();
        let mut p = HomeProjector::new(HomeConfig {
            root: d.path().to_path_buf(),
            skills_dir: Some(".claude/skills".into()),
            instruction_files: vec!["AGENTS.md".into()],
            pg: None,
            agent_cli,
        });
        let mut inputs =
            vec![json!({"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}})];
        if let Some(pt) = platform {
            inputs.push(json!({ "platform_tools": pt }));
        }
        let mut parts = generation(1, &[], &[]);
        parts[2] = part(1, 2, 6, "render_inputs", json!(inputs));
        apply_all(&mut p, &parts);
        assert!(
            !p.bridge_core_only(),
            "nothing installed: everything is listed"
        );
        p.commit(1).unwrap();
        let md = fs::read_to_string(d.path().join("AGENTS.md")).unwrap_or_default();
        (p.bridge_core_only(), md)
    };
    let pt = |mode: &str| {
        json!({"mode": mode, "families": [
            {"name": "memory", "purpose": "Notes.", "tools": ["memory_search"]}]})
    };
    let (core, md) = run(Some(pt("on_demand")), true);
    assert!(core);
    assert!(
        md.contains("## Platform tools") && md.contains("- memory ("),
        "{md}"
    );
    let (core, md) = run(Some(pt("all")), true);
    assert!(!core && !md.contains("Platform tools"));
    let (core, md) = run(Some(pt("on_demand")), false);
    assert!(
        !core && !md.contains("Platform tools"),
        "an older image loses nothing"
    );
    let (core, md) = run(None, true);
    assert!(
        !core && !md.contains("Platform tools"),
        "an older API sends no platform_tools"
    );
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
                           "summary": "Things.", "guide_path": ".aurora/apps/demo/app.md"}]}
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

/// (SEC-150) The agent's uid owns the home, and app skills land at predictable names under the
/// skills folder. A symlink the agent puts at the skills root, at a parent of it (`.claude`) or at
/// one skill folder must never be written, chmodded or renamed through: after the sync nothing is
/// in the outside directory (and its mode is unchanged) and the skills are in real directories.
fn link_attack(replace: impl Fn(&Path, &Path)) {
    use std::os::unix::fs::PermissionsExt as _;
    let d = TempDir::new();
    let outside = TempDir::new();
    let root = d.path();
    let out = outside.path();
    fs::set_permissions(out, fs::Permissions::from_mode(0o700)).unwrap();
    let skills = [
        (".claude/skills/demo/SKILL.md", "v1"),
        (".claude/skills/demo/scripts/run.sh", "echo"),
    ];
    let mut p = projector(root);
    apply_all(&mut p, &generation(1, &skills, &[]));
    p.commit(1).unwrap();
    // Now the agent swaps a directory of the managed path for a link to somewhere else.
    replace(root, out);
    let skills2 = [
        (".claude/skills/demo/SKILL.md", "v2"),
        (".claude/skills/demo/scripts/run.sh", "echo 2"),
        (".claude/skills/other/SKILL.md", "other"),
    ];
    apply_all(&mut p, &generation(2, &skills2, &[]));
    p.commit(2).unwrap();
    assert_eq!(
        fs::read_dir(out).unwrap().count(),
        0,
        "something was written outside the home"
    );
    assert_eq!(mode(out), 0o700, "the outside directory was chmodded");
    // The skills are real directories, all the way down, with the new content.
    for rel in [
        ".claude",
        ".claude/skills",
        ".claude/skills/demo",
        ".claude/skills/other",
    ] {
        let m = fs::symlink_metadata(root.join(rel)).unwrap();
        assert!(m.file_type().is_dir(), "{rel} is not a real directory");
    }
    assert_eq!(
        fs::read_to_string(root.join(".claude/skills/demo/SKILL.md")).unwrap(),
        "v2"
    );
    assert_eq!(
        fs::read_to_string(root.join(".claude/skills/other/SKILL.md")).unwrap(),
        "other"
    );
}

#[test]
fn a_skill_folder_replaced_by_a_symlink_is_not_written_through() {
    link_attack(|root, out| {
        let p = root.join(".claude/skills/demo");
        make_writable(&root.join(".claude/skills"));
        fs::remove_dir_all(&p).unwrap();
        std::os::unix::fs::symlink(out, &p).unwrap();
    });
}

#[test]
fn the_skills_root_replaced_by_a_symlink_is_not_written_through() {
    link_attack(|root, out| {
        let p = root.join(".claude/skills");
        make_writable(&root.join(".claude"));
        make_writable(&p);
        fs::remove_dir_all(&p).unwrap();
        std::os::unix::fs::symlink(out, &p).unwrap();
    });
}

#[test]
fn a_parent_of_the_skills_root_replaced_by_a_symlink_is_not_written_through() {
    link_attack(|root, out| {
        let p = root.join(".claude");
        make_writable(&p);
        make_writable(&p.join("skills"));
        fs::remove_dir_all(&p).unwrap();
        std::os::unix::fs::symlink(out, &p).unwrap();
    });
}

/// The projection makes its folders read-only; the agent's uid owns the parent and replaces them.
fn make_writable(p: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fn walk(p: &Path) {
        let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o755));
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    walk(&e.path());
                }
            }
        }
    }
    walk(p);
}

// ---- links the agent plants under the helpers root and the memory folder

/// A directory outside the home with a known mode and one file in it.
fn outside_dir() -> TempDir {
    use std::os::unix::fs::PermissionsExt as _;
    let d = TempDir::new();
    fs::write(d.path().join("victim"), "untouched").unwrap();
    fs::set_permissions(d.path().join("victim"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}

fn assert_untouched(out: &Path) {
    let names: Vec<_> = fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        ["victim"],
        "something was written or removed outside"
    );
    assert_eq!(fs::read_to_string(out.join("victim")).unwrap(), "untouched");
    assert_eq!(mode(&out.join("victim")), 0o600, "outside file chmodded");
    assert_eq!(mode(out), 0o700, "outside directory chmodded");
}

fn helper_link_attack(link: &str) {
    let d = TempDir::new();
    let outside = outside_dir();
    let root = d.path();
    let mut p = projector(root);
    let helpers = [
        (".aurora/helpers/run.sh", "v1", true),
        (".aurora/helpers/sub/x.sh", "v1", false),
    ];
    apply_all(&mut p, &helpers_generation(1, &helpers, true));
    p.commit(1).unwrap();
    let target = root.join(link);
    make_writable(&root.join(".aurora/helpers"));
    fs::remove_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(outside.path(), &target).unwrap();
    let helpers = [
        (".aurora/helpers/run.sh", "v2", true),
        (".aurora/helpers/sub/x.sh", "v2", false),
    ];
    apply_all(&mut p, &helpers_generation(2, &helpers, true));
    p.commit(2).unwrap();
    assert_untouched(outside.path());
    assert!(fs::symlink_metadata(&target).unwrap().file_type().is_dir());
    assert_eq!(
        fs::read_to_string(root.join(".aurora/helpers/sub/x.sh")).unwrap(),
        "v2"
    );
}

#[test]
fn the_helpers_root_replaced_by_a_symlink_is_not_written_through() {
    helper_link_attack(".aurora/helpers");
}

#[test]
fn a_helper_subfolder_replaced_by_a_symlink_is_not_written_through() {
    helper_link_attack(".aurora/helpers/sub");
}

#[test]
fn an_edited_or_deleted_helper_is_restored_by_the_next_generation() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let helpers = [
        (".aurora/helpers/run.sh", "original run", true),
        (".aurora/helpers/lib.sh", "original lib", false),
    ];
    apply_all(&mut p, &helpers_generation(1, &helpers, true));
    p.commit(1).unwrap();
    make_writable(&root.join(".aurora/helpers"));
    let run = root.join(".aurora/helpers/run.sh");
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(&run, "the agent's edit").unwrap();
    fs::remove_file(root.join(".aurora/helpers/lib.sh")).unwrap();
    apply_all(&mut p, &helpers_generation(2, &helpers, true));
    p.commit(2).unwrap();
    assert_eq!(fs::read_to_string(&run).unwrap(), "original run");
    assert_eq!(mode(&run), 0o555);
    assert_eq!(
        fs::read_to_string(root.join(".aurora/helpers/lib.sh")).unwrap(),
        "original lib"
    );
    assert_eq!(mode(&root.join(".aurora/helpers/lib.sh")), 0o444);
}

/// A writer for the `tracing` output of one closure.
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn warnings_of<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    let captured = Captured::default();
    let sink = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .finish();
    let out = tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    (out, text.lines().map(str::to_owned).collect())
}

/// One helper the sandbox cannot hold (a file that is also a folder) is skipped with a warning:
/// the generation still commits, with everything else in it.
#[test]
fn a_helper_that_cannot_be_written_does_not_stall_the_sync() {
    for pair in [
        [".aurora/helpers/run", ".aurora/helpers/run/x.sh"],
        [".aurora/helpers/run/x.sh", ".aurora/helpers/run"],
    ] {
        let d = TempDir::new();
        let root = d.path();
        let mut p = projector(root);
        let helpers = [
            (pair[0], "first", true),
            (pair[1], "second", true),
            (".aurora/helpers/ok.sh", "ok", true),
        ];
        let parts = helpers_generation(1, &helpers, true);
        let (outcome, warnings) = warnings_of(|| {
            let mut last = None;
            for s in &parts {
                last = Some(p.apply_part(s));
            }
            last.unwrap()
        });
        assert_eq!(outcome.unwrap(), PartOutcome::ReadyToCommit(1), "{pair:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        p.commit(1).unwrap();
        assert_eq!(fs::read_to_string(root.join(pair[0])).unwrap(), "first");
        assert_eq!(
            fs::read_to_string(root.join(".aurora/helpers/ok.sh")).unwrap(),
            "ok"
        );
    }
}

/// A helper may turn from a file into a folder (or back) between two generations: the old one is
/// pruned before the new one is installed.
#[test]
fn a_helper_can_change_between_a_file_and_a_folder() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    for (g, path) in [
        (1, ".aurora/helpers/run"),
        (2, ".aurora/helpers/run/x.sh"),
        (3, ".aurora/helpers/run"),
    ] {
        apply_all(
            &mut p,
            &helpers_generation(g, &[(path, "body", true)], true),
        );
        p.commit(g).unwrap();
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "body");
    }
}

/// Generation 1 installs the skill file `paths[0]`, generation 2 `paths[1]`: both generations
/// install, and so do their memory and instructions.
fn skill_changes_shape(paths: [&str; 2]) {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    for (g, path) in (1..).zip(paths) {
        let note = format!("note {g}");
        apply_all(
            &mut p,
            &generation(
                g,
                &[(path, "body"), (".claude/skills/s/SKILL.md", "s")],
                &[("likes", &note)],
            ),
        );
        p.commit(g)
            .unwrap_or_else(|e| panic!("generation {g}: {e}"));
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), "body");
        let memory = fs::read_to_string(root.join("memory/likes.md")).unwrap();
        assert!(memory.starts_with(&note), "{memory}");
        let agents = fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert!(agents.contains(&format!("brief gen {g}")), "{agents}");
        assert_eq!(mode(&root.join(".claude/skills/s/scripts")), 0o555);
    }
}

/// (FIX-258) A skill file may turn into a folder between two generations.
#[test]
fn a_skill_file_can_become_a_folder() {
    skill_changes_shape([
        ".claude/skills/s/scripts/run",
        ".claude/skills/s/scripts/run/x.sh",
    ]);
}

/// (FIX-258) And a folder may turn back into a file.
#[test]
fn a_skill_folder_can_become_a_file() {
    skill_changes_shape([
        ".claude/skills/s/scripts/run/x.sh",
        ".claude/skills/s/scripts/run",
    ]);
}

/// (FIX-258) A skill file the home cannot hold (a file that is also a folder) is skipped with a
/// warning: the rest of the generation is installed.
#[test]
fn a_skill_that_cannot_be_written_does_not_stall_the_sync() {
    for pair in [
        [".claude/skills/s/run", ".claude/skills/s/run/x.sh"],
        [".claude/skills/s/run/x.sh", ".claude/skills/s/run"],
    ] {
        let d = TempDir::new();
        let root = d.path();
        let mut p = projector(root);
        let parts = generation(
            1,
            &[
                (pair[0], "first"),
                (pair[1], "second"),
                (".claude/skills/s/SKILL.md", "s"),
            ],
            &[("likes", "tea")],
        );
        let (outcome, warnings) = warnings_of(|| {
            let mut last = None;
            for s in &parts {
                last = Some(p.apply_part(s));
            }
            last.unwrap()
        });
        assert!(
            matches!(outcome, Ok(PartOutcome::ReadyToCommit(1))),
            "{pair:?}: {outcome:?}"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        p.commit(1).unwrap();
        assert_eq!(fs::read_to_string(root.join(pair[0])).unwrap(), "first");
        assert_eq!(
            fs::read_to_string(root.join(".claude/skills/s/SKILL.md")).unwrap(),
            "s"
        );
        assert!(root.join("memory/likes.md").exists());
        assert!(root.join("AGENTS.md").exists());
    }
}

/// (FIX-258) The same at install time: a staged skill file whose place cannot be made is
/// skipped, and the generation commits.
#[test]
fn a_skill_that_cannot_be_installed_does_not_fail_the_commit() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    apply_all(
        &mut p,
        &generation(
            1,
            &[
                (".claude/skills/s/run/x.sh", "x"),
                (".claude/skills/s/SKILL.md", "s"),
            ],
            &[("likes", "tea")],
        ),
    );
    // A FIFO the prune does not remove, where a folder has to be made.
    fs::create_dir_all(root.join(".claude/skills/s")).unwrap();
    mknodat(
        CWD,
        root.join(".claude/skills/s/run"),
        FileType::Fifo,
        Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    p.commit(1).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(".claude/skills/s/SKILL.md")).unwrap(),
        "s"
    );
    assert!(root.join("memory/likes.md").exists());
    assert!(root.join("AGENTS.md").exists());
}

/// (FIX-258) A skill file whose new version is skipped at install (too large) is removed: the
/// old version must not run beside the new rest of the skill.
#[test]
fn a_skill_file_skipped_at_install_leaves_no_old_version() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let (run, skill) = (
        ".claude/skills/s/scripts/run.sh",
        ".claude/skills/s/SKILL.md",
    );
    apply_all(&mut p, &generation(1, &[(run, "v1"), (skill, "s1")], &[]));
    p.commit(1).unwrap();
    let oversized = "x".repeat(8 * 1024 * 1024 + 1);
    apply_all(
        &mut p,
        &generation(2, &[(run, &oversized), (skill, "s2")], &[]),
    );
    let (committed, warnings) = warnings_of(|| p.commit(2));
    committed.unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(fs::symlink_metadata(root.join(run)).is_err(), "stale file");
    assert_eq!(fs::read_to_string(root.join(skill)).unwrap(), "s2");
}

/// (FIX-258) The same for a file skipped at staging (here a file that is also a folder): its
/// old version goes, and what takes its place installs.
#[test]
fn a_skill_file_skipped_at_staging_leaves_no_old_version() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let (run, inner) = (".claude/skills/s/run", ".claude/skills/s/run/x.sh");
    apply_all(&mut p, &generation(1, &[(run, "v1")], &[]));
    p.commit(1).unwrap();
    apply_all(&mut p, &generation(2, &[(inner, "x2"), (run, "v2")], &[]));
    p.commit(2).unwrap();
    assert!(!root.join(run).is_file(), "stale file");
    assert_eq!(fs::read_to_string(root.join(inner)).unwrap(), "x2");
}

/// (FIX-258) And for a staged file that is refused because something else is at its name.
#[test]
fn a_refused_staged_skill_file_leaves_no_old_version() {
    let d = TempDir::new();
    let outside = outside_dir();
    let mut p = projector(d.path());
    let run = ".claude/skills/s/run.sh";
    apply_all(&mut p, &generation(1, &[(run, "v1")], &[]));
    p.commit(1).unwrap();
    apply_all(&mut p, &generation(2, &[(run, "v2")], &[]));
    let staged = d.path().join(".aurora/sync/files").join(run);
    fs::remove_file(&staged).unwrap();
    std::os::unix::fs::symlink(outside.path().join("victim"), &staged).unwrap();
    p.commit(2).unwrap();
    assert!(fs::symlink_metadata(d.path().join(run)).is_err(), "stale");
    assert_untouched(outside.path());
}

/// (FIX-258) A skipped helper leaves no old version either.
#[test]
fn a_skipped_helper_leaves_no_old_version() {
    let d = TempDir::new();
    let root = d.path();
    let mut p = projector(root);
    let helper = ".aurora/helpers/run.sh";
    apply_all(
        &mut p,
        &helpers_generation(1, &[(helper, "v1", true)], true),
    );
    p.commit(1).unwrap();
    let oversized = "x".repeat(8 * 1024 * 1024 + 1);
    apply_all(
        &mut p,
        &helpers_generation(2, &[(helper, &oversized, true)], true),
    );
    p.commit(2).unwrap();
    assert!(fs::symlink_metadata(root.join(helper)).is_err(), "stale");
}

/// A link left in the staging area before the commit is refused: nothing is installed under its
/// name, its target is left alone, and the rest of the generation is installed.
#[test]
fn a_staged_file_replaced_by_a_link_is_not_installed() {
    let d = TempDir::new();
    let outside = outside_dir();
    let mut p = projector(d.path());
    apply_all(
        &mut p,
        &generation(
            1,
            &[
                (".claude/skills/s/SKILL.md", "linked"),
                (".claude/skills/s/other.md", "fine"),
            ],
            &[],
        ),
    );
    let staged = d
        .path()
        .join(".aurora/sync/files/.claude/skills/s/SKILL.md");
    fs::remove_file(&staged).unwrap();
    std::os::unix::fs::symlink(outside.path().join("victim"), &staged).unwrap();
    p.commit(1).unwrap();
    assert!(fs::symlink_metadata(d.path().join(".claude/skills/s/SKILL.md")).is_err());
    assert_eq!(
        fs::read_to_string(d.path().join(".claude/skills/s/other.md")).unwrap(),
        "fine"
    );
    assert_untouched(outside.path());
}

/// A home with one locked memory installed, its sealed bytes, and the harvester over it.
fn locked_home() -> (TempDir, HomeProjector, Vec<u8>, Harvester) {
    let d = TempDir::new();
    let mut p = projector(d.path());
    let mut parts = generation(1, &[], &[("pinned", "the person's text")]);
    parts[1] = part(
        1,
        1,
        6,
        "memory",
        json!([{"path": "memory/pinned.md", "content": "the person's text", "locked": true}]),
    );
    apply_all(&mut p, &parts);
    p.commit(1).unwrap();
    let sealed = fs::read(d.path().join("memory/pinned.md")).unwrap();
    let h = Harvester::new(d.path());
    (d, p, sealed, h)
}

fn assert_restored(root: &Path, sealed: &[u8]) {
    let path = root.join("memory/pinned.md");
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_file());
    assert_eq!(fs::read(&path).unwrap(), sealed);
    assert_eq!(mode(&path), 0o444);
}

#[test]
fn a_link_at_the_restore_temp_file_is_not_written_through() {
    let (d, p, sealed, mut h) = locked_home();
    let outside = outside_dir();
    let path = d.path().join("memory/pinned.md");
    std::os::unix::fs::symlink(
        outside.path().join("victim"),
        d.path().join("memory/.pinned.md.aurora-tmp"),
    )
    .unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, "the agent's edit").unwrap();
    assert!(h.scan(&p.locked_memories()).is_empty());
    assert_untouched(outside.path());
    assert_restored(d.path(), &sealed);
}

#[test]
fn a_locked_file_replaced_by_a_link_is_restored_and_the_target_left_alone() {
    // Once with other bytes, once with the very bytes Aurora installed (nothing to restore by
    // content, but the link must not be chmodded through).
    for same_bytes in [false, true] {
        let (d, p, sealed, mut h) = locked_home();
        let outside = outside_dir();
        let victim = outside.path().join("victim");
        if same_bytes {
            fs::write(&victim, &sealed).unwrap();
        }
        let path = d.path().join("memory/pinned.md");
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        assert!(h.scan(&p.locked_memories()).is_empty());
        assert_eq!(mode(&victim), 0o600, "outside file chmodded");
        assert_eq!(
            fs::read(&victim).unwrap(),
            if same_bytes {
                sealed.clone()
            } else {
                b"untouched".to_vec()
            }
        );
        assert_restored(d.path(), &sealed);
    }
}

/// A FIFO in place of a locked file must not hang the harvest.
#[test]
fn a_fifo_in_place_of_a_locked_file_does_not_block_the_harvest() {
    let (d, p, sealed, mut h) = locked_home();
    let path = d.path().join("memory/pinned.md");
    fs::remove_file(&path).unwrap();
    mknodat(CWD, &path, FileType::Fifo, Mode::from(0o600), 0).unwrap();
    let locked = p.locked_memories();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = h.scan(&locked);
        let _ = tx.send(());
    });
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("the harvest blocked on the FIFO");
    assert_restored(d.path(), &sealed);
}

#[test]
fn a_memory_folder_replaced_by_a_link_is_not_written_through() {
    let (d, p, sealed, mut h) = locked_home();
    let outside = outside_dir();
    fs::remove_dir_all(d.path().join("memory")).unwrap();
    std::os::unix::fs::symlink(outside.path(), d.path().join("memory")).unwrap();
    assert!(h.scan(&p.locked_memories()).is_empty());
    assert_untouched(outside.path());
    assert!(
        fs::symlink_metadata(d.path().join("memory"))
            .unwrap()
            .file_type()
            .is_dir()
    );
    assert_restored(d.path(), &sealed);
}

/// The commit prunes `memory/` too: a link there must not let it remove files elsewhere.
#[test]
fn the_commit_does_not_prune_through_a_linked_memory_folder() {
    let d = TempDir::new();
    let outside = TempDir::new();
    let mut p = projector(d.path());
    fs::write(outside.path().join("other.md"), seal("someone else's")).unwrap();
    std::os::unix::fs::symlink(outside.path(), d.path().join("memory")).unwrap();
    apply_all(&mut p, &generation(1, &[], &[]));
    p.commit(1).unwrap();
    assert!(outside.path().join("other.md").exists());
}
