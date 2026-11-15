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
    let h = Harness::new()
        .await
        .with_source(config_source_custom(
            "[[auth.oidc_providers]]\nid = \"helm\"\ndisplay_name = \"Helm SSO\"\nissuer = \"https://idp.example.com\"\nclient_id = \"c\"\nclient_secret_env = \"HELM_SECRET\"\n",
            &[("HELM_SECRET", "s")],
        ));
    let a = h.user("root@x.io", true).await;
    let (_, admin) = h.good_session(a).await;
    W { h, admin }
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
            "defaults": { "cpus": 2, "memory_mib": 4096 } });
        sqlx::query(
            "INSERT INTO runners (id, name, token_hash, installation_fingerprint, capabilities) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(name)
        .bind(&[0u8; 32][..])
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
    // Runner configuration is not a setting.
    let (s, v) = w.put("agents", json!({ "runner.max_sandboxes": 9 })).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("setting_unknown")),
        "{v}"
    );
}
