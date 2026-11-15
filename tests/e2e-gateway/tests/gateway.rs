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
    // (FIX-116) `approvals.timeout_secs` has a 60 s floor (the api refuses to start below it), so
    // this test waits a full minute; `via_proxy`'s client bound stays above it.
    let f = fx(Opts {
        approvals_timeout_secs: Some(60),
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

/// (phase 2, RFE-025, SEC-52) BYO governance through the real stack: the source list is judged
/// from the TLS peer (127.0.0.x here), a disabled posture is refused with 403 and restored by
/// loosening, and the agent's token is never touched.
#[tokio::test]
async fn s17_byo_source_list_and_posture_through_the_stack() {
    let f = fx(Opts::default()).await;
    f.api.set_policy("allow", json!([])).await;
    let url = f.url("/byo");
    let c = via_proxy(&f.st.gw_a, &f.token, &f.st.ca_pem);
    let status = |c: reqwest::Client, url: String| async move {
        let r = c.get(url).send().await.unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    };
    // Wait for the invalidation to apply: the gateway re-reads the identity.
    let until = |want: u16| {
        let (c, url) = (c.clone(), url.clone());
        async move {
            let t = Instant::now();
            loop {
                let (s, body) = status(c.clone(), url.clone()).await;
                if s == want || t.elapsed() > Duration::from_secs(5) {
                    return (s, body);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    assert_eq!(until(200).await.0, 200);
    let byo = format!("/v1/agents/{}/byo", f.agent);
    // A list that excludes the loopback peer: refused after authentication (a bad token is 407).
    let (s, v) = f
        .api
        .put(
            &byo,
            json!({"source_cidrs": ["203.0.113.0/24"], "require_context": false}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, body) = until(403).await;
    assert_eq!(
        (s, err_code(&body).as_str()),
        (403, "source_not_permitted"),
        "{body}"
    );
    assert!(
        body.contains("127.0.0."),
        "details.source is the TLS peer: {body}"
    );
    let bad = via_proxy(&f.st.gw_a, "aug_not-a-token", &f.st.ca_pem);
    let r = bad.get(f.url("/byo")).send().await;
    assert!(
        r.is_err() || r.unwrap().status() == 407,
        "bad token: no 403 oracle"
    );
    // A list that includes it admits again.
    let (s, _) = f
        .api
        .put(
            &byo,
            json!({"source_cidrs": ["127.0.0.0/8"], "require_context": false}),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(until(200).await.0, 200);
    // Workspace posture disabled: refused 403 `byo_disabled`; nothing deleted; loosen restores it.
    let ws = format!("/v1/workspaces/{}", f.api.ws);
    let patch = |p: Value| {
        f.api
            .call(reqwest::Method::PATCH, &ws, Some(json!({"byo_posture": p})))
    };
    let (s, v) = patch(json!("disabled")).await;
    assert_eq!(s, 200, "{v}");
    let (s, body) = until(403).await;
    assert_eq!(
        (s, err_code(&body).as_str()),
        (403, "byo_disabled"),
        "{body}"
    );
    let (s, v) = patch(Value::Null).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(until(200).await.0, 200, "same token works again");
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
/// Bound on 127.0.0.2 (P4-HAR, as for e2e-hosted): an address outside a developer shell's
/// `NO_PROXY=127.0.0.1,localhost,::1`, so the matrix clients in s12 cannot bypass the proxy.
async fn https_upstream() -> (
    std::net::SocketAddr,
    String,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    https_upstream_for(&["127.0.0.2"]).await
}

/// As [`https_upstream`], with a leaf certificate valid for `names`.
async fn https_upstream_for(
    names: &[&str],
) -> (
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
    let leaf = CertificateParams::new(names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>())
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
    let bind_ip = names[0]
        .parse::<std::net::IpAddr>()
        .map_or("127.0.0.1", |_| "127.0.0.2");
    let l = tokio::net::TcpListener::bind(format!("{bind_ip}:0"))
        .await
        .unwrap();
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

// ---- (phase 1, RFE-002, docs/18) client matrix through the TLS-only proxy listener ----

/// Runs `cmd`; `Ok(true)` = it printed `ok` and exited 0, `Ok(false)` = client not installed.
async fn matrix_run(mut cmd: tokio::process::Command) -> Result<bool, String> {
    cmd.kill_on_drop(true);
    let out = match tokio::time::timeout(Duration::from_secs(120), cmd.output()).await {
        Err(_) => return Err("timed out".into()),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Ok(Err(e)) => return Err(e.to_string()),
        Ok(Ok(o)) => o,
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() && text.contains("ok") {
        Ok(true)
    } else {
        Err(text)
    }
}

const GO_CLIENT: &str = r#"package main

import (
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
)

func main() {
	pool := x509.NewCertPool()
	pem, _ := os.ReadFile(os.Args[1])
	pool.AppendCertsFromPEM(pem)
	p, _ := url.Parse(os.Args[2])
	c := &http.Client{Transport: &http.Transport{Proxy: http.ProxyURL(p), TLSClientConfig: &tls.Config{RootCAs: pool}}}
	r, err := c.Get(os.Args[3])
	if err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
	b, _ := io.ReadAll(r.Body)
	fmt.Println(r.StatusCode, string(b))
}
"#;

const PY_CLIENT: &str = r#"
import sys, requests
ca, proxy, url = sys.argv[1:4]
r = requests.get(url, proxies={"https": proxy}, verify=ca, timeout=30)
print(r.status_code, r.text)
sys.exit(0 if r.status_code == 200 else 1)
"#;

const NODE_CLIENT: &str = r#"
const https = require('https'), fs = require('fs');
const { HttpsProxyAgent } = require(process.argv[1]);
const [ca, proxy, url] = process.argv.slice(2);
const caPem = fs.readFileSync(ca);
const agent = new HttpsProxyAgent(proxy, { ca: caPem });
const u = new URL(url);
https.get(url, { agent, ca: caPem, servername: u.hostname }, (res) => {
  let b = ''; res.on('data', (d) => (b += d));
  res.on('end', () => { console.log(res.statusCode, b); process.exit(res.statusCode === 200 ? 0 : 1); });
}).on('error', (e) => { console.log(String(e)); process.exit(1); });
"#;

/// curl (`--proxy-cacert`), Python requests/urllib3 2, Node `https-proxy-agent`, Go: each
/// completes `CONNECT` through the TLS listener and gets the MITM'd upstream answer. Chromium is
/// not installed on CI hosts and is reported as not verified.
#[tokio::test]
async fn s12_client_matrix_connect_through_the_tls_listener() {
    let (addr, up_ca, _seen) = https_upstream().await;
    let st = Stack::start(Opts {
        extra_ca_bundle: Some(up_ca),
        ..Opts::default()
    })
    .await;
    let api = st.signup("owner@e2e.test").await;
    let (_agent, token) = api.byo_agent("e2e-agent").await;
    api.set_policy("allow", json!([])).await;
    let dir = std::env::temp_dir().join(format!("aurora-matrix-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let ca = dir.join("ca.pem");
    std::fs::write(&ca, &st.ca_pem).unwrap();
    let proxy = format!("https://agent:{token}@{}", st.gw_a);
    let target = format!("https://{addr}/secure");
    let mut results: Vec<(&str, Result<bool, String>)> = vec![];

    let mut curl = tokio::process::Command::new("curl");
    curl.args([
        "-sS",
        "--fail",
        "--max-time",
        "60",
        "--proxy",
        &proxy,
        "--proxy-cacert",
    ])
    .arg(&ca)
    .arg("--cacert")
    .arg(&ca)
    .arg(&target)
    .arg("-w")
    .arg("ok %{http_code}");
    results.push(("curl --proxy-cacert", matrix_run(curl).await));

    let mut py = tokio::process::Command::new("python3");
    py.args(["-I", "-c", PY_CLIENT])
        .arg(&ca)
        .arg(&proxy)
        .arg(&target);
    results.push(("python requests/urllib3 2", matrix_run(py).await));

    let go_dir = dir.join("go");
    std::fs::create_dir_all(&go_dir).unwrap();
    std::fs::write(go_dir.join("main.go"), GO_CLIENT).unwrap();
    let mut go = tokio::process::Command::new("go");
    go.current_dir(&go_dir)
        .env("GO111MODULE", "off")
        .env("GOFLAGS", "")
        .args(["run", "main.go"])
        .arg(&ca)
        .arg(&proxy)
        .arg(&target);
    // Go prints "200 ok": the shared success check looks for "ok".
    results.push(("go net/http", matrix_run(go).await));

    // Node: https-proxy-agent from the web workspace install, when present.
    let hpa = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../web/node_modules/.pnpm/https-proxy-agent@7.0.6_supports-color@10.2.2/node_modules/https-proxy-agent",
    );
    if hpa.exists() {
        let mut node = tokio::process::Command::new("node");
        node.args(["-e", NODE_CLIENT])
            .arg(hpa.canonicalize().unwrap())
            .arg(&ca)
            .arg(&proxy)
            .arg(&target);
        results.push(("node https-proxy-agent", matrix_run(node).await));
    }
    let mut ok = 0;
    for (name, r) in &results {
        match r {
            Ok(true) => {
                ok += 1;
                eprintln!("[matrix] {name}: ok");
            }
            Ok(false) => eprintln!("[matrix] {name}: NOT INSTALLED, not verified"),
            Err(e) => panic!("[matrix] {name} failed: {e}\n{}", st.log_tails()),
        }
    }
    eprintln!("[matrix] chromium: not available here, not verified");
    assert!(
        ok >= 2,
        "at least curl and one more client must be verified"
    );
    let _ = std::fs::remove_dir_all(&dir);
    st.finish().await;
}

/// SEC-40 black box: the real gateway binary closes a plaintext connection on 4102 without a
/// response and without resolving the credential it carries.
#[tokio::test]
async fn s13_plaintext_connection_to_the_proxy_port_is_refused() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = fx(Opts::default()).await;
    let req = format!(
        "GET http://{}/x HTTP/1.1\r\nHost: {}\r\nProxy-Authorization: Basic {}\r\n\r\n",
        host_of(&f.up),
        host_of(&f.up),
        base64_basic(&format!("agent:{}", f.token)),
    );
    let mut s = tokio::net::TcpStream::connect(&f.st.gw_a).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let r = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
    assert!(r.is_ok(), "plaintext connection must be closed");
    assert!(out.is_empty(), "no response to plaintext: {out:?}");
    // The upstream never saw a request, and the gateway still serves TLS clients.
    assert!(f.up.received_requests().await.unwrap().is_empty());
    f.api.set_policy("allow", json!([])).await;
    let r = f.client(&f.st.gw_a).get(f.url("/ok")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    f.st.finish().await;
}

fn base64_basic(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s)
}

/// P1-003 (RFE-003, SEC-41): open-lane probes against the real stack.
#[tokio::test]
async fn s16_open_lane_capability_posture_and_connect_time_rules() {
    let f = fx(Opts::default()).await;
    let target = host_of(&f.up);
    let gw = f.st.gw_a.clone();
    let ca = f.st.ca_pem.clone();
    let browser = f.st.boot_credential(&f.agent, &["browser"]).await;
    let plain_boot = f.st.boot_credential(&f.agent, &[]).await;

    // Capability: a BYO token (or a boot credential without `browser`) that merely sets the
    // username `open` gains nothing; only the boot credential with `browser` gets a tunnel.
    for tok in [&f.token, &plain_boot] {
        let (s, b) = raw_connect(&gw, &ca, "open", tok, &target).await;
        assert_eq!(s, 403, "{b}");
        assert!(b.contains("open_lane_not_permitted"), "{b}");
    }
    let (s, b) = raw_connect(&gw, &ca, "open", &browser, &target).await;
    assert_eq!(s, 200, "{b}");

    // CONNECT-time rules: a path-scoped block refuses the whole tunnel.
    let host = f.up.address().ip().to_string();
    f.api
        .set_policy(
            "none",
            json!([{"name": "admin-only", "action": "block",
                    "targets": [{"type": "network", "host": host, "path": "/admin/**"}]}]),
        )
        .await;
    let (s, b) = raw_connect(&gw, &ca, "open", &browser, &target).await;
    assert_eq!(s, 403, "{b}");
    assert!(
        b.contains("blocked_by_policy") && b.contains("connect_time"),
        "{b}"
    );
    // ... while the request lane still reaches other paths.
    assert_eq!(f.get(&gw, "/ok").await.0, 200);

    // Posture: a workspace that disables the lane refuses tunnels.
    f.api
        .set_policy_with("none", json!([]), json!({"open_lane_posture": "disabled"}))
        .await;
    let (s, b) = raw_connect(&gw, &ca, "open", &browser, &target).await;
    assert_eq!(s, 403, "{b}");
    assert!(b.contains("open_lane_disabled"), "{b}");
    // allowlist_only: refused until the host is on the workspace allow-list.
    f.api
        .set_policy_with(
            "none",
            json!([]),
            json!({"open_lane_posture": "allowlist_only"}),
        )
        .await;
    let (s, b) = raw_connect(&gw, &ca, "open", &browser, &target).await;
    assert_eq!(s, 403, "{b}");
    assert!(b.contains("blocked_by_egress_policy"), "{b}");
    f.api
        .set_policy_with(
            "none",
            json!([]),
            json!({"open_lane_posture": "allowlist_only", "uncredentialed_allowlist": [{"host": host}]}),
        )
        .await;
    let (s, b) = raw_connect(&gw, &ca, "open", &browser, &target).await;
    assert_eq!(s, 200, "{b}");
    f.st.finish().await;
}

// ---- D-74 / SEC-04 / SEC-59: self-hosted app instances through the real stack -----------------

const INST_HOST: &str = "desk.acme.example.test";
const SAAS_HOST: &str = "saas.instance.example.test";
const OTHER_HOST: &str = "elsewhere.example.test";

/// A CONNECT proxy standing in for the network: every tunnel goes to `target` and the requested
/// authority is recorded. The gateway dials upstreams through it (`gateway.egress_proxy`), so the
/// DNS names above need no resolution.
async fn connect_proxy(
    target: std::net::SocketAddr,
) -> (
    std::net::SocketAddr,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else {
                return;
            };
            let seen = seen2.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if head.len() > 8192 || c.read(&mut b).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(b[0]);
                }
                let line = String::from_utf8_lossy(&head);
                let authority = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                seen.lock().unwrap().push(authority);
                let Ok(mut up) = tokio::net::TcpStream::connect(target).await else {
                    return;
                };
                if c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .await
                    .is_err()
                {
                    return;
                }
                let _ = tokio::io::copy_bidirectional(&mut c, &mut up).await;
            });
        }
    });
    (addr, seen)
}

/// Trust the fixture key, upload and approve `fixture-instance`.
async fn install_instance_fixture(st: &Stack, api: &mut Api) {
    sqlx::query("UPDATE users SET is_instance_admin = true")
        .execute(st.pool())
        .await
        .unwrap();
    api.relogin("owner@e2e.test").await;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/fixtures");
    let pubkey = std::fs::read_to_string(root.join("test-key/test-signing-key.pub")).unwrap();
    let (s, v) = api
        .post(
            "/v1/admin/trusted-keys",
            json!({"publisher": "fixtures", "name": "fixture test key", "public_key": pubkey.trim()}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let pkg = std::fs::read(root.join("dist/fixture-instance.aurora-plugin"))
        .expect("run `bash scripts/fixtures/build.sh` first");
    let (s, v) = api.upload_plugin(&pkg).await;
    assert_eq!(s, 201, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let (s, v) = api
        .post(&format!("/v1/admin/plugins/{id}/approve"), json!({}))
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("active")), "{v}");
}

/// A sealed `instance-desk` connection (optionally bound to an instance), granted to `agent`.
async fn instance_connection(
    st: &Stack,
    api: &Api,
    agent: &str,
    instance: Option<uuid::Uuid>,
    token: &str,
) -> uuid::Uuid {
    use aurora_secrets::{Purpose, SealContext, SecretVault, StoreRegistry};
    let id = uuid::Uuid::now_v7();
    let (org, ws): (uuid::Uuid, uuid::Uuid) = (api.org.parse().unwrap(), api.ws.parse().unwrap());
    let keys = {
        use base64::Engine;
        format!(
            "k1:{}",
            base64::engine::general_purpose::STANDARD.encode([b'a'; 32])
        )
    };
    let keyring = std::sync::Arc::new(aurora_crypto::Keyring::parse(&keys, "k1").unwrap());
    let vault = SecretVault::new(
        StoreRegistry::from_config(&aurora_config::SecretsSection::default(), keyring).unwrap(),
    );
    let r = vault
        .seal(
            &SealContext {
                purpose: Purpose::ConnectionCredentials,
                owner_id: id,
                org_id: org,
                workspace_id: Some(ws),
            },
            aurora_core::Secret::from(serde_json::to_vec(&json!({"api_token": token})).unwrap()),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, instance_id) VALUES ($1,$2,$3,'instance-desk','fixture-instance','1.0.0','desk','active',$4,$5)")
        .bind(id)
        .bind(org)
        .bind(ws)
        .bind(r.to_column())
        .bind(instance)
        .execute(st.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(uuid::Uuid::now_v7())
        .bind(ws)
        .bind(agent.parse::<uuid::Uuid>().unwrap())
        .bind(id)
        .execute(st.pool())
        .await
        .unwrap();
    let rule = json!({"name": "Grant: fixture-instance", "enabled": true,
        "identities": [{"kind": "agent", "id": agent}],
        "targets": [{"type": "connection", "id": id}],
        "action": "allow", "require_approval": false, "rate_limit": null, "conditions": []});
    sqlx::query("INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) VALUES ($1,$2,$3,'grant',$4,0,$5,$6)")
        .bind(uuid::Uuid::now_v7())
        .bind(org)
        .bind(ws)
        .bind(id)
        .bind(uuid::Uuid::now_v7())
        .bind(rule)
        .execute(st.pool())
        .await
        .unwrap();
    // Rows written behind the API's back: tell the gateways (as the API would) so that no cached
    // resolution of an earlier request hides them.
    sqlx::query("SELECT pg_notify('aurora_gateway_invalidate', $1)")
        .bind(json!({"scope": "org", "id": org}).to_string())
        .execute(st.pool())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    id
}

#[tokio::test]
async fn s18_instance_bound_connection_is_injected_on_its_instance_host_only() {
    let (up, up_ca, seen) = https_upstream_for(&[INST_HOST, SAAS_HOST, OTHER_HOST]).await;
    let (proxy, tunnels) = connect_proxy(up).await;
    let st = Stack::start(Opts {
        extra_ca_bundle: Some(up_ca),
        egress_proxy: Some(format!("http://{proxy}")),
        ..Opts::default()
    })
    .await;
    let mut api = st.signup("owner@e2e.test").await;
    let (agent, token) = api.byo_agent("inst-agent").await;
    install_instance_fixture(&st, &mut api).await;
    api.set_policy("allow", json!([])).await;
    // A registered instance (the API probe cannot resolve the fake name, so the row is inserted
    // directly; its registration rules are covered by the API suite).
    let inst = uuid::Uuid::now_v7();
    let org: uuid::Uuid = api.org.parse().unwrap();
    sqlx::query("INSERT INTO app_instances (id, org_id, package_id, app_id, host, label) VALUES ($1,$2,'fixture-instance','instance-desk',$3,'Acme desk')")
        .bind(inst)
        .bind(org)
        .bind(INST_HOST)
        .execute(st.pool())
        .await
        .unwrap();
    let conn = instance_connection(&st, &api, &agent, Some(inst), "inst-secret").await;
    let client = via_proxy(&st.gw_a, &token, &st.ca_pem);
    let get = |host: &str| {
        client
            .get(format!("https://{host}/api/v1/items"))
            .header("authorization", "Bearer agent-forged")
            .send()
    };
    let heads = || seen.lock().unwrap().clone();

    // 1. The instance host: injected (and the agent's own header is gone).
    let r = get(INST_HOST).await.unwrap();
    assert_eq!(r.status(), 200, "{}", st.log_tails());
    let h = heads().last().unwrap().to_ascii_lowercase();
    assert!(h.contains("authorization: bearer inst-secret"), "{h}");
    assert!(
        h.contains("x-fixture: instance") && !h.contains("agent-forged"),
        "{h}"
    );

    // 2. The app's SaaS default host: the instance connection is refused like a tenant-bound
    //    one (421, expected_host), nothing reaches the upstream.
    let before = (heads().len(), tunnels.lock().unwrap().len());
    let r = get(SAAS_HOST).await.unwrap();
    let (s, b) = (r.status().as_u16(), r.text().await.unwrap());
    assert_eq!((s, err_code(&b).as_str()), (421, "host_mismatch"), "{b}");
    let v: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(v["error"]["details"]["expected_host"], INST_HOST);
    assert_eq!((heads().len(), tunnels.lock().unwrap().len()), before);

    // 3. An unrelated host: the credential is not injected there.
    let r = get(OTHER_HOST).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(!heads().last().unwrap().contains("inst-secret"));

    // 4. A default-host connection is injected on the default host only, never on the instance.
    instance_connection(&st, &api, &agent, None, "saas-secret").await;
    let r = get(SAAS_HOST).await.unwrap();
    assert_eq!(r.status(), 200, "{}", st.log_tails());
    let h = heads().last().unwrap().to_ascii_lowercase();
    assert!(h.contains("authorization: bearer saas-secret"), "{h}");
    let r = get(INST_HOST).await.unwrap();
    assert_eq!(r.status(), 200);
    let h = heads().last().unwrap().to_ascii_lowercase();
    assert!(
        h.contains("bearer inst-secret") && !h.contains("saas-secret"),
        "{h}"
    );

    // 5. Disabling the instance stops injection on its host at once (gateway invalidated).
    let (s, v) = api
        .call(
            reqwest::Method::PATCH,
            &format!("/v1/app-instances/{inst}"),
            Some(json!({"status": "disabled"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // (a refused request never reaches the upstream, so only a *new* upstream head can carry a token)
    let injects_on_instance = || async {
        let n = heads().len();
        let _ = get(INST_HOST).await.unwrap();
        let hs = heads();
        hs.len() > n && {
            let h = hs.last().unwrap().to_ascii_lowercase();
            h.contains("inst-secret") || h.contains("saas-secret")
        }
    };
    let mut injected = true;
    for _ in 0..40 {
        injected = injects_on_instance().await;
        if !injected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        !injected,
        "still injecting on a disabled instance\n{}",
        st.log_tails()
    );
    let (s, _) = api
        .call(
            reqwest::Method::PATCH,
            &format!("/v1/app-instances/{inst}"),
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(s, 200);

    // 6. Deleting the instance: the connection is disabled and nothing is injected on its host.
    let (s, _) = api
        .call(
            reqwest::Method::DELETE,
            &format!("/v1/app-instances/{inst}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let (status, reason, bound): (String, Option<String>, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT status, status_reason, instance_id FROM app_connections WHERE id = $1",
    )
    .bind(conn)
    .fetch_one(st.pool())
    .await
    .unwrap();
    assert_eq!(
        (status.as_str(), reason.as_deref(), bound),
        ("disabled", Some("instance_deleted"), None)
    );
    let mut injected = true;
    for _ in 0..40 {
        injected = injects_on_instance().await;
        if !injected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        !injected,
        "still injecting on a deleted instance\n{}",
        st.log_tails()
    );
    st.finish().await;
}

// ---- P4-GRANTS (D-123, SEC-67): custom_only activation seen by both gateways -------------------

const CUSTOM_HOST: &str = "api.custom.example.test";

fn custom_pkg(version: &str, grants: &str) -> String {
    format!(
        "package: {{ id: cust, version: {version}, contract: 1, min_core: 0.1.0, publisher: acme, name: P }}\napps:\n  - about: {{ id: cust_app, name: Cust }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]\n{grants}    tools:\n      read:\n        items:\n          - {{ id: list_items, name: List, host: {CUSTOM_HOST}, path: /items, methods: [GET] }}\n    hosts:\n      - match: {{ host: {CUSTOM_HOST} }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n"
    )
}

#[tokio::test]
async fn s19_custom_only_activation_is_seen_consistently_by_both_gateways() {
    use aurora_registry::builder::PackageBuilder;
    use aurora_secrets::{Purpose, SealContext, SecretVault, StoreRegistry};
    let (up, up_ca, seen) = https_upstream_for(&[CUSTOM_HOST]).await;
    let (proxy, _tunnels) = connect_proxy(up).await;
    let st = Stack::start(Opts {
        extra_ca_bundle: Some(up_ca),
        egress_proxy: Some(format!("http://{proxy}")),
        ..Opts::default()
    })
    .await;
    let mut api = st.signup("owner@e2e.test").await;
    let (agent, token) = api.byo_agent("cust-agent").await;
    sqlx::query("UPDATE users SET is_instance_admin = true")
        .execute(st.pool())
        .await
        .unwrap();
    api.relogin("owner@e2e.test").await;
    api.set_policy("allow", json!([])).await;

    // A trusted publisher key of our own; two versions of one package.
    let key = aurora_crypto::generate_signing_key().unwrap();
    let (s, v) = api
        .post(
            "/v1/admin/trusted-keys",
            json!({"publisher": "acme", "name": "e2e", "public_key": PackageBuilder::key_id(&key)}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let build = |version: &str, grants: &str| {
        let mut b = PackageBuilder::new(&custom_pkg(version, grants));
        b.sign(&key, &PackageBuilder::key_id(&key));
        b.build().unwrap()
    };
    let (s, v) = api.upload_plugin(&build("1.0.0", "")).await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = api
        .post(
            &format!("/v1/admin/plugins/{}/approve", v["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("active")), "{v}");

    // A sealed connection with a FULL grant (and the derived allow rule a full grant compiles to).
    let (org, ws): (uuid::Uuid, uuid::Uuid) = (api.org.parse().unwrap(), api.ws.parse().unwrap());
    let conn = uuid::Uuid::now_v7();
    let keys = {
        use base64::Engine;
        format!(
            "k1:{}",
            base64::engine::general_purpose::STANDARD.encode([b'a'; 32])
        )
    };
    let keyring = std::sync::Arc::new(aurora_crypto::Keyring::parse(&keys, "k1").unwrap());
    let vault = SecretVault::new(
        StoreRegistry::from_config(&aurora_config::SecretsSection::default(), keyring).unwrap(),
    );
    let sealed = vault
        .seal(
            &SealContext {
                purpose: Purpose::ConnectionCredentials,
                owner_id: conn,
                org_id: org,
                workspace_id: Some(ws),
            },
            aurora_core::Secret::from(
                serde_json::to_vec(&json!({"api_token": "CUSTSECRET"})).unwrap(),
            ),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'cust_app','cust','1.0.0','cc','active',$4)")
        .bind(conn).bind(org).bind(ws).bind(sealed.to_column())
        .execute(st.pool()).await.unwrap();
    let grant = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(grant).bind(ws).bind(agent.parse::<uuid::Uuid>().unwrap()).bind(conn)
        .execute(st.pool()).await.unwrap();
    sqlx::query("INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) VALUES ($1,$2,$3,'grant',$4,0,$5,$6)")
        .bind(uuid::Uuid::now_v7()).bind(org).bind(ws).bind(grant).bind(uuid::Uuid::now_v7())
        .bind(json!({"name": "Grant: cc", "enabled": true,
            "identities": [{"kind": "agent", "id": agent}],
            "targets": [{"type": "connection", "id": conn}],
            "action": "allow", "require_approval": false, "rate_limit": null, "conditions": []}))
        .execute(st.pool()).await.unwrap();
    sqlx::query("SELECT pg_notify('aurora_gateway_invalidate', $1)")
        .bind(json!({"scope": "org", "id": org}).to_string())
        .execute(st.pool())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let heads = || seen.lock().unwrap().clone();
    // One probe: does this gateway inject the connection's token on `path`?
    let probe = |gw: String, p: &'static str| {
        let (token, ca) = (token.clone(), st.ca_pem.clone());
        let n_before = heads().len();
        async move {
            let c = via_proxy(&gw, &token, &ca);
            let r = c
                .get(format!("https://{CUSTOM_HOST}{p}"))
                .send()
                .await
                .unwrap();
            (r.status().as_u16(), n_before)
        }
    };
    let injected = |n_before: usize| {
        let hs = heads();
        hs.len() > n_before && hs.last().unwrap().contains("CUSTSECRET")
    };

    // Before: a full grant injects on every path, on both gateways.
    for gw in [&st.gw_a, &st.gw_b] {
        for p in ["/items", "/anything"] {
            let (s, n) = probe(gw.clone(), p).await;
            assert_eq!(s, 200, "{gw} {p}\n{}", st.log_tails());
            assert!(injected(n), "{gw} {p} must inject before the upgrade");
        }
    }

    // Activation of 1.1.0 (custom_only): held, then approved. Both gateways are probed in a
    // tight loop from the moment of approval: once a gateway has refused an untooled request it
    // never injects one again, and both converge.
    let (s, v) = api
        .upload_plugin(&build(
            "1.1.0",
            "    grants: { custom_only: true, block_untooled: true }\n",
        ))
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status_reason"], "converts_grants", "{v}");
    let pid = v["id"].as_str().unwrap().to_owned();
    let (s, v) = api
        .post(&format!("/v1/admin/plugins/{pid}/approve"), json!({}))
        .await;
    assert_eq!((s, v["status"].as_str()), (200, Some("active")), "{v}");
    // Committed: the grant is custom in the database.
    let mode: String = sqlx::query_scalar("SELECT mode FROM grants WHERE id = $1")
        .bind(grant)
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(mode, "custom");

    let mut blocked = [false, false];
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !(blocked[0] && blocked[1]) {
        for (i, gw) in [&st.gw_a, &st.gw_b].into_iter().enumerate() {
            let (s, n) = probe(gw.clone(), "/anything").await;
            if injected(n) {
                assert!(
                    !blocked[i],
                    "gateway {i} injected an untooled request after it had refused one"
                );
            } else {
                assert_eq!(s, 403, "gateway {i}: {}", st.log_tails());
                blocked[i] = true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        blocked[0] && blocked[1],
        "gateways did not converge: {blocked:?}\n{}",
        st.log_tails()
    );

    // After convergence, on both gateways: the untooled request is refused as
    // app_untooled_blocked and never injected, and the catalogued tool of the converted
    // custom grant still is (what the agent could do with listed tools is unchanged).
    for gw in [&st.gw_a, &st.gw_b] {
        let n = heads().len();
        let c = via_proxy(gw, &token, &st.ca_pem);
        let r = c
            .get(format!("https://{CUSTOM_HOST}/anything"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
        let b = r.text().await.unwrap();
        let v: Value = serde_json::from_str(&b).unwrap();
        assert_eq!(
            v["error"]["details"]["reason"], "app_untooled_blocked",
            "{b}"
        );
        assert_eq!(heads().len(), n, "nothing reached the upstream");
        let (s, n) = probe(gw.clone(), "/items").await;
        assert_eq!(s, 200, "{}", st.log_tails());
        assert!(
            injected(n),
            "{gw}: the catalogued tool must still be injected"
        );
    }
    st.finish().await;
}
