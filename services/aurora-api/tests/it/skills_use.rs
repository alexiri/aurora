#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-SKILLS-1 (C-1173..C-1175, D-498, D-499, D-505, SEC-158): a skill's display name, turning
//! an inherited skill off for one agent (one rule for the effective set, the projected files,
//! the agent's tools and the fingerprint) and what counts as a use.

use super::*;
use crate::hosted;
use aurora_db::repo::hosted::HomeSyncClaim;
use std::collections::BTreeSet;

/// A stored skill at any tier: `(agent, workspace)`, both none => organization.
async fn put_skill(
    r: &Rig,
    org: Uuid,
    agent: Option<Uuid>,
    ws: Option<Uuid>,
    name: &str,
    enabled: bool,
) -> Uuid {
    insert_skill(r.h.pool(), org, agent, ws, name, enabled).await
}

async fn insert_skill(
    pool: &sqlx::PgPool,
    org: Uuid,
    agent: Option<Uuid>,
    ws: Option<Uuid>,
    name: &str,
    enabled: bool,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skills (id, org_id, agent_id, workspace_id, name, description, body, enabled,
            created_by_kind) VALUES ($1,$2,$3,$4,$5,'d','body of ' || $5,$6,'user')",
    )
    .bind(id)
    .bind(org)
    .bind(agent)
    .bind(ws)
    .bind(name)
    .bind(enabled)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// A person of the organization with a workspace role on the rig's workspace (or none).
async fn person(r: &Rig, email: &str, org_role: &str, ws_role: Option<&str>) -> (Uuid, String) {
    let u = r.h.user(email, false).await;
    r.h.member(r.org, u, org_role, "active").await;
    if let Some(role) = ws_role {
        r.h.bind_ws(r.ws, u, role).await;
    }
    let (_, st) = r.h.good_session(u).await;
    (u, cookie(&st))
}

async fn effective(r: &Rig, agent: Uuid) -> Value {
    let (s, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{agent}/skills/effective"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    v
}

fn item<'a>(v: &'a Value, list: &str, name: &str) -> Option<&'a Value> {
    v[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["skill"]["name"] == name)
}

fn off_uri(agent: Uuid, skill: Uuid) -> String {
    format!("/v1/agents/{agent}/skills/{skill}/off")
}

/// The skill folders the agent's computer is sent.
async fn projected(r: &Rig, agent: Uuid) -> BTreeSet<String> {
    let mut c = r.h.pool().acquire().await.unwrap();
    let claim = HomeSyncClaim {
        sandbox_id: Uuid::now_v7(),
        boot_id: Uuid::now_v7(),
        agent_id: agent,
        generation: 1,
        harness_id: Some("claude-code".into()),
    };
    aurora_api::domains::hosted::home::render(&r.h.state, &mut c, &claim)
        .await
        .unwrap()
        .iter()
        .filter(|s| s.part.kind.as_str() == "skills")
        .flat_map(|s| s.part.entries.iter())
        .filter_map(|e| {
            e["path"]
                .as_str()?
                .strip_prefix(".claude/skills/")?
                .strip_suffix("/SKILL.md")
                .map(str::to_owned)
        })
        .collect()
}

async fn generation(r: &Rig, agent: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT a.home_sync_generation + w.home_sync_generation FROM agents a
         JOIN workspaces w ON w.id = a.workspace_id WHERE a.id = $1",
    )
    .bind(agent)
    .fetch_one(r.h.pool())
    .await
    .unwrap()
}

async fn count(pool: &sqlx::PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn fingerprint_skills(r: &Rig, agent: Uuid) -> Vec<String> {
    let row = aurora_db::repo::agents::get(r.h.pool(), agent)
        .await
        .unwrap()
        .unwrap();
    let f = aurora_api::domains::evals::service::current_fingerprint(&r.h.state, &row)
        .await
        .unwrap();
    f.fingerprint["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_owned())
        .collect()
}

// ---- display name (C-1173) -------------------------------------------------------------------

#[tokio::test]
async fn title_is_trimmed_validated_kept_and_cleared_at_every_tier() {
    let r = rig().await;
    let (_, admin) = person(&r, "adm@x.io", "admin", None).await;
    let a = r.agent("daneel").await;
    for (uri, who) in [
        (format!("/v1/agents/{a}/skills"), r.cookie.clone()),
        (format!("/v1/workspaces/{}/skills", r.ws), r.cookie.clone()),
        (format!("/v1/orgs/{}/skills", r.org), admin.clone()),
    ] {
        let body = |name: &str, title: Value| json!({"name": name, "title": title, "description": "d", "body": "b"});
        let (s, v) = r
            .call(
                &who,
                Method::POST,
                &uri,
                Some(body("summarize-pr", json!("  Summarize pull requests  "))),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        assert_eq!(v["title"], "Summarize pull requests", "{uri}");
        assert_eq!(v["name"], "summarize-pr");
        let item_uri = format!("/v1/skills/{}", v["id"].as_str().unwrap());
        // Absent keeps; `""` clears; the folder name never changes.
        let (s, v) = r
            .call(
                &who,
                Method::PATCH,
                &item_uri,
                Some(json!({"description": "e"})),
            )
            .await;
        assert_eq!((s, &v["title"]), (200, &json!("Summarize pull requests")));
        let (s, v) = r.call(&who, Method::GET, &uri, None).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["items"][0]["title"], "Summarize pull requests");
        let (s, v) = r
            .call(&who, Method::PATCH, &item_uri, Some(json!({"title": ""})))
            .await;
        assert_eq!((s, &v["title"]), (200, &Value::Null), "{v}");
        // Blank on create means none.
        let (s, v) = r
            .call(&who, Method::POST, &uri, Some(body("blank", json!("   "))))
            .await;
        assert_eq!((s, &v["title"]), (201, &Value::Null), "{v}");
        for bad in [json!("x".repeat(81)), json!("two\nlines"), json!("a\tb")] {
            let (s, v) = r
                .call(&who, Method::POST, &uri, Some(body("bad", bad.clone())))
                .await;
            assert_eq!(s, 400, "{bad}: {v}");
            assert_eq!(v["error"]["details"]["fields"][0]["path"], "/title", "{v}");
            let (s, v) = r
                .call(&who, Method::PATCH, &item_uri, Some(json!({"title": bad})))
                .await;
            assert_eq!(s, 400, "{v}");
        }
        // 80 characters (not bytes) fit.
        let (s, v) = r
            .call(
                &who,
                Method::PATCH,
                &item_uri,
                Some(json!({"title": "é".repeat(80)})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }
    // The effective set carries it.
    let (s, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{a}/skills"),
            Some(json!({"name": "named", "title": "Named one", "description": "d", "body": "b"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let v = effective(&r, a).await;
    assert_eq!(
        item(&v, "items", "named").unwrap()["skill"]["title"],
        "Named one"
    );
}

#[tokio::test]
async fn the_agent_skill_tools_take_and_show_the_title() {
    let r = rig().await;
    let b = r.booted("daneel").await;
    let out = r
        .tool(
            &b,
            "skill_create",
            json!({"name": "triage-alerts", "title": "Triage alerts",
                   "description": "d", "body": "b"}),
        )
        .await;
    assert_eq!(out["ok"], true, "{out}");
    let list = r.tool(&b, "skill_list", json!({})).await;
    let skills = list["content"][0]["value"]["skills"].as_array().unwrap();
    assert_eq!(skills[0]["title"], "Triage alerts", "{list}");
    let out = r
        .tool(
            &b,
            "skill_update",
            json!({"name": "triage-alerts", "title": "Triage the alerts"}),
        )
        .await;
    assert_eq!(
        out["content"][0]["value"]["title"], "Triage the alerts",
        "{out}"
    );
    let out = r
        .tool(
            &b,
            "skill_update",
            json!({"name": "triage-alerts", "title": "x".repeat(81)}),
        )
        .await;
    assert_eq!(out["ok"], false, "{out}");
}

// ---- off for one agent (C-1174) --------------------------------------------------------------

#[tokio::test]
async fn off_for_one_agent_leaves_the_others_and_is_audited_once() {
    let r = rig().await;
    let daneel = r.booted("daneel").await;
    let other = r.agent("giskard").await;
    let hire = put_skill(&r, r.org, None, None, "onboarding-new-hire", true).await;
    put_skill(&r, r.org, None, Some(r.ws), "meeting-minutes", true).await;
    assert!(
        projected(&r, daneel.agent)
            .await
            .contains("onboarding-new-hire")
    );
    assert!(
        fingerprint_skills(&r, daneel.agent)
            .await
            .contains(&"onboarding-new-hire".to_owned())
    );
    let (g_daneel, g_other) = (
        generation(&r, daneel.agent).await,
        generation(&r, other).await,
    );

    let (s, v) = r
        .user(Method::PUT, &off_uri(daneel.agent, hire), None)
        .await;
    assert_eq!(s, 204, "{v}");
    // The home of this agent alone is synced again.
    assert!(generation(&r, daneel.agent).await > g_daneel);
    assert_eq!(generation(&r, other).await, g_other);
    // Effective set: gone from `items`, in `shadowed` as off for this agent.
    let v = effective(&r, daneel.agent).await;
    assert!(item(&v, "items", "onboarding-new-hire").is_none(), "{v}");
    let off = item(&v, "shadowed", "onboarding-new-hire").unwrap();
    assert_eq!(off["off_for_agent"], true);
    assert_eq!(off["reason"], "off_for_agent");
    assert_eq!(off["enabled"], true, "the skill itself stays on");
    assert_eq!(off["shadowed_by"], Value::Null);
    assert_eq!(
        item(&v, "items", "meeting-minutes").unwrap()["off_for_agent"],
        false
    );
    // The projected folder, the tools and the fingerprint agree.
    assert!(
        !projected(&r, daneel.agent)
            .await
            .contains("onboarding-new-hire")
    );
    assert!(
        projected(&r, daneel.agent)
            .await
            .contains("meeting-minutes")
    );
    assert!(
        !fingerprint_skills(&r, daneel.agent)
            .await
            .contains(&"onboarding-new-hire".to_owned())
    );
    let list = r.tool(&daneel, "skill_list", json!({})).await;
    let names: Vec<&str> = list["content"][0]["value"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert_eq!(names, ["meeting-minutes"], "{list}");
    let got = r
        .tool(&daneel, "skill_get", json!({"name": "onboarding-new-hire"}))
        .await;
    assert_eq!(got["ok"], false, "{got}");
    // Another agent of the workspace still has it.
    assert!(projected(&r, other).await.contains("onboarding-new-hire"));
    let v = effective(&r, other).await;
    assert!(item(&v, "items", "onboarding-new-hire").is_some(), "{v}");
    // The skill's own row is untouched.
    let (_, v) = r
        .user(Method::GET, &format!("/v1/orgs/{}/skills", r.org), None)
        .await;
    assert_eq!(v["items"][0]["enabled"], true, "{v}");

    // Idempotent both ways: one audit row per real change, one sync bump.
    let g = generation(&r, daneel.agent).await;
    assert_eq!(
        r.user(Method::PUT, &off_uri(daneel.agent, hire), None)
            .await
            .0,
        204
    );
    assert_eq!(generation(&r, daneel.agent).await, g);
    assert_eq!(
        r.user(Method::DELETE, &off_uri(daneel.agent, hire), None)
            .await
            .0,
        204
    );
    assert!(generation(&r, daneel.agent).await > g);
    assert_eq!(
        r.user(Method::DELETE, &off_uri(daneel.agent, hire), None)
            .await
            .0,
        204
    );
    assert!(
        projected(&r, daneel.agent)
            .await
            .contains("onboarding-new-hire")
    );
    for action in ["skill.agent_off", "skill.agent_on"] {
        let rows: Vec<(bool, Value)> =
            sqlx::query_as("SELECT critical, metadata FROM audit_log WHERE action = $1")
                .bind(action)
                .fetch_all(r.h.pool())
                .await
                .unwrap();
        assert_eq!(rows.len(), 1, "{action}");
        assert!(rows[0].0, "{action} commits with the change");
        assert_eq!(rows[0].1["agent_id"], json!(daneel.agent), "{action}");
        assert_eq!(rows[0].1["name"], "onboarding-new-hire");
    }
    // Deleting the skill or the agent takes the switch with it.
    r.user(Method::PUT, &off_uri(daneel.agent, hire), None)
        .await;
    sqlx::query("DELETE FROM skills WHERE id = $1")
        .bind(hire)
        .execute(r.h.pool())
        .await
        .unwrap();
    assert_eq!(
        count(r.h.pool(), "SELECT count(*) FROM agent_skills_off").await,
        0
    );
}

#[tokio::test]
async fn off_applies_only_to_inherited_skills_that_reach_the_agent() {
    let r = rig().await;
    let daneel = r.agent("daneel").await;
    let other = r.agent("giskard").await;
    let own = put_skill(&r, r.org, Some(daneel), None, "own-one", true).await;
    let others_own = put_skill(&r, r.org, Some(other), None, "others", true).await;
    let ws2 = r.h.workspace(r.org).await;
    let other_ws = put_skill(&r, r.org, None, Some(ws2), "elsewhere", true).await;
    let org2 = r.h.org().await;
    let foreign = put_skill(&r, org2, None, None, "foreign-org-skill", true).await;
    let ws_skill = put_skill(&r, r.org, None, Some(r.ws), "ws-one", true).await;

    for m in [Method::PUT, Method::DELETE] {
        let (s, v) = r.user(m.clone(), &off_uri(daneel, own), None).await;
        assert_eq!((s, code(&v)), (409, Some("skill_not_inherited")), "{m} {v}");
        // Whatever does not reach the agent is the same 404 as an unknown id.
        let mut messages = BTreeSet::new();
        for id in [others_own, other_ws, foreign, Uuid::now_v7()] {
            let (s, v) = r.user(m.clone(), &off_uri(daneel, id), None).await;
            assert_eq!((s, code(&v)), (404, Some("not_found")), "{m} {v}");
            messages.insert(v["error"]["message"].as_str().unwrap().to_owned());
        }
        assert_eq!(messages.len(), 1, "{messages:?}");
    }
    assert_eq!(
        count(r.h.pool(), "SELECT count(*) FROM agent_skills_off").await,
        0
    );

    // Who may: whoever may edit the agent (a plain member of its workspace, like its other
    // skill writes); never an administrator without own access, a colleague outside the
    // workspace or another organization.
    let (_, member) = person(&r, "m@x.io", "member", Some("member")).await;
    let (_, org_admin) = person(&r, "adm@x.io", "admin", None).await;
    let (_, unbound) = person(&r, "u@x.io", "member", None).await;
    let out = r.h.user("out@x.io", false).await;
    r.h.member(org2, out, "owner", "active").await;
    let outsider = cookie(&r.h.good_session(out).await.1);
    for (who, want) in [
        (&org_admin, (403, Some("workspace_access_required"))),
        (&unbound, (404, Some("not_found"))),
        (&outsider, (404, Some("not_found"))),
    ] {
        let (s, v) = r
            .call(who, Method::PUT, &off_uri(daneel, ws_skill), None)
            .await;
        assert_eq!((s, code(&v)), want, "{v}");
    }
    assert_eq!(
        count(r.h.pool(), "SELECT count(*) FROM agent_skills_off").await,
        0
    );
    let (s, v) = r
        .call(&member, Method::PUT, &off_uri(daneel, ws_skill), None)
        .await;
    assert_eq!(s, 204, "{v}");
    // An outsider cannot turn it back on either.
    let (s, _) = r
        .call(&outsider, Method::DELETE, &off_uri(daneel, ws_skill), None)
        .await;
    assert_eq!(s, 404);
    assert_eq!(
        count(r.h.pool(), "SELECT count(*) FROM agent_skills_off").await,
        1
    );
}

/// The effective set, the projected folders and the agent's skill tool come from one rule:
/// every combination of agent (absent, on, off by owners), workspace and organization (absent,
/// on, off by owners, off for the agent, both) of one name, and a lower tier applies when the
/// higher one is off for the agent.
#[tokio::test]
async fn effective_set_and_projection_agree_over_every_switch_combination() {
    let r = rig().await;
    let b = r.booted("daneel").await;
    // (stored, enabled, off for the agent)
    let own_states = [None, Some((true, false)), Some((false, false))];
    let inherited = [
        None,
        Some((true, false)),
        Some((false, false)),
        Some((true, true)),
        Some((false, true)),
    ];
    let mut expected: Vec<(String, Option<Uuid>)> = Vec::new();
    for (i, own) in own_states.iter().enumerate() {
        for (j, ws) in inherited.iter().enumerate() {
            for (k, org) in inherited.iter().enumerate() {
                let name = format!("n{i}{j}{k}");
                let mut winner = None;
                for (state, agent, wsid) in [
                    (own, Some(b.agent), None),
                    (ws, None, Some(r.ws)),
                    (org, None, None),
                ] {
                    let Some((enabled, off)) = *state else {
                        continue;
                    };
                    let id = put_skill(&r, r.org, agent, wsid, &name, enabled).await;
                    if off {
                        sqlx::query(
                            "INSERT INTO agent_skills_off (agent_id, skill_id) VALUES ($1,$2)",
                        )
                        .bind(b.agent)
                        .bind(id)
                        .execute(r.h.pool())
                        .await
                        .unwrap();
                    }
                    if enabled && !off && winner.is_none() {
                        winner = Some(id);
                    }
                }
                expected.push((name, winner));
            }
        }
    }
    let v = effective(&r, b.agent).await;
    let got: std::collections::BTreeMap<String, String> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["skill"]["name"].as_str().unwrap().to_owned(),
                i["skill"]["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let want: std::collections::BTreeMap<String, String> = expected
        .iter()
        .filter_map(|(n, w)| Some((n.clone(), w.as_ref()?.to_string())))
        .collect();
    assert_eq!(got, want);
    // The projection's rows are the same skills (ids), and so are the rendered folders.
    let mut c = r.h.pool().acquire().await.unwrap();
    let rows = aurora_db::repo::hosted::skills(&mut c, r.org, r.ws, b.agent)
        .await
        .unwrap();
    drop(c);
    let proj: std::collections::BTreeMap<String, String> = rows
        .into_iter()
        .map(|s| (s.name, s.id.to_string()))
        .collect();
    assert_eq!(proj, want);
    assert_eq!(
        projected(&r, b.agent).await,
        want.keys().cloned().collect::<BTreeSet<_>>()
    );
}

// ---- usage (C-1175) --------------------------------------------------------------------------

async fn uses(pool: &sqlx::PgPool) -> Vec<(Uuid, Uuid, Option<Uuid>, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT skill_id, agent_id, conversation_id, turn_id FROM skill_uses ORDER BY at, skill_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn skill_get_counts_one_use_per_turn_and_only_of_the_skill_that_applies() {
    let r = rig().await;
    let b = r.booted("daneel").await;
    let org_skill = put_skill(&r, r.org, None, None, "style-guide", true).await;
    let own_off = put_skill(&r, r.org, Some(b.agent), None, "draft-email", false).await;
    sqlx::query("INSERT INTO skill_files (skill_id, path, content) VALUES ($1,'a.md','x')")
        .bind(org_skill)
        .execute(r.h.pool())
        .await
        .unwrap();
    // A file read is not a use; neither is listing.
    r.tool(
        &b,
        "skill_get",
        json!({"name": "style-guide", "path": "a.md"}),
    )
    .await;
    r.tool(&b, "skill_list", json!({})).await;
    assert_eq!(uses(r.h.pool()).await, []);
    // Reading the instructions is, once per turn.
    for _ in 0..2 {
        let out = r
            .tool(&b, "skill_get", json!({"name": "style-guide"}))
            .await;
        assert_eq!(out["ok"], true, "{out}");
    }
    assert_eq!(
        uses(r.h.pool()).await,
        [(org_skill, b.agent, Some(b.conv), Some(b.turn))]
    );
    // An own skill that is switched off can be read (to fix it) but is not used.
    let out = r
        .tool(&b, "skill_get", json!({"name": "draft-email"}))
        .await;
    assert_eq!(out["ok"], true, "{out}");
    assert!(uses(r.h.pool()).await.iter().all(|u| u.0 != own_off));
}

#[tokio::test]
async fn usage_week_last_used_by_agents_and_retention() {
    let r = rig().await;
    let daneel = r.agent("daneel").await;
    let giskard = r.agent("giskard").await;
    let skill = put_skill(&r, r.org, None, None, "style-guide", true).await;
    let unused = put_skill(&r, r.org, None, Some(r.ws), "unused", true).await;
    let add = |agent: Uuid, days: i32| {
        let pool = r.h.pool().clone();
        async move {
            sqlx::query(
                "INSERT INTO skill_uses (skill_id, agent_id, at)
                 VALUES ($1,$2, now() - make_interval(days => $3))",
            )
            .bind(skill)
            .bind(agent)
            .bind(days)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    add(daneel, 1).await;
    add(daneel, 2).await;
    add(daneel, 12).await;
    add(giskard, 29).await;
    add(giskard, 89).await;
    // An agent whose only use is older than the 30 days of "used by".
    add(r.agent("third").await, 31).await;
    let v = effective(&r, daneel).await;
    let u = &item(&v, "items", "style-guide").unwrap()["usage"];
    assert_eq!(u["week"], 2, "{u}");
    assert!(u["last"]["at"].is_string(), "{u}");
    assert_eq!(u["last"]["conversation"], Value::Null);
    assert_eq!(
        item(&v, "items", "unused").unwrap()["usage"],
        json!({"week": 0, "last": null})
    );
    assert_eq!(
        item(&v, "items", "style-guide").unwrap()["skill"]["used_by_agents"],
        2
    );
    // Giskard used it, but not this week.
    let v = effective(&r, giskard).await;
    let u = &item(&v, "items", "style-guide").unwrap()["usage"];
    assert_eq!(
        (u["week"].as_i64(), u["last"].is_object()),
        (Some(0), true),
        "{u}"
    );
    // Organization and workspace lists, and the one skill, say how many agents.
    for (uri, want) in [
        (format!("/v1/orgs/{}/skills", r.org), 2),
        (format!("/v1/workspaces/{}/skills", r.ws), 0),
    ] {
        let (s, v) = r.user(Method::GET, &uri, None).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["items"][0]["used_by_agents"], want, "{uri}");
    }
    let (_, v) = r
        .user(Method::GET, &format!("/v1/skills/{unused}"), None)
        .await;
    assert_eq!(v["used_by_agents"], 0);
    // An agent's own skill has no such count.
    put_skill(&r, r.org, Some(daneel), None, "own", true).await;
    let (_, v) = r
        .user(Method::GET, &format!("/v1/agents/{daneel}/skills"), None)
        .await;
    assert_eq!(v["items"][0]["used_by_agents"], Value::Null, "{v}");
    // The daily pass removes what is older than 90 days, and nothing else.
    add(giskard, 91).await;
    let report = aurora_api::jobs::run_retention_prune(r.h.pool())
        .await
        .unwrap();
    assert_eq!(report.skill_uses, 1);
    assert_eq!(
        count(r.h.pool(), "SELECT count(*) FROM skill_uses").await,
        6
    );
}

/// SEC-158: the last use names its conversation only to people who may read it.
#[tokio::test]
async fn the_last_use_names_a_conversation_only_to_its_readers() {
    let r = rig().await;
    let (bob, bob_cookie) = person(&r, "bob@x.io", "member", Some("member")).await;
    let (carol, carol_cookie) = person(&r, "carol@x.io", "member", Some("member")).await;
    let daneel = r.agent("daneel").await;
    let skill = put_skill(&r, r.org, None, None, "style-guide", true).await;
    let conv = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id,
             title, last_activity_at)
         VALUES ($1,$2,$3,'direct',$1::text,$4,'Bob''s private chat', now())",
    )
    .bind(conv)
    .bind(daneel)
    .bind(r.ws)
    .bind(bob)
    .execute(r.h.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO skill_uses (skill_id, agent_id, conversation_id) VALUES ($1,$2,$3)")
        .bind(skill)
        .bind(daneel)
        .bind(conv)
        .execute(r.h.pool())
        .await
        .unwrap();
    let uri = format!("/v1/agents/{daneel}/skills/effective");
    let last = |v: &Value| item(v, "items", "style-guide").unwrap()["usage"]["last"].clone();
    let named = json!({"id": conv, "title": "Bob's private chat"});
    // Bob (the person of the chat) and the workspace owner read it.
    for who in [&bob_cookie, &r.cookie] {
        let (s, v) = r.call(who, Method::GET, &uri, None).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(last(&v)["conversation"], named, "{v}");
    }
    // Carol, a member who cannot read Bob's chat, learns only that it was used.
    let (s, v) = r.call(&carol_cookie, Method::GET, &uri, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(last(&v)["conversation"], Value::Null, "{v}");
    assert!(last(&v)["at"].is_string());
    assert!(!v.to_string().contains("private chat"), "{v}");
    // The same with Carol's own API key.
    let (_, key) = r.h.api_key(carol, r.org, Some(r.ws), false).await;
    let (s, v) = r.h.get(&uri, &[("authorization", &bearer(&key))]).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(last(&v)["conversation"], Value::Null, "{v}");
    // A deleted conversation keeps the use and loses the name.
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(conv)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (_, v) = r.call(&bob_cookie, Method::GET, &uri, None).await;
    assert_eq!(last(&v)["conversation"], Value::Null, "{v}");
}

fn ev(n: u64, data: Value) -> Value {
    let mut e = data;
    e["kind"] = json!("tool.started");
    json!({"n": n, "event": e})
}

fn started(n: u64, call: &str, skill: &str) -> Value {
    ev(
        n,
        json!({"call_id": call, "name": "Skill", "summary": "Skill (1 args)", "skill": skill}),
    )
}

/// SEC-158: a use the supervisor reports counts for the agent and conversation of that turn,
/// never for anything the report names itself, and only for a skill that reaches that agent
/// after its switches.
#[tokio::test]
async fn a_reported_use_counts_for_the_turns_agent_and_only_for_a_skill_that_reaches_it() {
    let r = hosted::rig().await;
    let lv = r.boot("daneel").await;
    let stranger = r.agent("giskard", false).await;
    let pool = r.h.pool().clone();
    let mine = insert_skill(&pool, r.org, None, Some(r.ws), "pdf", true).await;
    let _lower = insert_skill(&pool, r.org, None, None, "release-notes", true).await;
    let upper = insert_skill(&pool, r.org, None, Some(r.ws), "release-notes", true).await;
    insert_skill(&pool, r.org, Some(stranger), None, "strangers-own", true).await;
    let org2 = r.h.org().await;
    insert_skill(&pool, org2, None, None, "foreign", true).await;
    // Daneel does not use the workspace's release notes.
    sqlx::query("INSERT INTO agent_skills_off (agent_id, skill_id) VALUES ($1,$2)")
        .bind(lv.agent)
        .bind(upper)
        .execute(&pool)
        .await
        .unwrap();
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    let batch = json!({"kind": "turn.events", "turn_id": turn, "events": [
        started(1, "c1", "pdf"),
        started(2, "c2", "pdf"),
        started(3, "c3", "release-notes"),
        started(4, "c4", "strangers-own"),
        started(5, "c5", "foreign"),
        started(6, "c6", "../etc"),
        ev(7, json!({"call_id": "c7", "name": "Bash", "summary": "Bash: ls"})),
        // Fields a report invents are ignored.
        ev(8, json!({"call_id": "c8", "name": "Skill", "summary": "s", "skill": "unknown",
                     "agent_id": stranger, "conversation_id": Uuid::now_v7()})),
    ]});
    assert_eq!(r.report(&lv, batch.clone()).await, "applied");
    assert_eq!(r.report(&lv, batch).await, "duplicate");
    let lower: Uuid = sqlx::query_scalar(
        "SELECT id FROM skills WHERE name = 'release-notes' AND workspace_id IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut got = uses(&pool).await;
    got.sort();
    let turn_id: Uuid = turn.parse().unwrap();
    let mut want = vec![
        (mine, lv.agent, Some(lv.conv), Some(turn_id)),
        (lower, lv.agent, Some(lv.conv), Some(turn_id)),
    ];
    want.sort();
    assert_eq!(got, want);
    // The transcript keeps its own shape: the skill field is not part of it.
    let evs = r.events(lv.conv).await;
    assert!(
        evs.iter()
            .filter(|(t, _)| t == "tool.started")
            .all(|(_, p)| p.get("skill").is_none()),
        "{evs:?}"
    );
    // A report about another agent's turn records nothing.
    sqlx::query("DELETE FROM skill_uses")
        .execute(&pool)
        .await
        .unwrap();
    let foreign_turn = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind,
             speaker_label, text)
         SELECT $1, id, $2, 'turn', 'done', '{}', 'system', 's', 'x' FROM conversations
         WHERE id = $3",
    )
    .bind(foreign_turn)
    .bind(stranger)
    .bind(lv.conv)
    .execute(&pool)
    .await
    .unwrap();
    let status = r
        .report(
            &lv,
            json!({"kind": "turn.events", "turn_id": foreign_turn,
                   "events": [started(1, "x1", "pdf")]}),
        )
        .await;
    assert_eq!(status, "invalid");
    assert_eq!(uses(&pool).await, []);
    // Work the harness does after the turn counts for that turn.
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": turn, "status": "done"}),
    )
    .await;
    let status = r
        .report(
            &lv,
            json!({"kind": "turn.idle.events", "turn_id": turn,
                   "events": [started(1, "i1", "pdf")]}),
        )
        .await;
    assert_eq!(status, "applied");
    assert_eq!(
        uses(&pool).await,
        [(mine, lv.agent, Some(lv.conv), Some(turn_id))]
    );
}
