#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The helper files on an agent (P6-TPL-6, C-1143): the writer (`replace_all`/`delete` with the
//! C-1141 path rules and the per-agent caps), the list/read/remove routes with the C-903
//! headers, the audit row and the home-sync generation bumps.

use crate::common::*;
use aurora_api::domains::agent_helpers::service::{self, HelperFile};
use aurora_db::repo::agent_helpers as helpers_repo;
use aurora_db::repo::hosted::HomeSyncClaim;
use aurora_protocol::runner::{HomeSync, HomeSyncKind, MAX_HOME_SYNC_PART};
use axum::body::Body;
use http::{HeaderMap, Method, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    tok: String,
    /// A workspace of the same org the owner has no access to (its member's session).
    other_tok: String,
}

async fn world() -> W {
    let h = Harness::new().await;
    let org = h.org().await;
    let o = h.user("o@x.io", false).await;
    let m = h.user("m@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.member(org, m, "member", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, o, "owner").await;
    let other_ws = h.workspace(org).await;
    h.bind_ws(other_ws, m, "owner").await;
    let (_, ot) = h.good_session(o).await;
    let (_, mt) = h.good_session(m).await;
    W {
        h,
        org,
        ws,
        tok: ot,
        other_tok: mt,
    }
}

fn f<'a>(path: &'a str, content: &'a [u8], executable: bool) -> HelperFile<'a> {
    HelperFile {
        path,
        content,
        executable,
    }
}

impl W {
    async fn agent(&self, handle: &str) -> Uuid {
        self.h.hosted_agent(self.org, self.ws, handle).await
    }

    async fn generation(&self, agent: Uuid) -> i64 {
        sqlx::query_scalar("SELECT home_sync_generation FROM agents WHERE id = $1")
            .bind(agent)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    /// A committed `replace_all` through the writer's own transaction type.
    async fn seed(&self, agent: Uuid, files: &[HelperFile<'_>]) {
        let mut tx = self.h.pool().begin().await.unwrap();
        service::replace_all(&mut tx, agent, files).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn helper_count(&self, agent: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM agent_helper_files WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn get(&self, uri: &str, tok: &str) -> (u16, Value) {
        self.h.call_tok(tok, Method::GET, uri, None).await
    }

    async fn delete(&self, uri: &str, tok: &str) -> (u16, Value) {
        self.h.call_tok(tok, Method::DELETE, uri, None).await
    }

    /// The content route's raw bytes and headers (the JSON helper cannot carry binary).
    async fn raw(&self, uri: &str, tok: &str) -> (u16, HeaderMap, Vec<u8>) {
        let req = http::Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("cookie", cookie(tok))
            .header("origin", ORIGIN)
            .body(Body::empty())
            .unwrap();
        let res = self.h.app.clone().oneshot(req).await.unwrap();
        let (parts, body) = res.into_parts();
        (
            parts.status.as_u16(),
            parts.headers,
            body.collect().await.unwrap().to_bytes().to_vec(),
        )
    }

    /// The latest `agent.helpers_remove` row's `paths`, once at least `at_least` rows exist
    /// (the best-effort audit write is fire-and-forget, so poll briefly).
    async fn wait_audit_paths(&self, at_least: i64) -> Value {
        for _ in 0..100 {
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM audit_log WHERE action = 'agent.helpers_remove'",
            )
            .fetch_one(self.h.pool())
            .await
            .unwrap();
            if n >= at_least {
                let v: Value = sqlx::query_scalar::<_, Value>(
                    "SELECT metadata FROM audit_log WHERE action = 'agent.helpers_remove'
                     ORDER BY created_at DESC LIMIT 1",
                )
                .fetch_one(self.h.pool())
                .await
                .unwrap();
                return v["paths"].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("no agent.helpers_remove audit row");
    }
}

#[tokio::test]
async fn replace_all_in_a_rolled_back_tx_leaves_nothing() {
    let w = world().await;
    let a = w.agent("h1").await;
    let before = w.generation(a).await;
    let mut tx = w.h.pool().begin().await.unwrap();
    service::replace_all(
        &mut tx,
        a,
        &[
            f("run.sh", b"#!/bin/sh\necho hi\n", true),
            f("docs/note.md", b"# note\n", false),
        ],
    )
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_helper_files WHERE agent_id = $1")
        .bind(a)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 2, "both files are in the transaction");
    tx.rollback().await.unwrap();
    assert_eq!(w.helper_count(a).await, 0);
    assert_eq!(w.generation(a).await, before);
}

/// The C-1141 path rules come from `aurora_template::helper_path_ok` and the per-agent caps are
/// the writer's own: everything refused happens before any row is written.
#[tokio::test]
async fn writer_enforces_paths_and_caps() {
    let w = world().await;
    let a = w.agent("h1").await;
    for bad in ["../escape", ".hidden", "a//b", "a/b/c/d/e", ""] {
        let mut tx = w.h.pool().begin().await.unwrap();
        let err = service::replace_all(&mut tx, a, &[f(bad, b"x", false)])
            .await
            .unwrap_err();
        tx.rollback().await.unwrap();
        assert_eq!(
            (err.status(), err.code()),
            (StatusCode::UNPROCESSABLE_ENTITY, "template_helper_invalid"),
            "{bad}"
        );
    }
    let big = vec![b'x'; 256 * 1024 + 1];
    let mut tx = w.h.pool().begin().await.unwrap();
    let err = service::replace_all(&mut tx, a, &[f("big.bin", &big, false)])
        .await
        .unwrap_err();
    tx.rollback().await.unwrap();
    assert_eq!(err.code(), "template_helper_invalid", "{err}");
    // At most 50 files.
    let many: Vec<HelperFile<'_>> = (0..51)
        .map(|i| {
            let p: &'static str = Box::leak(format!("f{i}.sh").into_boxed_str());
            f(p, b"x", false)
        })
        .collect();
    let mut tx = w.h.pool().begin().await.unwrap();
    let err = service::replace_all(&mut tx, a, &many).await.unwrap_err();
    tx.rollback().await.unwrap();
    assert_eq!(err.code(), "helpers_limit", "{err}");
    // At most 2 MiB together (9 × 256 KiB is over).
    let chunk = vec![b'x'; 256 * 1024];
    let total: Vec<HelperFile<'_>> = (0..9)
        .map(|i| {
            let p: &'static str = Box::leak(format!("big{i}.bin").into_boxed_str());
            f(p, &chunk, false)
        })
        .collect();
    let mut tx = w.h.pool().begin().await.unwrap();
    let err = service::replace_all(&mut tx, a, &total).await.unwrap_err();
    tx.rollback().await.unwrap();
    assert_eq!(err.code(), "helpers_limit", "{err}");
    // The same path twice.
    let mut tx = w.h.pool().begin().await.unwrap();
    let err = service::replace_all(
        &mut tx,
        a,
        &[f("x.sh", b"a", false), f("x.sh", b"b", false)],
    )
    .await
    .unwrap_err();
    tx.rollback().await.unwrap();
    assert_eq!(err.code(), "template_helper_invalid", "{err}");
    assert_eq!(w.helper_count(a).await, 0);
}

#[tokio::test]
async fn list_read_and_delete() {
    let w = world().await;
    let a = w.agent("h1").await;
    let before = w.generation(a).await;
    let run = b"#!/bin/sh\necho hi\n";
    let note = b"# note\n";
    w.seed(a, &[f("docs/note.md", note, false), f("run.sh", run, true)])
        .await;
    assert!(
        w.generation(a).await > before,
        "a replace bumps the generation"
    );

    let (s, v) = w.get(&format!("/v1/agents/{a}/helpers"), &w.tok).await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{v}");
    assert_eq!(items[0]["path"], "docs/note.md");
    assert_eq!(items[1]["path"], "run.sh");
    assert_eq!(items[1]["size"], run.len() as i64);
    assert_eq!(items[1]["executable"], true);
    assert_eq!(items[0]["executable"], false);
    assert_eq!(items[1]["sha256"], hex::encode(Sha256::digest(run)), "{v}");

    // The content route: C-903 headers plus the executable bit, the exact bytes.
    let (s, h, body) = w
        .raw(
            &format!("/v1/agents/{a}/helpers/content?path=run.sh"),
            &w.tok,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(h["content-type"], "application/octet-stream");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["cache-control"], "no-store");
    assert_eq!(h["content-security-policy"], "sandbox; default-src 'none'");
    assert_eq!(
        h["content-disposition"],
        "attachment; filename=\"run.sh\"; filename*=UTF-8''run.sh"
    );
    assert_eq!(h["x-helper-executable"], "true");
    assert_eq!(h["content-length"], run.len().to_string());
    assert_eq!(body, run);
    let (_, h, _) = w
        .raw(
            &format!("/v1/agents/{a}/helpers/content?path=docs%2Fnote.md"),
            &w.tok,
        )
        .await;
    assert_eq!(h["x-helper-executable"], "false");

    let (s, v) = w
        .get(
            &format!("/v1/agents/{a}/helpers/content?path=nope.sh"),
            &w.tok,
        )
        .await;
    assert_eq!((s, v["error"]["code"].as_str()), (404, Some("not_found")));
    let (s, v) = w
        .get(&format!("/v1/agents/{a}/helpers/content"), &w.tok)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_request")),
        "a missing ?path= is the standard 400, not a 422"
    );

    // Remove one: 204, one file left, generation bumped, audit row with the path.
    let before = w.generation(a).await;
    let (s, v) = w
        .delete(&format!("/v1/agents/{a}/helpers?path=run.sh"), &w.tok)
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(w.helper_count(a).await, 1);
    assert!(w.generation(a).await > before);
    assert_eq!(w.wait_audit_paths(1).await, json!(["run.sh"]));

    // Remove all: 204, none left, generation bumped again, audit row with both paths.
    let before = w.generation(a).await;
    let (s, v) = w.delete(&format!("/v1/agents/{a}/helpers"), &w.tok).await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(w.helper_count(a).await, 0);
    assert!(w.generation(a).await > before);
    assert_eq!(w.wait_audit_paths(2).await, json!(["docs/note.md"]));

    // Removing from an empty set changes nothing: no generation bump.
    let before = w.generation(a).await;
    let (s, _) = w.delete(&format!("/v1/agents/{a}/helpers"), &w.tok).await;
    assert_eq!(s, 204);
    assert_eq!(w.generation(a).await, before);
}

/// A delete that removes nothing (an unknown path, a repeat call, an empty set) still answers
/// 204 but writes no `agent.helpers_remove` audit row and bumps no generation: only a real
/// removal is audited.
#[tokio::test]
async fn a_delete_that_removes_nothing_is_not_audited() {
    let w = world().await;
    let a = w.agent("h1").await;
    w.seed(a, &[f("run.sh", b"x", false)]).await;
    let before = w.generation(a).await;

    // An unknown path (twice) and later an empty set: 204 every time, nothing changed.
    for _ in 0..2 {
        let (s, _) = w
            .delete(&format!("/v1/agents/{a}/helpers?path=nope.sh"), &w.tok)
            .await;
        assert_eq!(s, 204);
        assert_eq!(
            w.helper_count(a).await,
            1,
            "an unknown path removes nothing"
        );
    }
    assert_eq!(
        w.generation(a).await,
        before,
        "no removal, no generation bump"
    );

    // A real removal is still audited with the removed path.
    let (s, _) = w
        .delete(&format!("/v1/agents/{a}/helpers?path=run.sh"), &w.tok)
        .await;
    assert_eq!(s, 204);
    assert_eq!(w.wait_audit_paths(1).await, json!(["run.sh"]));

    // Now the set is empty: a repeat delete answers 204, changes nothing and stays unaudited.
    let before = w.generation(a).await;
    let (s, _) = w.delete(&format!("/v1/agents/{a}/helpers"), &w.tok).await;
    assert_eq!(s, 204);
    assert_eq!(w.generation(a).await, before);
    // The audit write is fire-and-forget, so give a wrongly spawned row time to land.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'agent.helpers_remove'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 1, "the no-removal deletes wrote no audit row");
}

/// `can_use` of the agent's workspace: a member of another workspace of the same organization
/// gets 404 everywhere (ids are not confirmed).
#[tokio::test]
async fn another_workspaces_member_gets_404() {
    let w = world().await;
    let a = w.agent("h1").await;
    w.seed(a, &[f("run.sh", b"x", false)]).await;
    let base = format!("/v1/agents/{a}");
    let (s, _) = w.get(&format!("{base}/helpers"), &w.other_tok).await;
    assert_eq!(s, 404);
    let (s, _) = w
        .get(&format!("{base}/helpers/content?path=run.sh"), &w.other_tok)
        .await;
    assert_eq!(s, 404);
    let (s, _) = w
        .delete(&format!("{base}/helpers?path=run.sh"), &w.other_tok)
        .await;
    assert_eq!(s, 404);
    assert_eq!(w.helper_count(a).await, 1);
}

// ----------------------------------------------------------- the `helpers` home-sync part (C-1143)

impl W {
    /// The real home-sync renderer's parts for `agent`.
    async fn render_parts(&self, agent: Uuid) -> Vec<HomeSync> {
        let mut conn = self.h.pool().acquire().await.unwrap();
        let claim = HomeSyncClaim {
            sandbox_id: Uuid::now_v7(),
            boot_id: Uuid::now_v7(),
            agent_id: agent,
            generation: 7,
            harness_id: None,
        };
        aurora_api::domains::hosted::home::render(&self.h.state, &mut conn, &claim)
            .await
            .unwrap()
    }

    /// Insert one helper row directly (bypassing the writer's path rules), for the projection
    /// cases the writer refuses or skips.
    async fn seed_raw(&self, agent: Uuid, path: &str, content: &[u8], executable: bool) {
        let mut conn = self.h.pool().acquire().await.unwrap();
        helpers_repo::insert(
            &mut conn,
            agent,
            path,
            content,
            executable,
            &hex::encode(Sha256::digest(content)),
        )
        .await
        .unwrap();
    }
}

fn helpers_parts(parts: &[HomeSync]) -> Vec<&HomeSync> {
    parts
        .iter()
        .filter(|p| p.part.kind == HomeSyncKind::Helpers)
        .collect()
}

fn render_inputs_of(parts: &[HomeSync]) -> Value {
    parts
        .iter()
        .find(|p| p.part.kind == HomeSyncKind::RenderInputs)
        .unwrap()
        .part
        .entries[0]
        .clone()
}

/// The `helpers` part carries the agent's helper files at `.aurora/helpers/<path>` in the skills
/// file-entry shape, the manifest lists the paths, `render_inputs.helpers` names the dir and the
/// count, and the part sits between `apps` and `previews`. An agent without helpers has no part
/// and no fragment.
#[tokio::test]
async fn helpers_part_entries_manifest_and_render_inputs() {
    let w = world().await;
    let a = w.agent("h1").await;

    // No helpers: no part, no fragment.
    let parts = w.render_parts(a).await;
    assert!(helpers_parts(&parts).is_empty());
    assert!(render_inputs_of(&parts).get("helpers").is_none());

    w.seed(
        a,
        &[
            f("docs/readme.md", b"# helpers\n", false),
            f("run.sh", b"#!/bin/sh\necho hi\n", true),
        ],
    )
    .await;
    let parts = w.render_parts(a).await;
    let hp = helpers_parts(&parts);
    assert_eq!(hp.len(), 1, "small sets ride in one part");
    let entries = &hp[0].part.entries;
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert_eq!(
        entries[0],
        json!({
            "path": ".aurora/helpers/docs/readme.md",
            "content": "# helpers\n",
            "exec": false,
        })
    );
    assert_eq!(
        entries[1],
        json!({
            "path": ".aurora/helpers/run.sh",
            "content": "#!/bin/sh\necho hi\n",
            "exec": true,
        })
    );
    // The manifest lists both paths and only the manifest's paths survive the commit.
    let manifest = &parts
        .iter()
        .find(|p| p.part.kind == HomeSyncKind::Manifest)
        .unwrap()
        .part
        .entries;
    assert!(
        manifest.contains(&json!({"path": ".aurora/helpers/run.sh"})),
        "{manifest:?}"
    );
    assert!(
        manifest.contains(&json!({"path": ".aurora/helpers/docs/readme.md"})),
        "{manifest:?}"
    );
    // `render_inputs.helpers` names the dir and the projected count.
    assert_eq!(
        render_inputs_of(&parts)["helpers"],
        json!({"dir": ".aurora/helpers", "count": 2})
    );
    // Between `render_inputs` and `manifest` (apps may be absent; previews, when present, come
    // after; the manifest and final parts are last).
    let idx = |k: HomeSyncKind| parts.iter().position(|p| p.part.kind == k).unwrap();
    assert!(idx(HomeSyncKind::RenderInputs) < idx(HomeSyncKind::Helpers));
    assert!(idx(HomeSyncKind::Helpers) < idx(HomeSyncKind::Manifest));
}

/// Parts split under `HOME_SYNC_PART_MAX` as the other kinds do, every entry still delivered,
/// and a non-UTF-8 helper is skipped with a warning (content travels as UTF-8 text exactly as
/// skills carry it) — it stays listed and readable through the API.
#[tokio::test]
async fn helpers_split_across_parts_and_non_utf8_is_skipped() {
    let w = world().await;
    let a = w.agent("h1").await;
    // Four ~80 KiB files: over one part, each entry well under it.
    let chunk = vec![b'a'; 80 * 1024];
    w.seed(
        a,
        &[
            f("a1.bin", &chunk, false),
            f("a2.bin", &chunk, false),
            f("a3.bin", &chunk, false),
            f("a4.bin", &chunk, false),
        ],
    )
    .await;
    // A binary helper: stored, listed, downloadable — but not projected.
    w.seed_raw(a, "blob.bin", &[0xff, 0x00, 0xfe, 0x01], false)
        .await;

    let parts = w.render_parts(a).await;
    let hp = helpers_parts(&parts);
    assert!(hp.len() > 1, "the set must split, got {}", hp.len());
    let mut seen: Vec<Value> = Vec::new();
    for p in &hp {
        assert!(p.part.entries.len() <= MAX_HOME_SYNC_PART);
        seen.extend(p.part.entries.iter().cloned());
    }
    assert_eq!(seen.len(), 4, "{seen:?}");
    for (i, e) in seen.iter().enumerate() {
        assert_eq!(e["path"], format!(".aurora/helpers/a{}.bin", i + 1));
        assert_eq!(e["exec"], false);
    }
    assert_eq!(
        render_inputs_of(&parts)["helpers"]["count"],
        json!(4),
        "the count is the projected one"
    );
}
