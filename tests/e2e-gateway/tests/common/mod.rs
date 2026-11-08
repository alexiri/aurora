#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Black-box harness: real `aurora-api` + 2x `aurora-gateway` child processes on
//! a fresh database. Ports are hard-coded in the binaries (4101/4111, 4102/4103),
//! so isolation uses distinct loopback addresses (127.0.0.31..33) and tests run
//! serially (`--test-threads=1`).

use aurora_testkit::TestDb;
use base64::Engine;
use reqwest::Method;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

pub const API_IP: &str = "127.0.0.31";
pub const GW_A_IP: &str = "127.0.0.32";
pub const GW_B_IP: &str = "127.0.0.33";
pub const HH_IP: &str = "127.0.0.34";
const ORIGIN: &str = "http://localhost:4100";

#[derive(Default, Clone)]
pub struct Opts {
    pub approvals_timeout_secs: Option<u64>,
    /// LiteLLM endpoint `litellm` base URL.
    pub litellm_base: Option<String>,
    /// PEM bundle of extra upstream CAs (HTTPS mocks).
    pub extra_ca_bundle: Option<String>,
    /// Start `aurora-hookhost` (HOOK-004) on `HH_IP:4104`.
    pub hookhost: bool,
    /// Overrides `gateway.hookhost_url` (default: the started hook host).
    pub hookhost_url: Option<String>,
    /// `gateway.egress_proxy` (`host:port`): the gateway dials upstreams through this CONNECT
    /// proxy, so tests can serve DNS names that do not resolve (D-74 instance hosts).
    pub egress_proxy: Option<String>,
}

pub struct Stack {
    /// Development IdP (phase 1, RFE-004): the in-process OIDC mock the API signs users in through.
    #[allow(dead_code)]
    idp: aurora_testkit::oidc::MockIdp,
    pub db: Option<TestDb>,
    pub dir: PathBuf,
    pub api_addr: String,
    pub gw_a: String,
    pub gw_b: String,
    pub ca_pem: String,
    children: Vec<Child>,
    pub logs: Vec<PathBuf>,
}

fn target_dir() -> PathBuf {
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(t).join("debug");
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug")
}

fn bin(name: &str) -> PathBuf {
    let p = target_dir().join(name);
    assert!(
        p.exists(),
        "{} missing: run `cargo build -p aurora-api -p aurora-gateway -p aurora-hookhost` first",
        p.display()
    );
    p
}

impl Stack {
    pub async fn start(o: Opts) -> Stack {
        let db = TestDb::new().await;
        let idp = aurora_testkit::oidc::MockIdp::start().await;
        let idp_issuer = idp.issuer().to_owned();
        let dir = std::env::temp_dir().join(format!("aurora-e2e-gw-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(dir.join("ca")).unwrap();
        let st = std::process::Command::new(bin("aurora-gateway"))
            .args(["generate-ca", "--out"])
            .arg(dir.join("ca"))
            .output()
            .unwrap();
        assert!(st.status.success(), "generate-ca failed");
        let ca_pem = std::fs::read_to_string(dir.join("ca/ca.pem")).unwrap();

        let mut toml = format!(
            r#"[server]
external_url = "{ORIGIN}"
bind = "BIND"

[runner]
backend = "fake"
api_url = "http://{API_IP}:4101"

[auth]
signup_mode = "open"
org_creation = "anyone"

[auth.dev_idp]
enabled = true
issuer = "{idp_issuer}"

[telemetry]
log_level = "info"

[gateway]
allowed_private = ["127.0.0.1/32"]
{egress}ca_dir = "{ca}"
advertised_proxy_url = "https://{GW_A_IP}:4102"
hookhost_url = "{hh_url}"

# RFE-017: suites poll and hammer one principal far above real use; keep the limiter on, scaled up.
[ratelimit.api]
multiplier = 10.0
[gateway.proxy_tls]
names = ["{GW_B_IP}"]

[api]
hookhost_url = "http://127.0.0.1:9"

[hookhost]
api_internal_url = "http://{API_IP}:4111"
"#,
            ca = dir.join("ca").display(),
            egress = o
                .egress_proxy
                .as_ref()
                .map(|p| format!("egress_proxy = \"{p}\"\n"))
                .unwrap_or_default(),
            hh_url = o
                .hookhost_url
                .clone()
                .unwrap_or_else(|| format!("http://{HH_IP}:4104"))
        );
        if let Some(t) = o.approvals_timeout_secs {
            toml.push_str(&format!("\n[approvals]\ntimeout_secs = {t}\n"));
        }
        if let Some(b) = &o.extra_ca_bundle {
            let p = dir.join("extra-ca.pem");
            std::fs::write(&p, b).unwrap();
            toml.push_str(&format!("\n[tls]\nextra_ca_bundle = \"{}\"\n", p.display()));
        }
        if let Some(base) = &o.litellm_base {
            toml.push_str(&format!(
                "\n[llm]\nenabled_providers = [\"litellm\"]\nprovider_preference = [\"litellm\"]\n\n[[llm.endpoints]]\nid = \"litellm\"\nprovider = \"litellm\"\nbase_url = \"{base}\"\n"
            ));
        }
        let mut stack = Stack {
            idp,
            db: Some(db),
            dir,
            api_addr: format!("{API_IP}:4101"),
            gw_a: format!("{GW_A_IP}:4102"),
            gw_b: format!("{GW_B_IP}:4102"),
            ca_pem,
            children: vec![],
            logs: vec![],
        };
        // DEFECT (reported): no production path publishes a registry snapshot before
        // M3 (snapshot compiler), yet the gateway's /readyz stays 503 until one is
        // loaded. Seed the minimal empty snapshot a compiler would emit.
        sqlx::query(
            "INSERT INTO registry_snapshots (id, contract, content_hash, snapshot, compiled_by, activated_at) \
             VALUES ($1, 1, 'e2e-empty', $2, 'e2e', now())",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(json!({"format": 1, "contract": 1, "packages": [], "oauth_clients": [],
                     "apps": [], "hosts": [], "reserved_hosts": []}))
        .execute(stack.pool())
        .await
        .unwrap();
        let mut procs = vec![
            ("api", API_IP, "aurora-api", vec!["serve"]),
            ("gw-a", GW_A_IP, "aurora-gateway", vec!["serve"]),
            ("gw-b", GW_B_IP, "aurora-gateway", vec!["serve"]),
        ];
        if o.hookhost {
            procs.push(("hookhost", HH_IP, "aurora-hookhost", vec!["serve"]));
        }
        for (name, ip, exe, args) in procs {
            let cfg = stack.dir.join(format!("{name}.toml"));
            std::fs::write(&cfg, toml.replace("BIND", ip)).unwrap();
            let log = stack.dir.join(format!("{name}.log"));
            let f = std::fs::File::create(&log).unwrap();
            let child = Command::new(bin(exe))
                .args(&args)
                .env("AURORA_CONFIG", &cfg)
                .envs(secrets_env(stack.db.as_ref().unwrap().url()))
                .stdout(Stdio::from(f.try_clone().unwrap()))
                .stderr(Stdio::from(f))
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            stack.children.push(child);
            stack.logs.push(log);
        }
        let t0 = Instant::now();
        stack.wait_ready().await;
        eprintln!("[e2e] ready after {:?}", t0.elapsed());
        stack
    }

    /// Wait until the hook host has loaded a registry snapshot.
    pub async fn wait_hookhost_ready(&self) {
        let c = reqwest::Client::new();
        let u = format!("http://{HH_IP}:4104/readyz");
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            if let Ok(r) = c.get(&u).send().await
                && r.status().is_success()
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "hookhost not ready\n{}",
                self.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn wait_ready(&mut self) {
        let c = reqwest::Client::new();
        let urls = [
            format!("http://{API_IP}:4101/readyz"),
            format!("http://{GW_A_IP}:4103/readyz"),
            format!("http://{GW_B_IP}:4103/readyz"),
        ];
        let deadline = Instant::now() + Duration::from_secs(60);
        for u in urls {
            loop {
                if let Ok(r) = c.get(&u).send().await
                    && r.status().is_success()
                {
                    break;
                }
                if Instant::now() > deadline {
                    let logs = self.log_tails();
                    panic!("{u} not ready\n{logs}");
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }

    pub fn log_tails(&self) -> String {
        self.logs
            .iter()
            .map(|p| {
                let s = std::fs::read_to_string(p).unwrap_or_default();
                let tail: Vec<&str> = s.lines().rev().take(15).collect();
                format!(
                    "--- {} ---\n{}",
                    p.display(),
                    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub async fn finish(mut self) {
        for c in &mut self.children {
            let _ = c.kill().await;
        }
        self.children.clear();
        if let Some(db) = self.db.take() {
            let _ = db.cleanup().await;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        self.db.as_ref().unwrap().pool()
    }

    /// Sign a user in through the development IdP (open mode: the first sign-in creates the user,
    /// org + workspace + Daneel) and return a client. Phase 1 (RFE-004): there is no password.
    pub async fn signup(&self, email: &str) -> Api {
        let http = reqwest::Client::new();
        let base = format!("http://{}", self.api_addr);
        let cookie = aurora_testkit::oidc::sign_in_over_http(&base, "dev", email)
            .await
            .unwrap_or_else(|e| panic!("sign-in {email}: {e}"));
        let mut api = Api {
            http,
            base,
            cookie,
            org: String::new(),
            ws: String::new(),
        };
        let (s, me) = api.get("/v1/me").await;
        assert_eq!(s, 200, "{me}");
        api.org = me["memberships"][0]["org"]["id"].as_str().unwrap().into();
        api.ws = me["memberships"][0]["workspaces"][0]["id"]
            .as_str()
            .unwrap()
            .into();
        api
    }
}

fn secrets_env(db_url: &str) -> Vec<(&'static str, String)> {
    let k = base64::engine::general_purpose::STANDARD.encode([b'a'; 32]);
    vec![
        ("AURORA_DATABASE_URL", db_url.to_owned()),
        ("AURORA_ENCRYPTION_KEYS", format!("k1:{k}")),
        ("AURORA_ENCRYPTION_ACTIVE_KEY", "k1".into()),
        ("AURORA_SIGNING_KEY", k.clone()),
        ("AURORA_INTERNAL_SECRET", k.clone()),
        ("AURORA_RUNNER_SESSION_KEY", k),
        ("AURORA_RUNNER_TOKEN", "aur_e2etoken".into()),
        ("AURORA_CONNECTOR_TOKEN", "aun_e2etoken".into()),
    ]
}

pub struct Api {
    http: reqwest::Client,
    pub base: String,
    cookie: String,
    pub org: String,
    pub ws: String,
}

impl Api {
    pub async fn call(&self, m: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self
            .http
            .request(m, format!("{}{path}", self.base))
            .header("cookie", &self.cookie)
            .header("origin", ORIGIN)
            .timeout(Duration::from_secs(40));
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let s = resp.status().as_u16();
        let t = resp.text().await.unwrap();
        (s, serde_json::from_str(&t).unwrap_or(Value::String(t)))
    }
    pub async fn get(&self, p: &str) -> (u16, Value) {
        self.call(Method::GET, p, None).await
    }
    pub async fn post(&self, p: &str, b: Value) -> (u16, Value) {
        self.call(Method::POST, p, Some(b)).await
    }
    pub async fn put(&self, p: &str, b: Value) -> (u16, Value) {
        self.call(Method::PUT, p, Some(b)).await
    }

    /// Fresh login (new session, so a changed `users` row is not served from the auth cache).
    pub async fn relogin(&mut self, email: &str) {
        self.cookie = aurora_testkit::oidc::sign_in_over_http(&self.base, "dev", email)
            .await
            .unwrap_or_else(|e| panic!("re-sign-in {email}: {e}"));
    }

    /// `POST /v1/admin/plugins` (multipart `package`).
    pub async fn upload_plugin(&self, bytes: &[u8]) -> (u16, Value) {
        let boundary = "XAURORAE2EX";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"package\"; filename=\"p.aurora-plugin\"\r\nContent-Type: application/gzip\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let resp = self
            .http
            .post(format!("{}/v1/admin/plugins", self.base))
            .header("cookie", &self.cookie)
            .header("origin", ORIGIN)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .timeout(Duration::from_secs(40))
            .send()
            .await
            .unwrap();
        let s = resp.status().as_u16();
        let t = resp.text().await.unwrap();
        (s, serde_json::from_str(&t).unwrap_or(Value::String(t)))
    }

    /// Create a BYO agent; returns (agent_id, token).
    pub async fn byo_agent(&self, handle: &str) -> (String, String) {
        let (s, v) = self
            .post(
                &format!("/v1/workspaces/{}/agents", self.ws),
                json!({"name": handle, "handle": handle, "kind": "byo"}),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        (
            v["agent"]["id"].as_str().unwrap().into(),
            v["token"].as_str().unwrap().into(),
        )
    }

    pub async fn ws_secret(&self, body: Value) -> String {
        let (s, v) = self
            .post(&format!("/v1/workspaces/{}/secrets", self.ws), body)
            .await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().unwrap().into()
    }

    pub async fn grant(&self, agent: &str, secret: &str) {
        let (s, v) = self
            .put(
                &format!("/v1/agents/{agent}/grants/secrets/{secret}"),
                json!({}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }

    /// Replace the workspace draft and publish it.
    pub async fn set_policy(&self, default_action: &str, rules: Value) {
        self.set_policy_with(default_action, rules, json!({})).await;
    }

    pub async fn set_org_policy(&self, default_action: &str, rules: Value) {
        let base = format!("/v1/orgs/{}/policy", self.org);
        self.set_policy_at(&base, default_action, rules, json!({}))
            .await;
    }

    pub async fn set_policy_with(&self, default_action: &str, rules: Value, extra: Value) {
        let base = format!("/v1/workspaces/{}/policy", self.ws);
        self.set_policy_at(&base, default_action, rules, extra)
            .await;
    }

    async fn set_policy_at(&self, base: &str, default_action: &str, rules: Value, extra: Value) {
        let (s, doc) = self.get(base).await;
        assert_eq!(s, 200, "{doc}");
        let rev = doc["draft"]["draft_revision"].as_i64().unwrap();
        let (s, d) = self
            .put(&format!("{base}/draft"), {
                let mut b =
                    json!({"revision": rev, "default_action": default_action, "rules": rules});
                for (k, v) in extra.as_object().unwrap() {
                    b[k] = v.clone();
                }
                b
            })
            .await;
        assert_eq!(s, 200, "{d}");
        let rev = d["draft_revision"].as_i64().unwrap();
        let (s, p) = self
            .post(&format!("{base}/publish"), json!({"revision": rev}))
            .await;
        assert_eq!(s, 200, "{p}");
    }

    pub async fn pending(&self) -> Vec<Value> {
        let (s, v) = self
            .get(&format!(
                "/v1/workspaces/{}/approvals?status=pending",
                self.ws
            ))
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"].as_array().cloned().unwrap_or_default()
    }

    /// Poll until exactly one pending approval exists.
    pub async fn wait_pending(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let p = self.pending().await;
            if let Some(a) = p.first() {
                return a.clone();
            }
            assert!(Instant::now() < deadline, "no pending approval appeared");
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    pub async fn activity(&self) -> Vec<Value> {
        let (s, v) = self
            .get(&format!("/v1/workspaces/{}/activity?limit=200", self.ws))
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"].as_array().cloned().unwrap_or_default()
    }
}

pub fn net_rule(name: &str, action: &str, host: &str, extra: Value) -> Value {
    let mut r =
        json!({"name": name, "action": action, "targets": [{"type": "network", "host": host}]});
    if let (Some(o), Some(e)) = (r.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    r
}

/// reqwest client that goes through gateway `gw` ("ip:port") as `token`, trusting the gateway CA
/// (the proxy listener is TLS-only: the CA issues its certificate, so one anchor covers both legs).
pub fn via_proxy(gw: &str, token: &str, ca_pem: &str) -> reqwest::Client {
    let proxy = reqwest::Proxy::all(format!("https://{gw}"))
        .unwrap()
        .basic_auth("agent", token);
    reqwest::Client::builder()
        .proxy(proxy)
        .add_root_certificate(reqwest::Certificate::from_pem(ca_pem.as_bytes()).unwrap())
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap()
}

pub fn err_code(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("<no code: {body}>"))
}

/// Raw `CONNECT target` through gateway `gw` ("ip:port", TLS-only listener, trusting `ca_pem`) as
/// `user:token`; returns (status, response head + body as text).
pub async fn raw_connect(
    gw: &str,
    ca_pem: &str,
    user: &str,
    token: &str,
    target: &str,
) -> (u16, String) {
    use rustls::RootCertStore;
    use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut roots = RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
        roots.add(c.unwrap()).unwrap();
    }
    let cfg = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let addr: std::net::SocketAddr = gw.parse().unwrap();
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut tls = tokio_rustls::TlsConnector::from(std::sync::Arc::new(cfg))
        .connect(ServerName::IpAddress(addr.ip().into()), tcp)
        .await
        .unwrap();
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{token}"));
    tls.write_all(
        format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = vec![0u8; 8192];
    let n = tokio::time::timeout(Duration::from_secs(10), tls.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, text)
}

impl Stack {
    /// Mint a boot credential (`aub_`) for `agent` holding `capabilities`, as the API does for a
    /// hosted boot (08 §7); returns the token.
    pub async fn boot_credential(&self, agent: &str, capabilities: &[&str]) -> String {
        use aurora_core::token::{self, TokenPrefix};
        let tok = token::generate(TokenPrefix::Boot);
        let caps: Vec<String> = capabilities.iter().map(|c| (*c).to_owned()).collect();
        sqlx::query(
            "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, boot_id, capabilities)
             VALUES ($1,'boot',$2,$3,$4::uuid,$5,$6)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(&token::hash(tok.expose())[..])
        .bind(token::display_prefix(tok.expose()))
        .bind(agent)
        .bind(uuid::Uuid::now_v7())
        .bind(&caps)
        .execute(self.pool())
        .await
        .unwrap();
        tok.expose().to_owned()
    }
}
