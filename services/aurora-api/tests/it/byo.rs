#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BYO governance (phase 2, RFE-025, D-59, SEC-52): posture layering, creation / regeneration
//! authorization, source lists, require-context, workspace posture, org document, warning, audit.

use crate::common::*;
use aurora_config::{ApiConfig, ByoPosture};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    member: String,
}

async fn world_with(f: impl FnOnce(&mut ApiConfig) + Send + 'static) -> W {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = d;
        f(c);
    })
    .await;
    let org = h.org().await;
    let o = h.user("o@x.io", false).await;
    let m = h.user("m@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.member(org, m, "member", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, m, "member").await;
    let (_, owner) = h.good_session(o).await;
    let (_, member) = h.good_session(m).await;
    W {
        h,
        org,
        ws,
        owner,
        member,
    }
}

/// Default configuration: floor `owners_only`, master switch on.
async fn world() -> W {
    world_with(|_| {}).await
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
    async fn bearer(&self, m: Method, uri: &str, tok: &str, body: Option<Value>) -> (u16, Value) {
        let b = bearer(tok);
        let (s, _, v) = self
            .h
            .send(m, uri, &[("authorization", b.as_str())], body)
            .await;
        (s.as_u16(), v)
    }
    async fn create(&self, tok: &str, kind: &str, name: &str) -> (u16, Value) {
        self.call(
            Method::POST,
            &format!("/v1/workspaces/{}/agents", self.ws),
            tok,
            Some(json!({"name": name, "kind": kind})),
        )
        .await
    }
    async fn byo_agent(&self) -> (String, String) {
        let (s, v) = self.create(&self.owner, "byo", "Bot").await;
        assert_eq!(s, 201, "{v}");
        (
            v["agent"]["id"].as_str().unwrap().to_owned(),
            v["token"].as_str().unwrap().to_owned(),
        )
    }
    async fn set_ws_posture(&self, tok: &str, p: Value) -> (u16, Value) {
        self.call(
            Method::PATCH,
            &format!("/v1/workspaces/{}", self.ws),
            tok,
            Some(json!({"byo_posture": p})),
        )
        .await
    }
    async fn gov_put(&self, doc: Value) -> (u16, Value) {
        let (_, cur) = self
            .call(
                Method::GET,
                &format!("/v1/orgs/{}/governance", self.org),
                &self.owner,
                None,
            )
            .await;
        self.call(
            Method::PUT,
            &format!("/v1/orgs/{}/governance", self.org),
            &self.owner,
            Some(json!({"document": doc, "revision": cur["revision"]})),
        )
        .await
    }
    async fn audit(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

#[tokio::test]
async fn default_floor_owners_only_creation_and_regeneration() {
    let w = world().await;
    // member (can_use only): refused with the posture and its level.
    let (s, v) = w.create(&w.member, "byo", "Mine").await;
    assert_eq!((s, code(&v)), (403, "byo_not_permitted"), "{v}");
    assert_eq!(v["error"]["details"]["posture"], "owners_only");
    assert_eq!(v["error"]["details"]["level"], "instance");
    assert!(
        v["error"]["details"]["manage_url"]
            .as_str()
            .unwrap()
            .contains("/governance")
    );
    // owner (can_own): created; the response carries the agent's byo view.
    let (s, v) = w.create(&w.owner, "byo", "Bot").await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["agent"]["byo"]["posture"], "owners_only");
    assert_eq!(v["agent"]["byo"]["posture_level"], "instance");
    assert!(v["agent"]["byo"]["effective_source_cidrs"].is_null());
    assert_eq!(v["agent"]["byo"]["effective_require_context"], false);
    let id = v["agent"]["id"].as_str().unwrap().to_owned();
    // hosted agents are unaffected for everyone.
    let (s, v) = w.create(&w.member, "hosted", "Hosted").await;
    assert_eq!(s, 201, "{v}");
    assert!(v["agent"]["byo"].is_null());
    // regeneration needs the same posture check.
    let uri = format!("/v1/agents/{id}/token");
    let (s, v) = w.call(Method::POST, &uri, &w.member, None).await;
    assert_eq!((s, code(&v)), (403, "byo_not_permitted"));
    let (s, v) = w.call(Method::POST, &uri, &w.owner, None).await;
    assert_eq!(s, 200, "{v}");
    assert!(v["token"].as_str().unwrap().starts_with("aug_"));
    assert_eq!(w.audit("agent.token_regenerate").await, 1);
}

#[tokio::test]
async fn floor_enabled_keeps_phase_zero_behaviour() {
    let w = world_with(|c| c.governance.byo.minimum_posture = ByoPosture::Enabled).await;
    let (s, v) = w.create(&w.member, "byo", "Mine").await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["agent"]["byo"]["posture"], "enabled");
    let id = v["agent"]["id"].as_str().unwrap();
    let (s, _) = w
        .call(
            Method::POST,
            &format!("/v1/agents/{id}/token"),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn master_switch_disables_everything_whatever_the_layers_say() {
    let w = world_with(|c| {
        c.governance.byo.enabled = false;
        c.governance.byo.minimum_posture = ByoPosture::Enabled;
    })
    .await;
    for tok in [&w.owner, &w.member] {
        let (s, v) = w.create(tok, "byo", "x").await;
        assert_eq!((s, code(&v)), (403, "byo_disabled"), "{v}");
        assert_eq!(v["error"]["details"]["level"], "instance");
    }
    // The workspace cannot loosen it; the effective view says disabled/instance.
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/workspaces/{}", w.ws),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["effective_byo_posture"]["value"], "disabled");
    assert_eq!(v["effective_byo_posture"]["level"], "instance");
    // hosted unaffected
    let (s, _) = w.create(&w.owner, "hosted", "H").await;
    assert_eq!(s, 201);
}

#[tokio::test]
async fn workspace_posture_is_tighten_only_audited_and_owner_only() {
    let w = world_with(|c| c.governance.byo.minimum_posture = ByoPosture::Enabled).await;
    // member cannot edit.
    let (s, v) = w.set_ws_posture(&w.member, json!("disabled")).await;
    assert_eq!((s, code(&v)), (403, "forbidden"));
    // owner tightens to owners_only: the member can no longer create.
    let (s, v) = w.set_ws_posture(&w.owner, json!("owners_only")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["byo_posture"], "owners_only");
    assert_eq!(v["effective_byo_posture"]["level"], "workspace");
    let (s, v) = w.create(&w.member, "byo", "x").await;
    assert_eq!((s, code(&v)), (403, "byo_not_permitted"));
    assert_eq!(v["error"]["details"]["level"], "workspace");
    // bad value
    let (s, _) = w.set_ws_posture(&w.owner, json!("sometimes")).await;
    assert_eq!(s, 400);
    // disabled blocks even the owner
    let (s, _) = w.set_ws_posture(&w.owner, json!("disabled")).await;
    assert_eq!(s, 200);
    let (s, v) = w.create(&w.owner, "byo", "x").await;
    assert_eq!((s, code(&v)), (403, "byo_disabled"));
    assert_eq!(v["error"]["details"]["level"], "workspace");
    // null inherits again
    let (s, v) = w.set_ws_posture(&w.owner, Value::Null).await;
    assert_eq!(s, 200);
    assert!(v["byo_posture"].is_null());
    assert_eq!(v["effective_byo_posture"]["value"], "enabled");
    assert_eq!(w.audit("workspace.byo_posture.update").await, 3);
}

#[tokio::test]
async fn workspace_cannot_loosen_below_floor_or_org() {
    let w = world().await; // floor owners_only
    let (s, v) = w.set_ws_posture(&w.owner, json!("enabled")).await;
    assert_eq!(
        (s, code(&v)),
        (422, "governance_below_instance_minimum"),
        "{v}"
    );
    // org tightens to disabled: workspace owners_only is below it.
    let (s, v) = w.gov_put(json!({"byo": {"posture": "disabled"}})).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.set_ws_posture(&w.owner, json!("owners_only")).await;
    assert_eq!((s, code(&v)), (422, "governance_below_instance_minimum"));
    let (s, v) = w.create(&w.owner, "byo", "x").await;
    assert_eq!((s, code(&v)), (403, "byo_disabled"));
    assert_eq!(v["error"]["details"]["level"], "org");
    assert_eq!(w.audit("workspace.byo_posture.update").await, 0);
}

#[tokio::test]
async fn org_document_validation_and_effective_view() {
    let w = world_with(|c| {
        c.governance.byo.minimum_posture = ByoPosture::OwnersOnly;
        c.governance.byo.default_source_cidrs = vec!["10.0.0.0/8".into()];
    })
    .await;
    let (s, v) = w.gov_put(json!({"byo": {"posture": "enabled"}})).await;
    assert_eq!((s, code(&v)), (422, "governance_below_instance_minimum"));
    let (s, v) = w
        .gov_put(json!({"byo": {"source_cidrs": ["11.0.0.0/16"]}}))
        .await;
    assert_eq!((s, code(&v)), (422, "byo_cidr_outside_default"), "{v}");
    for bad in ["0.0.0.0/0", "::/0", "10.1.1.1/16", "x"] {
        let (s, v) = w.gov_put(json!({"byo": {"source_cidrs": [bad]}})).await;
        assert_eq!((s, code(&v)), (422, "governance_invalid"), "{bad}: {v}");
    }
    let (s, v) = w
        .gov_put(
            json!({"byo": {"source_cidrs": ["10.4.0.0/16"], "require_context_credentials": true}}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/orgs/{}/governance/effective", w.org),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["byo"]["posture"],
        json!({"value": "owners_only", "source": "instance"})
    );
    assert_eq!(
        v["byo"]["source_cidrs"],
        json!({"value": ["10.4.0.0/16"], "source": "org"})
    );
    assert_eq!(
        v["byo"]["require_context_credentials"],
        json!({"value": true, "source": "org"})
    );
    let (s, v) = w
        .call(Method::GET, "/v1/governance/instance", &w.member, None)
        .await;
    if s == 200 {
        assert_eq!(v["byo"]["minimum_posture"], "owners_only");
        assert_eq!(v["byo"]["default_source_cidrs"], json!(["10.0.0.0/8"]));
    }
}

#[tokio::test]
async fn put_byo_validates_authorizes_audits_and_inherits() {
    let w = world_with(|c| c.governance.byo.default_source_cidrs = vec!["10.0.0.0/8".into()]).await;
    let (id, _) = w.byo_agent().await;
    let uri = format!("/v1/agents/{id}/byo");
    let put = |tok: String, body: Value| {
        let (uri, w) = (uri.clone(), &w);
        async move { w.call(Method::PUT, &uri, &tok, Some(body)).await }
    };
    // can_own only
    let (s, v) = put(
        w.member.clone(),
        json!({"source_cidrs": null, "require_context": false}),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "forbidden"));
    // refusals
    for (list, expect) in [
        (json!([]), "byo_cidr_invalid"),
        (json!(["0.0.0.0/0"]), "byo_cidr_invalid"),
        (json!(["::/0"]), "byo_cidr_invalid"),
        (json!(["10.0.0.1/8"]), "byo_cidr_invalid"),
        (json!(["junk"]), "byo_cidr_invalid"),
        (json!(["11.1.0.0/16"]), "byo_cidr_outside_default"),
        (json!(["10.0.0.0/7"]), "byo_cidr_outside_default"),
    ] {
        let (s, v) = put(
            w.owner.clone(),
            json!({"source_cidrs": list, "require_context": false}),
        )
        .await;
        assert_eq!((s, code(&v)), (422, expect), "{list}: {v}");
    }
    let many: Vec<String> = (0..33).map(|n| format!("10.{n}.0.0/16")).collect();
    let (s, v) = put(
        w.owner.clone(),
        json!({"source_cidrs": many, "require_context": false}),
    )
    .await;
    assert_eq!((s, code(&v)), (422, "byo_cidr_invalid"));
    // nothing changed and nothing audited so far
    assert_eq!(w.audit("agent.byo_settings.update").await, 0);
    // accepted: normalised, deduplicated; view shows own and effective values
    let (s, v) = put(
        w.owner.clone(),
        json!({"source_cidrs": ["10.4.0.0/16", "10.4.0.0/16", "2001:db8::/32"], "require_context": true}),
    )
    .await;
    assert_eq!(
        (s, code(&v)),
        (422, "byo_cidr_outside_default"),
        "ipv6 is outside the v4 default: {v}"
    );
    let (s, v) = put(
        w.owner.clone(),
        json!({"source_cidrs": ["10.4.0.0/16", "10.4.0.0/16"], "require_context": true}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["byo"]["source_cidrs"], json!(["10.4.0.0/16"]));
    assert_eq!(v["byo"]["effective_source_cidrs"], json!(["10.4.0.0/16"]));
    assert_eq!(v["byo"]["effective_source_level"], "agent");
    assert_eq!(v["byo"]["require_context"], true);
    assert_eq!(v["byo"]["effective_require_context_level"], "agent");
    assert_eq!(w.audit("agent.byo_settings.update").await, 1);
    // GET shows the same
    let (_, g) = w
        .call(Method::GET, &format!("/v1/agents/{id}"), &w.member, None)
        .await;
    assert_eq!(g["byo"]["source_cidrs"], json!(["10.4.0.0/16"]));
    // null inherits the instance default again
    let (s, v) = put(
        w.owner.clone(),
        json!({"source_cidrs": null, "require_context": false}),
    )
    .await;
    assert_eq!(s, 200);
    assert!(v["byo"]["source_cidrs"].is_null());
    assert_eq!(v["byo"]["effective_source_cidrs"], json!(["10.0.0.0/8"]));
    assert_eq!(v["byo"]["effective_source_level"], "instance");
    // hosted agent: 422 not_byo
    let (_, h) = w.create(&w.owner, "hosted", "H").await;
    let hid = h["agent"]["id"].as_str().unwrap();
    let (s, v) = w
        .call(
            Method::PUT,
            &format!("/v1/agents/{hid}/byo"),
            &w.owner,
            Some(json!({"source_cidrs": null, "require_context": false})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "not_byo"));
    // the DB refuses stray values on hosted agents too
    let e = sqlx::query("UPDATE agents SET byo_require_context = true WHERE id = $1::uuid")
        .bind(hid)
        .execute(w.h.pool())
        .await;
    assert!(e.is_err());
}

#[tokio::test]
async fn agent_list_carries_byo_view_for_byo_agents_only() {
    let w = world().await;
    let (_id, _) = w.byo_agent().await;
    let (_, _) = w.create(&w.owner, "hosted", "H").await;
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/workspaces/{}/agents", w.ws),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 200);
    let items = v["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|a| a["kind"] == "byo" && a["byo"]["posture"] == "owners_only")
    );
    assert!(
        items
            .iter()
            .filter(|a| a["kind"] == "hosted")
            .all(|a| a["byo"].is_null())
    );
}

#[tokio::test]
async fn require_context_container_config_and_agent_token_routes() {
    let w = world_with(|c| c.governance.byo.require_context_credentials = true).await;
    let (id, tok) = w.byo_agent().await;
    // creation response: proxy url carries the placeholder and the notes say why.
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/agents/{id}/container-config"),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["proxy_url"].as_str().unwrap().contains("<CONTEXT_TOKEN>"));
    assert_eq!(v["byo"]["require_context"], true);
    assert_eq!(v["byo"]["posture"], "owners_only");
    assert!(
        v["notes"][0]
            .as_str()
            .unwrap()
            .contains("context credential")
    );
    // self config (agent token): same placeholder, the aug_ token is not embedded.
    let (s, v) = w
        .bearer(Method::GET, "/v1/agent/container-config", &tok, None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["proxy_url"].as_str().unwrap().contains("<CONTEXT_TOKEN>"));
    assert!(!v["proxy_url"].as_str().unwrap().contains(&tok));
    // minting works and is labelled
    let (s, c) = w
        .bearer(
            Method::POST,
            "/v1/agent/context-credentials",
            &tok,
            Some(json!({"label": "aurora run: make test", "ttl_secs": 60})),
        )
        .await;
    assert_eq!(s, 201, "{c}");
    let label: String =
        sqlx::query_scalar("SELECT context_label FROM gateway_credentials WHERE kind = 'context'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(label, "aurora run: make test");
}

#[tokio::test]
async fn disabled_scope_refuses_agent_token_routes_but_not_viewing() {
    let w = world_with(|c| c.governance.byo.minimum_posture = ByoPosture::Enabled).await;
    let (id, tok) = w.byo_agent().await;
    let (s, _) = w
        .bearer(Method::GET, "/v1/agent/container-config", &tok, None)
        .await;
    assert_eq!(s, 200);
    let (s, _) = w.set_ws_posture(&w.owner, json!("disabled")).await;
    assert_eq!(s, 200);
    let (s, v) = w
        .bearer(Method::GET, "/v1/agent/container-config", &tok, None)
        .await;
    assert_eq!((s, code(&v)), (403, "byo_disabled"), "{v}");
    assert_eq!(v["error"]["details"]["level"], "workspace");
    let (s, v) = w
        .bearer(
            Method::POST,
            "/v1/agent/context-credentials",
            &tok,
            Some(json!({"label": "x"})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "byo_disabled"));
    // viewing the placeholder document stays can_use
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/agents/{id}/container-config"),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["byo"]["posture"], "disabled");
    // nothing was deleted: the token still resolves in the gateway repo
    let found = aurora_db::repo::gateway::find_credential_identity(
        w.h.pool(),
        &aurora_core::token::hash(&tok),
    )
    .await
    .unwrap();
    assert!(found.is_some());
    // loosening restores the routes without a new token
    let (s, _) = w.set_ws_posture(&w.owner, Value::Null).await;
    assert_eq!(s, 200);
    let (s, _) = w
        .bearer(Method::GET, "/v1/agent/container-config", &tok, None)
        .await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn posture_and_settings_changes_notify_gateways_with_the_right_scope() {
    let w = world_with(|c| c.governance.byo.minimum_posture = ByoPosture::Enabled).await;
    let (id, _) = w.byo_agent().await;
    let mut l = aurora_db::bus::Listener::connect(
        w.h.pool(),
        &[aurora_db::bus::Channel::GatewayInvalidate],
    )
    .await
    .unwrap();
    let (s, _) = w.set_ws_posture(&w.owner, json!("owners_only")).await;
    assert_eq!(s, 200);
    let (s, _) = w
        .call(
            Method::PUT,
            &format!("/v1/agents/{id}/byo"),
            &w.owner,
            Some(json!({"source_cidrs": ["192.0.2.0/24"], "require_context": false})),
        )
        .await;
    assert_eq!(s, 200);
    let mut scopes = Vec::new();
    while scopes.len() < 2 {
        if let aurora_db::bus::BusEvent::Notification { payload, .. } =
            tokio::time::timeout(std::time::Duration::from_secs(5), l.next())
                .await
                .unwrap()
        {
            scopes.push(payload["scope"].as_str().unwrap().to_owned());
        }
    }
    assert_eq!(scopes, ["workspace", "agent"]);
}

#[tokio::test]
async fn audit_is_atomic_with_the_change() {
    let w = world_with(|c| c.governance.byo.minimum_posture = ByoPosture::Enabled).await;
    let (id, tok) = w.byo_agent().await;
    // Make every audit insert for the BYO actions fail.
    sqlx::query(
        "CREATE FUNCTION byo_audit_fail() RETURNS trigger AS $$ BEGIN \
         IF NEW.action IN ('agent.byo_settings.update','agent.token_regenerate','workspace.byo_posture.update') \
         THEN RAISE EXCEPTION 'audit down'; END IF; RETURN NEW; END $$ LANGUAGE plpgsql",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER byo_audit_fail BEFORE INSERT ON audit_log FOR EACH ROW EXECUTE FUNCTION byo_audit_fail()")
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .call(
            Method::PUT,
            &format!("/v1/agents/{id}/byo"),
            &w.owner,
            Some(json!({"source_cidrs": ["192.0.2.0/24"], "require_context": true})),
        )
        .await;
    assert_eq!((s, code(&v)), (503, "audit_unavailable"), "{v}");
    let (cidrs, req): (Option<Vec<String>>, bool) = sqlx::query_as(
        "SELECT byo_source_cidrs, byo_require_context FROM agents WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(cidrs.is_none() && !req, "rolled back");
    let (s, v) = w.set_ws_posture(&w.owner, json!("disabled")).await;
    assert_eq!((s, code(&v)), (503, "audit_unavailable"));
    let p: Option<String> = sqlx::query_scalar("SELECT byo_posture FROM workspaces WHERE id = $1")
        .bind(w.ws)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert!(p.is_none(), "posture change rolled back");
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/agents/{id}/token"),
            &w.owner,
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (503, "audit_unavailable"));
    let still = aurora_db::repo::gateway::find_credential_identity(
        w.h.pool(),
        &aurora_core::token::hash(&tok),
    )
    .await
    .unwrap();
    assert!(still.is_some(), "the old token was not revoked");
}

#[tokio::test]
async fn rule_applies_to_all_agents_warning() {
    let w = world_with(|c| c.governance.byo.minimum_posture = ByoPosture::Enabled).await;
    let secret = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'k','generic','api.example.com','{\"kind\":\"bearer\"}','managed','x:')")
        .bind(secret).bind(w.org).bind(w.ws).execute(w.h.pool()).await.unwrap();
    let (agent, _) = w.byo_agent().await;
    let rules = json!([
        {"name": "broad", "action": "allow", "targets": [{"type": "secret", "id": secret}]},
        {"name": "broad-kind", "action": "allow", "targets": [{"type": "secrets", "scope": "any"}]},
        {"name": "scoped", "action": "allow", "targets": [{"type": "secret", "id": secret}],
         "identities": [{"kind": "agent", "id": agent}]},
        {"name": "network", "action": "allow", "targets": [{"type": "network", "host": "a.example.com"}]},
        {"name": "block", "action": "block", "targets": [{"type": "secret", "id": secret}]},
        {"name": "off", "action": "allow", "enabled": false, "targets": [{"type": "secret", "id": secret}]}
    ]);
    let (s, v) = w
        .call(
            Method::PUT,
            &format!("/v1/workspaces/{}/policy/draft", w.ws),
            &w.owner,
            Some(json!({"revision": 0, "default_action": "none", "rules": rules})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/workspaces/{}/policy/warnings", w.ws),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let hits: Vec<&Value> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["code"] == "rule_applies_to_all_agents")
        .collect();
    let names: Vec<String> = hits
        .iter()
        .map(|h| h["message"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(hits.len(), 2, "{v}");
    assert!(
        names
            .iter()
            .all(|m| m.contains("including agents created later"))
    );
    assert!(
        names
            .iter()
            .all(|m| m.contains("members creating BYO agents")),
        "posture enabled"
    );
    assert_eq!(hits[0]["severity"], "warning");
    assert_eq!(hits[0]["details"]["rule_id"], hits[0]["rule_logical_id"]);
    assert!(hits[0]["details"]["targets"].as_array().unwrap().len() == 1);
    // tightening the workspace removes the "members creating BYO agents" clause.
    let (s, _) = w.set_ws_posture(&w.owner, json!("owners_only")).await;
    assert_eq!(s, 200);
    let (_, v) = w
        .call(
            Method::GET,
            &format!("/v1/workspaces/{}/policy/warnings", w.ws),
            &w.owner,
            None,
        )
        .await;
    let any_byo = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["code"] == "rule_applies_to_all_agents")
        .any(|i| i["message"].as_str().unwrap().contains("BYO"));
    assert!(!any_byo);
}

#[tokio::test]
async fn context_label_reaches_the_activity_log_and_approval_cards() {
    let w = world_with(|c| c.governance.byo.minimum_posture = ByoPosture::Enabled).await;
    let (agent, _) = w.byo_agent().await;
    let agent: Uuid = agent.parse().unwrap();
    let gw = Uuid::now_v7();
    sqlx::query("INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) VALUES ($1,'h','v',now(),now())")
        .bind(gw)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, method, host, path, summary, status, deadline, context_label)
         VALUES ($1,$2,$3,$4,$5,'POST','api.example.com','/x',
            '{\"title\":\"T\",\"fields\":[],\"text\":null,\"link\":null,\"group\":null,\"source\":\"generic\"}',
            'pending', now() + interval '5 minutes', 'aurora run: make deploy')",
    )
    .bind(Uuid::now_v7())
    .bind(w.org)
    .bind(w.ws)
    .bind(agent)
    .bind(gw)
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, credential_kind, lane, method, host, path, decision, status, latency_ms, context_label)
         VALUES ($1, now(), $2, $3, $4, 'context', 'mitm', 'GET', 'api.example.com', '/x', 'allow', 200, 3, 'aurora run: make deploy')",
    )
    .bind(Uuid::now_v7())
    .bind(w.org)
    .bind(w.ws)
    .bind(agent)
    .execute(w.h.pool())
    .await
    .unwrap();
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/workspaces/{}/activity", w.ws),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["context_label"], "aurora run: make deploy");
    let (s, v) = w
        .call(
            Method::GET,
            &format!("/v1/approvals/pending?workspace_id={}", w.ws),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["context_label"], "aurora run: make deploy");
}
