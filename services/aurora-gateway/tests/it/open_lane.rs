#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P1-003 (RFE-003, SEC-41): open-lane admission (credential capability), the open-lane
//! posture layered over instance / org / workspace, and CONNECT-time rule semantics.

use async_trait::async_trait;
use aurora_config::{EgressEntry, LlmSection, OpenLanePosture as CfgPosture};
use aurora_db::repo::gateway::{PolicyRuleRow, RequestLogRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::governance::{GovSource, Governance, instance_from_config};
use aurora_gateway::guard::Guard;
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, Resolver};
use aurora_secrets::{SecretVault, StoreRegistry};
use base64::Engine;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

struct Ids {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
}

/// Tokens: `aug_byo` (agent), `aub_plain` (boot without capability), `aub_browser` (boot with
/// `browser`), `auc_ctx` (context credential that claims the capability: it must be ignored).
struct Creds(Arc<Ids>);

#[async_trait]
impl CredentialResolver for Creds {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        let (kind, caps) = match token {
            "aug_byo" => (CredentialKind::Agent, vec![]),
            "aub_plain" => (CredentialKind::Boot, vec![]),
            "aub_browser" => (CredentialKind::Boot, vec!["browser".to_owned()]),
            "auc_ctx" => (CredentialKind::Context, vec!["browser".to_owned()]),
            _ => return Ok(None),
        };
        let w = &self.0;
        Ok(Some(RequestIdentity {
            credential_id: Uuid::nil(),
            kind,
            agent_id: w.agent,
            workspace_id: w.ws,
            org_id: w.org,
            conversation_id: None,
            turn_id: None,
            context_label: None,
            principals: vec![w.agent],
            capabilities: caps,
            byo: None,
        }))
    }
}

#[derive(Clone, Default)]
struct Rows {
    org: ScopePolicyRows,
    ws: ScopePolicyRows,
}

struct Src(Mutex<Rows>);

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        let r = self.0.lock().unwrap();
        Ok(if ws.is_some() {
            r.ws.clone()
        } else {
            r.org.clone()
        })
    }
    async fn secrets(&self, _o: Uuid, _w: Uuid) -> Result<Vec<SecretMetaRow>, String> {
        Ok(vec![])
    }
    async fn slugs(&self, _w: Uuid) -> Result<Option<(String, String)>, String> {
        Ok(Some(("acme".into(), "main".into())))
    }
}

struct Gov(Value);

#[async_trait]
impl GovSource for Gov {
    async fn org_governance(&self, _o: Uuid) -> Result<Option<Value>, String> {
        Ok(Some(self.0.clone()))
    }
    async fn write_origin(&self, _id: Uuid, _origin: Value, _status: &'static str) {}
}

#[derive(Default)]
struct Capture(Mutex<Vec<RequestLogRow>>);

#[async_trait]
impl LogWriter for Capture {
    async fn write(&self, rows: &[RequestLogRow]) -> Result<(), String> {
        self.0.lock().unwrap().extend_from_slice(rows);
        Ok(())
    }
}

struct Opts {
    /// `governance.egress.open_lane_minimum`.
    instance: CfgPosture,
    baseline: Vec<&'static str>,
    /// `organizations.governance` document.
    org_doc: Value,
    /// The workspace's published open-lane posture and allow-list.
    ws_posture: &'static str,
    ws_allow: Vec<Value>,
    org_rules: Vec<PolicyRuleRow>,
    ws_rules: Vec<PolicyRuleRow>,
    ws_uncredentialed_block: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            instance: CfgPosture::Enabled,
            baseline: vec![],
            org_doc: json!({}),
            ws_posture: "enabled",
            ws_allow: vec![],
            org_rules: vec![],
            ws_rules: vec![],
            ws_uncredentialed_block: false,
        }
    }
}

struct Env {
    proxy: SocketAddr,
    up: SocketAddr,
    log: Arc<RequestLog>,
    captured: Arc<Capture>,
}

fn rule(name: &str, action: &str, targets: Value, extra: Value) -> PolicyRuleRow {
    let g = |k: &str, d: Value| extra.get(k).cloned().unwrap_or(d);
    PolicyRuleRow {
        logical_id: Uuid::now_v7(),
        name: name.into(),
        enabled: true,
        identities: json!([]),
        targets,
        action: action.into(),
        require_approval: g("require_approval", json!(false)).as_bool().unwrap(),
        rate_limit: extra.get("rate_limit").cloned(),
        conditions: g("conditions", json!([])),
    }
}

async fn start(o: Opts) -> Env {
    let w = Arc::new(Ids {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    });
    // A tunnel target that accepts and echoes.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut b = [0u8; 256];
                while let Ok(n) = s.read(&mut b).await {
                    if n == 0 || s.write_all(&b[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let mut rows = Rows::default();
    rows.org.rules = o.org_rules;
    rows.ws.rules = o.ws_rules;
    rows.ws.open_lane_posture = o.ws_posture.into();
    rows.ws.uncredentialed_allowlist = json!(o.ws_allow);
    if o.ws_uncredentialed_block {
        rows.ws.uncredentialed_default = "block".into();
    }
    let llm_cfg = LlmSection {
        enabled_providers: vec![],
        provider_preference: vec![],
        endpoints: vec![],
    };
    let llm = Arc::new(aurora_llm::ProviderRegistry::from_config(&llm_cfg).unwrap());
    let gcfg = aurora_config::Governance {
        allowed_key_scopes: vec!["organization".into(), "workspace".into()],
        llm_minimum_posture: aurora_config::LlmPosture::Open,
        require_key_origin: false,
        known_llm_hosts: vec![],
        origin_recheck_secs: 3600,
        egress_minimum_posture: aurora_config::EgressPosture::Open,
        baseline_allowlist: o
            .baseline
            .iter()
            .map(|h| EgressEntry {
                host: (*h).into(),
                path: None,
            })
            .collect(),
        open_lane_minimum: o.instance,
        byo: Default::default(),
    };
    let gov = Arc::new(Governance::new(
        instance_from_config(&gcfg, &llm),
        llm.clone(),
        Arc::new(Gov(o.org_doc)),
    ));
    let src = Arc::new(Src(Mutex::new(rows)));
    let resolver = Arc::new(Resolver::new(src, llm).with_governance(gov.clone()));
    let keyring = Arc::new(
        aurora_crypto::Keyring::parse(
            &format!(
                "k1:{}",
                base64::engine::general_purpose::STANDARD.encode([7u8; 32])
            ),
            "k1",
        )
        .unwrap(),
    );
    let reg =
        StoreRegistry::from_config(&aurora_config::SecretsSection::default(), keyring).unwrap();
    let plaintext = Arc::new(PlaintextCache::new(Arc::new(SecretVault::new(reg))));
    let captured = Arc::new(Capture::default());
    let log = RequestLog::new(captured.clone());
    let handler = Arc::new(
        PolicyHandler::new(PolicyHandlerParts {
            resolver: resolver.clone(),
            plaintext,
            log: log.clone(),
            external_url: "https://aurora.test/".into(),
            holds: None,
            touch_pool: None,
        })
        .with_governance(gov),
    );
    let gw = ca::generate("gw").unwrap();
    let pl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = pl.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let (gw_ca, gw_tls) = crate::common::gateway_tls(&gw, proxy);
    let p = Proxy::new(ProxyParts {
        ca: gw_ca,
        tls: gw_tls,
        resolver: Arc::new(Creds(w)),
        handler,
        guard,
        header_timeout: Duration::from_secs(5),
        extra_ca_pem: None,
        egress_proxy: None,
    })
    .unwrap();
    tokio::spawn(p.run(pl, std::future::pending()));
    Env {
        proxy,
        up,
        log,
        captured,
    }
}

/// `CONNECT target` with username `user` and `token`; returns (status, body).
async fn connect(env: &Env, user: &str, token: &str, target: &str) -> (u16, Value) {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{token}"));
    s.write_all(
        format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
    let status = text.split(' ').nth(1).unwrap().parse().unwrap();
    let body = text
        .split_once("\r\n\r\n")
        .and_then(|(_, b)| serde_json::from_str(b).ok())
        .unwrap_or(Value::Null);
    (status, body)
}

/// Open-lane CONNECT with the browser-capable boot credential to the upstream.
async fn tunnel(env: &Env) -> (u16, Value) {
    connect(env, "open", "aub_browser", &env.up.to_string()).await
}

fn code(b: &Value) -> &str {
    b["error"]["code"].as_str().unwrap_or("")
}

// ---- SEC-41: capability ----

#[tokio::test]
async fn capability_matrix() {
    let env = start(Opts::default()).await;
    let target = env.up.to_string();
    let (st, _) = connect(&env, "open", "aub_browser", &target).await;
    assert_eq!(st, 200, "boot credential with the browser capability");
    for token in ["aug_byo", "aub_plain", "auc_ctx"] {
        let (st, b) = connect(&env, "open", token, &target).await;
        assert_eq!(st, 403, "{token}: {b}");
        assert_eq!(code(&b), "open_lane_not_permitted", "{token}");
    }
    // The default lane is unaffected by the capability.
    for token in ["aug_byo", "aub_plain", "aub_browser"] {
        let (st, _) = connect(&env, "agent", token, &target).await;
        assert_eq!(st, 200, "MITM lane {token}");
    }
}

#[tokio::test]
async fn refused_tunnels_are_not_dialled_or_logged_as_tunnels() {
    let env = start(Opts::default()).await;
    let (st, _) = connect(&env, "open", "aug_byo", &env.up.to_string()).await;
    assert_eq!(st, 403);
    env.log.flush().await;
    assert!(
        env.captured
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.decision != "allow")
    );
}

// ---- posture ----

#[tokio::test]
async fn disabled_at_each_level_refuses_with_manage_url() {
    // instance
    let env = start(Opts {
        instance: CfgPosture::Disabled,
        ..Opts::default()
    })
    .await;
    let (st, b) = tunnel(&env).await;
    assert_eq!((st, code(&b)), (403, "open_lane_disabled"), "{b}");
    assert!(
        b["error"]["details"]["manage_url"]
            .as_str()
            .unwrap()
            .contains("/o/acme/governance")
    );
    // org
    let env = start(Opts {
        org_doc: json!({"egress": {"open_lane": "disabled"}}),
        ..Opts::default()
    })
    .await;
    let (st, b) = tunnel(&env).await;
    assert_eq!((st, code(&b)), (403, "open_lane_disabled"), "{b}");
    // workspace
    let env = start(Opts {
        ws_posture: "disabled",
        ..Opts::default()
    })
    .await;
    let (st, b) = tunnel(&env).await;
    assert_eq!((st, code(&b)), (403, "open_lane_disabled"), "{b}");
    assert!(
        b["error"]["details"]["manage_url"]
            .as_str()
            .unwrap()
            .ends_with("/o/acme/w/main/policy")
    );
}

#[tokio::test]
async fn workspace_cannot_loosen_org_or_instance() {
    let env = start(Opts {
        instance: CfgPosture::Disabled,
        ws_posture: "enabled",
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 403);
    let env = start(Opts {
        org_doc: json!({"egress": {"open_lane": "disabled"}}),
        ws_posture: "allowlist_only",
        ws_allow: vec![json!({"host": "127.0.0.1"})],
        ..Opts::default()
    })
    .await;
    assert_eq!(
        tunnel(&env).await.0,
        403,
        "org disabled beats a workspace allow-list"
    );
    // An org stored value looser than the instance minimum is raised to the minimum.
    let env = start(Opts {
        instance: CfgPosture::Disabled,
        org_doc: json!({"egress": {"open_lane": "enabled"}}),
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 403);
}

#[tokio::test]
async fn allowlist_only_needs_every_level_to_cover() {
    let host = "127.0.0.1";
    // workspace level: its own uncredentialed allow-list
    let env = start(Opts {
        ws_posture: "allowlist_only",
        ..Opts::default()
    })
    .await;
    let (st, b) = tunnel(&env).await;
    assert_eq!((st, code(&b)), (403, "blocked_by_egress_policy"), "{b}");
    assert_eq!(b["error"]["details"]["open_lane"], true);
    let env = start(Opts {
        ws_posture: "allowlist_only",
        ws_allow: vec![json!({"host": host})],
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 200);
    // an allow-list entry with a path does not admit a tunnel
    let env = start(Opts {
        ws_posture: "allowlist_only",
        ws_allow: vec![json!({"host": host, "path": "/**"})],
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 403);
    // org level: org list ∪ baseline
    let doc = |list: Value| json!({"egress": {"open_lane": "allowlist_only", "allowlist": list}});
    let env = start(Opts {
        org_doc: doc(json!([])),
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 403);
    let env = start(Opts {
        org_doc: doc(json!([{"host": host}])),
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 200);
    let env = start(Opts {
        org_doc: doc(json!([])),
        baseline: vec![host],
        ..Opts::default()
    })
    .await;
    assert_eq!(
        tunnel(&env).await.0,
        200,
        "baseline counts for the org level"
    );
    let env = start(Opts {
        org_doc: doc(json!([{"host": host, "path": "/x"}])),
        ..Opts::default()
    })
    .await;
    assert_eq!(
        tunnel(&env).await.0,
        403,
        "path entry never admits a tunnel"
    );
    // both levels set: the workspace list must cover as well
    let env = start(Opts {
        org_doc: doc(json!([{"host": host}])),
        ws_posture: "allowlist_only",
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 403, "workspace level does not cover");
    let env = start(Opts {
        org_doc: doc(json!([{"host": host}])),
        ws_posture: "allowlist_only",
        ws_allow: vec![json!({"host": host})],
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 200);
}

#[tokio::test]
async fn instance_allowlist_only_uses_the_baseline() {
    let env = start(Opts {
        instance: CfgPosture::AllowlistOnly,
        // The org's own list does not count for the instance level.
        org_doc: json!({"egress": {"allowlist": [{"host": "127.0.0.1"}]}}),
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 403);
    let env = start(Opts {
        instance: CfgPosture::AllowlistOnly,
        baseline: vec!["127.0.0.1"],
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 200);
}

#[tokio::test]
async fn explicit_fully_knowable_allow_rule_admits_under_allowlist_only() {
    let net = |extra: Value| {
        let mut t = json!({"type": "network", "host": "127.0.0.1"});
        t.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        json!([t])
    };
    let env = start(Opts {
        ws_posture: "allowlist_only",
        ws_rules: vec![rule("ok", "allow", net(json!({})), json!({}))],
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 200);
    let env = start(Opts {
        ws_posture: "allowlist_only",
        ws_rules: vec![rule(
            "scoped",
            "allow",
            net(json!({"path": "/org/**"})),
            json!({}),
        )],
        ..Opts::default()
    })
    .await;
    assert_eq!(
        tunnel(&env).await.0,
        403,
        "a path-scoped allow must not admit a tunnel that can reach every path"
    );
}

// ---- CONNECT-time evaluation ----

#[tokio::test]
async fn path_and_method_scoped_block_refuses_the_tunnel() {
    for extra in [
        json!({"path": "/admin/**"}),
        json!({"methods": ["DELETE"]}),
        json!({}),
    ] {
        let mut t = json!({"type": "network", "host": "127.0.0.1"});
        t.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let env = start(Opts {
            ws_rules: vec![rule("nope", "block", json!([t]), json!({}))],
            ..Opts::default()
        })
        .await;
        let (st, b) = tunnel(&env).await;
        assert_eq!((st, code(&b)), (403, "blocked_by_policy"), "{extra}: {b}");
        assert_eq!(b["error"]["details"]["connect_time"], true);
        assert_eq!(b["error"]["details"]["rule_name"], "nope");
    }
}

#[tokio::test]
async fn body_conditioned_block_refuses_the_tunnel() {
    let env = start(Opts {
        org_rules: vec![rule(
            "body",
            "block",
            json!([{"type": "network", "host": "127.0.0.1"}]),
            json!({"conditions": [{"on": "body", "op": "contains", "value": "DROP"}]}),
        )],
        ..Opts::default()
    })
    .await;
    let (st, b) = tunnel(&env).await;
    assert_eq!((st, code(&b)), (403, "blocked_by_policy"), "{b}");
}

#[tokio::test]
async fn require_approval_refuses_the_tunnel_with_a_reason() {
    let env = start(Opts {
        ws_rules: vec![rule(
            "pay",
            "allow",
            json!([{"type": "network", "host": "127.0.0.1", "path": "/pay/**"}]),
            json!({"require_approval": true}),
        )],
        ..Opts::default()
    })
    .await;
    let (st, b) = tunnel(&env).await;
    assert_eq!((st, code(&b)), (403, "blocked_by_policy"), "{b}");
    assert_eq!(b["error"]["details"]["reason"], "approval_required");
}

#[tokio::test]
async fn credential_targets_never_match_a_tunnel() {
    let env = start(Opts {
        ws_rules: vec![
            rule(
                "secrets",
                "block",
                json!([{"type": "secrets", "scope": "any"}]),
                json!({}),
            ),
            rule(
                "conn",
                "block",
                json!([{"type": "connection", "id": Uuid::now_v7()}]),
                json!({}),
            ),
        ],
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 200);
}

#[tokio::test]
async fn path_scoped_allow_does_not_admit_under_the_uncredentialed_default() {
    let allow = |t: Value| rule("ok", "allow", json!([t]), json!({}));
    // workspace `uncredentialed_default = block`: only a fully knowable allow admits.
    let env = start(Opts {
        ws_uncredentialed_block: true,
        ws_rules: vec![allow(
            json!({"type": "network", "host": "127.0.0.1", "path": "/org/**"}),
        )],
        ..Opts::default()
    })
    .await;
    let (st, b) = tunnel(&env).await;
    assert_eq!((st, code(&b)), (403, "blocked_by_egress_policy"), "{b}");
    let env = start(Opts {
        ws_uncredentialed_block: true,
        ws_rules: vec![allow(json!({"type": "network", "host": "127.0.0.1"}))],
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 200);
}

#[tokio::test]
async fn rate_limit_counts_a_tunnel_as_one_request() {
    let env = start(Opts {
        ws_rules: vec![rule(
            "rl",
            "allow",
            json!([{"type": "network", "host": "127.0.0.1", "path": "/api/**"}]),
            json!({"rate_limit": {"count": 2, "per": "minute"}}),
        )],
        ..Opts::default()
    })
    .await;
    assert_eq!(tunnel(&env).await.0, 200);
    assert_eq!(tunnel(&env).await.0, 200);
    let (st, b) = tunnel(&env).await;
    assert_eq!(st, 429, "{b}");
}

#[tokio::test]
async fn tunnel_row_is_logged_on_the_open_lane() {
    let env = start(Opts::default()).await;
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode("open:aub_browser");
    let t = env.up.to_string();
    s.write_all(
        format!("CONNECT {t} HTTP/1.1\r\nHost: {t}\r\nProxy-Authorization: Basic {auth}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = vec![0u8; 512];
    let n = s.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    s.write_all(b"hello").await.unwrap();
    let _ = s.read(&mut buf).await.unwrap();
    drop(s);
    tokio::time::sleep(Duration::from_millis(300)).await;
    env.log.flush().await;
    let rows = env.captured.0.lock().unwrap().clone();
    let row = rows.iter().find(|r| r.decision == "allow").unwrap();
    assert_eq!(row.lane, "open");
    assert!(row.bytes_up > 0 && row.bytes_down > 0);
}
