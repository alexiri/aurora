#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FUP-M5 / P2-TEST-1: the memory checksum trailer round-trips between the two crates that touch it.
//!
//! The API renders `memory/<key>.md` entries into `home.sync` parts (`domains::hosted::home::render`,
//! with a real database); the supervisor's `HomeProjector` seals each file with the trailer when it
//! installs the generation and its `Harvester` decides, from the trailer alone, whether an agent
//! edited a file. This test connects the real functions of both crates (no copies): what the API
//! renders and the supervisor installs is *not* harvested as an edit, and every kind of tampering
//! is.
//!
//! Location: the test needs the API's test harness, so it lives here and `aurora-api` has
//! `aurora-supervisor` as a dev-dependency only (VEL-007: the library never uses it).

use crate::common::*;
use aurora_db::repo::hosted::HomeSyncClaim;
use aurora_protocol::runner::MemoryWrite;
use aurora_supervisor::harvest::{Harvester, TRAILER_PREFIX, strip_trailer, verify};
use aurora_supervisor::home::{HomeConfig, HomeProjector, PartOutcome};
use serde_json::Value;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("aurora-home-checksum-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn insert_memory(
    h: &Harness,
    agent: Uuid,
    key: &str,
    title: Option<&str>,
    description: Option<&str>,
    content: &str,
) {
    sqlx::query(
        "INSERT INTO memories (id, agent_id, key, title, description, content, revision) \
         VALUES ($1, $2, $3, $4, $5, $6, 1)",
    )
    .bind(Uuid::now_v7())
    .bind(agent)
    .bind(key)
    .bind(title)
    .bind(description)
    .bind(content)
    .execute(h.pool())
    .await
    .unwrap();
}

fn read(root: &Path, key: &str) -> String {
    std::fs::read_to_string(root.join("memory").join(format!("{key}.md"))).unwrap()
}

fn write(root: &Path, key: &str, text: &str) {
    let p = root.join("memory").join(format!("{key}.md"));
    // The projection is read-only for the agent's uid in a sandbox; the test process owns the file.
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o644);
    std::fs::set_permissions(&p, perm).unwrap();
    std::fs::write(p, text).unwrap();
}

fn keys(w: &[MemoryWrite]) -> Vec<&str> {
    w.iter().map(|m| m.key.as_str()).collect()
}

#[tokio::test]
async fn api_rendered_memory_round_trips_the_trailer_and_tampering_is_harvested() {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, effort, brief) VALUES ($1,$2,$3,'ada','Ada','hosted','fake','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    // The shapes the renderer produces: bare content without a trailing newline, front matter with
    // a title and a description (quoting), multi-line unicode, and content that quotes the trailer
    // marker itself.
    insert_memory(&h, agent, "plain", None, None, "just a note").await;
    insert_memory(
        &h,
        agent,
        "with-front-matter",
        // Quoted: the API JSON-escapes it, the harvest must unescape it (FIX-012).
        Some(r#"Café "menu""#),
        Some("what: the user likes"),
        "line one\nline two ü 日本語\n",
    )
    .await;
    insert_memory(
        &h,
        agent,
        "quotes-marker",
        None,
        None,
        &format!("see {TRAILER_PREFIX}{} -->\nafter", "0".repeat(64)),
    )
    .await;

    // API: render the generation with the real renderer.
    let mut conn = h.pool().acquire().await.unwrap();
    let claim = HomeSyncClaim {
        sandbox_id: Uuid::now_v7(),
        boot_id: Uuid::now_v7(),
        agent_id: agent,
        generation: 1,
        harness_id: None,
    };
    let parts = aurora_api::domains::hosted::home::render(&h.state, &mut conn, &claim)
        .await
        .unwrap();
    let rendered: Vec<Value> = parts
        .iter()
        .filter(|p| p.part.kind.as_str() == "memory")
        .flat_map(|p| p.part.entries.clone())
        .collect();
    let content_of = |path: &str| -> String {
        rendered
            .iter()
            .find(|e| e["path"] == path)
            .unwrap_or_else(|| panic!("{path} not rendered: {rendered:?}"))["content"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // Supervisor: install the generation with the real projector.
    let root = Scratch::new();
    let mut projector = HomeProjector::new(HomeConfig {
        root: root.0.clone(),
        skills_dir: Some(aurora_api::domains::hosted::home::skills_dir(None).to_owned()),
        instruction_files: vec![],
        pg: None,
        agent_cli: false,
    });
    let mut outcome = None;
    for part in &parts {
        outcome = Some(projector.apply_part(part).unwrap());
    }
    assert_eq!(outcome, Some(PartOutcome::ReadyToCommit(1)));
    projector.commit(1).unwrap();

    // The installed files carry a valid trailer and, without it, are exactly what the API rendered.
    for key in ["plain", "with-front-matter", "quotes-marker"] {
        let file = read(&root.0, key);
        assert!(verify(&file), "{key}: the trailer does not verify:\n{file}");
        assert!(file.trim_end().ends_with("-->"), "{key}: trailer is last");
        let rendered_content = content_of(&format!("memory/{key}.md"));
        let expected = if rendered_content.ends_with('\n') {
            rendered_content
        } else {
            format!("{rendered_content}\n")
        };
        assert_eq!(
            strip_trailer(&file),
            expected,
            "{key}: the harvester's view of the file is not what the API rendered"
        );
    }
    // The index is not a harvested file and carries no trailer.
    assert!(!verify(&read(&root.0, "index")));

    // Harvest: nothing was edited, so nothing is uploaded; scanning again is stable.
    let mut harvester = Harvester::new(&root.0);
    assert!(
        keys(&harvester.scan(&Default::default())).is_empty(),
        "an unmodified projection was harvested as an edit"
    );
    assert!(harvester.scan(&Default::default()).is_empty());
    assert!(harvester.take_notices().is_empty());

    // Tampering 1: the agent appends a line. Only that file is harvested; the content has the edit.
    let original = read(&root.0, "plain");
    write(&root.0, "plain", &format!("{original}agent added this\n"));
    let writes = harvester.scan(&Default::default());
    assert_eq!(keys(&writes), ["plain"], "{writes:?}");
    assert!(writes[0].content.contains("agent added this"), "{writes:?}");
    // The same edit is not re-sent on the next tick.
    assert!(harvester.scan(&Default::default()).is_empty());

    // Tampering 2: the body changes but the old trailer is kept (a replayed checksum).
    let original = read(&root.0, "with-front-matter");
    write(
        &root.0,
        "with-front-matter",
        &original.replace("line one", "line ONE"),
    );
    let writes = harvester.scan(&Default::default());
    assert_eq!(keys(&writes), ["with-front-matter"], "{writes:?}");
    assert!(writes[0].content.contains("line ONE"), "{writes:?}");
    assert_eq!(writes[0].title.as_deref(), Some(r#"Café "menu""#));
    assert_eq!(
        writes[0].description.as_deref(),
        Some("what: the user likes")
    );

    // Tampering 3: the trailer itself is altered (one hex digit flipped).
    let original = read(&root.0, "quotes-marker");
    let at = original.rfind(TRAILER_PREFIX).unwrap() + TRAILER_PREFIX.len();
    let mut bytes = original.clone().into_bytes();
    bytes[at] = if bytes[at] == b'0' { b'1' } else { b'0' };
    write(&root.0, "quotes-marker", &String::from_utf8(bytes).unwrap());
    assert!(!verify(&read(&root.0, "quotes-marker")));
    assert_eq!(
        keys(&harvester.scan(&Default::default())),
        ["quotes-marker"]
    );

    // Tampering 4: the trailer is removed altogether (the file is then an agent-authored file).
    write(&root.0, "plain", "rewritten by the agent, no trailer\n");
    assert!(!verify(&read(&root.0, "plain")));
    let writes = harvester.scan(&Default::default());
    assert_eq!(keys(&writes), ["plain"], "{writes:?}");
    assert_eq!(writes[0].content, "rewritten by the agent, no trailer\n");

    // Restoring the files to their rendered state (the next sync re-seals them) is not an edit.
    for part in &parts {
        projector.apply_part(part).unwrap();
    }
    projector.commit(1).unwrap();
    for key in ["plain", "with-front-matter", "quotes-marker"] {
        assert!(
            verify(&read(&root.0, key)),
            "{key}: re-render restores the trailer"
        );
    }
    let mut fresh = Harvester::new(&root.0);
    assert!(
        fresh.scan(&Default::default()).is_empty(),
        "a re-rendered projection is clean"
    );
}

/// Renders every memory of `agent` with the real API renderer and installs it into a fresh scratch
/// root with the real projector.
async fn render_install(h: &Harness, agent: Uuid, generation: i64) -> Scratch {
    let mut conn = h.pool().acquire().await.unwrap();
    let claim = HomeSyncClaim {
        sandbox_id: Uuid::now_v7(),
        boot_id: Uuid::now_v7(),
        agent_id: agent,
        generation,
        harness_id: None,
    };
    let parts = aurora_api::domains::hosted::home::render(&h.state, &mut conn, &claim)
        .await
        .unwrap();
    let root = Scratch::new();
    let mut projector = HomeProjector::new(HomeConfig {
        root: root.0.clone(),
        skills_dir: Some(aurora_api::domains::hosted::home::skills_dir(None).to_owned()),
        instruction_files: vec![],
        pg: None,
        agent_cli: false,
    });
    for part in &parts {
        projector.apply_part(part).unwrap();
    }
    projector.commit(generation as _).unwrap();
    root
}

/// FIX-012: titles and descriptions with escapes survive render -> harvest unchanged, and a second
/// round trip (the harvested values stored, rendered and harvested again) does not grow them.
#[tokio::test]
async fn escaped_titles_and_descriptions_survive_two_round_trips() {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, effort, brief) VALUES ($1,$2,$3,'ada','Ada','hosted','fake','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    // (key, title, description). The empty string harvests as "no value" (the harvester has always
    // skipped empty scalars), and must stay that way rather than turning into `""` text.
    let cases: [(&str, &str, &str); 8] = [
        ("dq", r#"Cafe "menu""#, r#"say "hi""#),
        ("bs", r"C:\temp\new", r"a\\b \n literal"),
        ("nl", "two\nlines", "line1\nline2\r\nline3"),
        ("tab", "a\tb", "\ttabbed\t"),
        ("uni", "Café 日本語 😀", "ü ñ \u{2028} é"),
        ("sq", "it's 'quoted'", "''"),
        ("empty", "", ""),
        ("mix", "\"\\'\n\t é", "all: of #them \"\\ ' \u{1} end"),
    ];
    for (key, t, d) in cases {
        insert_memory(&h, agent, key, Some(t), Some(d), "body\n").await;
    }
    let expect = |s: &str| (!s.is_empty()).then(|| s.to_owned());

    for round in 1..=2_i64 {
        let root = render_install(&h, agent, round).await;
        let mut harvester = Harvester::new(&root.0);
        assert!(
            harvester.scan(&Default::default()).is_empty(),
            "round {round}: clean projection"
        );
        for (key, ..) in cases {
            let file = read(&root.0, key);
            write(&root.0, key, &format!("{file}edit {round}\n"));
        }
        let writes = harvester.scan(&Default::default());
        assert_eq!(writes.len(), cases.len(), "round {round}: {writes:?}");
        for (key, t, d) in cases {
            let w = writes.iter().find(|w| w.key == key).unwrap();
            assert_eq!(w.title, expect(t), "round {round}: {key} title");
            assert_eq!(w.description, expect(d), "round {round}: {key} description");
            assert!(w.content.contains(&format!("edit {round}")), "{w:?}");
            // Store what the harvest produced (as the API would); an absent value keeps the old one.
            sqlx::query(
                "UPDATE memories SET title = COALESCE($2, title), description = COALESCE($3, description) \
                 WHERE agent_id = $1 AND key = $4",
            )
            .bind(agent)
            .bind(w.title.as_deref())
            .bind(w.description.as_deref())
            .bind(key)
            .execute(h.pool())
            .await
            .unwrap();
        }
    }
}

/// (P6-MEMLOCK, C-1149) A locked memory renders with `locked: true` on the entry and in the front
/// matter, an index line whose key is followed by ` (locked)`, installs 0444 and is never harvested: an edited,
/// replaced or deleted file is restored from the recorded bytes, while an unlocked memory behaves
/// exactly as before.
#[tokio::test]
async fn locked_memories_install_read_only_and_are_restored_not_uploaded() {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, effort, brief) VALUES ($1,$2,$3,'ada','Ada','hosted','fake','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    insert_memory(&h, agent, "free", None, None, "agent may edit this").await;
    insert_memory(
        &h,
        agent,
        "pinned",
        Some("Rules"),
        None,
        "the agent cannot change this",
    )
    .await;
    sqlx::query("UPDATE memories SET locked = true, locked_at = now() WHERE agent_id = $1 AND key = 'pinned'")
        .bind(agent)
        .execute(h.pool())
        .await
        .unwrap();

    let mut conn = h.pool().acquire().await.unwrap();
    let claim = HomeSyncClaim {
        sandbox_id: Uuid::now_v7(),
        boot_id: Uuid::now_v7(),
        agent_id: agent,
        generation: 1,
        harness_id: None,
    };
    let parts = aurora_api::domains::hosted::home::render(&h.state, &mut conn, &claim)
        .await
        .unwrap();
    let entries: Vec<Value> = parts
        .iter()
        .filter(|p| p.part.kind.as_str() == "memory")
        .flat_map(|p| p.part.entries.clone())
        .collect();
    let entry_of = |path: &str| {
        entries
            .iter()
            .find(|e| e["path"] == path)
            .unwrap_or_else(|| panic!("{path} not rendered: {entries:?}"))
    };
    // The locked entry carries `locked: true`; the unlocked one is unchanged (no key at all).
    assert_eq!(entry_of("memory/pinned.md")["locked"], true, "{entries:?}");
    assert!(entry_of("memory/free.md").get("locked").is_none());
    // The file's front matter ends with `locked: true`; the index marks the key.
    let pinned = entry_of("memory/pinned.md")["content"].as_str().unwrap();
    assert!(
        pinned.starts_with("---\ntitle: \"Rules\"\nlocked: true\n---\n"),
        "{pinned}"
    );
    let index = entry_of("memory/index.md")["content"].as_str().unwrap();
    assert!(index.contains("pinned (locked): Rules\n"), "{index}");
    assert!(index.contains("free: agent may edit this\n"), "{index}");
    // The recall index agrees with `memory/index.md`.
    let mems = aurora_db::repo::hosted::memories(&mut conn, agent)
        .await
        .unwrap();
    let idx = aurora_api::domains::hosted::home::memory_index(&mems);
    assert_eq!(idx, index);
    // `render_inputs.memory_locked` is set with at least one locked memory.
    let ri = parts
        .iter()
        .find(|p| p.part.kind.as_str() == "render_inputs")
        .unwrap();
    assert_eq!(ri.part.entries[0]["memory_locked"], Value::Bool(true));

    // Install with the real projector: the locked file is 0444, the unlocked one 0644.
    let root = Scratch::new();
    let mut projector = HomeProjector::new(HomeConfig {
        root: root.0.clone(),
        skills_dir: Some(aurora_api::domains::hosted::home::skills_dir(None).to_owned()),
        instruction_files: vec![],
        pg: None,
        agent_cli: false,
    });
    for part in &parts {
        projector.apply_part(part).unwrap();
    }
    projector.commit(1).unwrap();
    let mode_of = |key: &str| {
        let p = root.0.join("memory").join(format!("{key}.md"));
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&p).unwrap().permissions())
            & 0o777
    };
    assert_eq!(mode_of("pinned"), 0o444, "locked installs read-only");
    assert_eq!(mode_of("free"), 0o644);
    let recorded = projector.locked_memories();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded.get("pinned").unwrap(),
        &std::fs::read(root.0.join("memory/pinned.md")).unwrap()
    );

    // Harvest with the projector's map: a clean projection uploads nothing.
    let mut harvester = Harvester::new(&root.0);
    assert!(harvester.scan(&recorded).is_empty());
    assert!(harvester.take_notices().is_empty());

    // The agent edits the locked file: never uploaded, restored from the recorded bytes, one notice.
    let original = std::fs::read(root.0.join("memory/pinned.md")).unwrap();
    write(&root.0, "pinned", "rewritten by the agent\n");
    assert!(
        harvester.scan(&recorded).is_empty(),
        "a locked memory is never uploaded"
    );
    assert_eq!(
        std::fs::read(root.0.join("memory/pinned.md")).unwrap(),
        original
    );
    let notices = harvester.take_notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains("memory `pinned` is locked: a person set it")
            && notices[0].contains("Your change was undone."),
        "{notices:?}"
    );
    // The restored file is read-only again.
    assert_eq!(mode_of("pinned"), 0o444);
    // The same edit restored again does not repeat the notice (deduped by digest).
    write(&root.0, "pinned", "rewritten by the agent\n");
    let _ = harvester.scan(&recorded);
    assert!(harvester.take_notices().is_empty());

    // A chmod-ed locked file is set back to 0444 with no notice.
    let mut perm = std::fs::metadata(root.0.join("memory/pinned.md"))
        .unwrap()
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o644);
    std::fs::set_permissions(root.0.join("memory/pinned.md"), perm).unwrap();
    assert!(harvester.scan(&recorded).is_empty());
    assert_eq!(mode_of("pinned"), 0o444);
    assert!(harvester.take_notices().is_empty());

    // A deleted locked file is restored too.
    std::fs::remove_file(root.0.join("memory/pinned.md")).unwrap();
    assert!(harvester.scan(&recorded).is_empty());
    assert_eq!(
        std::fs::read(root.0.join("memory/pinned.md")).unwrap(),
        original
    );
    let notices = harvester.take_notices();
    assert_eq!(notices.len(), 1, "{notices:?}");

    // The unlocked memory is harvested exactly as before.
    let file = read(&root.0, "free");
    write(&root.0, "free", &format!("{file}agent added this\n"));
    let writes = harvester.scan(&recorded);
    assert_eq!(keys(&writes), ["free"], "{writes:?}");
    assert!(writes[0].content.contains("agent added this"), "{writes:?}");

    // Unlocking: the next generation installs the file 0644 again and the harvest uploads edits.
    sqlx::query("UPDATE memories SET locked = false, locked_at = NULL, locked_by = NULL WHERE agent_id = $1 AND key = 'pinned'")
        .bind(agent)
        .execute(h.pool())
        .await
        .unwrap();
    let mut conn = h.pool().acquire().await.unwrap();
    let claim = HomeSyncClaim {
        sandbox_id: Uuid::now_v7(),
        boot_id: Uuid::now_v7(),
        agent_id: agent,
        generation: 2,
        harness_id: None,
    };
    let parts = aurora_api::domains::hosted::home::render(&h.state, &mut conn, &claim)
        .await
        .unwrap();
    for part in &parts {
        projector.apply_part(part).unwrap();
    }
    projector.commit(2).unwrap();
    assert!(projector.locked_memories().is_empty());
    assert_eq!(mode_of("pinned"), 0o644);
    let mut harvester = Harvester::new(&root.0);
    let file = read(&root.0, "pinned");
    write(&root.0, "pinned", &format!("{file}edit after unlock\n"));
    let writes = harvester.scan(&projector.locked_memories());
    assert_eq!(keys(&writes), ["pinned"], "{writes:?}");
}
