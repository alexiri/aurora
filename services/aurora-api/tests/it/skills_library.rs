#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-GSL (D-148, SEC-83): agent tools and the home projection with library skills
//! (`18 §Git skill library`: read-only enforcement through every tool, projection exactly like
//! manual skills, a pending proposal changes nothing in the home).

use super::*;

/// A library row and one library-sourced skill of the agent (raw insert: the API paths are proven
/// in `skill_library.rs`).
async fn library_skill(r: &Rig, agent: Uuid, name: &str, body: &str) -> (Uuid, Uuid) {
    let lib = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skill_libraries (id, org_id, label, app_id, repo) VALUES ($1,$2,$3,'forge','acme/skills')",
    )
    .bind(lib)
    .bind(r.org)
    .bind(format!("lib-{name}"))
    .execute(r.h.pool())
    .await
    .unwrap();
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skills (id, org_id, agent_id, name, description, body, enabled, created_by_kind,
            library_id, library_label, library_repo, library_path, library_commit_sha, library_blobs,
            library_content_sha256, library_pinned_at)
         VALUES ($1,$2,$3,$4,'From the library',$5,true,'user',$6,'lib','acme/skills',$4,
                 repeat('a',40),'{\"SKILL.md\":\"b1\"}'::jsonb,'\\x00',now())",
    )
    .bind(id)
    .bind(r.org)
    .bind(agent)
    .bind(name)
    .bind(body)
    .bind(lib)
    .execute(r.h.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO skill_files (skill_id, path, content, executable) VALUES ($1,'run.sh','#!/bin/sh',true)")
        .bind(id)
        .execute(r.h.pool())
        .await
        .unwrap();
    (lib, id)
}

#[tokio::test]
async fn agent_tools_cannot_update_or_delete_library_skills_and_show_the_source() {
    let r = rig().await;
    let b = r.booted("lib1").await;
    let (_, id) = library_skill(&r, b.agent, "pinned", "library body").await;
    // skill_list and skill_get show the source
    let v = r.tool(&b, "skill_list", json!({})).await;
    let skills = v["content"][0]["value"]["skills"]
        .as_array()
        .unwrap()
        .clone();
    let s = skills.iter().find(|s| s["name"] == "pinned").unwrap();
    assert_eq!(s["source"]["kind"], "library");
    assert_eq!(s["source"]["repo"], "acme/skills");
    assert_eq!(s["source"]["detached"], false);
    let v = r.tool(&b, "skill_get", json!({"name": "pinned"})).await;
    assert_eq!(v["content"][0]["value"]["source"]["kind"], "library");
    // an ordinary skill has no source
    r.tool(
        &b,
        "skill_create",
        json!({"name": "mine", "description": "d", "body": "b"}),
    )
    .await;
    let v = r.tool(&b, "skill_get", json!({"name": "mine"})).await;
    assert!(v["content"][0]["value"]["source"].is_null());
    // update and delete are refused with the API's code, nothing changes
    for (t, a) in [
        ("skill_update", json!({"name": "pinned", "body": "x"})),
        ("skill_update", json!({"name": "pinned", "enabled": false})),
        ("skill_delete", json!({"name": "pinned"})),
    ] {
        let v = r.tool(&b, t, a).await;
        assert_eq!(v["ok"], false, "{t}: {v}");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("library_skill_readonly"),
            "{v}"
        );
    }
    let (body, enabled): (String, bool) =
        sqlx::query_as("SELECT body, enabled FROM skills WHERE id = $1")
            .bind(id)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!((body.as_str(), enabled), ("library body", true));
    // an agent cannot create a library-sourced skill: the tool has no such argument
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "sneaky", "description": "d", "body": "b",
                "from_library": {"library_id": Uuid::now_v7(), "path": "x", "commit_sha": "a".repeat(40)}}),
        )
        .await;
    assert_eq!(v["ok"], false, "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM skills WHERE name = 'sneaky'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

async fn rendered(r: &Rig, b: &Booted) -> Vec<Value> {
    let claim = aurora_db::repo::hosted::HomeSyncClaim {
        sandbox_id: b.sandbox,
        boot_id: b.boot,
        agent_id: b.agent,
        generation: 1,
        harness_id: Some("fake".into()),
    };
    let mut conn = r.h.pool().acquire().await.unwrap();
    aurora_api::domains::hosted::home::render(&r.h.state, &mut conn, &claim)
        .await
        .unwrap()
        .iter()
        .flat_map(|p| p.part.entries.clone())
        .collect()
}

#[tokio::test]
async fn library_skills_project_like_manual_ones_and_a_proposal_changes_nothing() {
    let r = rig().await;
    let b = r.booted("lib2").await;
    let (lib, id) = library_skill(&r, b.agent, "pinned", "library body").await;
    let entries = rendered(&r, &b).await;
    let find = |p: &str| entries.iter().find(|e| e["path"] == p).cloned();
    let md = find(".claude/skills/pinned/SKILL.md").expect("projected");
    assert_eq!(
        md["content"],
        "---\nname: pinned\ndescription: \"From the library\"\n---\n\nlibrary body"
    );
    assert_eq!(find(".claude/skills/pinned/run.sh").unwrap()["exec"], true);
    // a pending proposed update (and its generation) does not reach the home
    let gen_before = r.gen_of(b.agent).await;
    sqlx::query(
        "INSERT INTO skill_library_updates (id, skill_id, library_id, kind, status, commit_sha, proposed,
            content_sha256, bytes)
         VALUES ($1,$2,$3,'changed','pending','x',
            '{\"body\":\"NEW\",\"description\":\"NEW\",\"files\":[],\"blobs\":{}}'::jsonb,'\\x01',10)",
    )
    .bind(Uuid::now_v7())
    .bind(id)
    .bind(lib)
    .execute(r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        rendered(&r, &b).await,
        entries,
        "a proposal is invisible to the projection"
    );
    assert_eq!(r.gen_of(b.agent).await, gen_before);
    // a skill removed with its library (detached) still projects
    sqlx::query("UPDATE skills SET library_id = NULL, library_blobs = NULL WHERE id = $1")
        .bind(id)
        .execute(r.h.pool())
        .await
        .unwrap();
    assert_eq!(rendered(&r, &b).await, entries);
}
