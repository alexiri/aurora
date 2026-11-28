//! FIX-292 part 1: a skill's binary files reach the agent's home, byte for byte and with their
//! executable bit. The API renders the sync and the supervisor's real `HomeProjector` installs
//! it (`aurora-supervisor` is a dev-dependency only). A binary file that cannot be carried in
//! one sync entry is refused when written.

use super::*;
use aurora_supervisor::home::{HomeConfig, HomeProjector, PartOutcome};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

/// The largest binary file whose base64 form, as a JSON string, fits one sync entry
/// (`SKILL_ENTRY_JSON_MAX`).
const BINARY_MAX: usize = 145_917;

/// A scratch home removed on drop; the projection leaves read-only folders behind.
struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("aurora-skill-binary-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        fn open(p: &Path) {
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    open(&e.path());
                }
            }
        }
        open(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A PDF's header with its customary binary comment, then every byte value, NUL included.
fn pdf() -> Vec<u8> {
    let mut v = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    v.extend((0..=255_u8).cycle().take(5_000));
    v.extend_from_slice(b"\n%%EOF\n");
    assert!(std::str::from_utf8(&v).is_err());
    v
}

#[tokio::test]
async fn fix292_binary_skill_files_reach_the_agents_home_with_their_bytes_and_mode() {
    let r = rig().await;
    let b = r.booted("binary").await;
    let (s, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{}/skills", b.agent),
            Some(json!({"name": "themes", "description": "d", "body": "See showcase.pdf."})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let uri = format!("/v1/skills/{}", v["id"].as_str().unwrap());
    let largest = vec![0xff_u8; BINARY_MAX];
    let files: [(&str, Vec<u8>, bool); 3] = [
        ("showcase.pdf", pdf(), false),
        ("bin/tool", b"\x7fELF\x02\x01\x00\xff\xfe".to_vec(), true),
        ("assets/largest.bin", largest, false),
    ];
    for (path, bytes, exec) in &files {
        let (s, v) = r
            .raw_put(
                &r.cookie,
                &format!("{uri}/files/{path}?executable={exec}"),
                bytes,
            )
            .await;
        assert_eq!(s, 200, "{path}: {v}");
    }

    let claim = aurora_db::repo::hosted::HomeSyncClaim {
        sandbox_id: b.sandbox,
        boot_id: b.boot,
        agent_id: b.agent,
        generation: 1,
        harness_id: Some("fake".into()),
    };
    let mut conn = r.h.pool().acquire().await.unwrap();
    let parts = aurora_api::domains::hosted::home::render(&r.h.state, &mut conn, &claim)
        .await
        .unwrap();
    for p in &parts {
        let wire = serde_json::to_string(p).unwrap();
        assert!(wire.len() <= aurora_protocol::runner::MAX_HOME_SYNC_PART);
    }

    let home = Home::new();
    let mut projector = HomeProjector::new(HomeConfig {
        root: home.0.clone(),
        skills_dir: Some(".claude/skills".into()),
        instruction_files: vec!["AGENTS.md".into()],
        pg: None,
        agent_cli: false,
    });
    let mut outcome = PartOutcome::Staged;
    for p in &parts {
        outcome = projector.apply_part(p).unwrap();
    }
    assert_eq!(outcome, PartOutcome::ReadyToCommit(1));
    projector.commit(1).unwrap();

    for (path, bytes, exec) in &files {
        let installed = home.0.join(".claude/skills/themes").join(path);
        let got = std::fs::read(&installed).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(got.len(), bytes.len(), "{path}");
        assert!(got == *bytes, "{path}: the bytes differ");
        let mode = std::fs::metadata(&installed).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, if *exec { 0o555 } else { 0o444 }, "{path}");
    }
}

#[tokio::test]
async fn fix292_a_binary_file_too_big_to_carry_is_refused() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let (s, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{a}/skills"),
            Some(json!({"name": "heavy", "description": "d", "body": "b"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let (s, v) = r
        .raw_put(
            &r.cookie,
            &format!("/v1/skills/{id}/files/blob.bin"),
            &vec![0xff_u8; BINARY_MAX + 1],
        )
        .await;
    assert_eq!((s, code(&v)), (422, Some("file_too_large")), "{v}");
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("blob.bin") && msg.contains("base64"), "{v}");
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM skill_files WHERE skill_id = $1::uuid")
            .bind(&id)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(stored, 0, "nothing was kept");
}
