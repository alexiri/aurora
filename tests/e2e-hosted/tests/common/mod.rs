#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Black-box hosted-agent harness (TEST-013): real `aurora-api`, `aurora-gateway` and
//! `aurora-runner` (backend `podman`) child processes plus real sandbox containers.
//!
//! Networking: see `scripts/e2e/hosted.sh`. The whole suite runs inside podman's rootless network
//! namespace; the gateway and runner bind on the sandbox-network bridge IP (`BRIDGE`), which
//! sandboxes reach through `/etc/hosts` entries (`aurora-gateway`, `aurora-runner`).

use aurora_testkit::TestDb;
use base64::Engine;
use reqwest::Method;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

pub const API_IP: &str = "127.0.0.41";
pub const BRIDGE: &str = "10.89.251.1";
/// Non-sandbox address for the gateway :4103 / runner :4106 listeners (SEC-09).
pub const OPS_IP: &str = "127.0.0.42";
const ORIGIN: &str = "http://localhost:4100";

/// Origin sent by `Api` requests: `AURORA_KIND_ORIGIN` when the suite runs against a Kubernetes stack whose
/// `server.external_url` differs (the kind dev stack serves http://localhost:4000), else the default.
fn request_origin() -> String {
    std::env::var("AURORA_KIND_ORIGIN").unwrap_or_else(|_| ORIGIN.to_owned())
}

pub fn enabled() -> bool {
    let on = std::env::var("AURORA_TEST_PODMAN").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: AURORA_TEST_PODMAN != 1");
    }
    on
}

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} not set: run scripts/e2e/hosted.sh"))
}

#[derive(Default, Clone)]
pub struct Opts {
    pub idle_stop_secs: Option<u64>,
}

pub struct Stack {
    /// Development IdP (phase 1, RFE-004): the in-process OIDC mock the API signs users in through.
    #[allow(dead_code)]
    idp: aurora_testkit::oidc::MockIdp,
    pub db: Option<TestDb>,
    pub dir: PathBuf,
    pub api_addr: String,
    pub ca_pem: String,
    children: Vec<(String, Child)>,
    pub logs: Vec<PathBuf>,
    cfg: PathBuf,
}

fn target_dir() -> PathBuf {
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(t).join("debug");
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug")
}

fn bin(name: &str) -> PathBuf {
    let p = target_dir().join(name);
    assert!(p.exists(), "{} missing: build it first", p.display());
    p
}

/// Run `podman` (inside the rootless netns this reaches the user's podman); returns stdout.
pub fn podman(args: &[&str]) -> String {
    let o = std::process::Command::new("podman")
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn podman_rc(args: &[&str]) -> (i32, String) {
    let o = std::process::Command::new("podman")
        .args(args)
        .output()
        .unwrap();
    let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&o.stderr));
    (o.status.code().unwrap_or(-1), s)
}

pub fn fp() -> String {
    env("AURORA_E2EH_FP")
}

/// Names of this installation's containers (running or not).
pub fn containers() -> Vec<String> {
    podman(&[
        "ps",
        "-a",
        "--filter",
        &format!("label=aurora.installation={}", fp()),
        "--format",
        "{{.Names}}",
    ])
    .lines()
    .map(str::to_owned)
    .collect()
}

pub fn volumes() -> Vec<String> {
    podman(&[
        "volume",
        "ls",
        "--filter",
        &format!("label=aurora.installation={}", fp()),
        "--format",
        "{{.Name}}",
    ])
    .lines()
    .map(str::to_owned)
    .collect()
}

/// `podman exec` in the (single) container as uid 1000 running `bash -c script`.
pub fn exec_in(container: &str, script: &str) -> (i32, String) {
    podman_rc(&[
        "exec",
        "--user",
        "1000:1000",
        container,
        "bash",
        "-c",
        script,
    ])
}

impl Stack {
    pub async fn start(o: Opts) -> Stack {
        Self::start_with(o, "").await
    }

    /// Like [`Stack::start`], with extra top-level TOML appended to every service config
    /// (TEST-017: `[llm]` endpoints).
    pub async fn start_with(o: Opts, extra_toml: &str) -> Stack {
        let db = TestDb::new().await;
        let idp = aurora_testkit::oidc::MockIdp::start().await;
        let idp_issuer = idp.issuer().to_owned();
        let dir = std::env::temp_dir().join(format!("aurora-e2e-hosted-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(dir.join("ca")).unwrap();
        let st = std::process::Command::new(bin("aurora-gateway"))
            .args(["generate-ca", "--out"])
            .arg(dir.join("ca"))
            .output()
            .unwrap();
        assert!(st.status.success(), "generate-ca failed");
        let ca_pem = std::fs::read_to_string(dir.join("ca/ca.pem")).unwrap();
        let idle = o.idle_stop_secs.unwrap_or(1800);
        let toml = format!(
            r#"[server]
external_url = "{ORIGIN}"
bind = "BIND"

[runner]
ops_bind = "{OPS_IP}"
backend = "podman"
api_url = "http://{API_IP}:4101"
podman_socket = "{sock}"
sandbox_network = "{net}"
image = "{image}"
max_sandboxes = 4
reconcile_interval_secs = 600
capacity_wait_secs = 5

[runner.sandbox_hosts]
aurora-gateway = "{BRIDGE}"
aurora-runner = "{BRIDGE}"

[hosted]
enabled = true
allow_fake_harness = true
idle_stop_secs = {idle}

[auth]
signup_mode = "open"
org_creation = "anyone"

[auth.dev_idp]
enabled = true
issuer = "{idp_issuer}"

[telemetry]
log_level = "info"

[gateway]
internal_bind = "{OPS_IP}"
allowed_private = ["127.0.0.1/32"]
ca_dir = "{ca}"
advertised_proxy_url = "https://aurora-gateway:4102"

[gateway.proxy_tls]
names = ["{BRIDGE}"]

# RFE-017: suites poll and hammer one principal far above real use; keep the limiter on, scaled up.
[ratelimit.api]
multiplier = 10.0

[api]
hookhost_url = "http://127.0.0.1:9"
"#,
            sock = env("AURORA_E2EH_SOCK"),
            net = env("AURORA_E2EH_NETWORK"),
            image = std::env::var("AURORA_E2EH_IMAGE")
                .unwrap_or_else(|_| "localhost/aurora-sandbox:0.1.0".into()),
            ca = dir.join("ca").display(),
        ) + extra_toml;
        let mut stack = Stack {
            idp,
            db: Some(db),
            api_addr: format!("{API_IP}:4101"),
            ca_pem,
            children: vec![],
            logs: vec![],
            cfg: dir.join("cfg.toml"),
            dir,
        };
        // The API re-hashes the stored snapshot (`active_snapshot`): the hash must be real.
        let snapshot = json!({"format": 1, "contract": 1, "packages": [], "oauth_clients": [],
                              "apps": [], "hosts": [], "reserved_hosts": []});
        let hash: String = {
            use sha2::{Digest, Sha256};
            Sha256::digest(aurora_registry::snapshot::canonical_bytes(&snapshot))
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        sqlx::query(
            "INSERT INTO registry_snapshots (id, contract, content_hash, snapshot, compiled_by, activated_at) \
             VALUES ($1, 1, $2, $3, 'e2e', now())",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(&hash)
        .bind(&snapshot)
        .execute(stack.pool())
        .await
        .unwrap();
        for (name, ip, exe) in [
            ("api", API_IP, "aurora-api"),
            ("gateway", BRIDGE, "aurora-gateway"),
            ("runner", BRIDGE, "aurora-runner"),
        ] {
            let cfg = stack.dir.join(format!("{name}.toml"));
            std::fs::write(&cfg, toml.replace("BIND", ip)).unwrap();
            let _ = &stack.cfg;
            stack.spawn(name, exe, &cfg);
            if name == "api" {
                stack.wait_ok(&format!("http://{API_IP}:4101/readyz")).await;
            }
        }
        stack.wait_ok(&format!("http://{OPS_IP}:4103/readyz")).await;
        stack.wait_ok(&format!("http://{OPS_IP}:4106/readyz")).await;
        stack
    }

    fn spawn(&mut self, name: &str, exe: &str, cfg: &Path) {
        let log = self.dir.join(format!("{name}.log"));
        let f = std::fs::File::create(&log).unwrap();
        let child = Command::new(bin(exe))
            .arg("serve")
            .env("AURORA_CONFIG", cfg)
            .envs(secrets_env(self.db.as_ref().unwrap().url()))
            .stdout(Stdio::from(f.try_clone().unwrap()))
            .stderr(Stdio::from(f))
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        self.children.push((name.to_owned(), child));
        self.logs.push(log);
    }

    async fn wait_ok(&self, u: &str) {
        let c = reqwest::Client::new();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(r) = c.get(u).send().await
                && r.status().is_success()
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{u} not ready\n{}",
                self.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    pub fn log_tails(&self) -> String {
        self.logs
            .iter()
            .map(|p| {
                let s = std::fs::read_to_string(p).unwrap_or_default();
                let tail: Vec<&str> = s.lines().rev().take(25).collect();
                format!(
                    "--- {} ---\n{}",
                    p.display(),
                    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn log_text(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.join(format!("{name}.log"))).unwrap_or_default()
    }

    /// Kill children first (runner before anything can start a container), then remove every
    /// container/volume of this installation, then drop the database.
    pub async fn finish(mut self) {
        for (_, c) in self.children.iter_mut().rev() {
            let _ = c.kill().await;
        }
        self.children.clear();
        Self::wipe();
        if let Some(db) = self.db.take() {
            let _ = db.cleanup().await;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    pub fn wipe() {
        for c in containers() {
            let _ = podman_rc(&["rm", "-f", "-t", "1", &c]);
        }
        for v in volumes() {
            let _ = podman_rc(&["volume", "rm", "-f", &v]);
        }
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        self.db.as_ref().unwrap().pool()
    }

    /// Sign in through the development IdP (open mode: the first sign-in creates the user, org +
    /// workspace + Daneel), flip Daneel to the fake harness. Phase 1 (RFE-004): no password.
    pub async fn signup(&self, email: &str) -> Api {
        let http = reqwest::Client::new();
        let cookie = aurora_testkit::oidc::sign_in_over_http(
            &format!("http://{}", self.api_addr),
            "dev",
            email,
        )
        .await
        .unwrap_or_else(|e| panic!("sign-in {email}: {e}"));
        let mut api = Api {
            http,
            base: format!("http://{}", self.api_addr),
            cookie,
            org: String::new(),
            ws: String::new(),
            daneel: String::new(),
        };
        let (s, me) = api.get("/v1/me").await;
        assert_eq!(s, 200, "{me}");
        api.org = me["memberships"][0]["org"]["id"].as_str().unwrap().into();
        api.ws = me["memberships"][0]["workspaces"][0]["id"]
            .as_str()
            .unwrap()
            .into();
        let id: uuid::Uuid = sqlx::query_scalar(
            "UPDATE agents SET harness = 'fake' WHERE workspace_id = $1::uuid AND handle = 'daneel' RETURNING id",
        )
        .bind(&api.ws)
        .fetch_one(self.pool())
        .await
        .expect("Daneel exists after signup");
        api.daneel = id.to_string();
        api
    }
}

fn secrets_env(db_url: &str) -> Vec<(&'static str, String)> {
    let k = base64::engine::general_purpose::STANDARD.encode([b'a'; 32]);
    let token = env("AURORA_E2EH_TOKEN");
    vec![
        ("AURORA_DATABASE_URL", db_url.to_owned()),
        ("AURORA_ENCRYPTION_KEYS", format!("k1:{k}")),
        ("AURORA_ENCRYPTION_ACTIVE_KEY", "k1".into()),
        ("AURORA_SIGNING_KEY", k.clone()),
        ("AURORA_INTERNAL_SECRET", k.clone()),
        ("AURORA_RUNNER_SESSION_KEY", k),
        ("AURORA_RUNNER_TOKEN", token.clone()),
        ("AURORA_BOOTSTRAP_RUNNER_TOKEN", token),
        ("AURORA_CONNECTOR_TOKEN", "aun_e2etoken".into()),
    ]
}

pub struct Api {
    http: reqwest::Client,
    pub base: String,
    cookie: String,
    pub org: String,
    pub ws: String,
    pub daneel: String,
}

impl Api {
    /// Sign in over HTTP (development IdP) against a deployed API at `base` (kind e2e, OPS-011): no local database
    /// handle. `origin` must equal the deployment's `server.external_url`. Daneel is switched to
    /// the fake harness through the agents API.
    pub async fn connect(base: &str, origin: &str, email: &str) -> Api {
        let _ = origin; // phase 1 (RFE-004): sign-in is a browser-style redirect flow, no Origin header
        let http = reqwest::Client::new();
        let cookie = aurora_testkit::oidc::sign_in_over_http(base, "dev", email)
            .await
            .unwrap_or_else(|e| panic!("sign-in {email}: {e}"));
        let mut api = Api {
            http,
            base: base.to_owned(),
            cookie,
            org: String::new(),
            ws: String::new(),
            daneel: String::new(),
        };
        let (s, me) = api.get("/v1/me").await;
        assert_eq!(s, 200, "{me}");
        api.org = me["memberships"][0]["org"]["id"].as_str().unwrap().into();
        api.ws = me["memberships"][0]["workspaces"][0]["id"]
            .as_str()
            .unwrap()
            .into();
        let (s, list) = api.get(&format!("/v1/workspaces/{}/agents", api.ws)).await;
        assert_eq!(s, 200, "{list}");
        api.daneel = list["items"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["handle"] == "daneel"))
            .and_then(|x| x["id"].as_str())
            .unwrap_or_else(|| panic!("Daneel not found: {list}"))
            .to_owned();
        let (s, v) = api
            .call(
                Method::PATCH,
                &format!("/v1/agents/{}", api.daneel),
                Some(json!({"harness": "fake"})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        api
    }

    /// Wait until `turn` has a `turn.finished` event; returns its status.
    pub async fn wait_finished(&self, conv: &str, turn: &str, secs: u64) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            for e in self.events(conv).await {
                let t = e.to_string();
                if t.contains("turn.finished") && t.contains(turn) {
                    for st in ["done", "failed", "aborted"] {
                        if t.contains(&format!("\"status\":\"{st}\"")) {
                            return st.to_owned()
                                + &e["payload"]["error_code"]
                                    .as_str()
                                    .map(|c| format!(":{c}"))
                                    .unwrap_or_default();
                        }
                    }
                }
            }
            assert!(Instant::now() < deadline, "turn {turn} did not finish");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Poll the sandbox endpoint until `state` (no database handle needed).
    pub async fn wait_sandbox_state(&self, state: &str, secs: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let v = self.sandbox().await;
            if v["state"] == state {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "sandbox never reached {state}: {v}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

impl Api {
    pub async fn call(&self, m: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self
            .http
            .request(m, format!("{}{path}", self.base))
            .header("cookie", &self.cookie)
            .header("origin", &request_origin())
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
    pub async fn patch(&self, p: &str, b: Value) -> (u16, Value) {
        self.call(Method::PATCH, p, Some(b)).await
    }
    pub async fn put(&self, p: &str, b: Value) -> (u16, Value) {
        self.call(Method::PUT, p, Some(b)).await
    }

    /// Raw request with the session cookie (for SSE).
    pub fn raw(&self, p: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{p}", self.base))
            .header("cookie", &self.cookie)
            .header("origin", &request_origin())
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

    pub async fn set_policy(&self, default_action: &str, rules: Value) {
        let base = format!("/v1/workspaces/{}/policy", self.ws);
        let (s, doc) = self.get(&base).await;
        assert_eq!(s, 200, "{doc}");
        let rev = doc["draft"]["draft_revision"].as_i64().unwrap();
        let (s, d) = self
            .put(
                &format!("{base}/draft"),
                json!({"revision": rev, "default_action": default_action, "rules": rules}),
            )
            .await;
        assert_eq!(s, 200, "{d}");
        let rev = d["draft_revision"].as_i64().unwrap();
        let (s, p) = self
            .post(&format!("{base}/publish"), json!({"revision": rev}))
            .await;
        assert_eq!(s, 200, "{p}");
    }

    pub async fn wait_pending(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let (s, v) = self
                .get(&format!(
                    "/v1/workspaces/{}/approvals?status=pending",
                    self.ws
                ))
                .await;
            assert_eq!(s, 200, "{v}");
            if let Some(a) = v["items"].as_array().and_then(|a| a.first()) {
                return a.clone();
            }
            assert!(Instant::now() < deadline, "no pending approval appeared");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Direct conversation with Daneel.
    pub async fn direct(&self) -> String {
        let (s, v) = self
            .post(
                &format!("/v1/agents/{}/conversations/direct", self.daneel),
                json!({}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["id"].as_str().unwrap().into()
    }

    /// Post a message; returns the 202 body.
    pub async fn say(&self, conv: &str, text: &str) -> Value {
        let (s, v) = self
            .post(
                &format!("/v1/conversations/{conv}/messages"),
                json!({"text": text}),
            )
            .await;
        assert_eq!(s, 202, "{v}");
        v
    }

    pub async fn events(&self, conv: &str) -> Vec<Value> {
        let (s, v) = self
            .get(&format!("/v1/conversations/{conv}/events?limit=500"))
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"].as_array().cloned().unwrap_or_default()
    }

    pub async fn sandbox(&self) -> Value {
        let (s, v) = self
            .get(&format!("/v1/agents/{}/sandbox", self.daneel))
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }
}

impl Stack {
    /// Poll `turns.status` until it is one of `want`; returns (status, error_code).
    pub async fn wait_turn(
        &self,
        turn: &str,
        want: &[&str],
        secs: u64,
    ) -> (String, Option<String>) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let r: (String, Option<String>) =
                sqlx::query_as("SELECT status, error_code FROM turns WHERE id = $1::uuid")
                    .bind(turn)
                    .fetch_one(self.pool())
                    .await
                    .unwrap();
            if want.contains(&r.0.as_str()) {
                return r;
            }
            assert!(
                Instant::now() < deadline,
                "turn {turn} stuck at {r:?}, wanted {want:?}\n{}",
                self.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    pub async fn wait_sandbox(&self, api: &Api, state: &str, secs: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let v = api.sandbox().await;
            if v["state"] == state {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "sandbox never reached {state}: {v}\n{}",
                self.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// Concatenated text of all `text.delta`/message events in a transcript (best effort).
pub fn transcript_text(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

// ------------------------------------------------------------------------------------------------
// TEST-016: fake connector process manager (channels e2e).
// ------------------------------------------------------------------------------------------------

/// What `provision_connector` hands out: the anchor token and the manifest signature the fake
/// binary sends with `register` (I-28: the core verifies it against a trusted key).
pub struct ConnectorKit {
    pub token: String,
    pub signature: String,
    pub digest: String,
    /// Instance-admin session.
    pub admin: Api,
}

impl Stack {
    /// Make `admin` an instance admin, trust a fresh publisher key for `aurora`, create the
    /// connector `fake` (token) and sign the manifest of `FakeConnector::default()`.
    pub async fn provision_connector(&self, api: &Api, email: &str) -> ConnectorKit {
        use aurora_connector_sdk::Connector;
        use sha2::{Digest, Sha256};
        sqlx::query("UPDATE users SET is_instance_admin = true WHERE email = $1")
            .bind(email)
            .execute(self.pool())
            .await
            .unwrap();
        // The session cache holds the flag as of sign-in: log in again.
        let admin = api.relogin(email).await;
        let key = aurora_crypto::generate_signing_key().unwrap();
        let key_id = aurora_registry::builder::PackageBuilder::key_id(&key);
        let (s, v) = admin
            .post(
                "/v1/admin/trusted-keys",
                json!({"publisher": "e2e-fake", "name": "e2e-fake", "public_key": key_id}),
            )
            .await;
        assert_eq!(s, 201, "trusted key: {v}");
        let (s, v) = admin
            .post(
                "/v1/admin/connectors",
                json!({"id": "fake", "display_name": "Fake"}),
            )
            .await;
        assert_eq!(s, 201, "create connector: {v}");
        let token = v["token"].as_str().unwrap().to_owned();
        let manifest = aurora_connector_fake::FakeConnector::default()
            .with_publisher("e2e-fake")
            .manifest();
        let canon = serde_json::to_value(&manifest).unwrap();
        let d: [u8; 32] = Sha256::digest(aurora_registry::snapshot::canonical_bytes(&canon)).into();
        let signature = aurora_registry::builder::PackageBuilder::sign_digest(&key, &key_id, &d);
        let digest = d.iter().map(|b| format!("{b:02x}")).collect();
        ConnectorKit {
            token,
            signature,
            digest,
            admin,
        }
    }

    /// Spawn a fake connector replica (control API on `port`).
    pub fn spawn_fake(&mut self, kit: &ConnectorKit, port: u16) -> Fake {
        let log = self.dir.join(format!("fake-{port}.log"));
        let f = std::fs::File::create(&log).unwrap();
        let child = Command::new(bin("aurora-connector-fake"))
            .arg("serve")
            .env("AURORA_CORE_URL", format!("http://{API_IP}:4101"))
            .env("AURORA_CONNECTOR_TOKEN", &kit.token)
            .env("AURORA_FAKE_CONTROL", "true")
            .env("AURORA_FAKE_PORT", port.to_string())
            .env("AURORA_FAKE_MANIFEST_SIGNATURE", &kit.signature)
            .env("AURORA_FAKE_PUBLISHER", "e2e-fake")
            .stdout(Stdio::from(f.try_clone().unwrap()))
            .stderr(Stdio::from(f))
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        self.logs.push(log);
        Fake {
            port,
            child: Some(child),
            http: reqwest::Client::new(),
        }
    }

    /// Approve the connector once a replica registered (digest of the announced manifest).
    pub async fn approve_connector(&self, kit: &ConnectorKit) {
        let admin = &kit.admin;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let (s, v) = admin.get("/v1/admin/connectors").await;
            assert_eq!(s, 200, "{v}");
            let c = v["items"]
                .as_array()
                .and_then(|a| a.iter().find(|c| c["id"] == "fake"))
                .cloned()
                .unwrap_or(Value::Null);
            if c["status"] == "active" {
                return;
            }
            if c["status"] == "pending_approval" {
                assert_eq!(
                    c["manifest_digest"].as_str(),
                    Some(kit.digest.as_str()),
                    "the core computed another digest than the test: {c}"
                );
                let (s, v) = admin
                    .post(
                        "/v1/admin/connectors/fake/approve",
                        json!({"manifest_digest": kit.digest}),
                    )
                    .await;
                assert_eq!(s, 200, "approve: {v}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "connector never registered: {c}\n{}",
                self.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
}

impl Api {
    /// A second session of the same user (fresh principal, e.g. after an SQL promotion).
    pub async fn relogin(&self, email: &str) -> Api {
        let http = reqwest::Client::new();
        let cookie = aurora_testkit::oidc::sign_in_over_http(&self.base, "dev", email)
            .await
            .unwrap_or_else(|e| panic!("re-sign-in {email}: {e}"));
        Api {
            http,
            base: self.base.clone(),
            cookie,
            org: self.org.clone(),
            ws: self.ws.clone(),
            daneel: self.daneel.clone(),
        }
    }
}

/// Posts made through the connector: the shared bot appears as `shared-bot`, a dedicated bot as
/// its own ref; people are `U…`.
pub fn is_bot_author(a: &Value) -> bool {
    a.as_str()
        .is_some_and(|a| a == "shared-bot" || a.starts_with('B'))
}

/// A running fake connector replica and its `/fake/*` control API.
pub struct Fake {
    pub port: u16,
    child: Option<Child>,
    http: reqwest::Client,
}

impl Fake {
    fn url(&self, p: &str) -> String {
        format!("http://127.0.0.1:{}{p}", self.port)
    }

    pub async fn ctl(&self, p: &str, body: Value) -> (u16, Value) {
        let r = self
            .http
            .post(self.url(p))
            .json(&body)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn ctl_get(&self, p: &str) -> Value {
        match self
            .http
            .get(self.url(p))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(r) => r.json().await.unwrap_or(Value::Null),
            Err(_) => Value::Null,
        }
    }

    /// Integrations this replica currently serves (ownership).
    pub async fn open_integrations(&self) -> Vec<String> {
        self.ctl_get("/fake/status").await["integrations"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn wait_serving(&self, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while self.open_integrations().await.is_empty() {
            assert!(
                Instant::now() < deadline,
                "replica :{} never took an integration",
                self.port
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    pub async fn user(&self, reference: &str, email: Option<&str>, verified: bool, bot: bool) {
        let (s, v) = self
            .ctl(
                "/fake/users",
                json!({"ref": reference, "display_name": reference, "email": email,
                       "email_verified": verified, "is_bot": bot}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }

    /// DM the shared bot as `author`; returns the platform message ref.
    pub async fn dm(&self, author: &str, text: &str) -> String {
        let (s, v) = self
            .ctl(
                "/fake/messages",
                // Unique ids: the fake's counters restart per process, but the core dedupes
                // inbound events by id across replicas of the same integration.
                json!({"author_ref": author, "text": text,
                       "event_id": format!("ev-{}-{}", self.port, uuid::Uuid::now_v7()),
                       "message_ref": format!("x{}-{}", self.port, uuid::Uuid::now_v7())}),
            )
            .await;
        assert_eq!((s, v["ack"].as_str()), (200, Some("accepted")), "{v}");
        v["message_ref"].as_str().unwrap().to_owned()
    }

    /// Messages on the platform (optionally one space).
    pub async fn posted(&self, space: Option<&str>) -> Vec<Value> {
        let p = match space {
            Some(s) => format!("/fake/posted?space_ref={s}"),
            None => "/fake/posted".into(),
        };
        self.ctl_get(&p).await["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// Wait for a bot-authored message in `space` whose text contains `needle`.
    pub async fn wait_text(&self, space: &str, needle: &str, secs: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            for m in self.posted(Some(space)).await {
                if is_bot_author(&m["author_ref"])
                    && m["text"].as_str().is_some_and(|t| t.contains(needle))
                {
                    return m;
                }
            }
            assert!(
                Instant::now() < deadline,
                "no message containing {needle:?} in {space}: {:?}",
                self.posted(None).await
            );
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    /// First button labelled `label` on a card in `space`: `(token, message_ref)`.
    pub async fn find_button(&self, space: &str, label: &str) -> Option<(String, String)> {
        for m in self.posted(Some(space)).await {
            let blocks = m["card"]["blocks"].as_array().cloned().unwrap_or_default();
            for b in blocks {
                for it in b["items"].as_array().cloned().unwrap_or_default() {
                    if it["label"] == label && b["type"] == "buttons" {
                        return Some((
                            it["token"].as_str()?.to_owned(),
                            m["message_ref"].as_str()?.to_owned(),
                        ));
                    }
                }
            }
        }
        None
    }

    pub async fn wait_button(&self, space: &str, label: &str, secs: u64) -> (String, String) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(b) = self.find_button(space, label).await {
                return b;
            }
            assert!(
                Instant::now() < deadline,
                "no {label:?} button in {space}: {:?}",
                self.posted(None).await
            );
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    /// Click a card button as `clicker`.
    pub async fn click(&self, token: &str, clicker: &str, space: &str, msg: &str) -> Value {
        let (s, v) = self
            .ctl(
                "/fake/actions",
                json!({"token": token, "clicker_ref": clicker, "space_ref": space,
                       "message_ref": msg}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    /// SIGKILL the process (no retire: the ownership lease has to lapse).
    pub async fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill().await;
        }
    }
}

// ------------------------------------------------------------------------------------------------
// MM-002: helpers for connectors other than the fake one (append-only).
// ------------------------------------------------------------------------------------------------

impl Stack {
    /// Like [`Stack::provision_connector`] for any connector `id` with its typed `manifest`
    /// (as the binary will announce it), signed by a fresh trusted publisher key.
    pub async fn provision_connector_with(
        &self,
        api: &Api,
        email: &str,
        id: &str,
        publisher: &str,
        manifest: &Value,
    ) -> ConnectorKit {
        use sha2::{Digest, Sha256};
        sqlx::query("UPDATE users SET is_instance_admin = true WHERE email = $1")
            .bind(email)
            .execute(self.pool())
            .await
            .unwrap();
        let admin = api.relogin(email).await;
        let key = aurora_crypto::generate_signing_key().unwrap();
        let key_id = aurora_registry::builder::PackageBuilder::key_id(&key);
        let (s, v) = admin
            .post(
                "/v1/admin/trusted-keys",
                json!({"publisher": publisher, "name": publisher, "public_key": key_id}),
            )
            .await;
        assert_eq!(s, 201, "trusted key: {v}");
        let (s, v) = admin
            .post(
                "/v1/admin/connectors",
                json!({"id": id, "display_name": id}),
            )
            .await;
        assert_eq!(s, 201, "create connector: {v}");
        let token = v["token"].as_str().unwrap().to_owned();
        let d: [u8; 32] =
            Sha256::digest(aurora_registry::snapshot::canonical_bytes(manifest)).into();
        let signature = aurora_registry::builder::PackageBuilder::sign_digest(&key, &key_id, &d);
        let digest = d.iter().map(|b| format!("{b:02x}")).collect();
        ConnectorKit {
            token,
            signature,
            digest,
            admin,
        }
    }

    /// Approve connector `id` once a replica registered.
    pub async fn approve_connector_named(&self, kit: &ConnectorKit, id: &str) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let (s, v) = kit.admin.get("/v1/admin/connectors").await;
            assert_eq!(s, 200, "{v}");
            let c = v["items"]
                .as_array()
                .and_then(|a| a.iter().find(|c| c["id"] == id))
                .cloned()
                .unwrap_or(Value::Null);
            if c["status"] == "active" {
                return;
            }
            if c["status"] == "pending_approval" {
                assert_eq!(
                    c["manifest_digest"].as_str(),
                    Some(kit.digest.as_str()),
                    "the core computed another digest than the test: {c}"
                );
                let (s, v) = kit
                    .admin
                    .post(
                        &format!("/v1/admin/connectors/{id}/approve"),
                        json!({"manifest_digest": kit.digest}),
                    )
                    .await;
                assert_eq!(s, 200, "approve: {v}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "connector never registered: {c}\n{}",
                self.log_tails()
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// Spawn another service binary (logged next to the core's, killed on drop).
    pub fn spawn_extra(&mut self, name: &str, exe: &str, envs: &[(String, String)]) {
        let log = self.dir.join(format!("{name}.log"));
        let f = std::fs::File::create(&log).unwrap();
        let child = Command::new(bin(exe))
            .arg("serve")
            .envs(envs.iter().map(|(k, v)| (k, v)))
            .stdout(Stdio::from(f.try_clone().unwrap()))
            .stderr(Stdio::from(f))
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        self.children.push((name.to_owned(), child));
        self.logs.push(log);
    }

    /// Stop a service started with [`Stack::spawn_extra`] (or any child) by name.
    pub async fn kill_child(&mut self, name: &str) {
        if let Some(i) = self.children.iter().position(|(n, _)| n == name) {
            let (_, mut c) = self.children.remove(i);
            let _ = c.kill().await;
        }
    }
}
