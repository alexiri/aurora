#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-060: memory API, revisions, search, recall, tools and harvest.

use crate::runner_rig::{Rig, rig};
use aurora_api::domains::memory;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

impl Rig {
    async fn put(&self, agent: Uuid, key: &str, content: &str) -> (u16, Value) {
        self.user(
            Method::PUT,
            &format!("/v1/agents/{agent}/memories/{key}"),
            Some(json!({"content": content})),
        )
        .await
    }

    async fn set_lang(&self, lang: &str) {
        sqlx::query("UPDATE workspaces SET memory_language = $2 WHERE id = $1")
            .bind(self.ws)
            .bind(lang)
            .execute(self.h.pool())
            .await
            .unwrap();
    }

    async fn search(&self, agent: Uuid, q: &str) -> Vec<Value> {
        let (s, v) = self
            .user(
                Method::GET,
                &format!(
                    "/v1/agents/{agent}/memories/search?q={}",
                    q.replace(' ', "%20")
                ),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"].as_array().unwrap().clone()
    }
}

#[tokio::test]
async fn crud_and_limits() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let base = format!("/v1/agents/{a}/memories");
    let (s, v) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"key": "house-rules", "title": "Rules", "content": "No shoes"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["revision"], 1);
    assert_eq!(v["index_line"], "house-rules: Rules");
    let (s, v) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"key": "house-rules", "content": "x"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("memory_exists"))
    );
    for bad in ["Bad_Key", "a--b", "index", "search", ""] {
        let (s, _) = r
            .user(
                Method::POST,
                &base,
                Some(json!({"key": bad, "content": "x"})),
            )
            .await;
        assert_eq!(s, 400, "{bad}");
    }
    let (s, _) = r.put(a, "big", &"x".repeat(100_001)).await;
    assert_eq!(s, 400);
    // S-13: within the character limit but too large to sync once encoded (4-byte characters).
    let (s, v) = r.put(a, "emoji", &"\u{1F600}".repeat(100_000)).await;
    assert_eq!(s, 400, "{}", v["error"]);
    // 60 000 three-byte characters (~176 KiB encoded) still fit.
    let (s, v) = r.put(a, "cjk", &"\u{6F22}".repeat(60_000)).await;
    assert!(matches!(s, 200 | 201), "{s} {}", v["error"]);
    let (s, _) = r.user(Method::DELETE, &format!("{base}/cjk"), None).await;
    assert!(matches!(s, 200 | 204), "{s}");
    let (s, v) = r.put(a, "house-rules", "Shoes off").await;
    assert_eq!((s, v["revision"].clone()), (200, json!(2)));
    let (s, v) = r
        .user(Method::GET, &format!("{base}/house-rules"), None)
        .await;
    assert_eq!((s, v["content"].as_str()), (200, Some("Shoes off")));
    let (_, v) = r.user(Method::GET, &base, None).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);

    // MEMORY_MAX_PER_AGENT
    for i in 0..99 {
        sqlx::query(
            "INSERT INTO memories (id, agent_id, key, content, revision) VALUES ($1,$2,$3,'c',1)",
        )
        .bind(Uuid::now_v7())
        .bind(a)
        .bind(format!("k{i}"))
        .execute(r.h.pool())
        .await
        .unwrap();
    }
    let (s, v) = r.put(a, "one-too-many", "x").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("memory_limit_reached"))
    );
    // Replacing an existing one at the limit still works.
    assert_eq!(r.put(a, "house-rules", "again").await.0, 200);

    let (s, _) = r
        .user(Method::DELETE, &format!("{base}/house-rules"), None)
        .await;
    assert_eq!(s, 204);
    let (s, _) = r
        .user(Method::GET, &format!("{base}/house-rules"), None)
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn revisions_retention_redact_restore() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let base = format!("/v1/agents/{a}/memories/note");
    for i in 1..=51 {
        assert_eq!(r.put(a, "note", &format!("v{i}")).await.0, 200);
    }
    let (s, v) = r
        .user(Method::GET, &format!("{base}/revisions"), None)
        .await;
    assert_eq!(s, 200, "{v}");
    let revs = v["items"].as_array().unwrap();
    assert_eq!(revs.len(), 50);
    assert_eq!(revs[0]["revision"], 51);
    assert_eq!(revs[49]["revision"], 2);
    assert_eq!(revs[0]["provenance"], "dashboard");
    assert_eq!(revs[0]["author_kind"], "user");

    let (s, v) = r
        .user(Method::POST, &format!("{base}/revisions/51/redact"), None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("cannot_redact_latest"))
    );
    let (s, _) = r
        .user(Method::POST, &format!("{base}/revisions/3/redact"), None)
        .await;
    assert_eq!(s, 204);
    let (_, v) = r
        .user(Method::GET, &format!("{base}/revisions"), None)
        .await;
    let r3 = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["revision"] == 3)
        .unwrap();
    assert_eq!(
        (r3["redacted"].clone(), r3["content"].clone()),
        (json!(true), Value::Null)
    );
    let (s, v) = r
        .user(
            Method::POST,
            &format!("{base}/restore"),
            Some(json!({"revision": 3})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("revision_redacted"))
    );
    let (s, v) = r
        .user(
            Method::POST,
            &format!("{base}/restore"),
            Some(json!({"revision": 4})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["revision"].clone(), v["content"].as_str()),
        (json!(52), Some("v4"))
    );
    let (_, v) = r
        .user(Method::GET, &format!("{base}/revisions"), None)
        .await;
    assert_eq!(v["items"][0]["provenance"], "restore");
    let (s, _) = r
        .user(Method::POST, &format!("{base}/revisions/999/redact"), None)
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn search_ranking_and_languages() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let base = format!("/v1/agents/{a}/memories");
    // english: stemming; key/title weigh more than content.
    for (k, t, c) in [
        ("running-dogs", None, "walk the dogs daily"),
        ("misc", Some("Jogging"), "we were running in the park"),
        ("other", None, "nothing relevant at all"),
    ] {
        let (s, v) = r
            .user(
                Method::POST,
                &base,
                Some(json!({"key": k, "title": t, "content": c})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
    }
    let hits = r.search(a, "run").await;
    let keys: Vec<&str> = hits.iter().map(|h| h["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["running-dogs", "misc"], "{hits:?}");
    assert!(hits[0]["score"].as_f64().unwrap() > hits[1]["score"].as_f64().unwrap());
    assert!(hits.iter().all(|h| h["score"].as_f64().unwrap() <= 1.0));

    // 0.5 floor: substring of the key is not a lexeme.
    let hits = r.search(a, "ogs").await;
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!((hits[0]["score"].as_f64().unwrap() - 0.5).abs() < 1e-6);
    assert_eq!(r.search(a, "zzzz").await.len(), 0);

    // simple: no stemming.
    r.set_lang("simple").await;
    let hits = r.search(a, "walks").await;
    assert!(hits.is_empty(), "{hits:?}");
    r.set_lang("english").await;
    assert_eq!(r.search(a, "walks").await.len(), 1);

    // german: Häuser <-> Haus.
    let b = r.agent("b1").await;
    r.set_lang("german").await;
    let (s, _) = r
        .user(
            Method::PUT,
            &format!("/v1/agents/{b}/memories/wohnung"),
            Some(json!({"content": "Die Häuser sind groß"})),
        )
        .await;
    assert_eq!(s, 200);
    let hits = r.search(b, "Haus").await;
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0]["key"], "wohnung");
    assert!(hits[0]["snippet"].as_str().unwrap().contains("Häuser"));
    // The same query under english finds nothing (stemming is query-time).
    r.set_lang("english").await;
    assert!(r.search(b, "Haus").await.is_empty());

    // Empty query rejected; hits capped at MEMORY_SEARCH_HITS.
    let (s, _) = r
        .user(Method::GET, &format!("{base}/search?q=%20"), None)
        .await;
    assert_eq!(s, 400);
    let c = r.agent("c1").await;
    for i in 0..12 {
        assert_eq!(r.put(c, &format!("alpha-{i}"), "text").await.0, 200);
    }
    assert_eq!(r.search(c, "alpha").await.len(), 8);
}

#[tokio::test]
async fn recall_composition() {
    let r = rig().await;
    let a = r.agent("a1").await;
    for (k, c) in [
        ("cats", "the cat sleeps on the sofa"),
        ("cars", "cars need oil"),
        ("tax", "file taxes in april"),
        ("cat-food", "cat food is in the pantry"),
        ("cat-toys", "cat toys"),
        ("cat-bed", "cat bed"),
    ] {
        assert_eq!(r.put(a, k, c).await.0, 200);
    }
    let mut conn = r.h.pool().acquire().await.unwrap();
    let snippets = memory::recall_snippets(&mut conn, a, "cat").await.unwrap();
    assert_eq!(snippets.len(), 3, "{snippets:?}");
    assert!(
        snippets
            .iter()
            .all(|s| s.starts_with("cat") && s.chars().count() <= 800)
    );
    assert!(
        memory::recall_snippets(&mut conn, a, "quantum")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        memory::recall_snippets(&mut conn, a, "  ")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn memory_tools_via_tool_calls() {
    let r = rig().await;
    let b = r.booted("t1").await;
    let v = r
        .tool(
            &b,
            "memory_save",
            json!({"key": "plan", "title": "Plan", "content": "ship on friday"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["content"][0]["value"]["created"], true);
    let v = r.tool(&b, "memory_get", json!({"key": "plan"})).await;
    assert_eq!(v["content"][0]["value"]["content"], "ship on friday");
    let v = r.tool(&b, "memory_list", json!({})).await;
    assert_eq!(
        v["content"][0]["value"]["memories"][0]["index_line"],
        "plan: Plan"
    );
    let v = r
        .tool(&b, "memory_search", json!({"query": "friday"}))
        .await;
    assert_eq!(v["content"][0]["value"]["hits"][0]["key"], "plan");
    // Tool writes are capped lower than dashboard writes.
    let v = r
        .tool(
            &b,
            "memory_save",
            json!({"key": "big", "content": "x".repeat(12_001)}),
        )
        .await;
    assert_eq!(v["ok"], false, "{v}");
    let v = r
        .tool(
            &b,
            "memory_save",
            json!({"key": "plan", "content": "x", "bogus": 1}),
        )
        .await;
    assert_eq!(v["error"]["code"], "invalid_arguments");
    // Revision provenance is `tool`, with the agent as author.
    let (_, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{}/memories/plan/revisions", b.agent),
            None,
        )
        .await;
    assert_eq!(v["items"][0]["provenance"], "tool");
    assert_eq!(v["items"][0]["author_kind"], "agent");
    let v = r.tool(&b, "memory_delete", json!({"key": "plan"})).await;
    assert_eq!(v["ok"], true, "{v}");
    let v = r.tool(&b, "memory_get", json!({"key": "plan"})).await;
    assert_eq!(v["ok"], false);
    // Another agent never sees it.
    let o = r.booted("t2").await;
    r.tool(&o, "memory_save", json!({"key": "mine", "content": "x"}))
        .await;
    let v = r.tool(&b, "memory_get", json!({"key": "mine"})).await;
    assert_eq!(v["ok"], false);
}

#[tokio::test]
async fn harvest_memory_writes() {
    let r = rig().await;
    let b = r.booted("h1").await;
    let body = |key: &str, content: &str| {
        json!({"sandbox_id": b.sandbox, "boot_id": b.boot, "key": key,
               "title": "T", "content": content})
    };
    let (s, v) = r
        .post(
            Some(r.replica),
            "/v1/runner/memory-writes",
            body("notes", "from the sandbox"),
        )
        .await;
    assert_eq!((s, v["revision"].clone()), (200, json!(1)), "{v}");
    // Unchanged re-upload does not create a revision.
    let (_, v) = r
        .post(
            Some(r.replica),
            "/v1/runner/memory-writes",
            body("notes", "from the sandbox"),
        )
        .await;
    assert_eq!(v["revision"], 1);
    let (_, v) = r
        .post(
            Some(r.replica),
            "/v1/runner/memory-writes",
            body("notes", "edited"),
        )
        .await;
    assert_eq!(v["revision"], 2);
    let (_, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{}/memories/notes/revisions", b.agent),
            None,
        )
        .await;
    assert_eq!(v["items"][0]["provenance"], "harvest");
    assert_eq!(v["items"][0]["author_kind"], "agent");
    // Invalid key, stale boot and foreign sandboxes are rejected.
    let (s, _) = r
        .post(
            Some(r.replica),
            "/v1/runner/memory-writes",
            body("Bad Key", "x"),
        )
        .await;
    assert_eq!(s, 400);
    let mut stale = body("notes", "x");
    stale["boot_id"] = json!(Uuid::now_v7());
    let (s, _) = r
        .post(Some(r.replica), "/v1/runner/memory-writes", stale)
        .await;
    assert_eq!(s, 409);
    let mut foreign = body("notes", "x");
    foreign["sandbox_id"] = json!(Uuid::now_v7());
    let (s, _) = r
        .post(Some(r.replica), "/v1/runner/memory-writes", foreign)
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn language_change_bumps_generation() {
    let r = rig().await;
    let gen_of = || async {
        sqlx::query_scalar::<_, i64>("SELECT home_sync_generation FROM workspaces WHERE id = $1")
            .bind(r.ws)
            .fetch_one(r.h.pool())
            .await
            .unwrap()
    };
    let before = gen_of().await;
    let uri = format!("/v1/workspaces/{}", r.ws);
    let (s, v) = r
        .user(
            Method::PATCH,
            &uri,
            Some(json!({"memory_language": "german"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(gen_of().await, before + 1);
    // An unchanged language does not bump.
    r.user(
        Method::PATCH,
        &uri,
        Some(json!({"memory_language": "german"})),
    )
    .await;
    assert_eq!(gen_of().await, before + 1);
    // The workspace name is part of the rendered instructions (FIX-005): a rename bumps too.
    r.user(Method::PATCH, &uri, Some(json!({"name": "Renamed"})))
        .await;
    assert_eq!(gen_of().await, before + 2);
}

/// (P6-MEMLOCK, C-1149) The exact 409 message every agent-authored refusal carries.
const LOCKED_MESSAGE: &str = "This memory is locked: a person set it, and you can read it but not \
     change or delete it. If it looks wrong or out of date, tell the person.";

/// `audit::record` writes from a spawned task: poll until the action has landed.
async fn audit_count(pool: &sqlx::PgPool, action: &str) -> i64 {
    for _ in 0..60 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(pool)
            .await
            .unwrap();
        if n > 0 {
            return n;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    0
}

#[tokio::test]
async fn locked_memory_routes_and_the_agent_refusal() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let base = format!("/v1/agents/{a}/memories");
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'o@x.io'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let gen_of = || async {
        sqlx::query_scalar::<_, i64>("SELECT home_sync_generation FROM agents WHERE id = $1")
            .bind(a)
            .fetch_one(r.h.pool())
            .await
            .unwrap()
    };
    let revs_of = |key: String| async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM memory_revisions mr JOIN memories m ON m.id = mr.memory_id
             WHERE m.agent_id = $1 AND m.key = $2",
        )
        .bind(a)
        .bind(key)
        .fetch_one(r.h.pool())
        .await
        .unwrap()
    };

    // A create may arrive locked: locked_at/locked_by come from the caller (C-1148).
    let (s, v) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"key": "rules", "title": "Rules", "content": "no shoes", "locked": true})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["locked"], json!(true));
    assert!(v["locked_at"].is_string());
    assert_eq!(v["locked_by"]["id"], json!(uid.to_string()));
    assert_eq!(v["locked_by"]["display_name"], json!("U"));
    assert_eq!(v["index_line"], json!("rules: Rules (locked)"));

    // An unlocked create stores no locker (C-1148: locked_at/locked_by only when locked).
    let (s, v) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"key": "free-form", "title": "Free", "content": "anything"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["locked"], json!(false));
    assert!(v["locked_at"].is_null());
    assert!(v["locked_by"].is_null());
    let locker: Option<Uuid> = sqlx::query_scalar(
        "SELECT locked_by FROM memories WHERE agent_id = $1 AND key = 'free-form'",
    )
    .bind(a)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(locker.is_none(), "an unlocked create stores no locked_by");

    // The lock route: a change bumps the agent's home-sync generation and audits, no revision.
    assert_eq!(r.put(a, "note", "hello").await.0, 200);
    let gen_before = gen_of().await;
    let (s, v) = r
        .user(
            Method::PUT,
            &format!("{base}/note/locked"),
            Some(json!({"locked": true})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["locked"], json!(true));
    assert_eq!(v["locked_by"]["display_name"], json!("U"));
    assert_eq!(gen_of().await, gen_before + 1);
    assert_eq!(revs_of("note".into()).await, 1);
    assert_eq!(audit_count(r.h.pool(), "memory.lock").await, 1);

    // Idempotent: the same value changes nothing, audits nothing and bumps nothing (C-1148).
    let (s, v) = r
        .user(
            Method::PUT,
            &format!("{base}/note/locked"),
            Some(json!({"locked": true})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(gen_of().await, gen_before + 1);
    assert_eq!(revs_of("note".into()).await, 1);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let locks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'memory.lock'")
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(locks, 1);

    // A person's PUT on a locked memory works and keeps the lock; restore too (C-1148).
    let (s, v) = r.put(a, "note", "hello again").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["locked"].clone(), v["revision"].clone()),
        (json!(true), json!(2))
    );
    let (s, v) = r
        .user(
            Method::POST,
            &format!("{base}/note/restore"),
            Some(json!({"revision": 1})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["locked"], json!(true));

    // Unlock clears locked_at/locked_by and audits `memory.unlock`.
    let (s, v) = r
        .user(
            Method::PUT,
            &format!("{base}/note/locked"),
            Some(json!({"locked": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["locked"], json!(false));
    assert!(v["locked_at"].is_null());
    assert!(v["locked_by"].is_null());
    // Lock (+1), person save (+1), restore (+1) and unlock (+1).
    assert_eq!(gen_of().await, gen_before + 4);
    assert_eq!(audit_count(r.h.pool(), "memory.unlock").await, 1);

    // 404 for an unknown key (and the route has the same workspace access as the others).
    let (s, _) = r
        .user(
            Method::PUT,
            &format!("{base}/nope/locked"),
            Some(json!({"locked": true})),
        )
        .await;
    assert_eq!(s, 404);

    // --- the agent refusal (C-1149, SEC-151) ---
    // The tools act on the booted agent (`booted` creates its own): lock a memory of *that* agent.
    let b = r.booted("t1").await;
    let ta = b.agent;
    assert_eq!(r.put(ta, "note", "hello").await.0, 200);
    let (s, _) = r
        .user(
            Method::PUT,
            &format!("/v1/agents/{ta}/memories/note/locked"),
            Some(json!({"locked": true})),
        )
        .await;
    assert_eq!(s, 200);
    let revs_of_ta = |key: String| {
        let pool = r.h.pool();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM memory_revisions mr JOIN memories m ON m.id = mr.memory_id
                 WHERE m.agent_id = $1 AND m.key = $2",
            )
            .bind(ta)
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap()
        }
    };
    // Tool save on a locked key: refused with the exact message, nothing written.
    let v = r
        .tool(
            &b,
            "memory_save",
            json!({"key": "note", "content": "mine now"}),
        )
        .await;
    assert_eq!(v["ok"], json!(false), "{v}");
    assert_eq!(v["error"]["message"], json!(LOCKED_MESSAGE));
    assert_eq!(revs_of_ta("note".into()).await, 1);
    // Tool delete on a locked key: refused, the row is still there.
    let v = r.tool(&b, "memory_delete", json!({"key": "note"})).await;
    assert_eq!(v["ok"], json!(false), "{v}");
    assert_eq!(v["error"]["message"], json!(LOCKED_MESSAGE));
    let (s, _) = r
        .user(Method::GET, &format!("/v1/agents/{ta}/memories/note"), None)
        .await;
    assert_eq!(s, 200);
    // The harvest write on a locked key: 409, no revision.
    let body = json!({"sandbox_id": b.sandbox, "boot_id": b.boot, "key": "note",
                      "content": "harvested"});
    let (s, v) = r
        .post(Some(r.replica), "/v1/runner/memory-writes", body)
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], json!("memory_locked"));
    assert_eq!(v["error"]["message"], json!(LOCKED_MESSAGE));
    assert_eq!(revs_of_ta("note".into()).await, 1);
    // New keys are never refused (C-1149).
    let v = r
        .tool(&b, "memory_save", json!({"key": "fresh", "content": "ok"}))
        .await;
    assert_eq!(v["ok"], json!(true), "{v}");
    // A person's DELETE of a locked memory deletes it (C-1148).
    let (s, _) = r
        .user(
            Method::DELETE,
            &format!("/v1/agents/{ta}/memories/note"),
            None,
        )
        .await;
    assert_eq!(s, 204);
}

#[tokio::test]
async fn the_cap_counts_locked_memories() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let base = format!("/v1/agents/{a}/memories");
    for i in 0..100 {
        sqlx::query(
            "INSERT INTO memories (id, agent_id, key, content, revision) VALUES ($1,$2,$3,'c',1)",
        )
        .bind(Uuid::now_v7())
        .bind(a)
        .bind(format!("k{i}"))
        .execute(r.h.pool())
        .await
        .unwrap();
    }
    sqlx::query("UPDATE memories SET locked = true, locked_at = now() WHERE agent_id = $1")
        .bind(a)
        .execute(r.h.pool())
        .await
        .unwrap();
    // Every existing memory is locked, and the cap still refuses a new key (C-1148).
    let (s, v) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"key": "one-more", "content": "x"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("memory_limit_reached"))
    );
}

/// (P6-MEMLOCK, C-1149) The lock check runs inside the write's transaction after the agent row
/// lock, so a lock that commits while a tool write waits on that lock still wins.
#[tokio::test]
async fn a_lock_committing_first_wins_over_a_waiting_agent_write() {
    let r = rig().await;
    let a = r.agent("a1").await;
    assert_eq!(r.put(a, "note", "hello").await.0, 200);
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'o@x.io'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let note_id: Uuid = sqlx::query_scalar("SELECT id FROM memories WHERE agent_id = $1")
        .bind(a)
        .fetch_one(r.h.pool())
        .await
        .unwrap();

    // Hold the agent row lock; the agent's write blocks on it inside its own transaction.
    let mut tx = r.h.pool().begin().await.unwrap();
    aurora_db::repo::memory::lock_agent(&mut tx, a)
        .await
        .unwrap();
    let pool = r.h.pool().clone();
    let write = tokio::spawn(async move {
        let w = memory::service::Write {
            key: "note",
            title: None,
            description: None,
            content: "mine",
            max_content: 1_000,
            create_only: false,
            skip_unchanged: false,
            locked: false,
        };
        let author = memory::Author {
            kind: "agent",
            user_id: None,
            label: "A".into(),
        };
        memory::service::save(&pool, a, &w, &author, &memory::Source::Harvest).await
    });
    // The person locks while the write waits, then commits first.
    aurora_db::repo::memory::set_locked(&mut tx, note_id, true, Some(uid))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let e = write.await.unwrap().unwrap_err();
    assert_eq!(e.status().as_u16(), 409);
    assert_eq!(e.message(), LOCKED_MESSAGE);
    let locked: bool = sqlx::query_scalar("SELECT locked FROM memories WHERE id = $1")
        .bind(note_id)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert!(locked);
}
