#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P3-020 (RFE-020, D-64, SEC-56): workspace-tier skills — effective set, authorization per
//! tier, projection and generation bumps, agent tools, cascade (`18 §Workspace-tier skills`).

use super::*;

/// Raw skill insert at any tier: `(agent, workspace)`, both none => org.
async fn raw(
    r: &Rig,
    agent: Option<Uuid>,
    ws: Option<Uuid>,
    name: &str,
    desc: &str,
    enabled: bool,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skills (id, org_id, agent_id, workspace_id, name, description, body, enabled,
            created_by_kind) VALUES ($1,$2,$3,$4,$5,$6,'body',$7,'user')",
    )
    .bind(id)
    .bind(r.org)
    .bind(agent)
    .bind(ws)
    .bind(name)
    .bind(desc)
    .bind(enabled)
    .execute(r.h.pool())
    .await?;
    Ok(id)
}

/// A user with an org role and optionally a workspace role on `ws`; returns its cookie.
async fn actor(r: &Rig, email: &str, org_role: &str, ws_role: Option<&str>) -> String {
    let u = r.h.user(email, false).await;
    r.h.member(r.org, u, org_role, "active").await;
    if let Some(role) = ws_role {
        r.h.bind_ws(r.ws, u, role).await;
    }
    let (_, st) = r.h.good_session(u).await;
    cookie(&st)
}

async fn ws_gen(r: &Rig, ws: Uuid) -> i64 {
    sqlx::query_scalar("SELECT home_sync_generation FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

async fn desired(r: &Rig, agent: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT a.home_sync_generation + w.home_sync_generation FROM agents a
         JOIN workspaces w ON w.id = a.workspace_id WHERE a.id = $1",
    )
    .bind(agent)
    .fetch_one(r.h.pool())
    .await
    .unwrap()
}

fn tiers(v: &Value, key: &str) -> Vec<(String, String)> {
    v[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["skill"]["name"].as_str().unwrap().to_owned(),
                i["tier"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Effective set: every combination of one name at agent/workspace/org with enabled/disabled.
#[tokio::test]
async fn effective_set_all_tier_and_enabled_combinations() {
    let r = rig().await;
    let a = r.agent("e1").await;
    // 3 tiers x {absent, enabled, disabled} per tier = 27 names; "n{code}" encodes each.
    let mut expected: Vec<(String, Option<&str>)> = Vec::new();
    for ag in 0..3 {
        for ws in 0..3 {
            for org in 0..3 {
                let name = format!("n{ag}{ws}{org}");
                let mut winner = None;
                for (state, tier, agent, wsid) in [
                    (ag, "agent", Some(a), None),
                    (ws, "workspace", None, Some(r.ws)),
                    (org, "org", None, None),
                ] {
                    if state == 0 {
                        continue;
                    }
                    raw(&r, agent, wsid, &name, "d", state == 1).await.unwrap();
                    if state == 1 && winner.is_none() {
                        winner = Some(tier);
                    }
                }
                expected.push((name, winner));
            }
        }
    }
    let (s, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{a}/skills/effective"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let items = tiers(&v, "items");
    for (name, winner) in &expected {
        let got = items
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.as_str());
        assert_eq!(got, *winner, "{name}");
    }
    // Ordered by name; `enabled` is always true in items.
    let names: Vec<&String> = items.iter().map(|(n, _)| n).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["enabled"] == true)
    );
    // Every other skill of the three tiers is in `shadowed` with its winner (null when none).
    let shadowed = v["shadowed"].as_array().unwrap();
    let total: usize = expected
        .iter()
        .map(|(n, _)| n.bytes().skip(1).filter(|b| *b != b'0').count())
        .sum();
    assert_eq!(items.len() + shadowed.len(), total);
    for sh in shadowed {
        let name = sh["skill"]["name"].as_str().unwrap();
        let winner = expected.iter().find(|(n, _)| n == name).unwrap().1;
        match winner {
            Some(t) => assert_eq!(sh["shadowed_by"]["tier"], t, "{name}"),
            None => assert!(sh["shadowed_by"].is_null(), "{name}"),
        }
    }
    // `shadows` lists every lower-tier skill of the name, enabled or not: n111 -> agent wins.
    let n111 = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["skill"]["name"] == "n111")
        .unwrap();
    let sh: Vec<&str> = n111["shadows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["tier"].as_str().unwrap())
        .collect();
    assert_eq!(sh, ["workspace", "org"]);
    // The agent-tier disabled skill of n212 does not veto the workspace one.
    assert_eq!(
        items.iter().find(|(n, _)| n == "n212").unwrap().1,
        "workspace"
    );
}

/// Names are unique per tier, allowed across tiers; reserved names refused at every tier;
/// the CHECK forbids agent+workspace on one row; the replaced org index keeps org uniqueness.
#[tokio::test]
async fn names_tier_local_reserved_and_check_constraint() {
    let r = rig().await;
    let a = r.agent("u1").await;
    let ws_uri = format!("/v1/workspaces/{}/skills", r.ws);
    let org_admin = actor(&r, "adm@x.io", "admin", None).await;
    let body = json!({"name": "dup", "description": "d", "body": "b"});
    let (s, v) = r.user(Method::POST, &ws_uri, Some(body.clone())).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(
        (v["tier"].as_str(), v["scope"].as_str()),
        (Some("workspace"), Some("workspace"))
    );
    assert_eq!(v["workspace_id"], json!(r.ws));
    assert!(v["agent_id"].is_null());
    let (s, v) = r.user(Method::POST, &ws_uri, Some(body.clone())).await;
    assert_eq!((s, code(&v)), (409, Some("skill_exists")), "{v}");
    // The same name in the other tiers is fine.
    let (s, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{a}/skills"),
            Some(body.clone()),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["tier"], "agent");
    let org_uri = format!("/v1/orgs/{}/skills", r.org);
    let (s, v) = r
        .call(&org_admin, Method::POST, &org_uri, Some(body.clone()))
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["tier"], "org");
    assert_eq!(v["scope"], "organization");
    let (s, v) = r
        .call(&org_admin, Method::POST, &org_uri, Some(body.clone()))
        .await;
    assert_eq!((s, code(&v)), (409, Some("skill_exists")), "{v}");
    // Another workspace may use the same name.
    let ws2 = r.h.workspace(r.org).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM skills WHERE name = 'dup'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 3);
    raw(&r, None, Some(ws2), "dup", "d", true).await.unwrap();
    // Reserved and invalid names: 400 invalid_request at every tier.
    for uri in [
        ws_uri.clone(),
        org_uri.clone(),
        format!("/v1/agents/{a}/skills"),
    ] {
        for bad in ["aurora-x", "aurora", "Not_Kebab", "a--b", ""] {
            let (s, v) = r
                .call(
                    &org_admin,
                    Method::POST,
                    &uri,
                    Some(json!({"name": bad, "description": "d", "body": "b"})),
                )
                .await;
            let (s2, v2) = if s == 404 {
                // the org admin has no workspace role: use the owner for workspace/agent URIs
                r.user(
                    Method::POST,
                    &uri,
                    Some(json!({"name": bad, "description": "d", "body": "b"})),
                )
                .await
            } else {
                (s, v)
            };
            assert_eq!(
                (s2, code(&v2)),
                (400, Some("invalid_request")),
                "{uri} {bad}: {v2}"
            );
        }
    }
    // CHECK: an agent skill never carries a workspace id.
    let e = raw(&r, Some(a), Some(r.ws), "both", "d", true).await;
    assert!(e.is_err(), "agent+workspace must violate the CHECK");
    // Org-tier uniqueness is still enforced by the replaced index.
    let e = raw(&r, None, None, "dup", "d", true).await;
    assert!(e.is_err());
    // Oversized body at the workspace tier: 422 skill_too_large is checked by files below.
    let big = "x".repeat(aurora_core::limits::SKILL_BODY_MAX + 1);
    let (s, v) = r
        .user(
            Method::POST,
            &ws_uri,
            Some(json!({"name": "big", "description": "d", "body": big})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
}

/// Authorization matrix per tier: list / create / edit / delete / files.
#[tokio::test]
async fn workspace_skill_authorization_matrix() {
    let r = rig().await;
    let owner = r.cookie.clone(); // ws owner (+ org member)
    let member = actor(&r, "m@x.io", "member", Some("member")).await;
    let org_admin = actor(&r, "adm@x.io", "admin", None).await;
    let org_owner = actor(&r, "own@x.io", "owner", None).await;
    let outsider = {
        let org2 = r.h.org().await;
        let u = r.h.user("out@x.io", false).await;
        r.h.member(org2, u, "owner", "active").await;
        let (_, st) = r.h.good_session(u).await;
        cookie(&st)
    };
    let base = format!("/v1/workspaces/{}/skills", r.ws);
    let body = json!({"name": "deploy-checklist", "description": "d", "body": "b"});

    // Plain member: reads, never writes.
    let (s, v) = r
        .call(&member, Method::POST, &base, Some(body.clone()))
        .await;
    assert_eq!((s, code(&v)), (403, Some("forbidden")), "{v}");
    assert_eq!(r.call(&member, Method::GET, &base, None).await.0, 200);
    // Org owner/admin write as `can_own`; the ws owner too.
    let (s, v) = r
        .call(&org_admin, Method::POST, &base, Some(body.clone()))
        .await;
    assert_eq!(s, 201, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let (s, _) = r
        .call(
            &org_owner,
            Method::POST,
            &base,
            Some(json!({"name": "second", "description": "d", "body": "b"})),
        )
        .await;
    assert_eq!(s, 201);
    // Item routes.
    let item = format!("/v1/skills/{id}");
    assert_eq!(r.call(&member, Method::GET, &item, None).await.0, 200);
    for (m, b) in [
        (Method::PATCH, Some(json!({"body": "x"}))),
        (Method::DELETE, None),
    ] {
        let (s, v) = r.call(&member, m, &item, b).await;
        assert_eq!((s, code(&v)), (403, Some("forbidden")), "{v}");
    }
    let (s, v) = r
        .raw_put(&member, &format!("{item}/files/a.txt"), b"x")
        .await;
    assert_eq!((s, code(&v)), (403, Some("forbidden")), "{v}");
    let (s, v) = r
        .call(
            &member,
            Method::DELETE,
            &format!("{item}/files/a.txt"),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (403, Some("forbidden")), "{v}");
    // Invisible workspace: 404 on every route (the outsider is in another org).
    assert_eq!(r.call(&outsider, Method::GET, &base, None).await.0, 404);
    assert_eq!(
        r.call(&outsider, Method::POST, &base, Some(body.clone()))
            .await
            .0,
        404
    );
    assert_eq!(r.call(&outsider, Method::GET, &item, None).await.0, 404);
    assert_eq!(
        r.call(&outsider, Method::PATCH, &item, Some(json!({"body": "x"})))
            .await
            .0,
        404
    );
    // Owner and org admin edit, upload files, delete.
    let (s, v) = r
        .call(
            &owner,
            Method::PATCH,
            &item,
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["enabled"], false);
    let (s, _) = r
        .raw_put(&org_admin, &format!("{item}/files/run.sh"), b"#!/bin/sh")
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        r.call(
            &org_admin,
            Method::DELETE,
            &format!("{item}/files/run.sh"),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(r.call(&org_admin, Method::DELETE, &item, None).await.0, 204);
    assert_eq!(r.call(&owner, Method::GET, &item, None).await.0, 404);

    // Audit rows carry the skill's workspace id and the tier.
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'skill.create' AND workspace_id = $1
           AND metadata->>'scope' = 'workspace'",
    )
    .bind(r.ws)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(n >= 1, "workspace skill.create audit rows: {n}");
    // Org and agent tiers keep their rules: member cannot create org skills; ws member can
    // create agent skills (can_use).
    let a = r.agent("auth1").await;
    let (s, _) = r
        .call(
            &member,
            Method::POST,
            &format!("/v1/orgs/{}/skills", r.org),
            Some(body.clone()),
        )
        .await;
    assert_eq!(s, 403);
    let (s, _) = r
        .call(
            &member,
            Method::POST,
            &format!("/v1/agents/{a}/skills"),
            Some(body.clone()),
        )
        .await;
    assert_eq!(s, 201);
}

/// Agent tools never write workspace/org tiers; `skill_list` shows tier and `shadows`.
#[tokio::test]
async fn agent_tools_cannot_write_workspace_or_org_and_list_shows_tier_and_shadows() {
    let r = rig().await;
    let b = r.booted("t1").await;
    let ws_skill = raw(&r, None, Some(r.ws), "wsonly", "From ws", true)
        .await
        .unwrap();
    raw(&r, None, None, "orgonly", "From org", true)
        .await
        .unwrap();
    raw(&r, None, Some(r.ws), "both", "ws both", true)
        .await
        .unwrap();
    raw(&r, None, None, "both", "org both", true).await.unwrap();
    raw(&r, Some(b.agent), None, "mine-over-ws", "agent", true)
        .await
        .unwrap();
    raw(&r, None, Some(r.ws), "mine-over-ws", "ws", true)
        .await
        .unwrap();
    raw(&r, None, None, "mine-over-ws", "org", true)
        .await
        .unwrap();
    raw(&r, Some(b.agent), None, "off", "my disabled", false)
        .await
        .unwrap();

    let v = r.tool(&b, "skill_list", json!({})).await;
    let skills = v["content"][0]["value"]["skills"]
        .as_array()
        .unwrap()
        .clone();
    let find = |n: &str| skills.iter().find(|s| s["name"] == n).cloned().unwrap();
    assert_eq!(find("wsonly")["tier"], "workspace");
    assert_eq!(find("wsonly")["scope"], "workspace");
    assert_eq!(find("wsonly")["shadows"], json!([]));
    assert_eq!(find("orgonly")["tier"], "org");
    assert_eq!(find("orgonly")["scope"], "organization");
    // `both`: the workspace skill wins and appears once.
    assert_eq!(skills.iter().filter(|s| s["name"] == "both").count(), 1);
    assert_eq!(find("both")["tier"], "workspace");
    // An agent skill names the tiers it overrides.
    assert_eq!(find("mine-over-ws")["tier"], "agent");
    assert_eq!(find("mine-over-ws")["shadows"], json!(["workspace", "org"]));
    // The agent's own disabled skill stays listed so it can re-enable it.
    assert_eq!(find("off")["enabled"], false);

    // skill_get resolves in the effective set and reports the tier.
    let v = r.tool(&b, "skill_get", json!({"name": "wsonly"})).await;
    assert_eq!(v["content"][0]["value"]["tier"], "workspace");
    assert_eq!(v["content"][0]["value"]["body"], "body");
    let v = r.tool(&b, "skill_get", json!({"name": "both"})).await;
    assert_eq!(v["content"][0]["value"]["description"], "ws both");

    // Workspace and org skills are read-only for the agent: tool error, nothing changes.
    for name in ["wsonly", "orgonly"] {
        for (t, a) in [
            (
                "skill_update",
                json!({"name": name, "body": "x", "enabled": false}),
            ),
            ("skill_delete", json!({"name": name})),
        ] {
            let v = r.tool(&b, t, a).await;
            assert_eq!(v["ok"], false, "{t} {name}: {v}");
            let msg = v["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            assert!(msg.contains("read-only; ask an owner"), "{msg}");
        }
    }
    let msg = r.tool(&b, "skill_delete", json!({"name": "wsonly"})).await["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(msg.contains("workspace skill"), "{msg}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM skills WHERE name IN ('wsonly','orgonly') AND body = 'body'
           AND enabled",
    )
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 2);
    let _ = ws_skill;
    // skill_create over a workspace name makes an agent skill that shadows it.
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "wsonly", "description": "mine", "body": "mine"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    let v = r.tool(&b, "skill_list", json!({})).await;
    let skills = v["content"][0]["value"]["skills"]
        .as_array()
        .unwrap()
        .clone();
    let w = skills.iter().find(|s| s["name"] == "wsonly").unwrap();
    assert_eq!(
        (w["tier"].as_str(), w["shadows"].clone()),
        (Some("agent"), json!(["workspace"]))
    );
}

/// Projection and generation bumps: every agent of the workspace (and only it).
#[tokio::test]
async fn projection_reaches_workspace_agents_only_read_only_and_pruned() {
    let r = rig().await;
    let b1 = r.booted("p1").await;
    let b2 = r.booted("p2").await;
    let ws2 = r.h.workspace(r.org).await;
    let other = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,'other','other','hosted','fake')",
    )
    .bind(other)
    .bind(r.org)
    .bind(ws2)
    .execute(r.h.pool())
    .await
    .unwrap();
    let before = (
        desired(&r, b1.agent).await,
        desired(&r, b2.agent).await,
        desired(&r, other).await,
    );
    let wsg = ws_gen(&r, r.ws).await;
    let (s, v) = r
        .user(
            Method::POST,
            &format!("/v1/workspaces/{}/skills", r.ws),
            Some(json!({"name": "deploy", "description": "Deploy checklist", "body": "steps"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    // One bump of the workspace generation reaches every agent of it, no other workspace.
    assert_eq!(ws_gen(&r, r.ws).await, wsg + 1);
    assert_eq!(desired(&r, b1.agent).await, before.0 + 1);
    assert_eq!(desired(&r, b2.agent).await, before.1 + 1);
    assert_eq!(desired(&r, other).await, before.2);
    // File edits and enable/disable bump too.
    let item = format!("/v1/skills/{id}");
    assert_eq!(
        r.raw_put(
            &r.cookie,
            &format!("{item}/files/run.sh?executable=true"),
            b"#!/bin/sh"
        )
        .await
        .0,
        200
    );
    assert_eq!(desired(&r, b1.agent).await, before.0 + 2);
    assert_eq!(desired(&r, other).await, before.2);

    let render = |b: &Booted| {
        let claim = aurora_db::repo::hosted::HomeSyncClaim {
            sandbox_id: b.sandbox,
            boot_id: b.boot,
            agent_id: b.agent,
            generation: 1,
            harness_id: Some("fake".into()),
        };
        let r = &r;
        async move {
            let mut conn = r.h.pool().acquire().await.unwrap();
            let parts = aurora_api::domains::hosted::home::render(&r.h.state, &mut conn, &claim)
                .await
                .unwrap();
            let kind = |k: &str| -> Vec<Value> {
                parts
                    .iter()
                    .filter(|p| p.part.kind.as_str() == k)
                    .flat_map(|p| p.part.entries.clone())
                    .collect()
            };
            (kind("skills"), kind("manifest"))
        }
    };
    for b in [&b1, &b2] {
        let (entries, manifest) = render(b).await;
        let md = entries
            .iter()
            .find(|e| e["path"] == ".claude/skills/deploy/SKILL.md")
            .unwrap_or_else(|| panic!("workspace skill not projected: {entries:?}"));
        assert!(md["content"].as_str().unwrap().contains("Deploy checklist"));
        let run = entries
            .iter()
            .find(|e| e["path"] == ".claude/skills/deploy/run.sh")
            .unwrap();
        assert_eq!(run["exec"], true);
        for p in [
            ".claude/skills/deploy/SKILL.md",
            ".claude/skills/deploy/run.sh",
        ] {
            assert!(manifest.iter().any(|m| m["path"] == p), "{p} in manifest");
        }
    }
    // An agent skill of the same name shadows it: the workspace skill is not projected.
    raw(&r, Some(b1.agent), None, "deploy", "Agent deploy", true)
        .await
        .unwrap();
    let (entries, _) = render(&b1).await;
    let md: Vec<&Value> = entries
        .iter()
        .filter(|e| e["path"] == ".claude/skills/deploy/SKILL.md")
        .collect();
    assert_eq!(md.len(), 1);
    assert!(md[0]["content"].as_str().unwrap().contains("Agent deploy"));
    // ... and b2 still gets the workspace one.
    let (entries2, _) = render(&b2).await;
    assert!(entries2.iter().any(|e| {
        e["content"]
            .as_str()
            .is_some_and(|c| c.contains("Deploy checklist"))
    }));
    // A disabled workspace skill drops out; deleting it prunes by manifest (no entry, no path).
    let (s, _) = r
        .user(Method::PATCH, &item, Some(json!({"enabled": false})))
        .await;
    assert_eq!(s, 200);
    let (entries2, manifest2) = render(&b2).await;
    assert!(
        !entries2
            .iter()
            .any(|e| e["path"].as_str().unwrap().contains("/deploy/"))
    );
    assert!(
        !manifest2
            .iter()
            .any(|m| m["path"].as_str().unwrap().contains("/deploy/"))
    );
    assert_eq!(r.user(Method::DELETE, &item, None).await.0, 204);
    let (entries2, manifest2) = render(&b2).await;
    assert!(
        !entries2
            .iter()
            .any(|e| e["path"].as_str().unwrap().contains("/deploy/"))
    );
    assert!(
        !manifest2
            .iter()
            .any(|m| m["path"].as_str().unwrap().contains("/deploy/"))
    );
    // The other workspace's agent never had it.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM skills WHERE workspace_id = $1")
        .bind(ws2)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

/// Deleting a workspace deletes its skills and files (cascade); `overridden_in` lists the
/// agents whose own enabled skill overrides a workspace skill.
#[tokio::test]
async fn workspace_delete_cascades_and_overridden_in() {
    let r = rig().await;
    let a = r.agent("c1").await;
    let a2 = r.agent("c2").await;
    let (s, v) = r
        .user(
            Method::POST,
            &format!("/v1/workspaces/{}/skills", r.ws),
            Some(json!({"name": "shared", "description": "d", "body": "b"})),
        )
        .await;
    assert_eq!(s, 201);
    assert_eq!(v["overridden_in"], json!([]));
    let id = v["id"].as_str().unwrap().to_owned();
    raw(&r, Some(a), None, "shared", "mine", true)
        .await
        .unwrap();
    raw(
        &r,
        Some(a2),
        None,
        "shared",
        "disabled does not override",
        false,
    )
    .await
    .unwrap();
    let (s, v) = r.user(Method::GET, &format!("/v1/skills/{id}"), None).await;
    assert_eq!(s, 200);
    let ov = v["overridden_in"].as_array().unwrap();
    assert_eq!(ov.len(), 1, "{v}");
    assert_eq!(ov[0]["agent"]["id"], json!(a));
    assert_eq!(ov[0]["agent"]["handle"], "c1");
    // Agent and org skills carry null.
    let (_, l) = r
        .user(Method::GET, &format!("/v1/agents/{a}/skills"), None)
        .await;
    assert!(l["items"][0]["overridden_in"].is_null());
    r.raw_put(&r.cookie, &format!("/v1/skills/{id}/files/f.txt"), b"x")
        .await;
    sqlx::query("DELETE FROM agents WHERE workspace_id = $1")
        .bind(r.ws)
        .execute(r.h.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(r.ws)
        .execute(r.h.pool())
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM skills WHERE workspace_id = $1)
              + (SELECT count(*) FROM skill_files WHERE skill_id = $2)",
    )
    .bind(r.ws)
    .bind(id.parse::<Uuid>().unwrap())
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0);
}
