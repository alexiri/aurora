#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SET-3 (D-236, D-237, C-779): providers, preference and endpoints as live settings — Helm locks
//! per endpoint and per whole value, the shared validation with field paths, the cross-rules, live
//! readers in the API, the effect of removing an endpoint on existing keys, the preview and the
//! unauthenticated endpoint test.

use crate::common::*;
use aurora_api::domains::settings::handle;
use aurora_config::{KeyOrigin, LlmEndpoint};
use http::Method;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const GOOD_KEY: &str = "sk-good-secret-value";

struct W {
    h: Harness,
    org: Uuid,
    admin: String,
    owner: String,
    central: MockServer,
    team: MockServer,
}

async fn models_server() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "m1"}]})))
        .mount(&s)
        .await;
    s
}

/// A world whose deployment configuration defines the endpoint `central` (when `helm_central`)
/// and `extra` (more TOML), with `127.0.0.1` opened in `gateway.allowed_private` for the mocks.
async fn world(helm_central: bool, extra: &str) -> W {
    let central = models_server().await;
    let team = models_server().await;
    let base = central.uri();
    let b = base.clone();
    let h = Harness::with_config(move |c| {
        c.gateway_allowed_private = vec!["127.0.0.1".into()];
        if helm_central {
            c.llm.endpoints = vec![LlmEndpoint {
                id: "central".into(),
                provider: "litellm".into(),
                base_url: Some(b),
                ..LlmEndpoint::default()
            }];
        }
    })
    .await;
    let mut text = String::from(extra);
    if helm_central {
        text.push_str(&format!(
            "\n[[llm.endpoints]]\nid = \"central\"\nprovider = \"litellm\"\nbase_url = \"{base}\"\n"
        ));
    }
    let h = h.with_helm(&text);
    let org = h.org().await;
    let a = h.user("root@x.io", true).await;
    let (session, atok) = h.good_session(a).await;
    h.reauth(session).await;
    let o = h.user("o@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    let (_, owner) = h.good_session(o).await;
    W {
        h,
        org,
        admin: atok,
        owner,
        central,
        team,
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
            .call(Method::GET, "/v1/admin/settings/llm", &self.admin, None)
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }
    async fn put(&self, values: Value) -> (u16, Value) {
        let g = self.section().await["generation"].as_i64().unwrap();
        self.call(
            Method::PUT,
            "/v1/admin/settings/llm",
            &self.admin,
            Some(json!({ "generation": g, "values": values })),
        )
        .await
    }
    async fn preview(&self, values: Value) -> (u16, Value) {
        let g = self.section().await["generation"].as_i64().unwrap();
        self.call(
            Method::POST,
            "/v1/admin/settings/llm/preview",
            &self.admin,
            Some(json!({ "generation": g, "values": values })),
        )
        .await
    }
    fn team_entry(&self) -> Value {
        json!({"id": "team", "provider": "litellm", "base_url": self.team.uri()})
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
}

fn setting<'a>(section: &'a Value, key: &str) -> &'a Value {
    section["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap_or_else(|| panic!("no setting {key}"))
}

fn fields(v: &Value) -> Vec<(String, String)> {
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

fn endpoint_ids(eff: &Value) -> Vec<String> {
    eff["llm"]["endpoints"]
        .as_array()
        .or_else(|| eff["endpoints"].as_array())
        .map(|a| {
            a.iter()
                .map(|e| e["id"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Permit `endpoint` for the organization (stored directly, as the organization's own page would).
async fn permit(w: &W, endpoint: &str) {
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(w.org)
        .bind(json!({"llm": {"endpoints": {endpoint: {}}}}))
        .execute(w.h.pool())
        .await
        .unwrap();
}

/// Criterion 4 (per-entry lock) and the live API reader of criterion 2.
#[tokio::test]
async fn a_helm_endpoint_is_locked_and_dashboard_endpoints_sit_beside_it_live() {
    let w = world(true, "").await;
    let v = w.section().await;
    let eps = setting(&v, "llm.endpoints");
    let entries = eps["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{eps}");
    assert_eq!(
        (
            entries[0]["id"].as_str(),
            entries[0]["source"].as_str(),
            entries[0]["locked"].as_bool()
        ),
        (Some("central"), Some("helm"), Some(true))
    );
    assert!(
        v["extra"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "litellm"),
        "the page offers the providers of this release: {}",
        v["extra"]
    );

    // A dashboard endpoint reusing the Helm id: refused with the field path of C-773.
    let (s, e) = w
        .put(json!({"llm.endpoints": [{"id": "central", "provider": "litellm", "base_url": w.team.uri()}]}))
        .await;
    assert_eq!(s, 422, "{e}");
    assert!(
        fields(&e).contains(&(
            "/llm.endpoints/central/id".into(),
            "entry_id_in_helm".into()
        )),
        "{e}"
    );

    // Adding `team` takes effect without a restart in the API (provider table, effective view).
    assert!(w.h.state.llm().endpoint("team").is_none());
    let (s, e) = w.put(json!({"llm.endpoints": [w.team_entry()]})).await;
    assert_eq!(s, 200, "{e}");
    assert!(w.h.state.llm().endpoint("team").is_some());
    assert!(w.h.state.llm().endpoint("central").is_some());
    permit(&w, "team").await;
    let ids = endpoint_ids(&w.effective().await);
    assert!(ids.iter().any(|i| i == "team"), "{ids:?}");
    // The Helm entry was never changed by the write.
    let v = w.section().await;
    let entries = setting(&v, "llm.endpoints")["entries"]
        .as_array()
        .unwrap()
        .clone();
    let central = entries.iter().find(|e| e["id"] == "central").unwrap();
    assert_eq!(central["source"], "helm");
    assert_eq!(central["value"]["base_url"], w.central.uri());
    let team = entries.iter().find(|e| e["id"] == "team").unwrap();
    assert_eq!(
        (team["source"].as_str(), team["locked"].as_bool()),
        (Some("dashboard"), Some(false))
    );

    // An organization owner creates an LLM key on the new endpoint at once.
    let (s, k) = w
        .call(
            Method::POST,
            &format!("/v1/orgs/{}/secrets", w.org),
            &w.owner,
            Some(json!({"name": "team-key", "kind": "llm", "llm": {"endpoint_id": "team"}, "value": GOOD_KEY})),
        )
        .await;
    assert_eq!(s, 201, "{k}");
    // …and the picker of the key form lists it.
    let (s, p) = w
        .call(
            Method::GET,
            &format!("/v1/llm/providers?org={}", w.org),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 200, "{p}");
    assert!(p.to_string().contains("\"team\""), "{p}");

    // Mirroring: the operator drops `central` from the values file; it stays as a dashboard
    // entry with its last definition, and stops being locked.
    handle::start(&w.h.state.settings, w.h.pool())
        .await
        .unwrap();
    let h2 = w.h.with_helm("");
    handle::start(&h2.state.settings, h2.pool()).await.unwrap();
    let snap = h2.state.settings.snapshot();
    let k = &snap.effective.keys["llm.endpoints"];
    let entries = k.entries.as_ref().unwrap();
    let central = entries.iter().find(|e| e.id == "central").unwrap();
    assert_eq!(central.source, aurora_config::Source::Dashboard);
    assert_eq!(central.value["base_url"], w.central.uri());
}

/// Criterion 4a: whole values lock as a whole.
#[tokio::test]
async fn the_provider_list_and_the_preference_lock_as_whole_values() {
    let w = world(
        false,
        "[llm]\nenabled_providers = [\"litellm\", \"anthropic\", \"openai\"]\nprovider_preference = [\"anthropic\", \"litellm\"]\n",
    )
    .await;
    let v = w.section().await;
    for k in ["llm.enabled_providers", "llm.provider_preference"] {
        let s = setting(&v, k);
        assert_eq!(
            (s["source"].as_str(), s["locked"].as_bool()),
            (Some("helm"), Some(true)),
            "{k}: {s}"
        );
    }
    for (k, val) in [
        ("llm.enabled_providers", json!(["litellm"])),
        ("llm.provider_preference", json!(["litellm", "anthropic"])),
    ] {
        let (s, e) = w.put(json!({ k: val })).await;
        assert_eq!(
            (s, e["error"]["code"].as_str()),
            (409, Some("setting_locked")),
            "{k}: {e}"
        );
        assert_eq!(e["error"]["details"]["key"], k);
    }
    // The endpoints are not locked as a whole by this: a dashboard endpoint is still accepted.
    let (s, e) = w.put(json!({"llm.endpoints": [w.team_entry()]})).await;
    assert_eq!(s, 200, "{e}");
}

/// Criterion 1: validation failures of a write carry the key path of the entry and the stable code.
#[tokio::test]
async fn invalid_endpoints_and_lists_are_refused_with_their_paths() {
    let w = world(false, "").await;
    let base = "https://llm.example.com";
    let cases: Vec<(Value, &str, &str)> = vec![
        (
            json!({"id": "x", "provider": "litellm", "base_url": base, "api_key": "sk-1"}),
            "/llm.endpoints/x/api_key",
            "credential_in_entry",
        ),
        (
            json!({"id": "x", "provider": "litellm", "base_url": base, "client_secret": "s"}),
            "/llm.endpoints/x/client_secret",
            "credential_in_entry",
        ),
        (
            json!({"id": "x", "provider": "litellm", "base_url": base, "bogus": 1}),
            "/llm.endpoints/x/bogus",
            "key_unknown",
        ),
        (
            json!({"id": "x", "provider": "litellm", "base_url": base, "key_origin": {"method": "provider-acount"}}),
            "/llm.endpoints/x/key_origin/method",
            "invalid",
        ),
        (
            json!({"id": "x", "provider": "litellm", "base_url": base, "allowed_paths": ["no-slash"]}),
            "/llm.endpoints/x/allowed_paths",
            "allowed_paths_invalid",
        ),
        (
            json!({"id": "x", "provider": "anthropic", "base_url": "https://anthropic.example.com"}),
            "/llm.endpoints/x",
            "invalid",
        ),
        (
            json!({"id": "x", "provider": "litellm", "base_url": "ftp://llm.example.com"}),
            "/llm.endpoints/x/base_url",
            "invalid",
        ),
        (
            json!({"id": "x", "provider": "openai", "region": "eu-central-1"}),
            "/llm.endpoints/x/region",
            "key_unknown",
        ),
        (
            json!({"id": "x", "provider": "gemini", "base_url": base}),
            "/llm.endpoints/x/provider",
            "provider_not_enabled",
        ),
    ];
    for (entry, p, code) in cases {
        let (s, e) = w
            .put(json!({"llm.endpoints": [entry.clone()], "llm.enabled_providers": ["litellm", "anthropic", "openai"]}))
            .await;
        assert_eq!(s, 422, "{entry}: {e}");
        assert!(
            fields(&e).contains(&(p.to_owned(), code.to_owned())),
            "{entry}: expected {p} {code} in {e}"
        );
    }
    // A provider-specific rule needs its provider enabled first.
    let (s, e) = w
        .put(json!({
            "llm.enabled_providers": ["litellm", "anthropic", "vertex"],
            "llm.endpoints": [{"id": "x", "provider": "vertex", "project_id": "Bad", "location": "global", "models": ["m"], "default_model": "m"}],
        }))
        .await;
    assert_eq!(s, 422, "{e}");
    assert!(
        fields(&e).contains(&(
            "/llm.endpoints/x/project_id".into(),
            "project_id_invalid".into()
        )),
        "{e}"
    );
    // The lists.
    let (s, e) = w
        .put(json!({"llm.enabled_providers": ["litellm", "nope"]}))
        .await;
    assert_eq!(s, 422, "{e}");
    assert!(
        fields(&e).contains(&("/llm.enabled_providers".into(), "provider_unknown".into())),
        "{e}"
    );
    let (s, e) = w
        .put(json!({"llm.enabled_providers": ["litellm"], "llm.provider_preference": ["anthropic", "litellm"]}))
        .await;
    assert_eq!(s, 422, "{e}");
    assert!(
        fields(&e).contains(&(
            "/llm.provider_preference".into(),
            "provider_not_enabled".into()
        )),
        "{e}"
    );
    assert_eq!(w.section().await["generation"], 0, "nothing was written");
}

/// Criterion 3: the cross-section rule on the merged view, in both directions.
#[tokio::test]
async fn require_key_origin_names_the_endpoint_without_one() {
    let w = world(false, "").await;
    let put_gov = |v: Value| {
        let w = &w;
        async move {
            let (_, g) = w
                .call(Method::GET, "/v1/admin/settings/governance", &w.admin, None)
                .await;
            w.call(
                Method::PUT,
                "/v1/admin/settings/governance",
                &w.admin,
                Some(json!({"generation": g["generation"], "values": v})),
            )
            .await
        }
    };
    let (s, e) = w.put(json!({"llm.endpoints": [w.team_entry()]})).await;
    assert_eq!(s, 200, "{e}");
    // Turning the rule on while `team` has no key origin names the endpoint.
    let (s, e) = put_gov(json!({"governance.llm.require_key_origin": true})).await;
    assert_eq!(s, 422, "{e}");
    assert!(
        fields(&e)
            .iter()
            .any(|(p, _)| p == "/llm.endpoints/team/key_origin"),
        "{e}"
    );
    // The other direction: the rule is on, an endpoint without an origin is refused.
    let (s, e) = w.put(json!({})).await;
    assert_eq!(s, 200, "{e}");
    let (s, e) = put_gov(json!({"governance.llm.require_key_origin": true})).await;
    assert_eq!(s, 200, "{e}");
    let (s, e) = w.put(json!({"llm.endpoints": [w.team_entry()]})).await;
    assert_eq!(s, 422, "{e}");
    assert!(
        fields(&e)
            .iter()
            .any(|(p, _)| p == "/llm.endpoints/team/key_origin"),
        "{e}"
    );
    let mut with_origin = w.team_entry();
    with_origin["key_origin"] = json!({"method": "endpoint"});
    let (s, e) = w.put(json!({"llm.endpoints": [with_origin]})).await;
    assert_eq!(s, 200, "{e}");
}

/// Criterion 2 (removal), 5 of the effects: keys on a removed endpoint stop being usable; the
/// preview names the organization and the count first.
#[tokio::test]
async fn removing_an_endpoint_makes_its_keys_non_permitted_and_the_preview_says_so() {
    let w = world(false, "").await;
    let (s, e) = w.put(json!({"llm.endpoints": [w.team_entry()]})).await;
    assert_eq!(s, 200, "{e}");
    permit(&w, "team").await;
    let (s, k) = w
        .call(
            Method::POST,
            &format!("/v1/orgs/{}/secrets", w.org),
            &w.owner,
            Some(json!({"name": "team-key", "kind": "llm", "llm": {"endpoint_id": "team"}, "value": GOOD_KEY})),
        )
        .await;
    assert_eq!(s, 201, "{k}");
    assert!(
        w.effective().await["non_compliant_secrets"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let (s, p) = w.preview(json!({})).await;
    assert_eq!(s, 200, "{p}");
    let warn = p["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["code"] == "secrets_become_non_permitted")
        .unwrap_or_else(|| panic!("no warning: {p}"));
    assert!(
        warn["message"].as_str().unwrap().contains("1 LLM key"),
        "{warn}"
    );
    assert_eq!(p["affected_orgs_total"], 1, "{p}");
    assert_eq!(p["affected_orgs"][0]["org_id"], w.org.to_string());
    assert_eq!(
        p["affected_orgs"][0]["changes"][0]["before"]["usable_keys"],
        1
    );
    assert_eq!(
        p["affected_orgs"][0]["changes"][0]["after"]["usable_keys"],
        0
    );
    // A change that leaves the keys alone warns about nothing.
    let mut changed = w.team_entry();
    changed["display_name"] = json!("Team");
    let (s, p) = w.preview(json!({"llm.endpoints": [changed]})).await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["affected_orgs_total"], 0, "{p}");

    // Removing it: the key is not permitted any more, at once.
    let (s, e) = w.put(json!({})).await;
    assert_eq!(s, 200, "{e}");
    assert!(w.h.state.llm().endpoint("team").is_none());
    let eff = w.effective().await;
    let bad = eff["non_compliant_secrets"].as_array().unwrap();
    assert_eq!(bad.len(), 1, "{eff}");
    assert_eq!(bad[0]["reason"], "not_permitted");
}

/// Criterion 5: the endpoint test never sends a credential and names why it failed.
#[tokio::test]
async fn the_endpoint_test_probes_without_a_credential_through_the_guard() {
    let w = world(false, "").await;
    let url = "/v1/admin/settings/llm/endpoint-test";
    let (s, v) = w
        .call(
            Method::POST,
            url,
            &w.admin,
            Some(json!({"endpoint": w.team_entry()})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["reachable"].as_bool(), v["status"].as_u64()),
        (Some(true), Some(200)),
        "{v}"
    );
    assert_eq!(v["models"], json!(["m1"]));
    let seen = w.team.received_requests().await.unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].headers.get("authorization").is_none()
            && seen[0].headers.get("x-api-key").is_none()
            && seen[0].headers.get("api-key").is_none(),
        "no credential is ever sent: {:?}",
        seen[0].headers
    );

    // A private literal that `gateway.allowed_private` does not open.
    let (s, v) = w
        .call(
            Method::POST,
            url,
            &w.admin,
            Some(json!({"endpoint": {"id": "p", "provider": "litellm", "base_url": "http://10.255.255.1:9"}})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["reachable"].as_bool(), v["error_code"].as_str()),
        (Some(false), Some("destination_refused")),
        "{v}"
    );
    // A server error is not "reachable"; a 401 is.
    let broken = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&broken)
        .await;
    let (_, v) = w
        .call(
            Method::POST,
            url,
            &w.admin,
            Some(json!({"endpoint": {"id": "b", "provider": "litellm", "base_url": broken.uri()}})),
        )
        .await;
    assert_eq!(
        (
            v["reachable"].as_bool(),
            v["error_code"].as_str(),
            v["status"].as_u64()
        ),
        (Some(false), Some("unexpected_status"), Some(503)),
        "{v}"
    );
    let locked = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&locked)
        .await;
    let (_, v) = w
        .call(
            Method::POST,
            url,
            &w.admin,
            Some(json!({"endpoint": {"id": "l", "provider": "litellm", "base_url": locked.uri()}})),
        )
        .await;
    assert_eq!(
        (v["reachable"].as_bool(), v["status"].as_u64()),
        (Some(true), Some(401)),
        "{v}"
    );
    assert!(v["models"].is_null());
    // A slow endpoint times out.
    let slow = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&slow)
        .await;
    w.h.state
        .connect
        .set_probe_timeout(Duration::from_millis(300));
    let (_, v) = w
        .call(
            Method::POST,
            url,
            &w.admin,
            Some(json!({"endpoint": {"id": "s", "provider": "litellm", "base_url": slow.uri()}})),
        )
        .await;
    assert_eq!(
        (v["reachable"].as_bool(), v["error_code"].as_str()),
        (Some(false), Some("timeout")),
        "{v}"
    );
    // Credential-like keys never reach a request; a non-admin cannot probe.
    let (s, e) = w
        .call(
            Method::POST,
            url,
            &w.admin,
            Some(json!({"endpoint": {"id": "c", "provider": "litellm", "base_url": w.team.uri(), "api_key": "sk"}})),
        )
        .await;
    assert_eq!(s, 422, "{e}");
    assert!(
        fields(&e).contains(&("/endpoint/api_key".into(), "credential_in_entry".into())),
        "{e}"
    );
    let (s, _) = w
        .call(
            Method::POST,
            url,
            &w.owner,
            Some(json!({"endpoint": w.team_entry()})),
        )
        .await;
    assert_eq!(s, 403);
    assert_eq!(
        w.team.received_requests().await.unwrap().len(),
        1,
        "the refused calls sent nothing"
    );
}

/// The preview of a litellm endpoint without a default model keeps FIX-019's warning.
#[tokio::test]
async fn the_preview_warns_about_a_litellm_endpoint_without_a_default_model() {
    let w = world(false, "").await;
    let (s, p) = w.preview(json!({"llm.endpoints": [w.team_entry()]})).await;
    assert_eq!(s, 200, "{p}");
    assert!(
        p["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["code"] == "no_default_model"),
        "{p}"
    );
    let _ = KeyOrigin::default();
}
