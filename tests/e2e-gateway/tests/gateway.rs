#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! TEST-012: M2 acceptance, black box. Run with `--test-threads=1`.
mod common;

use common::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use wiremock_shim::*;

mod wiremock_shim {
    pub use aurora_testkit::wiremock::matchers::{method, path};
    pub use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
}

const SECRET: &str = "sk-e2e-generic-SECRET-value-123";
const LLM_KEY: &str = "sk-e2e-llm-KEY-value-456";

/// Mock API echoing request headers it received (as JSON) on every path.
async fn echo_server() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&s)
        .await;
    s
}

async fn litellm_server() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "m1"}]})))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&s)
        .await;
    s
}

fn hdr(req: &aurora_testkit::wiremock::Request, n: &str) -> Option<String> {
    req.headers.get(n).map(|v| v.to_str().unwrap().to_owned())
}

fn host_of(s: &MockServer) -> String {
    s.address().to_string()
}

/// Stack with one signed-up user, one BYO agent and a generic secret granted to it.
struct Fx {
    st: Stack,
    api: Api,
    agent: String,
    token: String,
    up: MockServer,
}

async fn fx(o: Opts) -> Fx {
    let t = std::time::Instant::now();
    let st = Stack::start(o).await;
    eprintln!("[e2e] stack up {:?}", t.elapsed());
    let api = st.signup("owner@e2e.test").await;
    let (agent, token) = api.byo_agent("e2e-agent").await;
    let up = echo_server().await;
    Fx {
        st,
        api,
        agent,
        token,
        up,
    }
}

impl Fx {
    async fn secret(&self) -> String {
        let id = self.api.ws_secret(json!({
            "name": "mock-api", "kind": "generic", "host_pattern": host_of(&self.up),
            "injection": {"kind": "header", "name": "x-api-key", "format": "{value}"}, "value": SECRET
        })).await;
        self.api.grant(&self.agent, &id).await;
        id
    }
    fn url(&self, p: &str) -> String {
        format!("http://{}{p}", host_of(&self.up))
    }
    fn client(&self, gw: &str) -> reqwest::Client {
        via_proxy(gw, &self.token, &self.st.ca_pem)
    }
}

#[tokio::test]
async fn s1_litellm_key_injected_bearer_agent_key_stripped() {
    let lite = litellm_server().await;
    let f = fx(Opts {
        litellm_base: Some(format!("http://{}", lite.address())),
        ..Opts::default()
    })
    .await;
    let (s, v) = f.api.post(&format!("/v1/orgs/{}/secrets", f.api.org),
        json!({"name": "lite", "kind": "llm", "llm": {"endpoint_id": "litellm"}, "value": LLM_KEY})).await;
    assert_eq!(s, 201, "{v}");
    f.api.grant(&f.agent, v["id"].as_str().unwrap()).await;
    f.api.set_policy("block", json!([])).await;
    let r = f
        .client(&f.st.gw_a)
        .post(
            format!("http://{}/v1/chat/completions", host_of(&lite))
                .replace("/v1/chat", "/v1/chat"),
        )
        .header("x-api-key", "agent-own-key")
        .body("{}")
        .send()
        .await
        .unwrap();
    let st = r.status();
    let body = r.text().await.unwrap();
    assert_eq!(st, 200, "{body}\n{}", f.st.log_tails());
    let got = lite.received_requests().await.unwrap();
    let req = got
        .iter()
        .find(|r| r.url.path() == "/v1/chat/completions")
        .unwrap();
    assert_eq!(
        hdr(req, "authorization").as_deref(),
        Some(&*format!("Bearer {LLM_KEY}"))
    );
    assert!(
        hdr(req, "x-api-key").is_none(),
        "agent key must be stripped"
    );
    // SEC-01/12: the key never appears in any API response or request-log row.
    f.assert_no_leak(&[LLM_KEY, "agent-own-key"]).await;
    f.st.finish().await;
}

impl Fx {
    /// Fetch every read surface and the raw request_log table; none may contain `needles`.
    async fn assert_no_leak(&self, needles: &[&str]) {
        let ws = &self.api.ws;
        let mut texts = vec![];
        for p in [
            format!("/v1/workspaces/{ws}/secrets"),
            format!("/v1/orgs/{}/secrets", self.api.org),
            format!("/v1/agents/{}/grants", self.agent),
            format!("/v1/agents/{}/effective", self.agent),
            format!("/v1/agents/{}/container-config", self.agent),
            format!("/v1/workspaces/{ws}/policy"),
            format!("/v1/workspaces/{ws}/approvals"),
            format!("/v1/workspaces/{ws}/activity?limit=200"),
            format!("/v1/orgs/{}/audit", self.api.org),
        ] {
            let (_, v) = self.api.get(&p).await;
            texts.push((p, v.to_string()));
        }
        let rows: Vec<String> = sqlx::query_scalar("SELECT to_jsonb(r)::text FROM request_log r")
            .fetch_all(self.st.pool())
            .await
            .unwrap();
        texts.push(("request_log".into(), rows.join("\n")));
        for (p, t) in texts {
            for n in needles {
                assert!(!t.contains(n), "{p} leaks `{n}`");
            }
        }
    }

    fn rule_block(&self, name: &str) -> Value {
        net_rule(name, "block", &host_of(&self.up), json!({}))
    }

    async fn get(&self, gw: &str, p: &str) -> (u16, String) {
        let r = self.client(gw).get(self.url(p)).send().await.unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    }

    async fn activity_until(&self, f: impl Fn(&Value) -> bool) -> Vec<Value> {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            let a = self.api.activity().await;
            if a.iter().any(&f) || Instant::now() > end {
                return a;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

#[tokio::test]
async fn s2_generic_secret_injected_and_never_leaked() {
    let f = fx(Opts::default()).await;
    f.secret().await;
    f.api.set_policy("block", json!([])).await;
    let r = f
        .client(&f.st.gw_a)
        .get(f.url("/thing"))
        .header("x-api-key", "agent-forged")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", f.st.log_tails());
    let got = f.up.received_requests().await.unwrap();
    assert_eq!(hdr(&got[0], "x-api-key").as_deref(), Some(SECRET));
    let a = f.activity_until(|e| e["path"] == "/thing").await;
    let e = a
        .iter()
        .find(|e| e["path"] == "/thing")
        .expect("request log row");
    assert_eq!(e["credential_kind"], "agent");
    assert_eq!(e["agent_id"], f.agent.as_str());
    assert_eq!(e["injected"], 1);
    assert_eq!(e["status"], 200);
    f.assert_no_leak(&[SECRET, "agent-forged"]).await;
    f.st.finish().await;
}

#[tokio::test]
async fn s3_block_rule_and_s4_default_block() {
    let f = fx(Opts::default()).await;
    // s4: default-deny. NOTE: `default_action = block` only applies to requests with an
    // injected credential and a grant always derives an allow rule, so the observable
    // default-deny for an ungranted host is the uncredentialed default (egress policy).
    f.api
        .set_policy_with(
            "block",
            json!([]),
            json!({"uncredentialed_default": "block"}),
        )
        .await;
    let (s, b) = f.get(&f.st.gw_a, "/x").await;
    assert_eq!(
        (s, err_code(&b).as_str()),
        (403, "blocked_by_egress_policy"),
        "{b}"
    );
    // Granted secret under default block: derived rule allows.
    f.secret().await;
    let (s, b) = f.get(&f.st.gw_a, "/x").await;
    assert_eq!(s, 200, "{b}");
    let before = f.up.received_requests().await.unwrap().len();
    // s3: explicit block rule on an otherwise-open default.
    f.api
        .set_policy("allow", json!([f.rule_block("no-mock")]))
        .await;
    let (s, b) = f.get(&f.st.gw_a, "/x").await;
    assert_eq!(
        (s, err_code(&b).as_str()),
        (403, "blocked_by_policy"),
        "{b}"
    );
    assert!(b.contains("no-mock"));
    assert_eq!(f.up.received_requests().await.unwrap().len(), before);
    f.st.finish().await;
}

#[tokio::test]
async fn s5_rate_limit() {
    let f = fx(Opts::default()).await;
    f.api
        .set_policy(
            "block",
            json!([net_rule(
                "rl",
                "allow",
                &host_of(&f.up),
                json!({"rate_limit": {"count": 2, "per": "minute"}})
            )]),
        )
        .await;
    for _ in 0..2 {
        assert_eq!(f.get(&f.st.gw_a, "/rl").await.0, 200);
    }
    let r = f.client(&f.st.gw_a).get(f.url("/rl")).send().await.unwrap();
    assert_eq!(r.status(), 429);
    assert!(r.headers().contains_key("retry-after"));
    let b = r.text().await.unwrap();
    assert_eq!(err_code(&b), "rate_limited", "{b}");
    f.st.finish().await;
}

async fn approval_flow(f: &Fx, gw: &str, decision: &str) -> (u16, String) {
    let c = f.client(gw);
    let url = f.url("/needs-approval");
    let h = tokio::spawn(async move {
        let r = c.get(url).send().await.unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    });
    let a = f.api.wait_pending().await;
    assert_eq!(a["status"], "pending");
    assert_eq!(a["path"], "/needs-approval");
    let (s, v) = f
        .api
        .post(
            &format!("/v1/approvals/{}/decision", a["id"].as_str().unwrap()),
            json!({"decision": decision}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    h.await.unwrap()
}

#[tokio::test]
async fn s6_approval_approve_deny_and_cross_replica() {
    let f = fx(Opts::default()).await;
    f.api
        .set_policy(
            "block",
            json!([net_rule(
                "ask",
                "allow",
                &host_of(&f.up),
                json!({"require_approval": true})
            )]),
        )
        .await;
    let (s, b) = approval_flow(&f, &f.st.gw_a, "approve").await;
    assert_eq!(s, 200, "{b}");
    let (s, b) = approval_flow(&f, &f.st.gw_a, "deny").await;
    assert_eq!(
        (s, err_code(&b).as_str()),
        (403, "manual_approval_denied"),
        "{b}"
    );
    // Held on gateway B, decided through the API: shared via the database.
    let (s, b) = approval_flow(&f, &f.st.gw_b, "approve").await;
    assert_eq!(s, 200, "{b}");
    let (s, b) = approval_flow(&f, &f.st.gw_b, "deny").await;
    assert_eq!(
        (s, err_code(&b).as_str()),
        (403, "manual_approval_denied"),
        "{b}"
    );
    let (_, list) = f
        .api
        .get(&format!("/v1/workspaces/{}/approvals", f.api.ws))
        .await;
    let st: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["status"].as_str().unwrap())
        .collect();
    assert_eq!(st.iter().filter(|s| **s == "approved").count(), 2, "{st:?}");
    assert_eq!(st.iter().filter(|s| **s == "denied").count(), 2, "{st:?}");
    f.st.finish().await;
}

#[tokio::test]
async fn s6b_approval_expires() {
    let f = fx(Opts {
        approvals_timeout_secs: Some(3),
        ..Opts::default()
    })
    .await;
    f.api
        .set_policy(
            "block",
            json!([net_rule(
                "ask",
                "allow",
                &host_of(&f.up),
                json!({"require_approval": true})
            )]),
        )
        .await;
    let (s, b) = f.get(&f.st.gw_a, "/slow-human").await;
    assert_eq!(
        (s, err_code(&b).as_str()),
        (403, "manual_approval_expired"),
        "{b}"
    );
    let a = f
        .api
        .get(&format!(
            "/v1/workspaces/{}/approvals?status=expired",
            f.api.ws
        ))
        .await
        .1;
    assert_eq!(a["items"].as_array().unwrap().len(), 1, "{a}");
    f.st.finish().await;
}

#[tokio::test]
async fn s7_invalidation_on_publish_is_immediate() {
    let f = fx(Opts::default()).await;
    f.api.set_policy("allow", json!([])).await;
    assert_eq!(f.get(&f.st.gw_a, "/warm").await.0, 200);
    assert_eq!(f.get(&f.st.gw_b, "/warm").await.0, 200);
    f.api
        .set_policy("allow", json!([f.rule_block("late-block")]))
        .await;
    for gw in [&f.st.gw_a, &f.st.gw_b] {
        // Immediately, no waiting for cache TTL (60s).
        let (s, b) = f.get(gw, "/warm").await;
        assert_eq!(
            (s, err_code(&b).as_str()),
            (403, "blocked_by_policy"),
            "{gw}: {b}"
        );
    }
    f.st.finish().await;
}

#[tokio::test]
async fn s8_context_credential_attribution() {
    let f = fx(Opts::default()).await;
    f.api
        .set_policy(
            "block",
            json!([net_rule(
                "ask",
                "allow",
                &host_of(&f.up),
                json!({"require_approval": true})
            )]),
        )
        .await;
    let r = reqwest::Client::new()
        .post(format!("{}/v1/agent/context-credentials", f.api.base))
        .bearer_auth(&f.token)
        .json(&json!({"label": "nightly job", "ttl_secs": 300}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let v: Value = r.json().await.unwrap();
    let cred = v["credential"].as_str().unwrap().to_owned();
    assert!(cred.starts_with("auc_"));
    let c = via_proxy(&f.st.gw_a, &cred, &f.st.ca_pem);
    let url = f.url("/ctx");
    let h = tokio::spawn(async move { c.get(url).send().await.unwrap().status().as_u16() });
    let a = f.api.wait_pending().await;
    assert!(a["conversation_id"].is_null(), "{a}");
    assert_eq!(a["agent"]["id"], f.agent.as_str());
    let (s, v) = f
        .api
        .post(
            &format!("/v1/approvals/{}/decision", a["id"].as_str().unwrap()),
            json!({"decision": "approve"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(h.await.unwrap(), 200);
    let act = f.activity_until(|e| e["path"] == "/ctx").await;
    let e = act.iter().find(|e| e["path"] == "/ctx").expect("log row");
    assert_eq!(e["credential_kind"], "context", "{e}");
    assert!(e["conversation_id"].is_null());
    assert_eq!(e["agent_id"], f.agent.as_str());
    let label: Option<String> =
        sqlx::query_scalar("SELECT context_label FROM gateway_credentials WHERE kind='context'")
            .fetch_optional(f.st.pool())
            .await
            .unwrap();
    assert_eq!(label.as_deref(), Some("nightly job"));
    f.st.finish().await;
}

#[tokio::test]
async fn s10_sanctioned_only_refuses_agent_supplied_llm_key() {
    let lite = litellm_server().await;
    let f = fx(Opts {
        litellm_base: Some(format!("http://{}", lite.address())),
        ..Opts::default()
    })
    .await;
    let gov = format!("/v1/orgs/{}/governance", f.api.org);
    let (s, g) = f.api.get(&gov).await;
    assert_eq!(s, 200, "{g}");
    let mut doc = g["document"].clone();
    doc["llm"]["posture"] = json!("sanctioned_only");
    let (s, v) = f
        .api
        .put(&gov, json!({"document": doc, "revision": g["revision"]}))
        .await;
    assert_eq!(s, 200, "{v}");
    f.api.set_policy("allow", json!([])).await;
    let r = f
        .client(&f.st.gw_a)
        .post(format!("http://{}/v1/chat/completions", host_of(&lite)))
        .header("authorization", "Bearer agent-own")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let b = r.text().await.unwrap();
    assert_eq!(err_code(&b), "llm_key_required", "{b}");
    assert!(
        lite.received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.url.path() != "/v1/chat/completions")
    );
    f.st.finish().await;
}

/// Minimal HTTPS upstream signed by a throwaway CA; returns (addr, ca_pem, seen request heads).
async fn https_upstream() -> (
    std::net::SocketAddr,
    String,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(vec![]).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["127.0.0.1".to_owned()])
        .unwrap()
        .signed_by(&leaf_key, &ca, &ca_key)
        .unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().clone()],
        rustls::pki_types::PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(cfg));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else {
                return;
            };
            let (acceptor, seen) = (acceptor.clone(), seen2.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = vec![0u8; 8192];
                let mut n = 0;
                while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf[n..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => n += k,
                    }
                }
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, ca.pem(), seen)
}

#[tokio::test]
async fn s11_https_connect_mitm_injects_secret() {
    let (addr, up_ca, seen) = https_upstream().await;
    let st = Stack::start(Opts {
        extra_ca_bundle: Some(up_ca),
        ..Opts::default()
    })
    .await;
    let api = st.signup("owner@e2e.test").await;
    let (agent, token) = api.byo_agent("e2e-agent").await;
    let sid = api
        .ws_secret(json!({
            "name": "tls-api", "kind": "generic", "host_pattern": addr.to_string(),
            "injection": {"kind": "bearer"}, "value": SECRET
        }))
        .await;
    api.grant(&agent, &sid).await;
    api.set_policy("block", json!([])).await;
    let r = via_proxy(&st.gw_b, &token, &st.ca_pem)
        .get(format!("https://{addr}/secure"))
        .header("authorization", "Bearer agent-forged")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", st.log_tails());
    let heads = seen.lock().unwrap().clone();
    let h = heads[0].to_ascii_lowercase();
    assert!(
        h.contains(&format!(
            "authorization: bearer {}",
            SECRET.to_ascii_lowercase()
        )),
        "{h}"
    );
    assert!(!h.contains("agent-forged"), "{h}");
    st.finish().await;
}

#[tokio::test]
async fn s4b_org_default_block_yields_blocked_by_default_policy() {
    let f = fx(Opts::default()).await;
    f.secret().await;
    f.api.set_policy("block", json!([])).await;
    assert_eq!(f.get(&f.st.gw_a, "/ok").await.0, 200);
    f.api.set_org_policy("block", json!([])).await;
    let (s, b) = f.get(&f.st.gw_a, "/ok").await;
    assert_eq!(
        (s, err_code(&b).as_str()),
        (403, "blocked_by_default_policy"),
        "{b}"
    );
    f.api.set_org_policy("none", json!([])).await;
    assert_eq!(f.get(&f.st.gw_a, "/ok").await.0, 200);
    f.st.finish().await;
}
