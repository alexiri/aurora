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
//! Location: `aurora-api` already depends on `aurora-supervisor` (and has it as a dev-dependency),
//! so the test lives here; the supervisor cannot dev-depend on the API without a cycle.

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
        keys(&harvester.scan()).is_empty(),
        "an unmodified projection was harvested as an edit"
    );
    assert!(harvester.scan().is_empty());
    assert!(harvester.take_notices().is_empty());

    // Tampering 1: the agent appends a line. Only that file is harvested; the content has the edit.
    let original = read(&root.0, "plain");
    write(&root.0, "plain", &format!("{original}agent added this\n"));
    let writes = harvester.scan();
    assert_eq!(keys(&writes), ["plain"], "{writes:?}");
    assert!(writes[0].content.contains("agent added this"), "{writes:?}");
    // The same edit is not re-sent on the next tick.
    assert!(harvester.scan().is_empty());

    // Tampering 2: the body changes but the old trailer is kept (a replayed checksum).
    let original = read(&root.0, "with-front-matter");
    write(
        &root.0,
        "with-front-matter",
        &original.replace("line one", "line ONE"),
    );
    let writes = harvester.scan();
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
    assert_eq!(keys(&harvester.scan()), ["quotes-marker"]);

    // Tampering 4: the trailer is removed altogether (the file is then an agent-authored file).
    write(&root.0, "plain", "rewritten by the agent, no trailer\n");
    assert!(!verify(&read(&root.0, "plain")));
    let writes = harvester.scan();
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
    assert!(fresh.scan().is_empty(), "a re-rendered projection is clean");
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
            harvester.scan().is_empty(),
            "round {round}: clean projection"
        );
        for (key, ..) in cases {
            let file = read(&root.0, key);
            write(&root.0, key, &format!("{file}edit {round}\n"));
        }
        let writes = harvester.scan();
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
