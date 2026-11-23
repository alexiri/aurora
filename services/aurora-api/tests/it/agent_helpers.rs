#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The helper files on an agent (P6-TPL-6, C-1143): the writer (`replace_all`/`delete` with the
//! C-1141 path rules and the per-agent caps), the list/read/remove routes with the C-903
//! headers, the audit row and the home-sync generation bumps.

use crate::common::*;
use aurora_api::domains::agent_helpers::service::{self, HelperFile};
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
    let (s, _) = w
        .get(&format!("/v1/agents/{a}/helpers/content"), &w.tok)
        .await;
    assert_eq!(s, 422);

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
