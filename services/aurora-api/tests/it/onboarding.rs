#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-ONB-2 (D-141, C-530..C-533): onboarding progress and the setup status.

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

async fn call(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    h.call_tok(tok, m, uri, body).await
}

fn step<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("step {id} in {v}"))
}

fn check<'a>(s: &'a Value, id: &str) -> Option<&'a Value> {
    s["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
}

#[tokio::test]
async fn a_new_user_has_not_seen_the_welcome_and_changes_stick() {
    let h = Harness::new().await;
    let u = h.user("new@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    // No row yet: defaults.
    let (s, v) = call(&h, &tok, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["welcome_seen"], false);
    assert_eq!(v["guide_dismissed"], json!([]));
    assert!(v["tour_completed_at"].is_null());

    let ws = Uuid::now_v7();
    let (s, v) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"welcome_seen": true, "guide_dismissed": [ws, ws], "tour_completed": true})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["welcome_seen"], true);
    assert_eq!(v["guide_dismissed"], json!([ws.to_string()]));
    assert!(v["tour_completed_at"].is_string());

    // Fields not sent do not change; `tour_completed: false` clears.
    let (s, v) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"tour_completed": false})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["welcome_seen"], true);
    assert_eq!(v["guide_dismissed"], json!([ws.to_string()]));
    assert!(v["tour_completed_at"].is_null());

    // Progress is per user.
    let other = h.user("other@x.io", false).await;
    let (_, tok2) = h.good_session(other).await;
    let (_, v) = call(&h, &tok2, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(v["welcome_seen"], false);
}

#[tokio::test]
async fn onboarding_rejects_unknown_fields_and_too_many_guides() {
    let h = Harness::new().await;
    let u = h.user("v@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    let (s, _) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"is_instance_admin": true})),
    )
    .await;
    assert_eq!(s, 400);
    let many: Vec<String> = (0..201).map(|_| Uuid::now_v7().to_string()).collect();
    let (s, v) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"guide_dismissed": many})),
    )
    .await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn notices_default_for_new_accounts_append_once_and_are_limited() {
    let h = Harness::new().await;
    // A new account (no row, or a row from a first PATCH) has seen no notice (D-407).
    let new = h.user("n@x.io", false).await;
    let (_, tok) = h.good_session(new).await;
    let (_, v) = call(&h, &tok, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(v["notices_seen"], json!([]), "{v}");
    let (s, v) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"welcome_seen": true})),
    )
    .await;
    assert_eq!((s, &v["notices_seen"]), (200, &json!([])), "{v}");

    // An account that existed before the release has an empty list (the migration's row).
    let old = h.user("o@x.io", false).await;
    sqlx::query("INSERT INTO user_onboarding (user_id, notices_seen) VALUES ($1, '{}')")
        .bind(old)
        .execute(h.pool())
        .await
        .unwrap();
    let (_, otok) = h.good_session(old).await;
    let (_, v) = call(&h, &otok, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(v["notices_seen"], json!([]), "{v}");
    // PATCH appends, once; other fields stay.
    for _ in 0..2 {
        let (s, v) = call(
            &h,
            &otok,
            Method::PATCH,
            "/v1/me/onboarding",
            Some(json!({"notice_seen": "renames-2026-11"})),
        )
        .await;
        assert_eq!(
            (s, &v["notices_seen"]),
            (200, &json!(["renames-2026-11"])),
            "{v}"
        );
    }
    // The key's shape and the list's size are bounded.
    for bad in ["", "Has Space", &"a".repeat(65)] {
        let (s, v) = call(
            &h,
            &otok,
            Method::PATCH,
            "/v1/me/onboarding",
            Some(json!({"notice_seen": bad})),
        )
        .await;
        assert_eq!(s, 400, "{bad}: {v}");
    }
    for i in 0..49 {
        let (s, v) = call(
            &h,
            &otok,
            Method::PATCH,
            "/v1/me/onboarding",
            Some(json!({"notice_seen": format!("n-{i}")})),
        )
        .await;
        assert_eq!(s, 200, "{i}: {v}");
    }
    let (s, v) = call(
        &h,
        &otok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"notice_seen": "one-too-many"})),
    )
    .await;
    assert_eq!(s, 400, "{v}");
    // Re-sending a known key at the limit is still fine.
    let (s, _) = call(
        &h,
        &otok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"notice_seen": "n-3"})),
    )
    .await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn setup_status_is_for_instance_admins_and_has_reason_codes() {
    let h = Harness::new().await;
    let member = h.user("m@x.io", false).await;
    let (_, mtok) = h.good_session(member).await;
    let (s, _) = call(&h, &mtok, Method::GET, "/v1/admin/setup", None).await;
    assert_eq!(s, 403);

    let admin = h.user("a@x.io", true).await;
    let (_, tok) = h.good_session(admin).await;
    h.unsave_placement().await;
    let (s, v) = call(&h, &tok, Method::GET, "/v1/admin/setup", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["placement_saved"], false);
    let ids: Vec<&str> = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["roles", "placement", "llm", "runners", "optional"]);
    let placement = step(&v, "placement");
    assert_eq!(placement["status"], "todo");
    assert_eq!(placement["reason"], "placement_unsaved");
    // No runner or gateway in the test instance.
    let runners = step(&v, "runners");
    assert_eq!(runners["status"], "todo");
    assert_eq!(runners["reason"], "no_runner");
    assert_eq!(check(runners, "gateway").unwrap()["reason"], "no_gateway");
    // Optional steps never block.
    assert_eq!(step(&v, "optional")["status"], "ok");
    // No secret-shaped text anywhere.
    assert!(!v.to_string().contains("aur_"));

    // Saving a placement flips the step.
    let (s, p) = call(
        &h,
        &tok,
        Method::PUT,
        "/v1/admin/placement",
        Some(json!({"mode": "shared_org", "create_default_org": "Users"})),
    )
    .await;
    assert_eq!(s, 200, "{p}");
    let (_, v) = call(&h, &tok, Method::GET, "/v1/admin/setup", None).await;
    assert_eq!(v["placement_saved"], true);
    assert_eq!(step(&v, "placement")["status"], "ok");
    let llm = step(&v, "llm");
    assert_eq!(check(llm, "org_key").unwrap()["reason"], "no_org_key");
    assert_eq!(
        check(llm, "starter_grant").unwrap()["reason"],
        "no_starter_grant"
    );
}

#[tokio::test]
async fn more_controls_defaults_off_persists_and_is_per_person() {
    // C-1056: the per-person "Show more controls" switch.
    let h = Harness::new().await;
    let a = h.user("mc-a@x.io", false).await;
    let b = h.user("mc-b@x.io", false).await;
    let (_, ta) = h.good_session(a).await;
    let (_, ta2) = h.good_session(a).await;
    let (_, tb) = h.good_session(b).await;
    let (s, v) = call(&h, &ta, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["more_controls"], false);

    let (s, v) = call(
        &h,
        &ta,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"more_controls": true})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["more_controls"], true);
    // Another session of the same person sees it.
    let (_, v) = call(&h, &ta2, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(v["more_controls"], true);

    // A patch of another field leaves it.
    let (s, v) = call(
        &h,
        &ta,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"welcome_seen": true})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["more_controls"], true);
    assert_eq!(v["welcome_seen"], true);

    // Another person is unaffected.
    let (_, v) = call(&h, &tb, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(v["more_controls"], false);
    // First write of another person with only the switch creates their row.
    let (s, v) = call(
        &h,
        &tb,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"more_controls": false})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["more_controls"], false);
    assert_eq!(v["welcome_seen"], false);
    let (_, v) = call(&h, &ta, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(v["more_controls"], true);

    // Wrong type and unknown sibling fields are 400.
    let (s, _) = call(
        &h,
        &ta,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"more_controls": "yes"})),
    )
    .await;
    assert_eq!(s, 400);
}

/// P5-REFINE-5 (C-1063): Health reads real calls first, named by the display name; Try it is
/// decided by the server from the admin's own finished turn.
#[tokio::test]
async fn refine5_health_reads_real_calls_and_try_it_completes_by_itself() {
    let h = Harness::with_config(|c| {
        c.llm.endpoints = vec![aurora_config::LlmEndpoint {
            id: "company".into(),
            provider: "litellm".into(),
            // Nothing listens here: the unauthenticated probe fails.
            base_url: Some("http://127.0.0.1:1".into()),
            display_name: Some("Company AI".into()),
            default_model: Some("claude-test".into()),
            ..Default::default()
        }];
    })
    .await;
    let admin = h.user("a@x.io", true).await;
    let (_, tok) = h.good_session(admin).await;
    let org = h.org().await;
    h.member(org, admin, "owner", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, admin, "owner").await;
    sqlx::query("UPDATE workspaces SET personal_owner_user_id = $2 WHERE id = $1")
        .bind(ws)
        .bind(admin)
        .execute(h.pool())
        .await
        .unwrap();
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, is_default) VALUES ($1,$2,$3,'daneel','Daneel','hosted','fake',true)")
        .bind(agent)
        .bind(org)
        .bind(ws)
        .execute(h.pool())
        .await
        .unwrap();
    let setup = || async {
        let (s, v) = call(&h, &tok, Method::GET, "/v1/admin/setup", None).await;
        assert_eq!(s, 200, "{v}");
        v
    };

    // No key yet: Try it is not offered; the endpoint check says it did not answer (the probe),
    // by its display name.
    let v = setup().await;
    assert_eq!(v["try_it"]["available"], false, "{v}");
    assert_eq!(v["try_it"]["done"], false);
    assert_eq!(v["try_it"]["agent_id"], json!(agent));
    assert_eq!(v["try_it"]["workspace_id"], json!(ws));
    let c = check(step(&v, "llm"), "endpoint:company").unwrap().clone();
    assert_eq!(c["status"], "warning", "{c}");
    assert_eq!(c["evidence"], "probe");
    let msg = c["message"].as_str().unwrap();
    assert!(
        msg.contains("Company AI") && msg.contains("did not answer"),
        "{msg}"
    );
    assert!(
        !msg.contains("from the API") && !msg.contains("(litellm)"),
        "{msg}"
    );

    // A key that an agent's real call went through successfully: ok on the evidence of the call,
    // although the probe still fails; and the agent can answer once the key is granted.
    let key = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref, last_ok_at) VALUES ($1,$2,'k','llm','litellm','api_key','company','127.0.0.1','{}','managed','x', now())")
        .bind(key)
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)")
        .bind(Uuid::now_v7())
        .bind(ws)
        .bind(agent)
        .bind(key)
        .execute(h.pool())
        .await
        .unwrap();
    let v = setup().await;
    let c = check(step(&v, "llm"), "endpoint:company").unwrap().clone();
    assert_eq!(
        (c["status"].as_str(), c["evidence"].as_str()),
        (Some("ok"), Some("call")),
        "{c}"
    );
    assert!(c["last_ok_at"].is_string());
    assert!(c["message"].as_str().unwrap().contains("Company AI"));
    assert_eq!(v["try_it"]["available"], true, "{v}");

    // A real call that failed after the last success: a warning on the evidence of the call.
    sqlx::query("UPDATE secrets SET last_error_at = now() + interval '1 second', last_error = 'unreachable' WHERE id = $1")
        .bind(key)
        .execute(h.pool())
        .await
        .unwrap();
    let v = setup().await;
    let c = check(step(&v, "llm"), "endpoint:company").unwrap().clone();
    assert_eq!(
        (c["status"].as_str(), c["evidence"].as_str()),
        (Some("warning"), Some("call")),
        "{c}"
    );
    assert!(c["last_error_at"].is_string());
    assert_eq!(
        c["last_error"], "unreachable",
        "the reason of the failed call: {c}"
    );

    // Try it is done after one finished turn of the admin's own, in any conversation.
    assert_eq!(setup().await["try_it"]["done"], false);
    let conv = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id, last_activity_at) VALUES ($1,$2,$3,'direct',$4,$5, now())")
        .bind(conv)
        .bind(agent)
        .bind(ws)
        .bind(format!("direct:{admin}"))
        .bind(admin)
        .execute(h.pool())
        .await
        .unwrap();
    let turn = |status: &'static str| {
        let (pool, conv, agent) = (h.pool().clone(), conv, agent);
        async move {
            sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_user_id, speaker_label, text) VALUES ($1,$2,$3,'turn',$4,'{}','person',$5,'a','hello')")
                .bind(Uuid::now_v7())
                .bind(conv)
                .bind(agent)
                .bind(status)
                .bind(admin)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    turn("queued").await;
    assert_eq!(
        setup().await["try_it"]["done"],
        false,
        "an unfinished turn is not it"
    );
    assert!(
        setup().await["try_it"]["done_at"].is_null(),
        "no time without a finished turn"
    );
    turn("done").await;
    let v = setup().await;
    assert_eq!(v["try_it"]["done"], true);
    assert!(
        v["try_it"]["done_at"].is_string(),
        "when it was answered: {v}"
    );
    assert_eq!(v["try_it"]["agent_name"], "Daneel", "{v}");
    assert_eq!(v["try_it"]["agent_id"], json!(agent));

    // (P5-REFINE-20 b1#2) The row names the agent that answered: a later finished turn with
    // another agent the admin owns moves it there, with its name.
    let rex = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, is_default) VALUES ($1,$2,$3,'rex','Rex','hosted','fake',false)")
        .bind(rex)
        .bind(org)
        .bind(ws)
        .execute(h.pool())
        .await
        .unwrap();
    let rex_conv = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id, last_activity_at) VALUES ($1,$2,$3,'direct',$4,$5, now())")
        .bind(rex_conv)
        .bind(rex)
        .bind(ws)
        .bind(format!("user:{admin}"))
        .bind(admin)
        .execute(h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_user_id, speaker_label, text, finished_at) VALUES ($1,$2,$3,'turn','done','{}','person',$4,'a','hello', now() + interval '1 minute')")
        .bind(Uuid::now_v7())
        .bind(rex_conv)
        .bind(rex)
        .bind(admin)
        .execute(h.pool())
        .await
        .unwrap();
    let v = setup().await;
    assert_eq!(v["try_it"]["agent_name"], "Rex", "{v}");
    assert_eq!(v["try_it"]["agent_id"], json!(rex));
    assert_eq!(v["try_it"]["workspace_id"], json!(ws));
}
