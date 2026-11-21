#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-061: skills API, limits, permissions, generation bump, tools and projection.

use crate::common::*;
use crate::runner_rig::{Booted, Rig, rig};
use http::Method;

#[path = "skills_library.rs"]
mod library_tier;
#[path = "skills_workspace.rs"]
mod workspace_tier;
use serde_json::{Value, json};
use uuid::Uuid;

impl Rig {
    async fn raw_put(&self, cookie: &str, uri: &str, body: &[u8]) -> (u16, Value) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let req = http::Request::builder()
            .method(Method::PUT)
            .uri(uri)
            .header("cookie", cookie)
            .header("origin", ORIGIN)
            .header("content-type", "application/octet-stream")
            .body(axum::body::Body::from(body.to_vec()))
            .unwrap();
        let res = self.h.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn as_user(&self, email: &str, role: &str) -> String {
        let u = self.h.user(email, false).await;
        self.h.member(self.org, u, role, "active").await;
        let (_, st) = self.h.good_session(u).await;
        cookie(&st)
    }

    async fn call(&self, cookie: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        self.h.call(cookie, m, uri, body).await
    }

    async fn gen_of(&self, agent: Uuid) -> i64 {
        sqlx::query_scalar("SELECT home_sync_generation FROM agents WHERE id = $1")
            .bind(agent)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn code(v: &Value) -> Option<&str> {
    v["error"]["code"].as_str()
}

#[tokio::test]
async fn crud_limits_and_names() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let base = format!("/v1/agents/{a}/skills");
    let g0 = r.gen_of(a).await;
    let (s, v) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"name": "deploy", "description": "How to deploy", "body": "# Steps"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        (v["scope"].as_str(), v["agent_id"].as_str()),
        (Some("agent"), Some(a.to_string().as_str()))
    );
    assert_eq!(v["total_size"], 7);
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(r.gen_of(a).await, g0 + 1);
    let (s, v) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"name": "deploy", "description": "d", "body": ""})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, Some("skill_exists")));
    for bad in [
        "Bad",
        "a--b",
        "aurora-core",
        "aurora-x",
        "",
        &"x".repeat(65),
    ] {
        let (s, _) = r
            .user(
                Method::POST,
                &base,
                Some(json!({"name": bad, "description": "d", "body": "b"})),
            )
            .await;
        assert_eq!(s, 400, "{bad}");
    }
    for d in ["", "two\nlines", &"d".repeat(501)] {
        let (s, _) = r
            .user(
                Method::POST,
                &base,
                Some(json!({"name": "okay", "description": d, "body": "b"})),
            )
            .await;
        assert_eq!(s, 400, "{d:?}");
    }
    let (s, _) = r
        .user(
            Method::POST,
            &base,
            Some(json!({"name": "okay", "description": "d", "body": "b".repeat(24_001)})),
        )
        .await;
    assert_eq!(s, 400);

    // Total limit: body 24k + files.
    let uri = format!("/v1/skills/{id}");
    let (s, _) = r
        .user(
            Method::PATCH,
            &uri,
            Some(json!({"body": "b".repeat(24_000)})),
        )
        .await;
    assert_eq!(s, 200);
    let f = format!("{uri}/files/scripts/run.sh");
    let (s, v) = r.raw_put(&r.cookie, &f, "x".repeat(8_000).as_bytes()).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["total_size"], 32_000);
    let (s, v) = r.raw_put(&r.cookie, &f, "x".repeat(8_001).as_bytes()).await;
    assert_eq!((s, code(&v)), (422, Some("skill_too_large")));
    // Replacing a file accounts for the old size.
    assert_eq!(r.raw_put(&r.cookie, &f, b"small").await.0, 200);

    let (s, v) = r.user(Method::GET, &base, None).await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1));
    assert!(v["items"][0]["files"][0].get("content").is_none());
    let (s, _) = r.user(Method::DELETE, &uri, None).await;
    assert_eq!(s, 204);
    assert_eq!(r.user(Method::GET, &uri, None).await.0, 404);
}

#[tokio::test]
async fn file_paths_and_executable_bit() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let (_, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{a}/skills"),
            Some(json!({"name": "tool", "description": "d", "body": "b"})),
        )
        .await;
    let uri = format!("/v1/skills/{}/files", v["id"].as_str().unwrap());
    for bad in [
        "..%2Fx",
        "a/../b",
        "a/./b",
        "a//b",
        "a/b/c/d/e",
        "sp%20ace",
        "SKILL.md",
        "dir/skill.MD",
        // (FIX-110) SKILL.md in any segment, not only the last.
        "skill.md/x",
        "a/SKILL.MD/b",
        "caf%C3%A9",
    ] {
        let (s, v) = r.raw_put(&r.cookie, &format!("{uri}/{bad}"), b"x").await;
        assert!(matches!(s, 400 | 404), "{bad}: {s} {v}");
        assert_ne!(s, 200, "{bad}");
    }
    let (s, _) = r.raw_put(&r.cookie, &format!("{uri}/a/b/c/d"), b"x").await;
    assert_eq!(s, 200);
    let (s, v) = r
        .raw_put(
            &r.cookie,
            &format!("{uri}/bin/run.sh?executable=true"),
            b"#!/bin/sh",
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let exec = |v: &Value, p: &str| {
        v["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == p)
            .map(|f| f["executable"].clone())
    };
    assert_eq!(exec(&v, "bin/run.sh"), Some(json!(true)));
    assert_eq!(exec(&v, "a/b/c/d"), Some(json!(false)));
    // Omitted flag keeps the bit; explicit false clears it.
    let (_, v) = r
        .raw_put(&r.cookie, &format!("{uri}/bin/run.sh"), b"#!/bin/sh\n")
        .await;
    assert_eq!(exec(&v, "bin/run.sh"), Some(json!(true)));
    let (_, v) = r
        .raw_put(
            &r.cookie,
            &format!("{uri}/bin/run.sh?executable=false"),
            b"x",
        )
        .await;
    assert_eq!(exec(&v, "bin/run.sh"), Some(json!(false)));
    // Binary counts as base64 length.
    let (_, v) = r
        .raw_put(&r.cookie, &format!("{uri}/blob.bin"), &[0xff, 0xfe, 0xfd])
        .await;
    let size = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "blob.bin")
        .unwrap()["size"]
        .clone();
    assert_eq!(size, 4);
    let (s, _) = r
        .user(Method::DELETE, &format!("{uri}/blob.bin"), None)
        .await;
    assert_eq!(s, 204);
    let (s, _) = r
        .user(Method::DELETE, &format!("{uri}/blob.bin"), None)
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn org_permissions_and_generation_bump() {
    let r = rig().await;
    let a1 = r.agent("a1").await;
    let a2 = r.agent("a2").await;
    let member = r.cookie.clone();
    let admin = r.as_user("adm@x.io", "admin").await;
    let base = format!("/v1/orgs/{}/skills", r.org);
    let body = json!({"name": "policy", "description": "Org policy", "body": "Be nice"});
    let (s, v) = r
        .call(&member, Method::POST, &base, Some(body.clone()))
        .await;
    assert_eq!((s, code(&v)), (403, Some("forbidden")));
    let (g1, g2) = (r.gen_of(a1).await, r.gen_of(a2).await);
    let (s, v) = r.call(&admin, Method::POST, &base, Some(body)).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["scope"], "organization");
    assert!(v["agent_id"].is_null());
    assert_eq!((r.gen_of(a1).await, r.gen_of(a2).await), (g1 + 1, g2 + 1));
    let id = v["id"].as_str().unwrap().to_owned();
    // Members read but cannot write.
    assert_eq!(r.call(&member, Method::GET, &base, None).await.0, 200);
    assert_eq!(
        r.call(&member, Method::GET, &format!("/v1/skills/{id}"), None)
            .await
            .0,
        200
    );
    let (s, _) = r
        .call(
            &member,
            Method::PATCH,
            &format!("/v1/skills/{id}"),
            Some(json!({"body": "x"})),
        )
        .await;
    assert_eq!(s, 403);
    assert_eq!(
        r.raw_put(&member, &format!("/v1/skills/{id}/files/x.txt"), b"x")
            .await
            .0,
        403
    );
    assert_eq!(
        r.call(&member, Method::DELETE, &format!("/v1/skills/{id}"), None)
            .await
            .0,
        403
    );
    // Admin edits bump every agent; file changes too.
    let (s, _) = r
        .call(
            &admin,
            Method::PATCH,
            &format!("/v1/skills/{id}"),
            Some(json!({"body": "Be kind"})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(r.gen_of(a2).await, g2 + 2);
    assert_eq!(
        r.raw_put(&admin, &format!("/v1/skills/{id}/files/x.txt"), b"x")
            .await
            .0,
        200
    );
    assert_eq!(r.gen_of(a1).await, g1 + 3);
    // Same name at agent tier is allowed (agent wins in the projection).
    let (s, _) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{a1}/skills"),
            Some(json!({"name": "policy", "description": "Mine", "body": "Mine"})),
        )
        .await;
    assert_eq!(s, 201);
    // Deleting bumps again.
    assert_eq!(
        r.call(&admin, Method::DELETE, &format!("/v1/skills/{id}"), None)
            .await
            .0,
        204
    );
    assert_eq!(r.gen_of(a2).await, g2 + 4);
    // A user without access to the org sees nothing.
    let u = r.h.user("out@x.io", false).await;
    let (_, st) = r.h.good_session(u).await;
    assert_eq!(r.call(&cookie(&st), Method::GET, &base, None).await.0, 404);
    assert_eq!(
        r.call(
            &cookie(&st),
            Method::GET,
            &format!("/v1/agents/{a1}/skills"),
            None
        )
        .await
        .0,
        404
    );
}

impl Rig {
    /// GET that keeps the bytes and the executable header: `(status, executable header, bytes)`.
    async fn raw_get(&self, cookie: &str, uri: &str) -> (u16, Option<String>, Vec<u8>) {
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let req = http::Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("cookie", cookie)
            .body(axum::body::Body::empty())
            .unwrap();
        let res = self.h.app.clone().oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let exec = res
            .headers()
            .get("x-skill-file-executable")
            .map(|v| v.to_str().unwrap().to_owned());
        if status == 200 {
            // Bytes people wrote answer like attachments, whatever the stored file is (C-1099).
            let h = res.headers();
            assert_eq!(h["content-type"], "application/octet-stream");
            assert_eq!(h["x-content-type-options"], "nosniff");
            let cd = h["content-disposition"].to_str().unwrap();
            assert!(cd.starts_with("attachment"), "{cd}");
            let last = uri.rsplit('/').next().unwrap();
            assert_eq!(cd, format!("attachment; filename=\"{last}\""));
        }
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, exec, bytes.to_vec())
    }
}

/// C-1099: `GET /v1/skills/{id}/files/{path}` returns the stored bytes, with the item route's authority.
#[tokio::test]
async fn skill_file_read_authority_paths_and_isolation() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let mk = |name: &'static str| {
        let r = &r;
        async move {
            let (s, v) = r
                .user(
                    Method::POST,
                    &format!("/v1/agents/{a}/skills"),
                    Some(json!({"name": name, "description": "d", "body": "b"})),
                )
                .await;
            assert_eq!(s, 201, "{v}");
            v["id"].as_str().unwrap().to_owned()
        }
    };
    let (one, two) = (mk("one").await, mk("two").await);
    let put = |id: &str, p: &str, body: &[u8]| {
        let uri = format!("/v1/skills/{id}/files/{p}");
        let r = &r;
        let body = body.to_vec();
        async move { assert_eq!(r.raw_put(&r.cookie, &uri, &body).await.0, 200) }
    };
    put(&one, "notes/a.txt", b"hello").await;
    put(&one, "bin/run.sh?executable=true", b"#!/bin/sh\n").await;
    put(&one, "blob.bin", &[0xff, 0xfe, 0xfd, 0x00]).await;
    put(&two, "only-two.txt", b"secret of two").await;

    // Allowed: the bytes as stored, text and binary, with the executable bit.
    let (s, exec, b) = r
        .raw_get(&r.cookie, &format!("/v1/skills/{one}/files/notes/a.txt"))
        .await;
    assert_eq!(
        (s, exec.as_deref(), b.as_slice()),
        (200, Some("false"), &b"hello"[..])
    );
    let (s, exec, b) = r
        .raw_get(&r.cookie, &format!("/v1/skills/{one}/files/bin/run.sh"))
        .await;
    assert_eq!(
        (s, exec.as_deref(), b.as_slice()),
        (200, Some("true"), &b"#!/bin/sh\n"[..])
    );
    let (s, _, b) = r
        .raw_get(&r.cookie, &format!("/v1/skills/{one}/files/blob.bin"))
        .await;
    assert_eq!((s, b.as_slice()), (200, &[0xff, 0xfe, 0xfd, 0x00][..]));
    // The skill lists its file paths (what the export iterates).
    let (_, v) = r
        .user(Method::GET, &format!("/v1/skills/{one}"), None)
        .await;
    let mut paths: Vec<&str> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, ["bin/run.sh", "blob.bin", "notes/a.txt"]);

    // A missing file is 404; so is another skill's file through this skill's id.
    let (s, _, _) = r
        .raw_get(&r.cookie, &format!("/v1/skills/{one}/files/missing.txt"))
        .await;
    assert_eq!(s, 404);
    let (s, _, b) = r
        .raw_get(&r.cookie, &format!("/v1/skills/{one}/files/only-two.txt"))
        .await;
    assert_eq!(s, 404, "{}", String::from_utf8_lossy(&b));
    // The same path under its own skill still reads.
    let (s, _, b) = r
        .raw_get(&r.cookie, &format!("/v1/skills/{two}/files/only-two.txt"))
        .await;
    assert_eq!((s, b.as_slice()), (200, &b"secret of two"[..]));

    // Path traversal and the other bad paths are refused as for PUT: never 200.
    for bad in [
        "..%2Fx",
        "a/../b",
        "a/./b",
        "a//b",
        "a/b/c/d/e",
        "sp%20ace",
        "SKILL.md",
        "skill.md/x",
        "..%2F..%2Fetc%2Fpasswd",
        "caf%C3%A9",
    ] {
        let (s, _, _) = r
            .raw_get(&r.cookie, &format!("/v1/skills/{one}/files/{bad}"))
            .await;
        assert!(matches!(s, 400 | 404), "{bad}: {s}");
    }
    let (s, _, _) = r
        .raw_get(&r.cookie, &format!("/v1/skills/{one}/files/a/b/c/d/e"))
        .await;
    assert_eq!(s, 400);

    // Forbidden: someone who cannot see the agent gets what the item route gives them (never the bytes).
    let u = r.h.user("out2@x.io", false).await;
    let (_, st) = r.h.good_session(u).await;
    let outsider = cookie(&st);
    let (item, _) = r
        .call(&outsider, Method::GET, &format!("/v1/skills/{one}"), None)
        .await;
    assert_ne!(item, 200);
    let (s, _, b) = r
        .raw_get(&outsider, &format!("/v1/skills/{one}/files/notes/a.txt"))
        .await;
    assert_eq!(s, item);
    assert_ne!(b.as_slice(), b"hello");
    // No cookie at all: 401.
    let (s, _, _) = r
        .raw_get("", &format!("/v1/skills/{one}/files/notes/a.txt"))
        .await;
    assert_eq!(s, 401);

    // An organization skill: a member reads (as the item route), an outsider does not; the admin's file is the bytes stored.
    let admin = r.as_user("adm2@x.io", "admin").await;
    let (s, v) = r
        .call(
            &admin,
            Method::POST,
            &format!("/v1/orgs/{}/skills", r.org),
            Some(json!({"name": "policy2", "description": "d", "body": "b"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let org_skill = v["id"].as_str().unwrap().to_owned();
    assert_eq!(
        r.raw_put(
            &admin,
            &format!("/v1/skills/{org_skill}/files/p.txt"),
            b"org bytes"
        )
        .await
        .0,
        200
    );
    let (s, _, b) = r
        .raw_get(&r.cookie, &format!("/v1/skills/{org_skill}/files/p.txt"))
        .await;
    assert_eq!((s, b.as_slice()), (200, &b"org bytes"[..]));
    let (s, _, _) = r
        .raw_get(&outsider, &format!("/v1/skills/{org_skill}/files/p.txt"))
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn skill_tools_via_tool_calls() {
    let r = rig().await;
    let b = r.booted("t1").await;
    // An org skill, created by an admin through the DB.
    let org_skill = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skills (id, org_id, name, description, body, created_by_kind)
         VALUES ($1,$2,'orgskill','From org','org body','user')",
    )
    .bind(org_skill)
    .bind(r.org)
    .execute(r.h.pool())
    .await
    .unwrap();
    let g0 = r.gen_of(b.agent).await;
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "mine", "description": "My skill", "body": "do it"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(r.gen_of(b.agent).await, g0 + 1);
    let v = r.tool(&b, "skill_list", json!({})).await;
    let names: Vec<&str> = v["content"][0]["value"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["mine", "orgskill"]);
    let v = r.tool(&b, "skill_get", json!({"name": "orgskill"})).await;
    assert_eq!(v["content"][0]["value"]["body"], "org body");
    let v = r.tool(&b, "skill_get", json!({"name": "mine"})).await;
    assert_eq!(v["content"][0]["value"]["scope"], "agent");
    // Org skills are read-only for the agent.
    for (t, a) in [
        ("skill_update", json!({"name": "orgskill", "body": "x"})),
        ("skill_delete", json!({"name": "orgskill"})),
    ] {
        let v = r.tool(&b, t, a).await;
        assert_eq!(v["ok"], false, "{t}: {v}");
    }
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM skills WHERE id = $1 AND body = 'org body'")
            .bind(org_skill)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 1);
    let v = r
        .tool(
            &b,
            "skill_update",
            json!({"name": "mine", "body": "better", "enabled": false}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "aurora-x", "description": "d", "body": "b"}),
        )
        .await;
    assert_eq!(v["ok"], false);
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "x", "description": "d", "body": "b", "bogus": 1}),
        )
        .await;
    assert_eq!(v["error"]["code"], "invalid_arguments");
    // Created by the agent.
    let k: String = sqlx::query_scalar("SELECT created_by_kind FROM skills WHERE name = 'mine'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(k, "agent");
    let v = r.tool(&b, "skill_delete", json!({"name": "mine"})).await;
    assert_eq!(v["ok"], true, "{v}");
    // Another agent never sees it.
    let o = r.booted("t2").await;
    let v = r.tool(&o, "skill_get", json!({"name": "mine"})).await;
    assert_eq!(v["ok"], false);
}

/// (FIX-110) An agent has at most 50 skills of its own and a skill at most 50 files: the 51st of
/// each is refused (409) and writes no row, through the route and through `skill_create`.
#[tokio::test]
async fn skill_and_file_counts_are_capped() {
    let r = rig().await;
    let b = r.booted("capped").await;
    sqlx::query(
        "INSERT INTO skills (id, org_id, agent_id, name, description, body, enabled, created_by_kind)
         SELECT gen_random_uuid(), $1, $2, 'bulk-' || n, 'd', 'b', true, 'user'
         FROM generate_series(1, 49) n",
    )
    .bind(r.org)
    .bind(b.agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let a = b.agent;
    let (s, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{a}/skills"),
            Some(json!({"name": "fiftieth", "description": "d", "body": "b"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let skill = v["id"].as_str().unwrap().to_owned();
    let (s, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{a}/skills"),
            Some(json!({"name": "fifty-first", "description": "d", "body": "b"})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, Some("skill_limit_reached")), "{v}");
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "by-the-agent", "description": "d", "body": "b"}),
        )
        .await;
    assert_eq!(v["ok"], false, "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM skills WHERE agent_id = $1")
        .bind(a)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 50);

    // Files: 50 fit, the 51st is refused; replacing an existing path is not a new file.
    sqlx::query(
        "INSERT INTO skill_files (skill_id, path, content) SELECT $1::uuid, 'f' || n, 'x'::bytea
         FROM generate_series(1, 50) n",
    )
    .bind(&skill)
    .execute(r.h.pool())
    .await
    .unwrap();
    let uri = format!("/v1/skills/{skill}/files");
    let (s, v) = r.raw_put(&r.cookie, &format!("{uri}/f51"), b"x").await;
    assert_eq!(
        (s, code(&v)),
        (409, Some("skill_files_limit_reached")),
        "{v}"
    );
    let (s, v) = r.raw_put(&r.cookie, &format!("{uri}/f7"), b"y").await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM skill_files WHERE skill_id = $1::uuid")
        .bind(&skill)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 50);
}

#[tokio::test]
async fn projection_entries_in_home_sync() {
    let r = rig().await;
    let b = r.booted("p1").await;
    let mk = |agent: Option<Uuid>, name: &str, desc: &str, enabled: bool| {
        let (org, name, desc) = (r.org, name.to_owned(), desc.to_owned());
        let pool = r.h.pool().clone();
        async move {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO skills (id, org_id, agent_id, name, description, body, enabled, created_by_kind)
                 VALUES ($1,$2,$3,$4,$5,'body',$6,'user')",
            )
            .bind(id).bind(org).bind(agent).bind(name).bind(desc).bind(enabled)
            .execute(&pool).await.unwrap();
            id
        }
    };
    mk(None, "shared", "Org shared", true).await;
    mk(Some(b.agent), "shared", "Agent shared", true).await;
    mk(Some(b.agent), "off", "Disabled agent", false).await;
    mk(None, "off", "Org off", true).await;
    // Files go through the API so the exec flag is exercised end to end.
    let (_, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{}/skills", b.agent),
            Some(json!({"name": "withfiles", "description": "d", "body": "b"})),
        )
        .await;
    let uri = format!("/v1/skills/{}/files", v["id"].as_str().unwrap());
    r.raw_put(
        &r.cookie,
        &format!("{uri}/run.sh?executable=true"),
        b"#!/bin/sh",
    )
    .await;
    r.raw_put(&r.cookie, &format!("{uri}/notes.txt"), b"hi")
        .await;

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
    let entries: Vec<Value> = parts
        .iter()
        .filter(|p| p.part.kind.as_str() == "skills")
        .flat_map(|p| p.part.entries.clone())
        .collect();
    let find = |path: &str| entries.iter().find(|e| e["path"] == path).cloned();
    let shared = find(".claude/skills/shared/SKILL.md").unwrap();
    assert!(shared["content"].as_str().unwrap().contains("Agent shared"));
    assert_eq!(
        entries
            .iter()
            .filter(|e| e["path"] == ".claude/skills/shared/SKILL.md")
            .count(),
        1
    );
    // A disabled agent skill does not veto the enabled org skill of the same name.
    let off = find(".claude/skills/off/SKILL.md").unwrap();
    assert!(off["content"].as_str().unwrap().contains("Org off"));
    assert_eq!(
        find(".claude/skills/withfiles/run.sh").unwrap()["exec"],
        true
    );
    assert_eq!(
        find(".claude/skills/withfiles/notes.txt").unwrap()["exec"],
        false
    );
}

/// TEST-014: parts stay within `HOME_SYNC_PART_MAX` at worst-case encodings (docs/12 §Sync
/// protocol: CJK x3 bytes, JSON-escaped controls x6) with every entry delivered exactly once.
#[tokio::test]
async fn home_sync_parts_pack_within_the_wire_max_at_worst_case_encodings() {
    let r = rig().await;
    let b = r.booted("pk1").await;
    // Skill bodies of control characters: each is escaped to `\u0001` (6 bytes) on the wire.
    let ctrl = "\u{1}".repeat(aurora_core::limits::SKILL_BODY_MAX);
    for i in 0..3 {
        sqlx::query(
            "INSERT INTO skills (id, org_id, agent_id, name, description, body, enabled, created_by_kind)
             VALUES ($1,$2,$3,$4,'d',$5,true,'user')",
        )
        .bind(Uuid::now_v7())
        .bind(r.org)
        .bind(b.agent)
        .bind(format!("ctrl-{i}"))
        .bind(&ctrl)
        .execute(r.h.pool())
        .await
        .unwrap();
    }
    // Memories at the tool content cap in 3-byte CJK.
    let cjk = "漢".repeat(aurora_core::limits::MEMORY_TOOL_CONTENT_MAX);
    for i in 0..30 {
        sqlx::query(
            "INSERT INTO memories (id, agent_id, key, content, revision) VALUES ($1,$2,$3,$4,1)",
        )
        .bind(Uuid::now_v7())
        .bind(b.agent)
        .bind(format!("cjk-{i}"))
        .bind(&cjk)
        .execute(r.h.pool())
        .await
        .unwrap();
    }
    let claim = aurora_db::repo::hosted::HomeSyncClaim {
        sandbox_id: b.sandbox,
        boot_id: b.boot,
        agent_id: b.agent,
        generation: 7,
        harness_id: Some("fake".into()),
    };
    let mut conn = r.h.pool().acquire().await.unwrap();
    let parts = aurora_api::domains::hosted::home::render(&r.h.state, &mut conn, &claim)
        .await
        .unwrap();
    let count = u32::try_from(parts.len()).unwrap();
    for (i, p) in parts.iter().enumerate() {
        let wire = serde_json::to_string(p).unwrap();
        assert!(
            wire.len() <= aurora_protocol::runner::MAX_HOME_SYNC_PART,
            "part {i} ({}) is {} bytes",
            p.part.kind.as_str(),
            wire.len()
        );
        assert_eq!(
            (p.generation, p.part_index, p.part_count),
            (7, u32::try_from(i).unwrap(), count)
        );
    }
    assert_eq!(parts.last().unwrap().part.kind.as_str(), "final");
    let paths = |kind: &str| -> Vec<String> {
        parts
            .iter()
            .filter(|p| p.part.kind.as_str() == kind)
            .flat_map(|p| p.part.entries.iter())
            .map(|e| e["path"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(paths("skills").len(), 3);
    assert!(
        parts
            .iter()
            .filter(|p| p.part.kind.as_str() == "skills")
            .count()
            >= 3
    );
    // 30 memories + index, each once, and the manifest names every managed path.
    let mem = paths("memory");
    assert_eq!(mem.len(), 31);
    let manifest = paths("manifest");
    for p in paths("skills").iter().chain(mem.iter()) {
        assert!(manifest.contains(p), "{p} missing from the manifest");
    }
}
