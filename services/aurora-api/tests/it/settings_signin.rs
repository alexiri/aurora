#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SET-4 (D-235(b), D-236, D-237, D-238(b), D-309, C-780, C-781): sign-in and e-mail as live
//! settings — providers added in the dashboard (sealed secrets, live on every replica), Helm
//! providers locked, the last-provider and own-provider rules, preview warnings, sessions that
//! follow the lifetimes in force, the e-mail transport and the break-glass path.

use crate::common::*;
use aurora_api::domains::settings::service::{self, Via};
use aurora_testkit::oidc::{CLIENT_ID, CLIENT_SECRET, MockIdp};
use http::Method;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

const PW: &str = "smtp-canary-password";

struct W {
    h: Harness,
    helm: MockIdp,
    corp: MockIdp,
    admin_id: Uuid,
    session: Uuid,
    admin: String,
}

fn helm_toml(idp: &MockIdp, extra: &str) -> String {
    format!(
        "[auth]\ninstance_admins = [\"root@x.io\", \"new@x.io\"]\n{extra}\n[[auth.oidc_providers]]\nid = \"helm\"\ndisplay_name = \"Helm SSO\"\nissuer = \"{}\"\nclient_id = \"{CLIENT_ID}\"\nclient_secret_env = \"HELM_SECRET\"\n",
        idp.issuer()
    )
}

/// An instance whose deployment configuration defines the provider `helm` (when `helm_provider`)
/// and nothing else about sign-in. `127.0.0.1` is opened for the mock issuers.
async fn world_with(helm_provider: bool, extra_env: &[(&str, &str)], extra: &str) -> W {
    let helm = MockIdp::start().await;
    let corp = MockIdp::start().await;
    let text = if helm_provider {
        helm_toml(&helm, extra)
    } else {
        format!("[auth]\ninstance_admins = [\"root@x.io\", \"new@x.io\"]\n{extra}\n")
    };
    let mut env = vec![("HELM_SECRET", CLIENT_SECRET)];
    env.extend_from_slice(extra_env);
    // `auth.instance_admins` (Helm-only) is read from the boot configuration by sign-in.
    let h = Harness::with_config(|c| {
        c.gateway_allowed_private = vec!["127.0.0.1".into()];
        c.auth.instance_admins = vec!["root@x.io".into(), "new@x.io".into()];
    })
    .await
    .with_source(config_source_custom(&text, &env));
    let admin_id = h.user("root@x.io", true).await;
    let (session, admin) = h.good_session(admin_id).await;
    W {
        h,
        helm,
        corp,
        admin_id,
        session,
        admin,
    }
}

async fn world() -> W {
    world_with(true, &[], "").await
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
    async fn preview(&self, id: &str, values: Value) -> (u16, Value) {
        let g = self.section(id).await["generation"].as_i64().unwrap();
        self.call(
            Method::POST,
            &format!("/v1/admin/settings/{id}/preview"),
            Some(json!({ "generation": g, "values": values })),
        )
        .await
    }
    fn corp_entry(&self) -> Value {
        json!({
            "id": "corp",
            "display_name": "Corp SSO",
            "issuer": self.corp.issuer(),
            "client_id": CLIENT_ID,
            "client_secret": { "$secret": CLIENT_SECRET },
        })
    }
    async fn add_corp(&self) {
        let (s, v) = self
            .put(
                "sign_in",
                json!({ "auth.oidc_providers": [self.corp_entry()] }),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }
    async fn provider_ids(&self) -> Vec<String> {
        let (s, v) = self.h.get("/v1/instance", &[]).await;
        assert_eq!(s, 200, "{v}");
        v["oidc_providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_owned())
            .collect()
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

fn field_codes(v: &Value) -> Vec<(String, String)> {
    v["error"]["details"]["fields"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    (
                        f["path"].as_str().unwrap().to_owned(),
                        f["code"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------------------- providers

#[tokio::test]
async fn a_provider_added_in_the_dashboard_is_live_sealed_and_signs_in_on_another_replica() {
    let w = world().await;
    assert_eq!(w.provider_ids().await, ["helm"]);
    w.add_corp().await;
    // Live without a restart: the sign-in page's list and a real sign-in through it.
    assert_eq!(w.provider_ids().await, ["helm", "corp"]);
    let s = w.h.try_sign_in("corp", "new@x.io").await.unwrap();
    assert!(!s.cookie.is_empty());
    assert!(w.h.try_sign_in("helm", "root@x.io").await.is_ok());

    // The secret is sealed inline (set, with a time) and appears nowhere.
    let sec = w.section("sign_in").await;
    let list = setting(&sec, "auth.oidc_providers");
    let corp = list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "corp")
        .unwrap();
    assert_eq!(corp["source"], "dashboard");
    assert_eq!(corp["value"]["client_secret"]["$secret"]["set"], true);
    let helm = list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "helm")
        .unwrap();
    assert_eq!(
        (helm["source"].as_str(), helm["locked"].as_bool()),
        (Some("helm"), Some(true))
    );
    let (_, export) = w
        .call(Method::GET, "/v1/admin/settings/export?format=json", None)
        .await;
    let audit: Vec<String> = sqlx::query_scalar(
        "SELECT metadata::text FROM audit_log WHERE action LIKE 'instance_settings.%'",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    let mails: String =
        w.h.mail
            .sent()
            .iter()
            .map(|m| format!("{} {}", m.subject, m.body))
            .collect();
    for (what, text) in [
        ("section", sec.to_string()),
        ("export", export.to_string()),
        ("audit", audit.join(" ")),
        ("mail", mails),
    ] {
        assert!(
            !text.contains(CLIENT_SECRET),
            "the client secret leaked into the {what}"
        );
    }
    let stored: String =
        sqlx::query_scalar("SELECT value::text FROM instance_settings WHERE section = 'sign_in'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(!stored.contains(CLIENT_SECRET), "stored in plain text");
    assert!(
        stored.contains("inline:"),
        "sealed in the inline store: {stored}"
    );

    // Another replica reloads the section and can open the sealed secret.
    let replica = aurora_api::app::AppState::new(
        w.h.pool().clone(),
        config(|c| c.gateway_allowed_private = vec!["127.0.0.1".into()]),
    )
    .with_config_source(config_source_custom(
        &helm_toml(&w.helm, ""),
        &[("HELM_SECRET", CLIENT_SECRET)],
    ));
    replica.settings.reload(&replica.pool, "api").await.unwrap();
    let live = replica.auth();
    let p = live
        .auth
        .oidc_providers
        .iter()
        .find(|p| p.id == "corp")
        .expect("the provider is live on the other replica");
    let opened = aurora_api::domains::settings::live::open_sealed(
        &replica.vault,
        p.client_secret.as_ref().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(opened, CLIENT_SECRET);

    // Removed: its callback and start are unknown at once.
    let (s, v) = w.put("sign_in", json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.provider_ids().await, ["helm"]);
    let (st, _, _) =
        w.h.send(Method::GET, "/v1/auth/oidc/corp/start", &[], None)
            .await;
    assert_eq!(st, 404);
}

#[tokio::test]
async fn a_helm_provider_is_locked_and_its_id_cannot_be_reused() {
    let w = world().await;
    let mut reuse = w.corp_entry();
    reuse["id"] = json!("helm");
    let (s, v) = w
        .put("sign_in", json!({ "auth.oidc_providers": [reuse] }))
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(field_codes(&v).contains(&(
        "/auth.oidc_providers/helm/id".into(),
        "entry_id_in_helm".into()
    )));
    // A dashboard entry with another id coexists; the Helm one is untouched.
    w.add_corp().await;
    assert_eq!(w.provider_ids().await, ["helm", "corp"]);
    let sec = w.section("sign_in").await;
    let helm = &setting(&sec, "auth.oidc_providers")["entries"][0];
    assert_eq!(helm["locked"], true);
    assert_eq!(helm["value"]["client_secret_env"], "HELM_SECRET");
}

#[tokio::test]
async fn no_environment_reference_can_be_named_from_the_dashboard() {
    let w = world().await;
    let mut e = w.corp_entry();
    e["client_secret"] = Value::Null;
    e.as_object_mut().unwrap().remove("client_secret");
    e["client_secret_env"] = json!("AURORA_SIGNING_KEY");
    let (s, v) = w
        .put("sign_in", json!({ "auth.oidc_providers": [e] }))
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(
        field_codes(&v)
            .iter()
            .any(|(_, c)| c == "dashboard_reference_refused"),
        "{v}"
    );
    // A key that names a file or a credential is refused by name.
    let mut e = w.corp_entry();
    e["client_secret_file"] = json!("/etc/passwd");
    let (s, v) = w
        .put("sign_in", json!({ "auth.oidc_providers": [e] }))
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(
        field_codes(&v)
            .iter()
            .any(|(p, c)| p.ends_with("/client_secret_file") && c == "credential_in_entry"),
        "{v}"
    );
}

#[tokio::test]
async fn keep_never_carries_a_secret_to_another_issuer_or_client() {
    let w = world().await;
    w.add_corp().await;
    // Editing roles and names keeps the secret.
    let mut e = w.corp_entry();
    e["client_secret"] = json!({ "$secret": "keep" });
    e["display_name"] = json!("Corp");
    let (s, v) = w
        .put("sign_in", json!({ "auth.oidc_providers": [e.clone()] }))
        .await;
    assert_eq!(s, 200, "{v}");
    // A new client id with keep → the secret must be entered again.
    e["client_id"] = json!("someone-else");
    let (s, v) = w
        .put("sign_in", json!({ "auth.oidc_providers": [e.clone()] }))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("secret_reentry_required")),
        "{v}"
    );
    // Another issuer with keep → same.
    let other = MockIdp::start().await;
    let mut e2 = w.corp_entry();
    e2["client_secret"] = json!({ "$secret": "keep" });
    e2["issuer"] = json!(other.issuer());
    let (s, v) = w
        .put("sign_in", json!({ "auth.oidc_providers": [e2] }))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("secret_reentry_required")),
        "{v}"
    );
}

#[tokio::test]
async fn discovery_is_checked_on_save_and_the_destination_guard_applies() {
    let w = world().await;
    let mut e = w.corp_entry();
    e["issuer"] = json!("http://127.0.0.1:1");
    let (s, v) = w
        .put("sign_in", json!({ "auth.oidc_providers": [e] }))
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(
        field_codes(&v)
            .iter()
            .any(|(p, c)| p == "/auth.oidc_providers/corp/issuer" && c == "oidc_discovery_failed"),
        "{v}"
    );
    // A public name over plain http, and a metadata address, are refused before any request.
    for issuer in ["http://idp.example.com", "https://169.254.169.254"] {
        let mut e = w.corp_entry();
        e["issuer"] = json!(issuer);
        let (s, v) = w
            .put("sign_in", json!({ "auth.oidc_providers": [e] }))
            .await;
        assert_eq!(s, 422, "{issuer}: {v}");
        assert!(
            field_codes(&v)
                .iter()
                .any(|(_, c)| c == "issuer_not_https" || c == "destination_refused"),
            "{issuer}: {v}"
        );
    }
    // The provider test reports discovery without saving.
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/settings/sign_in/provider-test",
            Some(json!({ "provider": { "id": "corp", "issuer": w.corp.issuer(), "client_id": CLIENT_ID } })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["token_endpoint"].is_string());
    let (_, v) = w
        .call(
            Method::POST,
            "/v1/admin/settings/sign_in/provider-test",
            Some(json!({ "provider": { "id": "x", "issuer": "http://127.0.0.1:1", "client_id": "c" } })),
        )
        .await;
    assert_eq!(
        (v["ok"].clone(), v["error_code"].as_str()),
        (json!(false), Some("unreachable")),
        "{v}"
    );
    assert_eq!(w.provider_ids().await, ["helm"]);
}

// ------------------------------------------------------------------------------ the rules

#[tokio::test]
async fn removing_the_last_provider_is_refused_and_orphaned_identities_are_counted() {
    let w = world_with(false, &[], "").await;
    w.add_corp().await;
    w.h.try_sign_in("corp", "new@x.io").await.unwrap();
    // The writing admin has no identity at `corp`, so only the last-provider rule applies.
    let (s, v) = w.put("sign_in", json!({})).await;
    assert_eq!(s, 422, "{v}");
    assert!(
        field_codes(&v)
            .iter()
            .any(|(_, c)| c == "last_oidc_provider"),
        "{v}"
    );
    // Replacing it with another provider is allowed; the preview counts the identities that stop working.
    let other = MockIdp::start().await;
    let next = json!({ "id": "next", "issuer": other.issuer(), "client_id": CLIENT_ID, "client_secret": { "$secret": CLIENT_SECRET } });
    let (s, v) = w
        .preview("sign_in", json!({ "auth.oidc_providers": [next.clone()] }))
        .await;
    assert_eq!(s, 200, "{v}");
    let warn = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["code"] == "identities_orphaned")
        .expect("identities_orphaned");
    assert!(
        warn["message"].as_str().unwrap().starts_with("1 account"),
        "{warn}"
    );
    assert!(
        v["loosening"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["entry_id"] == "next"),
        "{v}"
    );
}

#[tokio::test]
async fn a_mapping_to_instance_admin_is_flagged_and_loosening() {
    let w = world().await;
    let mut e = w.corp_entry();
    e["role_claim"] = json!("roles");
    e["role_mappings"] = json!([{ "value": "aurora-admins", "instance_admin": true }]);
    let (s, v) = w
        .preview("sign_in", json!({ "auth.oidc_providers": [e] }))
        .await;
    assert_eq!(s, 200, "{v}");
    let codes: Vec<&str> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"grants_instance_admin"), "{v}");
    assert!(
        v["loosening"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["entry_id"] == "corp"),
        "{v}"
    );
    // The preview sealed nothing and wrote nothing.
    assert_eq!(w.provider_ids().await, ["helm"]);
}

#[tokio::test]
async fn re_pointing_the_provider_the_admin_signed_in_with_never_strands_them() {
    let w = world_with(false, &[], "").await;
    w.add_corp().await;
    // The admin signs in through `corp` (an identity is linked) and then edits that provider.
    let s = w.h.try_sign_in("corp", "root@x.io").await.unwrap();
    assert_eq!(
        s.user_id, w.admin_id,
        "the listed admin signs in as the same user"
    );
    let other = MockIdp::start().await;
    let mut moved = w.corp_entry();
    moved["issuer"] = json!(other.issuer());
    moved["client_secret"] = json!({ "$secret": CLIENT_SECRET });
    let keep_one = json!({ "id": "second", "issuer": w.helm.issuer(), "client_id": CLIENT_ID, "client_secret": { "$secret": CLIENT_SECRET } });
    // Re-pointing `corp` while it is the only provider the admin can sign in with is refused…
    let (s, v) = w
        .put(
            "sign_in",
            json!({ "auth.oidc_providers": [moved.clone(), keep_one.clone()] }),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert!(
        field_codes(&v)
            .iter()
            .any(|(_, c)| c == "removes_own_provider"),
        "{v}"
    );
    let (_, p) = w
        .preview(
            "sign_in",
            json!({ "auth.oidc_providers": [moved, keep_one] }),
        )
        .await;
    assert!(
        p["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["code"] == "removes_own_provider"),
        "{p}"
    );
    // …and nothing changed.
    assert_eq!(w.provider_ids().await, ["corp"]);
    // Adding a provider and keeping `corp` as it is stays allowed.
    let (s, v) = w.put("sign_in", json!({ "auth.oidc_providers": [w.corp_entry_keep(), json!({ "id": "second", "issuer": w.helm.issuer(), "client_id": CLIENT_ID, "client_secret": { "$secret": CLIENT_SECRET } })] })).await;
    assert_eq!(s, 200, "{v}");
}

impl W {
    fn corp_entry_keep(&self) -> Value {
        let mut e = self.corp_entry();
        e["client_secret"] = json!({ "$secret": "keep" });
        e
    }
}

// ------------------------------------------------------------------------------- sessions

#[tokio::test]
async fn a_shorter_absolute_lifetime_ends_an_older_session_at_its_next_request() {
    let w = world().await;
    let u = w.h.user("member@x.io", false).await;
    let (id, tok) = w.h.good_session(u).await;
    sqlx::query("UPDATE sessions SET created_at = now() - interval '2 hours' WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    let me = |t: &str| {
        let c = cookie(t);
        let h = &w.h;
        async move { h.get("/v1/me", &[("cookie", c.as_str())]).await.0.as_u16() }
    };
    assert_eq!(me(&tok).await, 200);
    let (s, v) = w
        .put(
            "sign_in",
            json!({ "auth.session_idle_secs": 1800, "auth.session_absolute_secs": 3600 }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        me(&tok).await,
        401,
        "created 2 h ago, the new lifetime is 1 h"
    );
    // A longer lifetime does not extend a session whose stored expiry has passed.
    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, _) = w
        .put(
            "sign_in",
            json!({ "auth.session_idle_secs": 1800, "auth.session_absolute_secs": 864_000 }),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(me(&tok).await, 401);
    // The admin's own session (made now) is untouched by both changes.
    assert_eq!(me(&w.admin).await, 200);
    let _ = w.session;
}

#[tokio::test]
async fn sign_up_cli_key_lifetime_and_loosening_orders_are_settings() {
    let w = world().await;
    let (s, v) = w
        .preview("sign_in", json!({ "auth.signup_mode": "open", "auth.org_creation": "anyone", "auth.cli_key_ttl_days": 0 }))
        .await;
    assert_eq!(s, 200, "{v}");
    let keys: Vec<&str> = v["loosening"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["key"].as_str().unwrap())
        .collect();
    for k in [
        "auth.signup_mode",
        "auth.org_creation",
        "auth.cli_key_ttl_days",
    ] {
        assert!(keys.contains(&k), "{k} loosens: {v}");
    }
    let (s, v) = w
        .put("sign_in", json!({ "auth.signup_mode": "sso_jit" }))
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, inst) = w.h.get("/v1/instance", &[]).await;
    assert_eq!(inst["signup_mode"], "sso_jit");
    // Idle may not exceed absolute (startup rule on the merged view).
    let (s, v) = w
        .put(
            "sign_in",
            json!({ "auth.session_idle_secs": 7200, "auth.session_absolute_secs": 3600 }),
        )
        .await;
    assert_eq!(s, 422, "{v}");
}

// ----------------------------------------------------------------------------------- email

/// A minimal SMTP server: accepts every message, records `(recipient, body)`.
struct Smtp {
    port: u16,
    got: Arc<Mutex<Vec<(String, String)>>>,
}

async fn smtp() -> Smtp {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let got: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let g = got.clone();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = l.accept().await else {
                return;
            };
            let g = g.clone();
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut r = BufReader::new(r);
                let _ = w.write_all(b"220 test ESMTP\r\n").await;
                let (mut rcpt, mut data, mut in_data) = (String::new(), String::new(), false);
                let mut line = String::new();
                loop {
                    line.clear();
                    if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    if in_data {
                        if line == ".\r\n" {
                            in_data = false;
                            g.lock()
                                .unwrap()
                                .push((rcpt.clone(), std::mem::take(&mut data)));
                            let _ = w.write_all(b"250 queued\r\n").await;
                        } else {
                            data.push_str(&line);
                        }
                        continue;
                    }
                    let up = line.to_ascii_uppercase();
                    let reply: &[u8] = if up.starts_with("EHLO") {
                        b"250 test\r\n"
                    } else if up.starts_with("RCPT") {
                        rcpt = line.split(['<', '>']).nth(1).unwrap_or_default().to_owned();
                        b"250 ok\r\n"
                    } else if up.starts_with("DATA") {
                        in_data = true;
                        b"354 go\r\n"
                    } else if up.starts_with("QUIT") {
                        let _ = w.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        b"250 ok\r\n"
                    };
                    let _ = w.write_all(reply).await;
                }
            });
        }
    });
    Smtp { port, got }
}

impl Smtp {
    async fn wait_for(&self, pred: impl Fn(&(String, String)) -> bool) -> (String, String) {
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(m) = self.got.lock().unwrap().iter().find(|m| pred(m)) {
                return m.clone();
            }
            assert!(Instant::now() < end, "no matching mail arrived");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[tokio::test]
async fn email_settings_are_live_the_test_mail_goes_only_to_the_caller_and_the_password_is_sealed()
{
    let mut w = world().await;
    // Use the live mailer (the harness default is the in-memory one).
    w.h.state.mailer = w.h.state.mail_live.clone();
    w.h.app = test_router(w.h.state.clone());
    let smtp = smtp().await;
    let url = format!("smtp://127.0.0.1:{}", smtp.port);
    // No transport yet: the test mail says so.
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/settings/email/test",
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (s, v["sent"].clone(), v["error_code"].as_str()),
        (200, json!(false), Some("log_only")),
        "{v}"
    );
    // Proposed values: sent to the caller, nothing saved.
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/settings/email/test",
            Some(json!({ "values": { "email.smtp_url": url, "email.from": "aurora@x.io" } })),
        )
        .await;
    assert_eq!((s, v["sent"].clone()), (200, json!(true)), "{v}");
    let m = smtp.wait_for(|m| m.1.contains("test e-mail")).await;
    assert_eq!(m.0, "root@x.io", "only to the caller");
    assert_eq!(
        setting(&w.section("email").await, "email.smtp_url")["source"],
        "default"
    );
    // Saved: every later mail uses the new transport (the admin notice of the next write).
    let (s, v) = w
        .put(
            "email",
            json!({ "email.smtp_url": url, "email.from": "aurora@x.io" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .put(
            "email",
            json!({ "email.smtp_url": url, "email.from": "aurora2@x.io" }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let m = smtp
        .wait_for(|m| {
            m.1.contains("instance settings changed") || m.1.contains("changed instance settings")
        })
        .await;
    assert_eq!(m.0, "root@x.io");
    // An unreachable server is a code, not a crash.
    let (_, v) = w
        .call(Method::POST, "/v1/admin/settings/email/test", Some(json!({ "values": { "email.smtp_url": "smtp://127.0.0.1:1", "email.from": "a@x.io" } })))
        .await;
    assert_eq!(
        (v["sent"].clone(), v["error_code"].as_str()),
        (json!(false), Some("connect")),
        "{v}"
    );
}

#[tokio::test]
async fn the_smtp_password_is_write_only_and_locked_while_the_environment_supplies_it() {
    let w = world().await;
    let (s, v) = w
        .put("email", json!({ "email.smtp_url": "smtp://127.0.0.1:1", "email.from": "a@x.io", "email.smtp_username": "u", "email.smtp_password": { "$secret": PW } }))
        .await;
    assert_eq!(s, 200, "{v}");
    let sec = w.section("email").await;
    let p = setting(&sec, "email.smtp_password");
    assert_eq!(
        (p["secret"]["set"].clone(), p["value"].clone()),
        (json!(true), Value::Null)
    );
    assert!(!sec.to_string().contains(PW));
    // A user name without a password (or the reverse) is the startup rule's refusal.
    let (s, v) = w
        .put("email", json!({ "email.smtp_url": "smtp://127.0.0.1:1", "email.from": "a@x.io", "email.smtp_username": "u" }))
        .await;
    assert_eq!(s, 422, "{v}");
    // The environment's password locks the key.
    let w2 = world_with(true, &[("AURORA_SMTP_PASSWORD", "from-env")], "").await;
    let sec = w2.section("email").await;
    let p = setting(&sec, "email.smtp_password");
    assert_eq!(
        (p["locked"].clone(), p["secret"]["from_env"].as_str()),
        (json!(true), Some("AURORA_SMTP_PASSWORD")),
        "{p}"
    );
    let (s, v) = w2
        .put("email", json!({ "email.smtp_password": { "$secret": PW } }))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("setting_locked")),
        "{v}"
    );
}

/// SEC-90 / D-310: a stored password is bound to the server and user it was saved with.
#[tokio::test]
async fn a_stored_smtp_password_is_never_carried_to_another_server_or_user() {
    let w = world().await;
    let (s, v) = w
        .put("email", json!({ "email.smtp_url": "smtp://127.0.0.1:1", "email.from": "a@x.io", "email.smtp_username": "u", "email.smtp_password": { "$secret": PW } }))
        .await;
    assert_eq!(s, 200, "{v}");
    // The attacker's server (an opened loopback address): it must see no connection.
    let attacker = smtp().await;
    let evil = format!("smtp://127.0.0.1:{}", attacker.port);
    let keep = json!({ "$secret": "keep" });
    let code = |v: &Value| v["error"]["code"].as_str().map(str::to_owned);
    for (url, user) in [(evil.as_str(), "u"), ("smtp://127.0.0.1:1", "other")] {
        let body = json!({ "email.smtp_url": url, "email.from": "a@x.io", "email.smtp_username": user, "email.smtp_password": keep });
        // Test route.
        let (s, v) = w
            .call(
                Method::POST,
                "/v1/admin/settings/email/test",
                Some(json!({ "values": body })),
            )
            .await;
        assert_eq!(
            (s, code(&v).as_deref()),
            (422, Some("secret_reentry_required")),
            "test {url} {user}: {v}"
        );
        // Write path.
        let (s, v) = w.put("email", body).await;
        assert_eq!(
            (s, code(&v).as_deref()),
            (422, Some("secret_reentry_required")),
            "put {url} {user}: {v}"
        );
    }
    assert!(attacker.got.lock().unwrap().is_empty());
    // The same connection keeps it.
    let (s, v) = w
        .put("email", json!({ "email.smtp_url": "smtp://127.0.0.1:1", "email.from": "b@x.io", "email.smtp_username": "u", "email.smtp_password": keep }))
        .await;
    assert_eq!(s, 200, "{v}");
}

/// D-310: with Helm's password, the dashboard cannot name the server or the user.
#[tokio::test]
async fn while_helm_supplies_the_smtp_password_the_server_and_user_are_not_editable() {
    let w = world_with(true, &[("AURORA_SMTP_PASSWORD", "from-env")], "").await;
    for body in [
        json!({ "email.smtp_url": "smtp://127.0.0.1:9", "email.from": "a@x.io" }),
        json!({ "email.smtp_username": "someone", "email.from": "a@x.io" }),
    ] {
        let (s, v) = w.put("email", body.clone()).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (409, Some("setting_locked")),
            "put {v}"
        );
        let (s, v) = w
            .call(
                Method::POST,
                "/v1/admin/settings/email/test",
                Some(json!({ "values": body })),
            )
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (409, Some("setting_locked")),
            "test {v}"
        );
    }
}

/// SEC-89: the SMTP server a dashboard names passes the same destination guard as Vault stores.
#[tokio::test]
async fn the_smtp_server_named_on_the_page_passes_the_destination_guard() {
    let w = world().await;
    for url in ["smtp://169.254.169.254:25", "smtp://10.0.0.5:25"] {
        let body = json!({ "email.smtp_url": url, "email.from": "a@x.io" });
        let (s, v) = w.put("email", body.clone()).await;
        assert_eq!(s, 422, "put {url}: {v}");
        assert!(
            field_codes(&v)
                .iter()
                .any(|(p, c)| p == "/email.smtp_url" && c == "destination_refused"),
            "put {url}: {v}"
        );
        let (s, v) = w
            .call(
                Method::POST,
                "/v1/admin/settings/email/test",
                Some(json!({ "values": body })),
            )
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("destination_refused")),
            "test {url}: {v}"
        );
    }
}

// ------------------------------------------------------------------------------ break-glass

#[tokio::test]
async fn break_glass_a_broken_stored_set_keeps_the_helm_provider_and_the_cli_reset_restores_it() {
    let w = world().await;
    w.add_corp().await;
    // A stored row that no longer validates (hand-edited): reloads refuse it and keep what runs.
    sqlx::query(
        "UPDATE instance_settings SET value = jsonb_set(value, '{auth.oidc_providers,0,issuer}', '\"not a url\"'), generation = generation + 1 WHERE section = 'sign_in'",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    assert!(w.h.state.settings.reload(w.h.pool(), "api").await.is_err());
    // Sign-in through the Helm provider works regardless.
    assert!(w.h.try_sign_in("helm", "root@x.io").await.is_ok());
    // The shell resets the section to its defaults, audited with source `cli`.
    let changes = service::reset(&w.h.state, &Via::Cli, "sign_in", None, None, false)
        .await
        .unwrap();
    assert!(!changes.is_empty());
    assert_eq!(w.provider_ids().await, ["helm"]);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'instance_settings.reset' AND source = 'cli' AND critical",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
}
