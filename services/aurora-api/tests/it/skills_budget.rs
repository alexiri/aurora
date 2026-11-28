//! FIX-291: the skills a tier writes itself have a byte budget like its library skills (4 / 8 /
//! 16 MiB per agent / workspace / organization), counted the same way (instructions plus
//! files). A create or edit that would take the tier over it is refused; one that does not
//! grow it never is, so a tier already over can still be tidied.

use super::*;

const MIB: usize = 1024 * 1024;

/// One own skill whose instructions are `chars` long, written past the API's per-skill limits
/// to fill a tier up.
async fn fill(r: &Rig, agent: Option<Uuid>, ws: Option<Uuid>, chars: usize) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skills (id, org_id, agent_id, workspace_id, name, description, body, enabled,
                             created_by_kind)
         VALUES ($1,$2,$3,$4,'filler','d',repeat('x', $5),true,'user')",
    )
    .bind(id)
    .bind(r.org)
    .bind(agent)
    .bind(ws)
    .bind(i32::try_from(chars).unwrap())
    .execute(r.h.pool())
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn fix291_each_tiers_own_skills_have_a_byte_budget() {
    let r = rig().await;
    let a = r.agent("a1").await;
    for (agent, ws, uri, budget) in [
        (Some(a), None, format!("/v1/agents/{a}/skills"), 4 * MIB),
        (
            None,
            Some(r.ws),
            format!("/v1/workspaces/{}/skills", r.ws),
            8 * MIB,
        ),
        (None, None, format!("/v1/orgs/{}/skills", r.org), 16 * MIB),
    ] {
        fill(&r, agent, ws, budget - 100).await;
        let new = |name: &str, body: usize| {
            Some(json!({"name": name, "description": "d", "body": "b".repeat(body)}))
        };
        let (s, v) = r.user(Method::POST, &uri, new("over", 101)).await;
        assert_eq!(
            (s, code(&v)),
            (422, Some("own_skill_budget_exceeded")),
            "{uri}: {v}"
        );
        assert_eq!(v["error"]["details"]["bytes_limit"], budget, "{v}");
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.contains(&budget.to_string()), "{msg}");
        let (s, v) = r.user(Method::POST, &uri, new("fits", 60)).await;
        assert_eq!(s, 201, "{uri}: {v}");
        let id = v["id"].as_str().unwrap().to_owned();
        // Files and edits count like the instructions: 40 characters are left.
        let file = format!("/v1/skills/{id}/files/a.txt");
        let (s, v) = r.raw_put(&r.cookie, &file, &[b'a'; 41]).await;
        assert_eq!(
            (s, code(&v)),
            (422, Some("own_skill_budget_exceeded")),
            "{uri}: {v}"
        );
        let (s, v) = r.raw_put(&r.cookie, &file, &[b'a'; 40]).await;
        assert_eq!(s, 200, "{uri}: {v}");
        let (s, v) = r
            .user(
                Method::PATCH,
                &format!("/v1/skills/{id}"),
                Some(json!({"body": "b".repeat(61)})),
            )
            .await;
        assert_eq!(
            (s, code(&v)),
            (422, Some("own_skill_budget_exceeded")),
            "{uri}: {v}"
        );
        // A refusal writes nothing.
        let (len, files): (i32, i64) = sqlx::query_as(
            "SELECT length(body), (SELECT count(*) FROM skill_files f WHERE f.skill_id = s.id)
             FROM skills s WHERE id = $1::uuid",
        )
        .bind(&id)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
        assert_eq!((len, files), (60, 1));
    }
}

#[tokio::test]
async fn fix291_a_tier_over_its_budget_can_still_be_tidied_and_the_agent_is_told_why() {
    let r = rig().await;
    let b = r.booted("full").await;
    // Over already (as skills written before the budget, or a deleted library's, can be).
    let filler = fill(&r, Some(b.agent), None, 4 * MIB + 1_000).await;
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "mine", "description": "d", "body": "b"}),
        )
        .await;
    assert_eq!(v["ok"], false, "{v}");
    assert!(
        v.to_string().contains("this agent's own skills"),
        "the reason in words: {v}"
    );
    // Renaming, switching off and shrinking never grow the tier.
    let uri = format!("/v1/skills/{filler}");
    for patch in [
        json!({"title": "Old notes"}),
        json!({"enabled": false}),
        json!({"body": "short"}),
    ] {
        let (s, v) = r.user(Method::PATCH, &uri, Some(patch.clone())).await;
        assert_eq!(s, 200, "{patch}: {v}");
    }
    let v = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "mine", "description": "d", "body": "b"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
}
