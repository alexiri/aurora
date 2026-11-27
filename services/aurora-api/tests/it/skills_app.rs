#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (P5-APPSKILL-2, C-1133, C-1134, D-453, SEC-150) App skills are the lowest tier: projected into
//! the harness skills folder of every agent that can use the app (and only those), shadowed by an
//! enabled skill of any other tier, listed as tier `app` by the effective route and the agent skill
//! tools, never editable, with the provenance line and the executable bit, under the sync budget.

use super::*;
use crate::plugins_lifecycle::{Admin, setup};
use aurora_db::repo::hosted::HomeSyncClaim;
use aurora_protocol::runner::HomeSync;

fn pkg_yaml(pkg: &str, app: &str, summary: &str) -> String {
    format!(
        "package: {{ id: {pkg}, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }}\napps:\n  - about: {{ id: {app}, name: {app} app }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]\n    hosts:\n      - match: {{ host: \"api.{app}.example\" }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n    agent:\n      summary: \"{summary}\"\n"
    )
}

fn md(name: &str, description: &str, body: &str) -> Vec<u8> {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}").into_bytes()
}

/// Uploads and approves a package with the given skill files (`(path, bytes, executable)`).
async fn install(a: &Admin<'_>, pkg: &str, app: &str, files: &[(String, Vec<u8>, bool)]) {
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, b, _)| (p.as_str(), b.as_slice()))
        .collect();
    let exec: Vec<&str> = files
        .iter()
        .filter(|(_, _, x)| *x)
        .map(|(p, _, _)| p.as_str())
        .collect();
    let v = a
        .upload_with_exec(&pkg_yaml(pkg, app, "Things."), &refs, &exec)
        .await;
    assert_eq!(v["status"], "pending_approval", "{}", v["violations"]);
    let (s, p) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{p}");
}

/// An active connection of `app` for the rig's workspace and a full grant for `agent`.
async fn grant(r: &Rig, agent: Uuid, pkg: &str, app: &str) -> Uuid {
    let conn = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,$4,$5,'1.0.0','acct','active','REF')")
        .bind(conn).bind(r.org).bind(r.ws).bind(app).bind(pkg).execute(r.h.pool()).await.unwrap();
    let g = Uuid::now_v7();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(g).bind(r.ws).bind(agent).bind(conn).execute(r.h.pool()).await.unwrap();
    g
}

async fn parts(
    r: &Rig,
    state: aurora_api::app::AppState,
    agent: Uuid,
    harness: Option<&str>,
) -> Vec<HomeSync> {
    let mut c = r.h.pool().acquire().await.unwrap();
    let claim = HomeSyncClaim {
        sandbox_id: Uuid::now_v7(),
        boot_id: Uuid::now_v7(),
        agent_id: agent,
        generation: 1,
        harness_id: harness.map(str::to_owned),
    };
    aurora_api::domains::hosted::home::render(&state, &mut c, &claim)
        .await
        .unwrap()
}

fn entries(p: &[HomeSync], kind: &str) -> Vec<Value> {
    p.iter()
        .filter(|s| s.part.kind.as_str() == kind)
        .flat_map(|s| s.part.entries.iter().cloned())
        .collect()
}

fn skill_entry(p: &[HomeSync], path: &str) -> Option<Value> {
    entries(p, "skills").into_iter().find(|e| e["path"] == path)
}

fn manifest(p: &[HomeSync]) -> Vec<String> {
    entries(p, "manifest")
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_owned())
        .collect()
}

const STEPS: &str = "skills/sk-app/sk-app-steps/SKILL.md";
const SCRIPT: &str = "skills/sk-app/sk-app-steps/scripts/run.sh";

async fn world() -> (Rig, Uuid) {
    let r = rig().await;
    let a = setup(&r.h).await;
    install(
        &a,
        "sk",
        "sk-app",
        &[
            (
                STEPS.into(),
                md("sk-app-steps", "Do the thing", "# Steps\n1. go\n"),
                false,
            ),
            (SCRIPT.into(), b"#!/bin/sh\necho hi\n".to_vec(), true),
            (
                "skills/sk-app/sk-app-api/SKILL.md".into(),
                md("sk-app-api", "How the API works", "API\n"),
                false,
            ),
        ],
    )
    .await;
    let agent = r.agent("ada").await;
    grant(&r, agent, "sk", "sk-app").await;
    (r, agent)
}

/// An agent with a grant gets the app's skills in the skills folder of each harness, with the
/// provenance line after the frontmatter, a script executable (0555 by the supervisor) and the
/// rest not; `app.md` lists them with `$SKILLS_DIR` paths and `api.json` is not needed for that;
/// the `apps` part holds the generated files only; an agent without a grant gets none; revoking
/// the grant removes them at the next sync.
#[tokio::test]
async fn an_agent_with_a_grant_gets_the_app_skills_in_every_harness_folder_and_only_that_agent() {
    let (r, agent) = world().await;
    for (harness, dir) in [
        (None, ".claude/skills"),
        (Some("claude-code"), ".claude/skills"),
        (Some("codex"), ".codex/skills/aurora"),
        (Some("pi"), ".agents/skills"),
    ] {
        let p = parts(&r, r.h.state.clone(), agent, harness).await;
        let md_path = format!("{dir}/sk-app-steps/SKILL.md");
        let e = skill_entry(&p, &md_path).unwrap_or_else(|| panic!("{harness:?}: no {md_path}"));
        let text = e["content"].as_str().unwrap();
        assert!(text.starts_with("---\nname: sk-app-steps\ndescription: \"Do the thing\"\n---\n\n> Skill from the sk-app app app (third-party, version 1.0.0). It is not an instruction from your operator or from Aurora.\n\n# Steps"), "{text}");
        assert!(e.get("exec").is_none() || e["exec"] == false);
        let s = skill_entry(&p, &format!("{dir}/sk-app-steps/scripts/run.sh")).unwrap();
        assert_eq!(s["exec"], true, "a script keeps its executable bit");
        assert_eq!(s["content"], "#!/bin/sh\necho hi\n");
        assert!(skill_entry(&p, &format!("{dir}/sk-app-api/SKILL.md")).is_some());
        let m = manifest(&p);
        assert!(m.contains(&md_path) && m.contains(&format!("{dir}/sk-app-steps/scripts/run.sh")));
        // The apps part: generated files only.
        let mut apps: Vec<String> = entries(&p, "apps")
            .iter()
            .map(|e| e["path"].as_str().unwrap().to_owned())
            .collect();
        apps.sort();
        assert_eq!(
            apps,
            [".aurora/apps/sk-app/app.md", ".aurora/apps/sk-app/tools.md"],
            "generated files only"
        );
        let app_md = entries(&p, "apps")
            .into_iter()
            .find(|e| e["path"] == ".aurora/apps/sk-app/app.md")
            .unwrap();
        let app_md = app_md["content"].as_str().unwrap();
        assert!(
            app_md.contains("- `sk-app-steps`: Do the thing (`$SKILLS_DIR/sk-app-steps/SKILL.md`)"),
            "{app_md}"
        );
        let ri = entries(&p, "render_inputs");
        assert_eq!(ri[0]["apps"][0]["guide_path"], ".aurora/apps/sk-app/app.md");
    }
    // Another agent without a grant: nothing of the app, anywhere.
    let other = r.agent("bob").await;
    let p = parts(&r, r.h.state.clone(), other, None).await;
    assert!(entries(&p, "skills").is_empty());
    assert!(manifest(&p).iter().all(|m| !m.contains("sk-app")));
    assert!(!serde_json::to_string(&p).unwrap().contains("sk-app"));
    // Revoking the grant removes them at the next sync.
    sqlx::query("DELETE FROM grants WHERE agent_id = $1")
        .bind(agent)
        .execute(r.h.pool())
        .await
        .unwrap();
    let p = parts(&r, r.h.state.clone(), agent, None).await;
    assert!(entries(&p, "skills").is_empty());
    assert!(manifest(&p).iter().all(|m| !m.contains("sk-app")));
}

/// (C-1174) A skill turned off for the agent holds no name: the app skill below it applies, in
/// the effective set and in the projected folder alike.
#[tokio::test]
async fn turning_an_inherited_skill_off_lets_the_app_skill_of_its_name_apply() {
    let (r, agent) = world().await;
    db_skill(&r, None, None, "sk-app-api", "org version", true).await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM skills WHERE name = 'sk-app-api'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let path = ".claude/skills/sk-app-api/SKILL.md";
    let content = |p: &[HomeSync]| {
        skill_entry(p, path).unwrap()["content"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(content(&parts(&r, r.h.state.clone(), agent, None).await).contains("org version"));
    let (s, v) = r
        .user(
            Method::PUT,
            &format!("/v1/agents/{agent}/skills/{id}/off"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let p = parts(&r, r.h.state.clone(), agent, None).await;
    assert!(content(&p).contains("How the API works"), "{}", content(&p));
    let (_, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{agent}/skills/effective"),
            None,
        )
        .await;
    let find = |list: &str| {
        v[list]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["skill"]["name"] == "sk-app-api")
            .cloned()
            .unwrap()
    };
    assert_eq!(find("items")["tier"], "app", "{v}");
    assert_eq!(find("items")["usage"], Value::Null);
    let off = find("shadowed");
    assert_eq!(
        (off["tier"].as_str(), off["reason"].as_str()),
        (Some("org"), Some("off_for_agent")),
        "{off}"
    );
}

async fn db_skill(
    r: &Rig,
    agent: Option<Uuid>,
    ws: Option<Uuid>,
    name: &str,
    desc: &str,
    enabled: bool,
) {
    sqlx::query(
        "INSERT INTO skills (id, org_id, agent_id, workspace_id, name, description, body, enabled, created_by_kind) VALUES ($1,$2,$3,$4,$5,$6,'tier body',$7,'user')",
    )
    .bind(Uuid::now_v7()).bind(r.org).bind(agent).bind(ws).bind(name).bind(desc).bind(enabled)
    .execute(r.h.pool()).await.unwrap();
}

/// An enabled skill of the agent, the workspace or the organization with the app skill's name wins
/// (the app skill is `shadowed` with `shadowed_by`, `app.md` says so, only the winner is projected);
/// a disabled one does not veto.
#[tokio::test]
async fn another_tier_always_wins_and_a_disabled_skill_does_not_veto() {
    let (r, agent) = world().await;
    // agent tier over `sk-app-steps`, workspace tier over `sk-app-api`, org tier disabled over a third.
    db_skill(&r, Some(agent), None, "sk-app-steps", "agent version", true).await;
    db_skill(
        &r,
        None,
        Some(r.ws),
        "sk-app-api",
        "workspace version",
        true,
    )
    .await;
    let p = parts(&r, r.h.state.clone(), agent, None).await;
    let steps = skill_entry(&p, ".claude/skills/sk-app-steps/SKILL.md").unwrap();
    assert!(steps["content"].as_str().unwrap().contains("agent version"));
    assert!(
        !steps["content"]
            .as_str()
            .unwrap()
            .contains("Skill from the"),
        "the app skill is not projected"
    );
    assert!(
        skill_entry(&p, ".claude/skills/sk-app-steps/scripts/run.sh").is_none(),
        "its files neither"
    );
    let api = skill_entry(&p, ".claude/skills/sk-app-api/SKILL.md").unwrap();
    assert!(
        api["content"]
            .as_str()
            .unwrap()
            .contains("workspace version")
    );
    let app_md = entries(&p, "apps")
        .into_iter()
        .find(|e| e["path"] == ".aurora/apps/sk-app/app.md")
        .unwrap();
    let t = app_md["content"].as_str().unwrap();
    assert!(
        t.contains("a skill of the same name from your agent is used instead"),
        "{t}"
    );
    assert!(
        t.contains("a skill of the same name from your workspace is used instead"),
        "{t}"
    );
    // The effective route: tiers win, the app skills are shadowed by them.
    let (s, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{agent}/skills/effective"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let item = |n: &str| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["skill"]["name"] == n)
            .cloned()
    };
    assert_eq!(item("sk-app-steps").unwrap()["tier"], "agent");
    assert_eq!(item("sk-app-steps").unwrap()["shadows"][0]["tier"], "app");
    let sh = v["shadowed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["skill"]["name"] == "sk-app-steps" && i["tier"] == "app")
        .unwrap();
    assert_eq!(sh["shadowed_by"]["tier"], "agent");
    assert!(sh["shadowed_by"]["id"].is_string());
    assert_eq!(sh["app"]["app_id"], "sk-app");
    assert_eq!(sh["skill"]["id"], Value::Null);
    // Disabled organization skill of a third name: the app skill (a new one) is not vetoed.
    drop(p);
    db_skill(&r, None, None, "sk-app-api", "org disabled", false).await;
    let (_, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{agent}/skills/effective"),
            None,
        )
        .await;
    assert_eq!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["skill"]["name"] == "sk-app-api")
            .unwrap()["tier"],
        "workspace"
    );
    // Remove the workspace skill: the disabled org skill is no veto, the app skill returns.
    sqlx::query("DELETE FROM skills WHERE workspace_id IS NOT NULL")
        .execute(r.h.pool())
        .await
        .unwrap();
    let (_, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{agent}/skills/effective"),
            None,
        )
        .await;
    let api = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["skill"]["name"] == "sk-app-api")
        .unwrap();
    assert_eq!(api["tier"], "app", "{api}");
    let p = parts(&r, r.h.state.clone(), agent, None).await;
    assert!(
        skill_entry(&p, ".claude/skills/sk-app-api/SKILL.md").unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("Skill from the")
    );
}

/// The effective route lists an app skill as tier `app` with `app`, `id: null`, `enabled: true`, no
/// library fields; `skill_list` and `skill_get` show it (body and files); `skill_update` and
/// `skill_delete` of that name answer as for an unknown agent-tier skill and nothing changes.
#[tokio::test]
async fn the_effective_route_and_the_agent_tools_show_app_skills_read_only() {
    let r = rig().await;
    let a = setup(&r.h).await;
    install(
        &a,
        "sk",
        "sk-app",
        &[
            (
                STEPS.into(),
                md("sk-app-steps", "Do the thing", "# Steps\n1. go\n"),
                false,
            ),
            (SCRIPT.into(), b"#!/bin/sh\necho hi\n".to_vec(), true),
        ],
    )
    .await;
    let b = r.booted("tl").await;
    grant(&r, b.agent, "sk", "sk-app").await;
    let (s, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{}/skills/effective", b.agent),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let it = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["skill"]["name"] == "sk-app-steps")
        .unwrap();
    assert_eq!(it["tier"], "app");
    assert_eq!(it["enabled"], true);
    assert_eq!(
        it["app"],
        json!({"app_id": "sk-app", "name": "sk-app app", "version": "1.0.0"})
    );
    assert_eq!(it["skill"]["id"], Value::Null);
    assert_eq!(it["skill"]["source"], Value::Null);
    assert_eq!(it["skill"]["body"], "# Steps\n1. go\n");
    assert_eq!(
        it["skill"]["files"],
        json!([{"path": "scripts/run.sh", "size": 18, "executable": true}])
    );
    // Tools.
    let v = r.tool(&b, "skill_list", json!({})).await;
    let skills = v["content"][0]["value"]["skills"]
        .as_array()
        .unwrap()
        .clone();
    let s = skills.iter().find(|s| s["name"] == "sk-app-steps").unwrap();
    assert_eq!(
        (s["tier"].as_str(), s["app_id"].as_str()),
        (Some("app"), Some("sk-app"))
    );
    let v = r
        .tool(&b, "skill_get", json!({"name": "sk-app-steps"}))
        .await;
    assert_eq!(v["content"][0]["value"]["body"], "# Steps\n1. go\n");
    assert_eq!(v["content"][0]["value"]["tier"], "app");
    let v = r
        .tool(
            &b,
            "skill_get",
            json!({"name": "sk-app-steps", "path": "scripts/run.sh"}),
        )
        .await;
    assert_eq!(v["content"][0]["value"]["content"], "#!/bin/sh\necho hi\n");
    // Writes never reach it.
    for (t, args) in [
        ("skill_update", json!({"name": "sk-app-steps", "body": "x"})),
        ("skill_delete", json!({"name": "sk-app-steps"})),
    ] {
        let v = r.tool(&b, t, args).await;
        assert_eq!(v["ok"], false, "{t}: {v}");
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM skills")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    // An agent without the grant sees none of it.
    let other = r.booted("tl2").await;
    let v = r.tool(&other, "skill_list", json!({})).await;
    assert!(
        v["content"][0]["value"]["skills"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let v = r
        .tool(&other, "skill_get", json!({"name": "sk-app-steps"}))
        .await;
    assert_eq!(v["ok"], false);
}

/// Two apps of two packages whose ids overlap (`ov` and `ov-x`) ship a skill of the same name
/// `ov-x-thing`: the app whose id is the longest prefix wins. (D-465(c), FIX-221) The name belongs to
/// `ov-x`, so `ov`'s copy is claimed: left out of the candidates, it is neither given nor listed as
/// shadowed (its admin view says `claimed_by`).
#[tokio::test]
async fn between_apps_the_longest_prefix_of_the_name_wins() {
    let r = rig().await;
    let a = setup(&r.h).await;
    let file = |app: &str| {
        (
            format!("skills/{app}/ov-x-thing/SKILL.md"),
            md("ov-x-thing", &format!("from {app}"), "b\n"),
            false,
        )
    };
    install(&a, "pov", "ov", &[file("ov")]).await;
    install(&a, "povx", "ov-x", &[file("ov-x")]).await;
    let agent = r.agent("ada").await;
    grant(&r, agent, "pov", "ov").await;
    grant(&r, agent, "povx", "ov-x").await;
    let p = parts(&r, r.h.state.clone(), agent, None).await;
    let e = skill_entry(&p, ".claude/skills/ov-x-thing/SKILL.md").unwrap();
    assert!(
        e["content"].as_str().unwrap().contains("from ov-x")
            && e["content"].as_str().unwrap().contains("ov-x app app"),
        "{e}"
    );
    assert_eq!(
        entries(&p, "skills")
            .iter()
            .filter(|e| e["path"] == ".claude/skills/ov-x-thing/SKILL.md")
            .count(),
        1
    );
    let (_, v) = r
        .user(
            Method::GET,
            &format!("/v1/agents/{agent}/skills/effective"),
            None,
        )
        .await;
    let win = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["skill"]["name"] == "ov-x-thing")
        .unwrap();
    assert_eq!(win["app"]["app_id"], "ov-x");
    let ov_copy = |list: &Value| {
        list.as_array()
            .unwrap()
            .iter()
            .any(|i| i["skill"]["name"] == "ov-x-thing" && i["app"]["app_id"] == "ov")
    };
    assert!(!ov_copy(&v["items"]), "ov's claimed copy is not given: {v}");
    assert!(
        !ov_copy(&v["shadowed"]),
        "ov's claimed copy is left out, not shadowed: {v}"
    );
}

/// Over `APP_SKILLS_SYNC_MAX` the first app that does not fit and every later one project no skills,
/// their `app.md` says so, a warning is logged and `aurora_home_app_skills_budget_exceeded_total`
/// is incremented once; the generated files are never dropped.
#[tokio::test]
async fn over_the_budget_the_later_apps_project_no_skills_and_app_md_says_so() {
    let (r, agent) = world().await;
    let tight = r.h.state.clone().with_app_skills_sync_max(10);
    let series = "aurora_home_app_skills_budget_exceeded_total";
    let _ = metrics_text();
    let before = counter_value(series);
    let p = parts(&r, tight, agent, None).await;
    assert_eq!(counter_value(series) - before, 1.0);
    assert!(entries(&p, "skills").is_empty());
    assert!(
        manifest(&p)
            .iter()
            .all(|m| !m.contains(".claude/skills/sk-app"))
    );
    let app_md = entries(&p, "apps")
        .into_iter()
        .find(|e| e["path"] == ".aurora/apps/sk-app/app.md")
        .unwrap();
    assert!(
        app_md["content"]
            .as_str()
            .unwrap()
            .contains("were left out of your skills folder")
    );
    let api = entries(&p, "apps")
        .into_iter()
        .find(|e| e["path"] == ".aurora/apps/sk-app/api.json");
    if let Some(api) = api {
        let v: Value = serde_json::from_str(api["content"].as_str().unwrap()).unwrap();
        assert_eq!(v["skills"], json!([]));
    }
    // The default budget sends them and does not count.
    let before = counter_value(series);
    let p = parts(&r, r.h.state.clone(), agent, None).await;
    assert!(skill_entry(&p, ".claude/skills/sk-app-steps/SKILL.md").is_some());
    assert_eq!(counter_value(series), before);
}
