#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SET-2 (D-236, D-237, C-778, C-783): the instance governance ceiling as live settings — Helm
//! locks, live readers in the API without a restart, the re-evaluation job, the preview's
//! affected organizations and the organizations view.

use crate::common::*;
use aurora_config::{KeyOrigin, LlmEndpoint};
use http::Method;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    admin: String,
    owner: String,
}

async fn world_with(helm: Option<&str>) -> W {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let ca_dir = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = ca_dir;
        c.llm.endpoints = vec![LlmEndpoint {
            id: "anthropic".into(),
            provider: "anthropic".into(),
            allowed_models: vec!["claude-a".into(), "claude-b".into()],
            key_origin: Some(KeyOrigin {
                method: "provider_account".into(),
                allowed_organization_ids: vec!["org-a".into(), "org-b".into()],
                ..KeyOrigin::default()
            }),
            ..LlmEndpoint::default()
        }];
    })
    .await;
    let h = match helm {
        Some(t) => h.with_helm(t),
        None => h,
    };
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let a = h.user("root@x.io", true).await;
    let (_, atok) = h.good_session(a).await;
    let o = h.user("o@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    let (_, owner) = h.good_session(o).await;
    W {
        h,
        org,
        ws,
        admin: atok,
        owner,
    }
}

impl W {
    async fn call(&self, m: Method, uri: &str, tok: &str, body: Option<Value>) -> (u16, Value) {
        let c = cookie(tok);
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", c.as_str()), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }
    async fn section(&self) -> Value {
        let (s, v) = self
            .call(
                Method::GET,
                "/v1/admin/settings/governance",
                &self.admin,
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }
    async fn put(&self, values: Value) -> (u16, Value) {
        let g = self.section().await["generation"].as_i64().unwrap();
        self.call(
            Method::PUT,
            "/v1/admin/settings/governance",
            &self.admin,
            Some(json!({ "generation": g, "values": values })),
        )
        .await
    }
    async fn effective(&self) -> Value {
        let (s, v) = self
            .call(
                Method::GET,
                &format!("/v1/orgs/{}/governance/effective", self.org),
                &self.owner,
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }
    async fn create_byo(&self) -> (u16, Value) {
        self.call(
            Method::POST,
            &format!("/v1/workspaces/{}/agents", self.ws),
            &self.owner,
            Some(json!({"name": "Bot", "kind": "byo"})),
        )
        .await
    }
}

fn setting<'a>(section: &'a Value, key: &str) -> &'a Value {
    section["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap_or_else(|| panic!("no setting {key}"))
}

const KEYS: [&str; 12] = [
    "governance.llm.allowed_key_scopes",
    "governance.llm.minimum_posture",
    "governance.llm.require_key_origin",
    "governance.llm.known_llm_hosts_extra",
    "governance.egress.minimum_posture",
    "governance.egress.baseline_allowlist",
    "governance.egress.open_lane_minimum",
    "governance.agent_tokens.max_age_days",
    "byo.enabled",
    "byo.minimum_posture",
    "byo.default_source_cidrs",
    "byo.require_context_credentials",
];

#[tokio::test]
async fn get_shows_every_key_with_its_source_and_a_helm_key_is_locked() {
    let w = world_with(Some(
        "[governance.llm]\nminimum_posture = \"sanctioned_only\"\n",
    ))
    .await;
    let v = w.section().await;
    for k in KEYS {
        let s = setting(&v, k);
        assert!(s["source"].is_string(), "{k}: {s}");
    }
    let locked = setting(&v, "governance.llm.minimum_posture");
    assert_eq!(
        (locked["source"].as_str(), locked["locked"].as_bool()),
        (Some("helm"), Some(true))
    );
    assert_eq!(locked["value"], "sanctioned_only");
    assert_eq!(
        setting(&v, "byo.enabled")["source"],
        "default",
        "unset keys are defaults"
    );
    assert_eq!(v["extra"]["orgs_url"], "/v1/admin/settings/governance/orgs");
    // Writing the locked key is refused (SEC-89/SEC-90) and nothing changes.
    let (s, e) = w
        .put(json!({ "governance.llm.minimum_posture": "open" }))
        .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("setting_locked")),
        "{e}"
    );
    assert_eq!(w.section().await["generation"], 0);
    // An unlocked key of the same section still writes.
    let (s, e) = w.put(json!({ "byo.minimum_posture": "enabled" })).await;
    assert_eq!(s, 200, "{e}");
}

#[tokio::test]
async fn a_raised_posture_is_effective_at_once_and_names_its_setting() {
    let w = world_with(None).await;
    let before = w.effective().await;
    assert_eq!(before["llm"]["posture"]["value"], "open");
    let (s, v) = w
        .put(json!({ "governance.llm.minimum_posture": "sanctioned_only" }))
        .await;
    assert_eq!(s, 200, "{v}");
    // No restart: the same API state serves the new ceiling (C-783 names the setting).
    let after = w.effective().await;
    assert_eq!(after["llm"]["posture"]["value"], "sanctioned_only");
    assert_eq!(after["llm"]["posture"]["source"], "instance");
    assert_eq!(
        after["llm"]["posture"]["instance_setting"],
        json!({"section": "governance", "key": "governance.llm.minimum_posture"}),
        "{after}"
    );
    let (s, inst) = w
        .call(Method::GET, "/v1/governance/instance", &w.owner, None)
        .await;
    assert_eq!(s, 200, "{inst}");
    assert_eq!(inst["llm"]["minimum_posture"], "sanctioned_only");
}

#[tokio::test]
async fn the_byo_switch_is_live_and_back_again() {
    let w = world_with(None).await;
    let (s, v) = w.put(json!({ "byo.minimum_posture": "enabled" })).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.create_byo().await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = w.put(json!({ "byo.enabled": false })).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.create_byo().await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("byo_disabled")),
        "{v}"
    );
    assert_eq!(
        v["error"]["details"]["instance_setting"],
        json!({"section": "governance", "key": "byo.enabled"})
    );
    let (s, v) = w.put(json!({ "byo.enabled": true })).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.create_byo().await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
async fn the_reevaluation_job_marks_secrets_and_clears_overrides_once() {
    let w = world_with(None).await;
    let sid = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref, origin) VALUES ($1,$2,'ant','llm','anthropic','api_key','anthropic','api.anthropic.com','{\"kind\":\"bearer\"}','managed','x:',$3)")
        .bind(sid).bind(w.org)
        .bind(json!({"status": "verified", "method": "provider_account", "organization_id": "org-a", "value_version": 1}))
        .execute(w.h.pool()).await.unwrap();
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, brief, model_provider, model_endpoint_id, model_id) VALUES ($1,$2,$3,'bot','Bot','byo','','anthropic','anthropic','claude-a')")
        .bind(agent).bind(w.org).bind(w.ws).execute(w.h.pool()).await.unwrap();
    // The organization's own document (stored directly: only a change of the instance
    // settings, not an organization write, may bring the job to it) excludes both.
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(w.org)
        .bind(json!({"llm": {"endpoints": {"anthropic": {
            "allowed_models": ["claude-b"],
            "origin": {"allowed_organization_ids": ["org-b"]}}}}}))
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .put(json!({ "governance.llm.minimum_posture": "sanctioned_only" }))
        .await;
    assert_eq!(s, 200, "{v}");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let m: Option<String> =
            sqlx::query_scalar("SELECT model_endpoint_id FROM agents WHERE id = $1")
                .bind(agent)
                .fetch_one(w.h.pool())
                .await
                .unwrap();
        if m.is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the job did not clear the override"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let st: Value = sqlx::query_scalar("SELECT origin FROM secrets WHERE id = $1")
        .bind(sid)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(st["status"], "mismatch");
    let audits = |h: &Harness| {
        let pool = h.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM audit_log WHERE action = 'governance.update' \
                 AND metadata->>'reason' = 'instance_settings'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    // A rerun finds nothing to change: idempotent, and it writes nothing. The background job can
    // still hold its lock for its last writes after the override is cleared (run 44367188880):
    // wait for the lock, then count the audit rows (the job's one, none from the rerun).
    let inst = w.h.state.gov_instance();
    let deadline = Instant::now() + Duration::from_secs(10);
    let out = loop {
        if let Some(out) = aurora_api::domains::governance::reeval::run(w.h.pool(), &inst)
            .await
            .unwrap()
        {
            break out;
        }
        assert!(Instant::now() < deadline, "the job's lock was not released");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        (
            out.orgs_changed,
            out.secrets_marked,
            out.agent_overrides_cleared
        ),
        (0, 0, 0)
    );
    assert_eq!(audits(&w.h).await, 1);
}

#[tokio::test]
async fn preview_and_the_orgs_view_report_stricter_and_clamped_organizations() {
    let w = world_with(None).await;
    let stricter = w.h.org().await;
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(stricter)
        .bind(json!({"llm": {"posture": "sanctioned_only"}}))
        .execute(w.h.pool())
        .await
        .unwrap();
    let g = w.section().await["generation"].as_i64().unwrap();
    // Preview: raising the floor changes the org at `open` (and only it), with before/after.
    let (s, p) = w
        .call(
            Method::POST,
            "/v1/admin/settings/governance/preview",
            &w.admin,
            Some(json!({"generation": g, "values":
                {"governance.llm.minimum_posture": "sanctioned_only"}})),
        )
        .await;
    assert_eq!(s, 200, "{p}");
    let affected = p["affected_orgs"].as_array().unwrap();
    assert!(
        affected.iter().any(|o| o["org_id"] == w.org.to_string()
            && o["changes"][0]["key"] == "governance.llm.minimum_posture"
            && o["changes"][0]["before"] == "open"
            && o["changes"][0]["after"] == "sanctioned_only"),
        "{p}"
    );
    assert!(
        !affected.iter().any(|o| o["org_id"] == stricter.to_string()),
        "an org already above the floor is unaffected: {p}"
    );
    assert!(p["loosening"].as_array().unwrap().is_empty(), "{p}");
    // The view before the change: `stricter` lists the key.
    let (s, v) = w
        .call(
            Method::GET,
            "/v1/admin/settings/governance/orgs",
            &w.admin,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let row = |v: &Value, id: Uuid| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["org_id"] == id.to_string())
            .cloned()
            .unwrap()
    };
    assert_eq!(
        row(&v, stricter)["stricter"],
        json!(["governance.llm.minimum_posture"])
    );
    assert_eq!(row(&v, w.org)["llm_posture"], "open");
    // Apply the floor: the `open` org is clamped… (its stored posture is `open`, the floor is
    // `sanctioned_only`) and the stricter one is now simply at the floor.
    let (s, e) = w
        .put(json!({ "governance.llm.minimum_posture": "sanctioned_only" }))
        .await;
    assert_eq!(s, 200, "{e}");
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(w.org)
        .bind(json!({"llm": {"posture": "open"}}))
        .execute(w.h.pool())
        .await
        .unwrap();
    let (_, v) = w
        .call(
            Method::GET,
            "/v1/admin/settings/governance/orgs",
            &w.admin,
            None,
        )
        .await;
    let r = row(&v, w.org);
    assert_eq!(r["llm_posture"], "sanctioned_only");
    assert_eq!(r["clamped"], json!(["governance.llm.minimum_posture"]));
    // Non-admins get nothing.
    let (s, _) = w
        .call(
            Method::GET,
            "/v1/admin/settings/governance/orgs",
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 403);
}

/// SET-2 criterion 7: SET-1's write guards reach this section — 403 for a non-admin and one
/// `instance_settings.update` audit row per write. (FIX-157, D-317) No re-authentication is
/// required: an admin session that never re-authenticated writes.
#[tokio::test]
async fn writes_here_need_an_instance_admin_and_are_audited() {
    let w = world_with(None).await;
    let g = w.section().await["generation"].as_i64().unwrap();
    let body = json!({ "generation": g, "values": {"byo.enabled": false} });
    let uri = "/v1/admin/settings/governance";
    // An org owner: 403.
    let (s, e) = w.call(Method::PUT, uri, &w.owner, Some(body.clone())).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (403, Some("instance_admin_required")),
        "{e}"
    );
    let audits = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_log WHERE action = 'instance_settings.update'",
        )
        .fetch_one(w.h.pool())
        .await
        .unwrap()
    };
    assert_eq!(audits().await, 0, "refused writes leave no audit row");
    // The instance admin (never re-authenticated): 200, one audit row for this section.
    let (s, v) = w.call(Method::PUT, uri, &w.admin, Some(body)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(audits().await, 1);
}

#[tokio::test]
async fn loosening_is_reported_with_the_key_and_clamped_orgs_follow() {
    let w = world_with(None).await;
    let (s, v) = w
        .put(json!({ "governance.egress.minimum_posture": "credentialed_only" }))
        .await;
    assert_eq!(s, 200, "{v}");
    let g = w.section().await["generation"].as_i64().unwrap();
    let (s, p) = w
        .call(
            Method::POST,
            "/v1/admin/settings/governance/preview",
            &w.admin,
            Some(json!({"generation": g, "values": {}})),
        )
        .await;
    assert_eq!(s, 200, "{p}");
    assert!(
        p["loosening"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["key"] == "governance.egress.minimum_posture"),
        "{p}"
    );
}
