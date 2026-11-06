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
        let db = TestDb::new().await;
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

[telemetry]
log_level = "info"

[gateway]
internal_bind = "{OPS_IP}"
allowed_private = ["127.0.0.1/32"]
ca_dir = "{ca}"
advertised_proxy_url = "http://aurora-gateway:4102"

[api]
hookhost_url = "http://127.0.0.1:9"
"#,
            sock = env("AURORA_E2EH_SOCK"),
            net = env("AURORA_E2EH_NETWORK"),
            ca = dir.join("ca").display(),
        );
        let mut stack = Stack {
            db: Some(db),
            api_addr: format!("{API_IP}:4101"),
            ca_pem,
            children: vec![],
            logs: vec![],
            cfg: dir.join("cfg.toml"),
            dir,
        };
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

    /// Sign up (open mode: org + workspace + Daneel), flip Daneel to the fake harness.
    pub async fn signup(&self, email: &str) -> Api {
        let http = reqwest::Client::new();
        let r = http
            .post(format!("http://{}/v1/auth/signup", self.api_addr))
            .header("origin", ORIGIN)
            .json(&json!({"email": email, "password": "correct horse battery staple 42", "display_name": "E2E"}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "signup: {}", r.text().await.unwrap());
        let cookie = r
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap().to_owned())
            .collect::<Vec<_>>()
            .join("; ");
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

    /// Raw request with the session cookie (for SSE).
    pub fn raw(&self, p: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{p}", self.base))
            .header("cookie", &self.cookie)
            .header("origin", ORIGIN)
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
