#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SET-5 (D-235(a), D-236, D-237, C-782, C-783): the operational sections as live settings —
//! `agents`, `approvals`, `retention`, `skills`, `previews`: validation names the key, readers
//! follow a write without a restart, runners are read-only.

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct W {
    h: Harness,
    admin: String,
}

async fn world() -> W {
    world_with("").await
}

async fn world_with(extra: &str) -> W {
    let h = Harness::new()
        .await
        .with_source(config_source_custom(
            &format!("{extra}\n[[auth.oidc_providers]]\nid = \"helm\"\ndisplay_name = \"Helm SSO\"\nissuer = \"https://idp.example.com\"\nclient_id = \"c\"\nclient_secret_env = \"HELM_SECRET\"\n"),
            &[("HELM_SECRET", "s")],
        ));
    let a = h.user("root@x.io", true).await;
    let (_, admin) = h.good_session(a).await;
    W { h, admin }
}

// P4-011B-2 (C-829, D-357): `approvals.park.*` are live settings; the cross-key rule is checked
// only while parking is on and when a write turns it on; `GET /v1/policy/approval-limits` and the
// rule/grant timeout bound follow the switch.
#[tokio::test]
async fn park_settings_are_live_and_the_cross_key_rule_follows_the_switch() {
    let w = world().await;
    // Parking off: a hold maximum below the 900 s threshold is accepted (D-357).
    let (s, v) = w
        .put("approvals", json!({ "approvals.max_timeout_secs": 600 }))
        .await;
    assert_eq!(s, 200, "{v}");
    // Turning parking on with values that break the rule is refused, naming the key and code.
    let (s, v) = w
        .put(
            "approvals",
            json!({ "approvals.max_timeout_secs": 600, "approvals.park.enabled": true }),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("settings_invalid")),
        "{v}"
    );
    assert!(
        v.to_string().contains("park_threshold_above_hold_max"),
        "{v}"
    );
    // Consistent values turn it on; the readers follow without a restart.
    let (s, v) = w
        .put(
            "approvals",
            json!({
                "approvals.max_timeout_secs": 600,
                "approvals.park.enabled": true,
                "approvals.park.threshold_secs": 300,
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(Method::GET, "/v1/policy/approval-limits", None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["park"]["enabled"], true);
    assert_eq!(v["park"]["threshold_secs"], 300);
    assert_eq!(v["max_timeout_secs"], 600);
    // A hold maximum above the park maximum is refused while parking is on.
    let (s, v) = w
        .put(
            "approvals",
            // (A write replaces the section's values, so the switch is part of it.)
            json!({
                "approvals.park.enabled": true,
                "approvals.park.max_timeout_secs": 3600,
                "approvals.max_timeout_secs": 7200,
            }),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(v.to_string().contains("park_max_below_hold_max"), "{v}");
}

impl W {
    async fn call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let c = cookie(&self.admin);
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", c.as_str()), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }
    async fn section(&self, id: &str) -> Value {
        let (s, v) = self
            .call(Method::GET, &format!("/v1/admin/settings/{id}"), None)
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }
    async fn put(&self, id: &str, values: Value) -> (u16, Value) {
        let g = self.section(id).await["generation"].as_i64().unwrap();
        self.call(
            Method::PUT,
            &format!("/v1/admin/settings/{id}"),
            Some(json!({ "generation": g, "values": values })),
        )
        .await
    }
    async fn runner(&self, name: &str, harnesses: &[&str]) {
        let caps = json!({ "backend": "podman", "max_sandboxes": 7, "harnesses": harnesses,
            "defaults": { "cpu_millis": 2000, "memory_mib": 4096, "pids": 512, "home_gib": 5 } });
        // The token hash is unique: one per runner name.
        let mut token_hash = [0u8; 32];
        let n = name.len().min(32);
        token_hash[..n].copy_from_slice(&name.as_bytes()[..n]);
        sqlx::query(
            "INSERT INTO runners (id, name, token_hash, installation_fingerprint, capabilities) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(name)
        .bind(&token_hash[..])
        .bind(format!("fp-{name}"))
        .bind(caps)
        .execute(self.h.pool())
        .await
        .unwrap();
    }
}

fn paths(v: &Value) -> Vec<(String, String)> {
    v["error"]["details"]["fields"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    (
                        f["path"].as_str().unwrap_or_default().to_owned(),
                        f["code"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn startup_rules_fail_a_write_naming_the_other_key() {
    let w = world().await;
    // The turn ceiling may not fall below approvals.max_timeout_secs + 900 (default 3600).
    let (s, v) = w
        .put("agents", json!({ "hosted.turn_ceiling_secs": 3000 }))
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(v.to_string().contains("approvals.max_timeout_secs"), "{v}");
    // A deadline below its own minimum names its key.
    let (s, v) = w
        .put("approvals", json!({ "approvals.timeout_secs": 5 }))
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(v.to_string().contains("approvals.timeout_secs"), "{v}");
    // A default harness outside the enabled list names the list.
    let (s, v) = w
        .put("agents", json!({ "hosted.default_harness": "codex" }))
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(v.to_string().contains("hosted.default_harness"), "{v}");
}

#[tokio::test]
async fn a_written_value_is_read_by_the_next_use_without_a_restart() {
    let w = world().await;
    assert_eq!(w.h.state.ops().hosted.idle_stop_secs, 30 * 60);
    let (s, v) = w
        .put(
            "agents",
            json!({ "hosted.idle_stop_secs": 3600, "agents.default_name": "Ada" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let ops = w.h.state.ops();
    assert_eq!(ops.hosted.idle_stop_secs, 3600);
    assert_eq!(ops.agents.default_name, "Ada");
    let (s, v) = w
        .put("approvals", json!({ "approvals.max_timeout_secs": 7200 }))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.h.state.ops().approvals.max_timeout_secs, 7200);
    let (s, v) = w
        .put(
            "retention",
            json!({ "retention.conversation_days_max": 90, "attachments.org_quota_mib_max": 1024 }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let ops = w.h.state.ops();
    assert_eq!(ops.retention.conversation_days_max, Some(90));
    assert_eq!(ops.attachments.org_quota_mib_max, 1024);
    let (s, v) = w
        .put("previews", json!({ "previews.shares_per_agent": 3 }))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.h.state.ops().previews.shares_per_agent, 3);
    let (s, v) = w
        .put(
            "skills",
            json!({ "skills.library.refresh_interval_secs": 100 }),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(
        v.to_string()
            .contains("skills.library.refresh_interval_secs"),
        "{v}"
    );
}

#[tokio::test]
async fn the_agents_page_lists_the_harnesses_runners_offer_and_null_with_no_runner() {
    let w = world().await;
    // (C-1028) Nothing is judged without a runner, so there is nothing to list.
    let sec = w.section("agents").await;
    assert_eq!(sec["extra"]["offered_harnesses"], json!(null), "{sec}");
    // The union of what the registered runners report, sorted.
    w.runner("a", &["codex", "claude-code"]).await;
    w.runner("b", &["goose", "codex"]).await;
    let sec = w.section("agents").await;
    assert_eq!(
        sec["extra"]["offered_harnesses"],
        json!(["claude-code", "codex", "goose"]),
        "{sec}"
    );
}

#[tokio::test]
async fn harnesses_must_be_reported_by_a_runner_and_runner_sizes_are_read_only() {
    let w = world().await;
    w.runner("main", &["claude-code", "codex"]).await;
    let (s, v) = w
        .put(
            "agents",
            json!({ "hosted.enabled_harnesses": ["claude-code", "mystery"] }),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(
        paths(&v)
            .iter()
            .any(|(p, c)| p == "/hosted.enabled_harnesses" && c == "harness_not_available"),
        "{v}"
    );
    let (s, v) = w
        .put(
            "agents",
            json!({ "hosted.enabled_harnesses": ["claude-code", "codex"] }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let sec = w.section("agents").await;
    let r = &sec["extra"]["runners"][0];
    assert_eq!(r["name"], "main", "{sec}");
    assert_eq!(r["max_sandboxes"], 7);
    // (C-782) The sizes as the runner reports them.
    assert_eq!(
        (
            r["sandbox_memory_mib"].as_i64(),
            r["sandbox_cpus"].as_f64(),
            r["sandbox_pids"].as_i64(),
            r["home_gib"].as_i64()
        ),
        (Some(4096), Some(2.0), Some(512), Some(5)),
        "{sec}"
    );
    assert_eq!(r["harnesses"], json!(["claude-code", "codex"]));
    // Runner configuration is not a setting.
    let (s, v) = w.put("agents", json!({ "runner.max_sandboxes": 9 })).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("setting_unknown")),
        "{v}"
    );
}

/// A value the deployment sets is locked: the write is refused naming the Helm key, and the
/// other keys of the section stay editable.
#[tokio::test]
async fn a_helm_set_key_is_locked_and_its_neighbours_are_not() {
    let w = world_with("[retention]\nrequest_log_days = 45\n").await;
    let sec = w.section("retention").await;
    let k = sec["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "retention.request_log_days")
        .unwrap();
    assert_eq!(
        (k["locked"].clone(), k["value"].clone()),
        (json!(true), json!(45))
    );
    let (s, v) = w
        .put("retention", json!({ "retention.request_log_days": 10 }))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("setting_locked")),
        "{v}"
    );
    assert_eq!(v["error"]["details"]["key"], "retention.request_log_days");
    let (s, v) = w
        .put(
            "retention",
            json!({ "retention.request_log_days": 45, "retention.conversation_days_max": 400 }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.h.state.ops().retention.conversation_days_max, Some(400));
    assert_eq!(w.h.state.ops().retention.request_log_days, 45);
}

/// (C-782, D-237(d)) The terminal and SCIM switches cannot be turned on while the deployment does
/// not provide the wiring; the refusal names the key and the Helm value.
#[tokio::test]
async fn switches_need_the_deployments_wiring() {
    let w = world().await;
    for (section, key, requires) in [
        ("terminal", "terminal.enabled", "terminal.available"),
        ("scim", "scim.enabled", "scim.available"),
    ] {
        let sec = w.section(section).await;
        assert_eq!(sec["extra"]["available"], json!(false), "{sec}");
        let (s, v) = w.put(section, json!({ key: true })).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("setting_requires_deployment")),
            "{v}"
        );
        assert_eq!(v["error"]["details"]["key"], key);
        assert_eq!(v["error"]["details"]["requires"], requires);
        // Off is always fine, and the section's other keys are editable.
        let (s, v) = w.put(section, json!({ key: false })).await;
        assert_eq!(s, 200, "{v}");
    }
    assert!(!w.h.state.ops().terminal.enabled && !w.h.state.ops().scim.enabled);
}

/// With the wiring in place the switches default on, and turning them off applies to the next
/// request without a restart: the terminal answers 404 `terminal_disabled`, `/scim/v2` is 404.
#[tokio::test]
async fn the_terminal_and_scim_switches_apply_live() {
    let w = world_with("[terminal]\navailable = true\n[scim]\navailable = true\n").await;
    let sec = w.section("terminal").await;
    let k = sec["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "terminal.enabled")
        .unwrap();
    assert_eq!(
        (k["value"].clone(), k["source"].clone()),
        (json!(true), json!("default")),
        "{sec}"
    );
    assert_eq!(sec["extra"]["available"], json!(true));
    let agent = Uuid::now_v7();
    let term = format!("/v1/agents/{agent}/terminal");
    let get = |uri: String| {
        let h = &w.h;
        async move { h.send(Method::GET, &uri, &[], None).await }
    };
    // On: the route goes past the switch (anonymous → 401); SCIM likewise.
    let (s, _, v) = get(term.clone()).await;
    assert_eq!(s.as_u16(), 401, "{v}");
    let (s, _, v) = get("/scim/v2/Users".into()).await;
    assert_eq!(s.as_u16(), 401, "{v}");

    let (s, v) = w
        .put("terminal", json!({ "terminal.enabled": false }))
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.put("scim", json!({ "scim.enabled": false })).await;
    assert_eq!(s, 200, "{v}");
    let (s, _, v) = get(term.clone()).await;
    assert_eq!(
        (s.as_u16(), v["error"]["code"].as_str()),
        (404, Some("terminal_disabled")),
        "{v}"
    );
    let (s, _, _) = get("/scim/v2/Users".into()).await;
    assert_eq!(s.as_u16(), 404);

    // Back on: wiring present, so it is allowed again.
    let (s, v) = w.put("terminal", json!({ "terminal.enabled": true })).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.put("scim", json!({ "scim.enabled": true })).await;
    assert_eq!(s, 200, "{v}");
    let (s, _, _) = get(term).await;
    assert_eq!(s.as_u16(), 401);
    let (s, _, _) = get("/scim/v2/Users".into()).await;
    assert_eq!(s.as_u16(), 401);
}

/// FIX-162 (D-358): with SSH on, the terminal switch cannot be turned off; the refusal names the
/// key; with SSH off (the other tests above) it can.
#[tokio::test]
async fn the_terminal_cannot_be_turned_off_while_ssh_is_on() {
    let h = Harness::with_config(|c| {
        c.ssh.enabled = true;
        c.ssh_ca_key = Some(aurora_config::Redacted::new(
            "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_owned(),
        ));
    })
        .await
        .with_source(config_source_custom(
            "[terminal]\navailable = true\n\n[[auth.oidc_providers]]\nid = \"helm\"\ndisplay_name = \"Helm SSO\"\nissuer = \"https://idp.example.com\"\nclient_id = \"c\"\nclient_secret_env = \"HELM_SECRET\"\n",
            &[("HELM_SECRET", "s")],
        ));
    let a = h.user("root@x.io", true).await;
    let (_, admin) = h.good_session(a).await;
    let w = W { h, admin };
    let (s, v) = w
        .put("terminal", json!({ "terminal.enabled": false }))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("settings_invalid")),
        "{v}"
    );
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"],
        "/terminal.enabled"
    );
    assert!(w.h.state.ops().terminal.enabled, "unchanged");
    let (s, v) = w.put("terminal", json!({ "terminal.enabled": true })).await;
    assert_eq!(s, 200, "{v}");
}

/// `ratelimit.api.multiplier` moves the 429 threshold without a restart (the harness scales every
/// bucket by 100 until the first write), and a class the configuration overrides is locked alone.
#[tokio::test]
async fn the_rate_multiplier_applies_live_and_a_configured_class_is_locked_alone() {
    let w = world_with("[ratelimit.api.classes.write]\nper_minute = 600\nburst = 100\n").await;
    let sec = w.section("limits").await;
    let classes = sec["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "ratelimit.api.classes")
        .unwrap();
    assert_eq!(classes["list"], "keyed", "{sec}");
    let entries = classes["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{sec}");
    assert_eq!(
        (entries[0]["id"].clone(), entries[0]["locked"].clone()),
        (json!("write"), json!(true))
    );
    // The class Helm defines cannot be redefined; another class is a dashboard entry.
    let (s, v) = w
        .put(
            "limits",
            json!({ "ratelimit.api.classes": [{ "class": "write", "per_minute": 5, "burst": 5 }] }),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(
        paths(&v)
            .iter()
            .any(|(p, c)| p.starts_with("/ratelimit.api.classes/write") && c == "entry_id_in_helm"),
        "{v}"
    );
    let (s, v) = w
        .put(
            "limits",
            json!({ "ratelimit.api.classes": [{ "class": "export", "per_hour": 50, "burst": 10 }] }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let api = &w.h.state.ops().ratelimit.api;
    assert_eq!(api.classes["write"].per_minute, Some(600));
    assert_eq!(api.classes["export"].per_hour, Some(50));

    // A multiplier below one lowers every bucket at once: the general bucket (600/min, burst
    // 120) scaled by 0.1 holds 12 tokens, so a loop of reads is refused before it ends.
    let cookie_hdr = cookie(&w.admin);
    let hdrs = [("cookie", cookie_hdr.as_str()), ("origin", ORIGIN)];
    for _ in 0..30 {
        let (s, _, v) = w.h.send(Method::GET, "/v1/me", &hdrs, None).await;
        assert_eq!(s.as_u16(), 200, "{v}");
    }
    let (s, v) = w
        .put("limits", json!({ "ratelimit.api.multiplier": 0.1 }))
        .await;
    assert_eq!(s, 200, "{v}");
    let mut refused = None;
    for i in 0..60 {
        let (s, _, _) = w.h.send(Method::GET, "/v1/me", &hdrs, None).await;
        if s.as_u16() == 429 {
            refused = Some(i);
            break;
        }
    }
    assert!(
        refused.is_some(),
        "no 429 after the multiplier dropped to 0.1"
    );
}

/// The preview flags a change that adds a private destination (C-782).
#[tokio::test]
async fn adding_an_allowed_private_entry_is_a_loosening_with_a_warning() {
    let w = world().await;
    let g = w.section("limits").await["generation"].as_i64().unwrap();
    let (s, p) = w
        .call(
            Method::POST,
            "/v1/admin/settings/limits/preview",
            Some(json!({ "generation": g, "values": { "gateway.allowed_private": ["10.1.0.0/16"] } })),
        )
        .await;
    assert_eq!(s, 200, "{p}");
    assert!(
        p["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["code"] == "opens_private_destinations"),
        "{p}"
    );
    assert_eq!(p["loosening"][0]["key"], "gateway.allowed_private", "{p}");
    // Removing it again is not.
    let (s, v) = w
        .put(
            "limits",
            json!({ "gateway.allowed_private": ["10.1.0.0/16"] }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        w.h.state.ops().gateway_allowed_private,
        vec!["10.1.0.0/16".to_owned()]
    );
    let g = w.section("limits").await["generation"].as_i64().unwrap();
    let (_, p) = w
        .call(
            Method::POST,
            "/v1/admin/settings/limits/preview",
            Some(json!({ "generation": g, "values": { "gateway.allowed_private": [] } })),
        )
        .await;
    assert!(
        p["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["code"] != "opens_private_destinations"),
        "{p}"
    );
}

/// (C-783) Lowering a retention ceiling lists the organizations that set a value above it; the
/// value they stored is clamped where it is read.
#[tokio::test]
async fn a_lowered_retention_ceiling_lists_the_organizations_above_it() {
    let w = world().await;
    let org = w.h.org().await;
    sqlx::query(
        "UPDATE organizations SET settings = jsonb_set(settings, '{retention}', \
         '{\"conversations_days\": 365, \"attachment_quota_mib\": 0}') WHERE id = $1",
    )
    .bind(org)
    .execute(w.h.pool())
    .await
    .unwrap();
    let g = w.section("retention").await["generation"].as_i64().unwrap();
    let (s, p) = w
        .call(
            Method::POST,
            "/v1/admin/settings/retention/preview",
            Some(json!({ "generation": g, "values": {
                "retention.conversation_days_max": 90, "attachments.org_quota_mib_max": 2048 } })),
        )
        .await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["affected_orgs_total"], json!(1), "{p}");
    let o = &p["affected_orgs"][0];
    assert_eq!(o["org_id"], org.to_string());
    let keys: Vec<&str> = o["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["key"].as_str().unwrap())
        .collect();
    assert!(
        keys.contains(&"conversations_days") && keys.contains(&"attachment_quota_mib"),
        "{p}"
    );
    // Not lowered (same ceilings as in force): nobody is listed.
    let (_, p) = w
        .call(
            Method::POST,
            "/v1/admin/settings/retention/preview",
            Some(json!({ "generation": g, "values": {} })),
        )
        .await;
    assert_eq!(p["affected_orgs_total"], json!(0), "{p}");
}

/// P5-IA-5 (C-972, D-341): every section answers one `fields[]` entry per setting, in order, with a
/// written label, help and default; enumerations carry a label and sentence per choice; the index
/// carries the same words for All settings' search; no section is read-only any more.
#[tokio::test]
async fn sections_answer_their_fields_in_words() {
    let w = world().await;
    let (s, index) = w.call(Method::GET, "/v1/admin/settings", None).await;
    assert_eq!(s, 200, "{index}");
    for sec in index["sections"].as_array().unwrap() {
        let id = sec["section"].as_str().unwrap();
        // The harness registers two test-only sections: they have no words.
        if id.starts_with("settings_test") {
            continue;
        }
        let v = w.section(id).await;
        let keys: Vec<&str> = v["settings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["key"].as_str().unwrap())
            .collect();
        let fields: Vec<&str> = v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["key"].as_str().unwrap())
            .collect();
        assert_eq!(keys, fields, "{id}: fields follow the settings");
        assert_eq!(
            sec["fields"].as_array().unwrap().len(),
            fields.len(),
            "{id}: index"
        );
        assert_eq!(v["editable"], json!(true), "{id}");
        assert!(v["current"].is_null(), "{id}");
        for f in v["fields"].as_array().unwrap() {
            for k in ["label", "help", "default_display", "kind"] {
                assert!(f[k].as_str().is_some_and(|t| !t.is_empty()), "{id}: {f}");
            }
            let text = format!("{} {}", f["label"], f["help"]);
            assert!(!text.contains('§') && !text.contains("docs/"), "{id}: {f}");
        }
    }
    let g = w.section("governance").await;
    let posture = g["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == "governance.llm.minimum_posture")
        .unwrap();
    assert_eq!(posture["kind"], "enum");
    let choices = posture["choices"].as_array().unwrap();
    assert_eq!(choices.len(), 2);
    assert!(
        choices
            .iter()
            .all(|c| c["label"] != c["value"] && c["description"].as_str().is_some())
    );
    // The e-mail password is a secret field: its value is never in the answer.
    let e = w.section("email").await;
    let pw = e["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "email.smtp_password")
        .unwrap();
    assert!(pw["value"].is_null());
}
