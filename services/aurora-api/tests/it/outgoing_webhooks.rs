//! (phase 5, P5-WEBHOOK-2, D-320, C-909, C-912, SEC-127) The `webhook` package (a send-mode
//! secret-link app, the manifest and presets as shipped): connect with a configuration and secrets,
//! the refusals, the edit, the preview, the presets, the send test and the org's switch and
//! ceilings. The URL, the secret values and the signing secret (the canaries) appear in no
//! response, no read endpoint, no audit row and no stored column.
//!
//! A child module of `connect` (it reuses that file's world and helpers).
use super::*;
use aurora_testkit::wiremock::matchers::{method, path};

const PATH_CANARY: &str = "WHPATHCANARY0xA1B2C3";
const QUERY_CANARY: &str = "WHQUERYCANARY0xD4E5F6";
const SECRET_CANARY: &str = "WHSECRETCANARY0x778899";
const SIGNING_CANARY: &str = "whsec_WHSIGNINGCANARY0123456789abcdef";

fn secret_path() -> String {
    format!("/hooks/{PATH_CANARY}/post")
}

fn url_on(host: &str) -> String {
    format!("https://{host}{}?key={QUERY_CANARY}", secret_path())
}

fn manifest() -> String {
    include_str!("../../../../plugins/builtin/webhook/plugin.yaml")
        .replace("publisher: aurora", "publisher: acme")
}

fn presets() -> Vec<(String, Vec<u8>)> {
    macro_rules! p {
        ($f:literal) => {
            (
                concat!("presets/", $f).to_owned(),
                include_bytes!(concat!("../../../../plugins/builtin/webhook/presets/", $f))
                    .to_vec(),
            )
        };
    }
    vec![
        p!("slack.json"),
        p!("mattermost.json"),
        p!("google-chat.json"),
        p!("discord.json"),
        p!("teams-workflow.json"),
        p!("ntfy-json.json"),
        p!("home-assistant.json"),
        p!("generic-json.json"),
        p!("ifttt.json"),
    ]
}

struct L {
    w: W,
    server: MockServer,
}

async fn lane() -> L {
    let mut w = world_with_files(None, &manifest(), false, &presets()).await;
    let server = MockServer::start().await;
    w.h.state = w.h.state.clone().with_connect_override(&server.uri());
    w.h.app = test_router(w.h.state.clone());
    L { w, server }
}

/// A template with a slot, a secret value in the body and one in a header.
fn config() -> Value {
    json!({
        "version": 1, "method": "POST", "content_type": "application/json",
        "template": "{\"text\": {{text}}, \"topic\": \"{{secret:topic}}\"}",
        "slots": [{"name": "text", "type": "string", "required": true, "description": "The message"}],
        "headers": [{"name": "Authorization", "value": "Bearer {{secret:topic}}"}],
        "description": "Posts to ops", "signing": false,
        "rate": {"per_minute": null, "per_day": null}, "preset": null
    })
}

fn body_with(extra: Value) -> Value {
    let mut c = config();
    for (k, v) in extra.as_object().unwrap() {
        c[k] = v.clone();
    }
    c["secret_values"] = json!({"topic": SECRET_CANARY});
    c
}

impl L {
    async fn receiver(&self, status: u16, body: &str) {
        Mock::given(method("POST"))
            .and(path(secret_path()))
            .respond_with(ResponseTemplate::new(status).set_body_string(body.to_owned()))
            .mount(&self.server)
            .await;
    }

    async fn connect_with(&self, url: &str, webhook: Option<Value>) -> (u16, Value) {
        let mut b = self.w.connect_body("link");
        b["fields"] = json!({"name": "Ops alerts", "url": url});
        if let Some(w) = webhook {
            b["webhook"] = w;
        }
        let (s, _, v) = self
            .w
            .call(
                &self.w.owner,
                Method::POST,
                "/v1/apps/webhook/connect",
                Some(b),
            )
            .await;
        (s, v)
    }

    async fn connect(&self, webhook: Value) -> Value {
        let (s, v) = self
            .connect_with(&url_on("hooks.example"), Some(webhook))
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self.w.call(&self.w.owner, m, uri, body).await;
        (s, v)
    }

    /// No canary in a response, a read endpoint, an audit row or a stored column.
    async fn clean(&self, what: &str, v: &Value) {
        let mut seen = v.to_string();
        let (_, list) = self
            .call(
                Method::GET,
                &format!("/v1/workspaces/{}/connections", self.w.ws),
                None,
            )
            .await;
        seen.push_str(&list.to_string());
        let audit: Vec<String> = sqlx::query_scalar("SELECT metadata::text FROM audit_log")
            .fetch_all(self.w.h.pool())
            .await
            .unwrap();
        seen.push_str(&audit.join("\n"));
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT concat_ws(' ', label, metadata::text, tenant_host, credentials_ref, config::text) \
             FROM app_connections",
        )
        .fetch_all(self.w.h.pool())
        .await
        .unwrap();
        seen.push_str(&rows.join("\n"));
        for canary in [PATH_CANARY, QUERY_CANARY, SECRET_CANARY, SIGNING_CANARY] {
            assert!(!seen.contains(canary), "{what}: `{canary}` leaked: {seen}");
        }
    }

    async fn set_org(&self, patch: Value) -> (u16, Value) {
        self.call(
            Method::PATCH,
            &format!("/v1/orgs/{}", self.w.org),
            Some(json!({ "settings": patch })),
        )
        .await
    }
}

#[tokio::test]
async fn connect_seals_the_secrets_stores_the_config_and_sends_nothing() {
    let l = lane().await;
    l.receiver(200, "ok").await;
    let v = l.connect(body_with(json!({}))).await;
    assert_eq!(v["kind"], "connected");
    assert_eq!(v["connection"]["label"], "Ops alerts");
    assert_eq!(v["connection"]["metadata"]["host"], "hooks.example");
    assert!(v.get("signing_secret").is_none(), "{v}");
    // Nothing reached the receiver at connect.
    assert!(l.server.received_requests().await.unwrap().is_empty());
    let id = v["connection"]["id"].as_str().unwrap();
    let cfg: Value = sqlx::query_scalar("SELECT config FROM app_connections WHERE id = $1::uuid")
        .bind(id)
        .fetch_one(l.w.h.pool())
        .await
        .unwrap();
    assert_eq!(cfg["template"], config()["template"]);
    assert!(cfg.get("secret_values").is_none());
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT unnest(credential_keys) FROM app_connections WHERE id = $1::uuid",
    )
    .bind(id)
    .fetch_all(l.w.h.pool())
    .await
    .unwrap();
    assert!(
        keys.contains(&"url".to_owned()) && keys.contains(&"secret_values".to_owned()),
        "{keys:?}"
    );
    l.clean("connect", &v).await;
}

#[tokio::test]
async fn a_generated_signing_secret_is_returned_once_and_never_again() {
    let l = lane().await;
    let v = l.connect(body_with(json!({"signing": true}))).await;
    let secret = v["signing_secret"].as_str().expect("returned on connect");
    assert!(
        secret.starts_with("whsec_") && secret.len() > 40,
        "{secret}"
    );
    let id = v["connection"]["id"].as_str().unwrap();
    let (s, view) = l
        .call(Method::GET, &format!("/v1/connections/{id}/webhook"), None)
        .await;
    assert_eq!(s, 200, "{view}");
    assert_eq!(view["signing"], true);
    assert!(!view.to_string().contains(secret));
    let (_, one) = l
        .call(Method::GET, &format!("/v1/connections/{id}"), None)
        .await;
    assert!(!one.to_string().contains(secret));
    let audit: Vec<String> = sqlx::query_scalar("SELECT metadata::text FROM audit_log")
        .fetch_all(l.w.h.pool())
        .await
        .unwrap();
    assert!(!audit.join("\n").contains(secret));
}

#[tokio::test]
async fn every_refusal_is_a_fixed_code_and_nothing_is_stored_or_sent() {
    let l = lane().await;
    let cases: Vec<(String, Option<Value>, u16, &str)> = vec![
        (
            url_on("hooks.example").replace("https://", "http://"),
            Some(body_with(json!({}))),
            422,
            "link_not_https",
        ),
        (
            format!("not a url {PATH_CANARY}"),
            Some(body_with(json!({}))),
            422,
            "link_invalid",
        ),
        (
            format!("https://10.1.2.3/{PATH_CANARY}?key={QUERY_CANARY}"),
            Some(body_with(json!({}))),
            422,
            "link_destination_refused",
        ),
        (url_on("hooks.example"), None, 422, "webhook_config_invalid"),
        (
            url_on("hooks.example"),
            Some(body_with(json!({"template": "{\"a\": {{nope}}}"}))),
            422,
            "webhook_config_invalid",
        ),
    ];
    for (url, wh, status, code) in &cases {
        let (s, v) = l.connect_with(url, wh.clone()).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (*status, Some(*code)),
            "{v}"
        );
        l.clean(code, &v).await;
    }
    // A bad configuration names its field paths and codes.
    let (_, v) = l
        .connect_with(
            &url_on("hooks.example"),
            Some(body_with(json!({"template": "{\"a\": {{nope}}}"}))),
        )
        .await;
    let fields = v["error"]["details"]["fields"].as_array().unwrap();
    assert!(
        fields
            .iter()
            .any(|f| f["path"] == "/template" && f["code"] == "placeholder_unknown"),
        "{v}"
    );
    // An unknown key is refused, not ignored.
    let (s, v) = l
        .connect_with(
            &url_on("hooks.example"),
            Some(body_with(json!({"extra": 1}))),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("webhook_config_invalid")),
        "{v}"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_connections")
        .fetch_one(l.w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert!(l.server.received_requests().await.unwrap().is_empty());
    // A private address is accepted where `gateway.allowed_private` covers it.
    l.w.h
        .state
        .connect
        .set_allowed_private(&["10.0.0.0/8".to_owned()]);
    let (s, v) = l
        .connect_with(
            &format!("https://10.1.2.3/{PATH_CANARY}"),
            Some(body_with(json!({}))),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn the_org_switch_refuses_connect_and_send_test() {
    let l = lane().await;
    let v = l.connect(body_with(json!({}))).await;
    let id = v["connection"]["id"].as_str().unwrap().to_owned();
    let (s, r) = l.set_org(json!({"outgoing_webhooks_enabled": false})).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["settings"]["outgoing_webhooks_enabled"], false);
    let (s, v) = l
        .connect_with(&url_on("hooks.example"), Some(body_with(json!({}))))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("outgoing_webhooks_disabled")),
        "{v}"
    );
    let (s, v) = l
        .call(
            Method::POST,
            &format!("/v1/connections/{id}/webhook/test"),
            Some(json!({"values": {"text": "x"}})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("outgoing_webhooks_disabled")),
        "{v}"
    );
    // Back on.
    let (s, _) = l.set_org(json!({"outgoing_webhooks_enabled": true})).await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn edit_sets_deletes_and_keeps_secrets_and_bumps_the_version_only_when_they_change() {
    let l = lane().await;
    let v = l.connect(body_with(json!({}))).await;
    let id = v["connection"]["id"].as_str().unwrap().to_owned();
    let uri = format!("/v1/connections/{id}/webhook");
    let version = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT credentials_version FROM app_connections WHERE id = $1::uuid",
        )
        .bind(&id)
        .fetch_one(l.w.h.pool())
        .await
        .unwrap()
    };
    let (s, view) = l.call(Method::GET, &uri, None).await;
    assert_eq!(s, 200, "{view}");
    assert_eq!(view["host"], "hooks.example");
    assert_eq!(view["secret_value_names"], json!(["topic"]));
    assert_eq!(
        view["effective_rate"],
        json!({"per_minute": 30, "per_day": 500, "source": "instance"})
    );
    assert!(!view.to_string().contains(SECRET_CANARY));
    let v0 = version().await;
    // The configuration only: the secret is kept, the version does not move.
    let mut put = config();
    put["description"] = json!("changed");
    put["secret_values"] = json!({});
    put["signing_secret"] = json!({"mode": "keep"});
    let (s, r) = l.call(Method::PUT, &uri, Some(put.clone())).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["description"], "changed");
    assert_eq!(r["secret_value_names"], json!(["topic"]));
    assert_eq!(version().await, v0);
    // Set another secret, with a template that uses it: the version moves.
    let mut put2 = put.clone();
    put2["template"] =
        json!("{\"text\": {{text}}, \"topic\": \"{{secret:topic}}\", \"k\": \"{{secret:key}}\"}");
    put2["secret_values"] = json!({"key": "SECOND-SECRET-VALUE"});
    let (s, r) = l.call(Method::PUT, &uri, Some(put2.clone())).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["secret_value_names"], json!(["key", "topic"]));
    assert_eq!(version().await, v0 + 1);
    // Delete one (and stop using it): refused while the template still uses it.
    let mut del = put.clone();
    del["secret_values"] = json!({"key": null});
    let (s, r) = l.call(Method::PUT, &uri, Some(put2.clone())).await;
    assert_eq!(s, 200, "{r}");
    let (s, r) = l.call(Method::PUT, &uri, Some(json!({"x": 1}))).await;
    assert_eq!(
        (s, r["error"]["code"].as_str()),
        (422, Some("webhook_config_invalid")),
        "{r}"
    );
    let (s, r) = l.call(Method::PUT, &uri, Some(del)).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["secret_value_names"], json!(["topic"]));
    // Signing: generate (returned once), keep, provided, off.
    let mut sig = put.clone();
    sig["signing"] = json!(true);
    sig["signing_secret"] = json!({"mode": "generate"});
    let (s, r) = l.call(Method::PUT, &uri, Some(sig.clone())).await;
    assert_eq!(s, 200, "{r}");
    assert!(r["signing_secret"].as_str().unwrap().starts_with("whsec_"));
    assert_eq!(r["signing"], true);
    sig["signing_secret"] = json!({"mode": "keep"});
    let (_, r) = l.call(Method::PUT, &uri, Some(sig.clone())).await;
    assert!(r.get("signing_secret").is_none(), "{r}");
    assert_eq!(r["signing"], true);
    sig["signing_secret"] = json!({"mode": "provided", "value": SIGNING_CANARY});
    let (s, r) = l.call(Method::PUT, &uri, Some(sig.clone())).await;
    assert_eq!(s, 200, "{r}");
    assert!(
        r.get("signing_secret").is_none(),
        "a provided secret is never echoed: {r}"
    );
    sig["signing_secret"] = json!({"mode": "provided", "value": "short"});
    let (s, _) = l.call(Method::PUT, &uri, Some(sig.clone())).await;
    assert_eq!(s, 422);
    sig["signing"] = json!(false);
    sig["signing_secret"] = json!({"mode": "off"});
    let (_, r) = l.call(Method::PUT, &uri, Some(sig)).await;
    assert_eq!(r["signing"], false);
    l.clean("edit", &r).await;
}

#[tokio::test]
async fn a_reconnect_keeps_the_configuration_and_the_secrets_unless_given() {
    let l = lane().await;
    let v = l.connect(body_with(json!({"signing": true}))).await;
    let id = v["connection"]["id"].as_str().unwrap().to_owned();
    let mut b = l.w.connect_body("link");
    b["fields"] = json!({"name": "Ops alerts", "url": url_on("other.example")});
    b["reconnect_connection_id"] = json!(id);
    let (s, _, r) =
        l.w.call(
            &l.w.owner,
            Method::POST,
            "/v1/apps/webhook/connect",
            Some(b),
        )
        .await;
    assert_eq!(s, 200, "{r}");
    assert!(r.get("signing_secret").is_none(), "{r}");
    let (_, view) = l
        .call(Method::GET, &format!("/v1/connections/{id}/webhook"), None)
        .await;
    assert_eq!(view["host"], "other.example");
    assert_eq!(view["description"], "Posts to ops");
    assert_eq!(view["secret_value_names"], json!(["topic"]));
    assert_eq!(view["signing"], true);
    l.clean("reconnect", &r).await;
}

#[tokio::test]
async fn preview_and_presets_need_no_network() {
    let l = lane().await;
    let (s, v) = l
        .call(
            Method::POST,
            "/v1/apps/webhook/webhook-preview",
            Some(json!({
                "config": {
                    "version": 1, "method": "POST", "content_type": "application/json",
                    "template": "{\"text\": {{text}}, \"topic\": \"{{secret:topic}}\"}",
                    "slots": [{"name": "text", "type": "string", "required": true, "description": ""}],
                    "headers": [{"name": "X-Key", "value": "{{secret:topic}}"}], "description": "",
                    "signing": false, "rate": {"per_minute": null, "per_day": null}, "preset": null,
                    "secret_value_names": ["topic"]
                },
                "values": {"text": "hi \"there\""}
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["method"], "POST");
    assert_eq!(v["headers"], json!(["X-Key"]));
    let body: Value = serde_json::from_str(v["body"].as_str().unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"text": "hi \"there\"", "topic": "«secret:topic»"})
    );
    let (s, v) = l
        .call(
            Method::POST,
            "/v1/apps/webhook/webhook-preview",
            Some(json!({"config": {
                "version": 1, "method": "POST", "content_type": "application/json", "template": "{\"a\": {{x}}}",
                "slots": [{"name": "x", "type": "string", "required": true, "description": ""}]
            }, "values": {}})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("invalid_slot")),
        "{v}"
    );
    assert_eq!(
        v["error"]["details"],
        json!({"slot": "x", "rule": "missing"})
    );
    assert!(l.server.received_requests().await.unwrap().is_empty());
    // The nine presets of the installed package.
    let (s, p) = l
        .call(Method::GET, "/v1/apps/webhook/webhook-presets", None)
        .await;
    assert_eq!(s, 200, "{p}");
    let ids: Vec<&str> = p
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 9, "{ids:?}");
    assert!(ids.contains(&"slack") && ids.contains(&"ntfy-json"));
    let slack = p
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == "slack")
        .unwrap();
    assert_eq!(slack["suggest_for_hosts"], json!(["hooks.slack.com"]));
    let (s, _) = l
        .call(Method::GET, "/v1/apps/ics-calendar/webhook-presets", None)
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn the_send_test_posts_the_rendered_body_with_the_allowed_headers_and_a_verifying_signature()
{
    let l = lane().await;
    l.receiver(200, "received").await;
    let v = l.connect(body_with(json!({"signing": true}))).await;
    let id = v["connection"]["id"].as_str().unwrap().to_owned();
    let signing = v["signing_secret"].as_str().unwrap().to_owned();
    let uri = format!("/v1/connections/{id}/webhook/test");
    // A dry run renders the stored configuration with the secrets masked and sends nothing.
    let (s, d) = l
        .call(
            Method::POST,
            &uri,
            Some(json!({"values": {"text": "hello"}, "dry_run": true})),
        )
        .await;
    assert_eq!(s, 200, "{d}");
    assert!(
        d["body"].as_str().unwrap().contains("«secret:topic»")
            && !d.to_string().contains(SECRET_CANARY)
    );
    assert!(l.server.received_requests().await.unwrap().is_empty());
    // A real send.
    let (s, r) = l
        .call(
            Method::POST,
            &uri,
            Some(json!({"values": {"text": "hello"}})),
        )
        .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["status"], 200);
    assert_eq!(r["body"], "received");
    let delivery = r["delivery_id"].as_str().unwrap();
    let got = l.server.received_requests().await.unwrap();
    assert_eq!(got.len(), 1);
    let req = &got[0];
    let sent: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(sent, json!({"text": "hello", "topic": SECRET_CANARY}));
    let h = |n: &str| req.headers.get(n).map(|v| v.to_str().unwrap().to_owned());
    assert_eq!(h("content-type").as_deref(), Some("application/json"));
    assert_eq!(
        h("authorization").unwrap(),
        format!("Bearer {SECRET_CANARY}")
    );
    assert_eq!(h("x-aurora-delivery").as_deref(), Some(delivery));
    assert_eq!(h("user-agent").as_deref(), Some("Aurora-Webhook/1"));
    // The signature verifies: HMAC-SHA256(secret, "<t>.<body>").
    let sig = h("x-aurora-signature").unwrap();
    let t: u64 = sig
        .split(',')
        .next()
        .unwrap()
        .trim_start_matches("t=")
        .parse()
        .unwrap();
    assert_eq!(sig, aurora_webhook::sign(signing.as_bytes(), t, &req.body));
    // An invalid value is refused with the slot and rule, before anything is sent.
    let (s, e) = l
        .call(Method::POST, &uri, Some(json!({"values": {"text": 1}})))
        .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (422, Some("invalid_slot")),
        "{e}"
    );
    assert_eq!(l.server.received_requests().await.unwrap().len(), 1);
    // The audit row names the status and delivery id, nothing else.
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT metadata::text FROM audit_log WHERE action = 'webhook.test_sent'",
    )
    .fetch_all(l.w.h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].contains(delivery));
    l.clean("send test", &r).await;
}

#[tokio::test]
async fn the_send_test_is_rate_counted_and_withholds_an_echoed_secret() {
    let l = lane().await;
    let mut c = body_with(json!({}));
    c["rate"] = json!({"per_minute": 2, "per_day": null});
    let v = l.connect(c).await;
    let id = v["connection"]["id"].as_str().unwrap().to_owned();
    let uri = format!("/v1/connections/{id}/webhook/test");
    l.receiver(200, "ok").await;
    for _ in 0..2 {
        let (s, r) = l
            .call(Method::POST, &uri, Some(json!({"values": {"text": "x"}})))
            .await;
        assert_eq!(s, 200, "{r}");
    }
    let (s, r) = l
        .call(Method::POST, &uri, Some(json!({"values": {"text": "x"}})))
        .await;
    assert_eq!(
        (s, r["error"]["code"].as_str()),
        (429, Some("webhook_rate_limited")),
        "{r}"
    );
    assert!(r["error"]["details"]["retry_after_secs"].as_u64().unwrap() >= 1);
    // A dry run is not rate counted.
    let (s, _) = l
        .call(
            Method::POST,
            &uri,
            Some(json!({"values": {"text": "x"}, "dry_run": true})),
        )
        .await;
    assert_eq!(s, 200);
    // A receiver that echoes a secret value: the answer is withheld.
    let l2 = lane().await;
    l2.receiver(200, &format!("you sent {SECRET_CANARY}")).await;
    let v = l2.connect(body_with(json!({}))).await;
    let id = v["connection"]["id"].as_str().unwrap();
    let (s, r) = l2
        .call(
            Method::POST,
            &format!("/v1/connections/{id}/webhook/test"),
            Some(json!({"values": {"text": "x"}})),
        )
        .await;
    assert_eq!(
        (s, r["error"]["code"].as_str()),
        (502, Some("upstream_echoed_credential")),
        "{r}"
    );
    assert!(!r.to_string().contains(SECRET_CANARY));
}

#[tokio::test]
async fn the_org_ceiling_is_checked_against_the_instances_and_lowers_an_existing_connection() {
    let l = lane().await;
    let mut c = body_with(json!({}));
    c["rate"] = json!({"per_minute": 20, "per_day": null});
    let v = l.connect(c).await;
    let id = v["connection"]["id"].as_str().unwrap().to_owned();
    let uri = format!("/v1/connections/{id}/webhook");
    let (_, view) = l.call(Method::GET, &uri, None).await;
    assert_eq!(
        view["effective_rate"],
        json!({"per_minute": 20, "per_day": 500, "source": "connection"})
    );
    // Above the instance's ceiling (30 a minute): refused.
    let (s, r) = l
        .set_org(json!({"outgoing_webhooks_per_minute_max": 31}))
        .await;
    assert_eq!(
        (s, r["error"]["code"].as_str()),
        (422, Some("above_instance_ceiling")),
        "{r}"
    );
    // Lowering the org's ceiling applies at once to the connection that set more.
    let (s, r) = l
        .set_org(
            json!({"outgoing_webhooks_per_minute_max": 5, "outgoing_webhooks_per_day_max": 100}),
        )
        .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["settings"]["outgoing_webhooks_per_minute_max"], 5);
    let (_, view) = l.call(Method::GET, &uri, None).await;
    assert_eq!(
        view["effective_rate"],
        json!({"per_minute": 5, "per_day": 100, "source": "org"})
    );
    // A connection's own limit above the org's ceiling is refused on edit.
    let mut put = config();
    put["secret_values"] = json!({});
    put["signing_secret"] = json!({"mode": "keep"});
    put["rate"] = json!({"per_minute": 6, "per_day": null});
    let (s, r) = l.call(Method::PUT, &uri, Some(put)).await;
    assert_eq!(s, 422, "{r}");
    assert!(
        r["error"]["details"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "rate_above_ceiling")
    );
    // `null` clears the org's value: the instance's applies again.
    let (s, _) = l.set_org(json!({"outgoing_webhooks_per_minute_max": null, "outgoing_webhooks_per_day_max": null})).await;
    assert_eq!(s, 200);
    let (_, view) = l.call(Method::GET, &uri, None).await;
    assert_eq!(view["effective_rate"]["per_minute"], 20);
}
