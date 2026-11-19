#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-005: LLM and egress governance (step 6a, org egress posture, origin preflight, backstop).
//! The mock provider is Anthropic-style: `GET /v1/models` reports the key's account in
//! `anthropic-organization-id`, `POST /v1/messages` does too.

use async_trait::async_trait;
use aurora_config::{EgressEntry, LlmEndpoint, LlmSection, SecretsSection};
use aurora_db::repo::gateway::{PolicyRuleRow, RequestLogRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::governance::{GovSource, Governance, instance_from_config};
use aurora_gateway::guard::Guard;
use aurora_gateway::invalidation::{Invalidatable, Invalidate, Scope};
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, Resolver};
use aurora_secrets::{MemoryStore, SecretRef, SecretVault, StoreRegistry};
use base64::Engine;
use bytes::Bytes;
use http::{Request, Response};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

const TOKEN: &str = "aug_good";
const BOOT: &str = "aub_browser";

struct World {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
}

struct Static(Arc<World>);

#[async_trait]
impl CredentialResolver for Static {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        // `BOOT` is a boot credential minted with the `browser` capability (RFE-003).
        if token != TOKEN && token != BOOT {
            return Ok(None);
        }
        let boot = token == BOOT;
        let w = &self.0;
        Ok(Some(RequestIdentity {
            credential_id: Uuid::nil(),
            kind: if boot {
                CredentialKind::Boot
            } else {
                CredentialKind::Agent
            },
            agent_id: w.agent,
            workspace_id: w.ws,
            org_id: w.org,
            conversation_id: None,
            turn_id: None,
            trace_context: None,
            context_label: None,
            speaker_user_id: None,
            speaker_group_ids: Vec::new(),
            capabilities: if boot { vec!["browser".into()] } else { vec![] },
            byo: None,
        }))
    }
}

struct Src(Mutex<Vec<SecretMetaRow>>, Mutex<ScopePolicyRows>);

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        Ok(if ws.is_some() {
            self.1.lock().unwrap().clone()
        } else {
            ScopePolicyRows::default()
        })
    }
    async fn secrets(&self, _o: Uuid, _w: Uuid) -> Result<Vec<SecretMetaRow>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn slugs(&self, _w: Uuid) -> Result<Option<(String, String)>, String> {
        Ok(Some(("acme".into(), "main".into())))
    }
}

#[derive(Default)]
struct TestGov {
    doc: Mutex<Value>,
    writes: Mutex<Vec<(Uuid, Value, String)>>,
}

#[async_trait]
impl GovSource for TestGov {
    async fn org_governance(&self, _o: Uuid) -> Result<Option<Value>, String> {
        Ok(Some(self.doc.lock().unwrap().clone()))
    }
    async fn write_origin(&self, id: Uuid, origin: Value, status: &'static str) {
        self.writes
            .lock()
            .unwrap()
            .push((id, origin, status.to_owned()));
    }
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

/// `(path, headers)` of one upstream request.
type Seen = (String, Vec<(String, String)>);

#[derive(Default)]
struct Counters {
    verifies: AtomicUsize,
    messages: AtomicUsize,
    /// Requests to any other path (FIX-105: management and storage routes).
    other: AtomicUsize,
    /// (P4-LLM-1) `(path, headers)` of every request the upstream received.
    seen: Mutex<Vec<Seen>>,
}

fn org_of(key: &str) -> &'static str {
    if key.contains("foreign") {
        "org_bad"
    } else {
        "org_ok"
    }
}

async fn mock(
    req: Request<Incoming>,
    c: Arc<Counters>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            // LiteLLM keys travel as `Authorization: Bearer`.
            req.headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        })
        .unwrap_or("")
        .to_owned();
    c.seen.lock().unwrap().push((
        req.uri().path().to_owned(),
        req.headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
            .collect(),
    ));
    let mut r = Response::new(Full::new(Bytes::from(format!("key={key}"))));
    // (P4-LLM-1) OpenAI-style answers: a status picked by the key, an SSE stream on `/v1/responses`.
    // (Only for the `openai` endpoint, recognised by the project header the gateway sets.)
    if req.uri().path() == "/v1/responses" && req.headers().contains_key("openai-project") {
        r.headers_mut()
            .insert("content-type", "text/event-stream".parse().unwrap());
        *r.body_mut() = Full::new(Bytes::from(SSE));
    }
    for (needle, status) in [("e401", 401), ("e429", 429), ("e500", 500)] {
        if key.contains(needle) && req.uri().path() == "/v1/chat/completions" {
            *r.status_mut() = http::StatusCode::from_u16(status).unwrap();
            *r.body_mut() = Full::new(Bytes::from(
                "{\"error\":{\"code\":\"insufficient_quota\",\"message\":\"upstream\"}}",
            ));
        }
    }
    match req.uri().path() {
        "/v1/models" => {
            c.verifies.fetch_add(1, Ordering::SeqCst);
            if key.contains("down") {
                *r.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
            } else {
                r.headers_mut()
                    .insert("anthropic-organization-id", org_of(&key).parse().unwrap());
                *r.body_mut() = Full::new(Bytes::from("{\"data\":[]}"));
            }
        }
        "/v1/messages" => {
            c.messages.fetch_add(1, Ordering::SeqCst);
            let org = if key.contains("sneaky") {
                Some("org_bad")
            } else if key.contains("nohdr") {
                None
            } else {
                Some(org_of(&key))
            };
            if let Some(o) = org {
                r.headers_mut()
                    .insert("anthropic-organization-id", o.parse().unwrap());
            }
        }
        _ => {
            c.other.fetch_add(1, Ordering::SeqCst);
        }
    }
    Ok(r)
}

struct Opts {
    /// Org governance document.
    doc: Value,
    llm_min: aurora_config::LlmPosture,
    egress_min: aurora_config::EgressPosture,
    instance_models: Vec<String>,
    /// Keys held by Aurora: (stored key, workspace scoped, stored origin).
    secrets: Vec<(&'static str, bool, Option<Value>)>,
    known_hosts: Vec<String>,
    /// (FIX-105) `Some(allowed_paths)`: the endpoint is a LiteLLM one (`central`) instead of
    /// Anthropic's, with those operator-declared extra paths.
    litellm: Option<Vec<String>>,
    /// (P4-LLM-1, D-260(b)) `allowed_paths` of the Anthropic endpoint.
    anthropic_paths: Vec<String>,
    /// (P4-LLM-1) The endpoint is an `openai` one (`openai`) whose fixed origin is replaced by the mock
    /// (`llm.dev_upstream_overrides`), configured with `organization = org-cfg`, `project = proj-cfg`.
    openai: bool,
    /// (P4-LLM-2) The endpoint is a `vertex` one (`vertex`, project `proj-1`, `us-east5`) whose Vertex host and Google's
    /// token endpoint are replaced by mocks (`llm.dev_upstream_overrides`); the secret is a generated service-account key.
    vertex: bool,
    /// (P4-LLM-3) The endpoint is an `azure_openai` one (`azure`, deployment `gpt-4o`) on a mock resource; `Some(true)`:
    /// the secret is an Entra application (token from the authority mock, `llm.dev_upstream_overrides`), `Some(false)`:
    /// the resource key.
    azure: Option<bool>,
    /// (P4-LLM-4) The endpoint is a `bedrock` one (`bedrock`, `us-east-1`, the mock as its VPC `base_url`; STS replaced
    /// through `llm.dev_upstream_overrides`) used with this credential mode; the organization row (with its external id)
    /// is seeded in a scratch database.
    bedrock: Option<BedrockMode>,
    /// (P4-LLM-4) The seeded organization's `external_id` (empty: none, the role must not be assumed).
    org_external_id: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BedrockMode {
    AssumeRole,
    Sigv4,
    ApiKey,
}

/// Byte-exact SSE body the mock streams (acceptance 3: passthrough).
const SSE: &str = "event: response.created\ndata: {\"a\":1}\n\n: keep-alive\n\ndata: [DONE]\n\n";

fn doc() -> Value {
    json!({"llm": {"posture": "open", "endpoints": {"anthropic": {"allowed": true}}}})
}

fn verified(org: &str) -> Value {
    json!({"status": "verified", "method": "provider_account", "organization_id": org,
           "value_version": 1, "checked_at": null})
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            doc: doc(),
            llm_min: aurora_config::LlmPosture::Open,
            egress_min: aurora_config::EgressPosture::Open,
            instance_models: vec![],
            secrets: vec![("sk-good", true, Some(verified("org_ok")))],
            known_hosts: vec![],
            litellm: None,
            anthropic_paths: vec![],
            openai: false,
            vertex: false,
            azure: None,
            bedrock: None,
            org_external_id: "ext-org-1",
        }
    }
}

struct Env {
    /// (FIX-180) The proxy, for its test override of the body idle limit.
    px: Arc<Proxy>,
    proxy: SocketAddr,
    llm: SocketAddr,
    plain: SocketAddr,
    gov: Arc<Governance>,
    gsrc: Arc<TestGov>,
    resolver: Arc<Resolver>,
    counters: Arc<Counters>,
    mem: Arc<MemoryStore>,
    captured: Arc<Capture>,
    log: Arc<RequestLog>,
    secret_ids: Vec<Uuid>,
    /// (P4-LLM-2) The Vertex mocks of an `Opts::vertex` run.
    vertex: Option<VertexRig>,
    /// (P4-LLM-3) The Azure mocks of an `Opts::azure` run.
    azure: Option<AzureRig>,
    /// (P4-LLM-4) The AWS mocks of an `Opts::bedrock` run.
    bedrock: Option<BedrockRig>,
    org: Uuid,
}

const BEDROCK_SOURCE_KEY: &str = "AKIASOURCEKEY00001";
const BEDROCK_SOURCE_SECRET: &str = "source-secret-never-seen-by-bedrock";
const BEDROCK_STATIC_KEY: &str = "AKIASTATICKEY00001";
const BEDROCK_STATIC_SECRET: &str = "static-secret-for-inference";
const BEDROCK_API_KEY: &str = "bedrock-api-key-value";
const BEDROCK_ROLE: &str = "arn:aws:iam::123456789012:role/aurora-bedrock";

struct BedrockRig {
    mock: aurora_testkit::llm::bedrock::MockBedrock,
    sts: aurora_testkit::llm::sts::MockSts,
    /// Keeps the scratch database (the organization row) alive.
    _db: aurora_testkit::TestDb,
}

const AZURE_KEY: &str = "azure-resource-key";
const AZURE_TENANT: &str = "11111111-2222-3333-4444-555555555555";

struct AzureRig {
    app: aurora_testkit::llm::entra::TestApp,
    entra: aurora_testkit::llm::entra::MockEntra,
    upstream: aurora_testkit::llm::azure::MockAzure,
}

struct VertexRig {
    key: aurora_testkit::llm::google_token::TestKey,
    google: aurora_testkit::llm::google_token::MockGoogleToken,
    upstream: aurora_testkit::llm::vertex::MockVertex,
}

async fn serve_mock(c: Arc<Counters>) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let c = c.clone();
            tokio::spawn(async move {
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(s), service_fn(move |r| mock(r, c.clone())))
                    .await;
            });
        }
    });
    a
}

async fn start(o: Opts) -> Env {
    let w = Arc::new(World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    });
    let counters = Arc::new(Counters::default());
    let mut llm_addr = serve_mock(counters.clone()).await;
    let vertex = if o.vertex {
        let key = aurora_testkit::llm::google_token::TestKey::generate(
            "key-proj",
            "sa@key-proj.iam.gserviceaccount.com",
        );
        let google = aurora_testkit::llm::google_token::MockGoogleToken::start(&key).await;
        let upstream = aurora_testkit::llm::vertex::MockVertex::start().await;
        llm_addr = upstream
            .base()
            .trim_start_matches("http://")
            .parse()
            .unwrap();
        Some(VertexRig {
            key,
            google,
            upstream,
        })
    } else {
        None
    };
    let azure = if o.azure.is_some() {
        let app = aurora_testkit::llm::entra::TestApp {
            tenant_id: AZURE_TENANT.into(),
            client_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            client_secret: "entra-client-secret-value".into(),
        };
        let entra = aurora_testkit::llm::entra::MockEntra::start(&app).await;
        let upstream = aurora_testkit::llm::azure::MockAzure::start(AZURE_KEY).await;
        llm_addr = upstream
            .base()
            .trim_start_matches("http://")
            .parse()
            .unwrap();
        Some(AzureRig {
            app,
            entra,
            upstream,
        })
    } else {
        None
    };
    let bedrock = if let Some(mode) = o.bedrock {
        let db = aurora_testkit::TestDb::new().await;
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'bedrock-org','O',$2)",
        )
        .bind(w.org)
        .bind(o.org_external_id)
        .execute(db.pool())
        .await
        .unwrap();
        let mock = aurora_testkit::llm::bedrock::MockBedrock::start().await;
        let sts = aurora_testkit::llm::sts::MockSts::start(
            BEDROCK_SOURCE_KEY,
            BEDROCK_SOURCE_SECRET,
            "123456789012",
        )
        .await;
        match mode {
            BedrockMode::AssumeRole => {
                let m = mock.clone();
                sts.on_session(move |ak, sk, tok| m.accept_key(ak, sk, Some(tok)));
            }
            BedrockMode::Sigv4 => mock.accept_key(BEDROCK_STATIC_KEY, BEDROCK_STATIC_SECRET, None),
            BedrockMode::ApiKey => mock.accept_api_key(BEDROCK_API_KEY),
        }
        llm_addr = mock.base().trim_start_matches("http://").parse().unwrap();
        Some(BedrockRig { mock, sts, _db: db })
    } else {
        None
    };
    let plain = serve_mock(Arc::new(Counters::default())).await;
    let mem = Arc::new(MemoryStore::new("mem"));
    mem.allow_reference(w.org, "*");
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
    let mut reg = StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap();
    reg.register(mem.clone());
    let mut rows = Vec::new();
    let mut policy = ScopePolicyRows::default();
    let mut secret_ids = Vec::new();
    for (i, (key, ws_scoped, origin)) in o.secrets.iter().enumerate() {
        let path = format!("k{i}");
        match (&vertex, &azure) {
            _ if o.bedrock.is_some() => {
                let value = match o.bedrock {
                    Some(BedrockMode::AssumeRole) => json!({
                        "access_key_id": BEDROCK_SOURCE_KEY,
                        "secret_access_key": BEDROCK_SOURCE_SECRET,
                        "role_arn": BEDROCK_ROLE,
                    })
                    .to_string(),
                    Some(BedrockMode::Sigv4) => json!({
                        "access_key_id": BEDROCK_STATIC_KEY,
                        "secret_access_key": BEDROCK_STATIC_SECRET,
                    })
                    .to_string(),
                    _ => BEDROCK_API_KEY.to_owned(),
                };
                mem.preload(&path, None, value.as_bytes());
            }
            (Some(v), _) => mem.preload(&path, None, v.key.json().as_bytes()),
            (_, Some(a)) if o.azure == Some(true) => {
                mem.preload(&path, None, a.app.json().as_bytes())
            }
            (_, Some(_)) => mem.preload(&path, None, AZURE_KEY.as_bytes()),
            _ => mem.preload(&path, None, key.as_bytes()),
        }
        let id = Uuid::now_v7();
        secret_ids.push(id);
        rows.push(SecretMetaRow {
            id,
            workspace_id: ws_scoped.then_some(w.ws),
            name: format!("key{i}"),
            kind: "llm".into(),
            llm_provider: Some(
                if o.vertex {
                    "vertex"
                } else if o.bedrock.is_some() {
                    "bedrock"
                } else if o.azure.is_some() {
                    "azure_openai"
                } else if o.openai {
                    "openai"
                } else if o.litellm.is_some() {
                    "litellm"
                } else {
                    "anthropic"
                }
                .into(),
            ),
            llm_auth_mode: Some(
                if o.vertex {
                    "gcp_service_account"
                } else if o.bedrock == Some(BedrockMode::AssumeRole) {
                    "aws_assume_role"
                } else if o.bedrock == Some(BedrockMode::Sigv4) {
                    "aws_sigv4"
                } else if o.azure == Some(true) {
                    "azure_entra"
                } else {
                    "api_key"
                }
                .into(),
            ),
            llm_endpoint_id: Some(
                if o.vertex {
                    "vertex"
                } else if o.bedrock.is_some() {
                    "bedrock"
                } else if o.azure.is_some() {
                    "azure"
                } else if o.openai {
                    "openai"
                } else if o.litellm.is_some() {
                    "central"
                } else {
                    "anthropic"
                }
                .into(),
            ),
            host_pattern: llm_addr.to_string(),
            path_pattern: None,
            injection: json!({}),
            value_ref: SecretRef::ext("mem", &path, None, "1").unwrap().to_column(),
            value_version: 1,
            origin: origin.clone(),
        });
        policy.rules.push(PolicyRuleRow {
            logical_id: Uuid::now_v7(),
            name: "grant".into(),
            enabled: true,
            identities: json!([]),
            targets: json!([{"type":"secret","id":id}]),
            action: "allow".into(),
            require_approval: false,
            approval_timeout_secs: None,
            rate_limit: None,
            conditions: json!([]),
        });
    }
    let src = Arc::new(Src(Mutex::new(rows), Mutex::new(policy)));
    let cfg = match &o.litellm {
        _ if o.bedrock.is_some() => {
            let b = bedrock.as_ref().unwrap();
            LlmSection {
                enabled_providers: vec!["bedrock".into()],
                provider_preference: vec![],
                dev_upstream_overrides: [(
                    "https://sts.us-east-1.amazonaws.com".to_owned(),
                    b.sts.base().to_owned(),
                )]
                .into(),
                endpoints: vec![LlmEndpoint {
                    id: "bedrock".into(),
                    provider: "bedrock".into(),
                    region: Some("us-east-1".into()),
                    base_url: Some(b.mock.base().to_owned()),
                    allowed_models: o.instance_models.clone(),
                    ..Default::default()
                }],
            }
        }
        _ if o.azure.is_some() => {
            let a = azure.as_ref().unwrap();
            LlmSection {
                enabled_providers: vec!["azure_openai".into()],
                provider_preference: vec![],
                dev_upstream_overrides: [(
                    "https://login.microsoftonline.com".to_owned(),
                    a.entra.base().to_owned(),
                )]
                .into(),
                endpoints: vec![LlmEndpoint {
                    id: "azure".into(),
                    provider: "azure_openai".into(),
                    base_url: Some(a.upstream.base().to_owned()),
                    models: vec!["gpt-4o".into(), "gpt-4o-mini".into()],
                    default_model: Some("gpt-4o".into()),
                    allowed_models: o.instance_models.clone(),
                    ..Default::default()
                }],
            }
        }
        _ if o.vertex => {
            let v = vertex.as_ref().unwrap();
            LlmSection {
                enabled_providers: vec!["vertex".into()],
                provider_preference: vec![],
                dev_upstream_overrides: [
                    (
                        "https://oauth2.googleapis.com".to_owned(),
                        v.google.base().to_owned(),
                    ),
                    (
                        "https://us-east5-aiplatform.googleapis.com".to_owned(),
                        v.upstream.base().to_owned(),
                    ),
                ]
                .into(),
                endpoints: vec![LlmEndpoint {
                    id: "vertex".into(),
                    provider: "vertex".into(),
                    project_id: Some("proj-1".into()),
                    location: Some("us-east5".into()),
                    models: vec!["claude-x".into()],
                    allowed_models: o.instance_models.clone(),
                    ..Default::default()
                }],
            }
        }
        _ if o.openai => LlmSection {
            enabled_providers: vec!["openai".into()],
            provider_preference: vec![],
            dev_upstream_overrides: [(
                "https://api.openai.com".to_owned(),
                format!("http://{llm_addr}"),
            )]
            .into(),
            endpoints: vec![LlmEndpoint {
                id: "openai".into(),
                provider: "openai".into(),
                allowed_models: o.instance_models.clone(),
                organization: Some("org-cfg".into()),
                project: Some("proj-cfg".into()),
                ..Default::default()
            }],
        },
        Some(paths) => LlmSection {
            enabled_providers: vec!["litellm".into()],
            provider_preference: vec![],
            dev_upstream_overrides: Default::default(),
            endpoints: vec![LlmEndpoint {
                id: "central".into(),
                provider: "litellm".into(),
                base_url: Some(format!("http://{llm_addr}")),
                allowed_paths: paths.clone(),
                ..Default::default()
            }],
        },
        None => LlmSection {
            enabled_providers: vec!["anthropic".into()],
            provider_preference: vec![],
            dev_upstream_overrides: Default::default(),
            endpoints: vec![LlmEndpoint {
                id: "anthropic".into(),
                provider: "anthropic".into(),
                allowed_models: o.instance_models.clone(),
                allowed_paths: o.anthropic_paths.clone(),
                key_origin: Some(aurora_config::KeyOrigin {
                    method: "provider_account".into(),
                    allowed_organization_ids: vec!["org_ok".into()],
                    ..Default::default()
                }),
                ..Default::default()
            }],
        },
    };
    let llm = Arc::new(
        if o.litellm.is_some() || o.openai || o.vertex || o.azure.is_some() || o.bedrock.is_some() {
            aurora_llm::ProviderRegistry::from_config(&cfg).unwrap()
        } else {
            aurora_llm::ProviderRegistry::from_config_with_anthropic_base(
                &cfg,
                &format!("http://{llm_addr}"),
            )
            .unwrap()
        },
    );
    let gcfg = aurora_config::Governance {
        allowed_key_scopes: vec!["organization".into(), "workspace".into()],
        llm_minimum_posture: o.llm_min,
        require_key_origin: false,
        known_llm_hosts: o.known_hosts.clone(),
        origin_recheck_secs: 3600,
        egress_minimum_posture: o.egress_min,
        baseline_allowlist: Vec::<EgressEntry>::new(),
        open_lane_minimum: aurora_config::OpenLanePosture::Enabled,
        byo: Default::default(),
        agent_token_max_age_days: None,
    };
    let gsrc = Arc::new(TestGov {
        doc: Mutex::new(o.doc.clone()),
        ..Default::default()
    });
    let gov = Arc::new(Governance::new(
        instance_from_config(&gcfg, &llm),
        llm.clone(),
        gsrc.clone(),
    ));
    let resolver = Arc::new(Resolver::new(src, llm.clone()).with_governance(gov.clone()));
    let plaintext = Arc::new(PlaintextCache::with_ttl(
        Arc::new(SecretVault::new(reg)),
        Duration::from_millis(1),
    ));
    let captured = Arc::new(Capture::default());
    let log = RequestLog::new(captured.clone());
    let mint_guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![]).unwrap());
    let llm_mint = Arc::new(
        aurora_gateway::llm_mint::LlmMint::new(
            Arc::new(
                aurora_gateway::upstream::Upstream::new(
                    mint_guard,
                    None,
                    None,
                    Duration::from_secs(10),
                )
                .unwrap(),
            ),
            llm.overrides().clone(),
        )
        .with_limits(
            100,
            Duration::from_secs(300),
            Duration::from_secs(30),
            Duration::from_secs(10),
        ),
    );
    let handler = Arc::new(
        PolicyHandler::new(PolicyHandlerParts {
            resolver: resolver.clone(),
            plaintext,
            log: log.clone(),
            external_url: "https://aurora.test/".into(),
            holds: None,
            touch_pool: bedrock.as_ref().map(|b| b._db.pool().clone()),
        })
        .with_governance(gov.clone())
        .with_insecure_loopback_injection(true)
        .with_llm_mint(llm_mint),
    );
    let gw = ca::generate("gw").unwrap();
    let pl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = pl.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let (gw_ca, gw_tls) = crate::common::gateway_tls(&gw, proxy);
    let p = Proxy::new(ProxyParts {
        ca: gw_ca,
        tls: gw_tls,
        resolver: Arc::new(Static(w.clone())),
        handler,
        guard,
        header_timeout: Duration::from_secs(5),
        extra_ca_pem: None,
        egress_proxy: None,
    })
    .unwrap();
    let px = p.clone();
    tokio::spawn(p.run(pl, std::future::pending()));
    Env {
        px,
        proxy,
        llm: llm_addr,
        plain,
        gov,
        gsrc,
        resolver,
        counters,
        mem,
        captured,
        log,
        secret_ids,
        vertex,
        azure,
        bedrock,
        org: w.org,
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    body: String,
    /// What the read ended with when no status came back: `"timeout (N bytes)"`, `"closed (N
    /// bytes)"` or the read error. A diagnostic only.
    ended: String,
    /// (FIX-177) TLS records of the request were still queued in the client's session when the
    /// body's `write_all` returned (the socket was full), and the flush sent them. A diagnostic.
    queued: bool,
}

fn auth(user: &str) -> String {
    // The open lane needs a boot credential with the `browser` capability (RFE-003).
    let token = if user == "open" { BOOT } else { TOKEN };
    format!(
        "Proxy-Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{token}"))
    )
}

async fn request(
    env: &Env,
    target: SocketAddr,
    method: &str,
    path: &str,
    extra: &str,
    body: &[u8],
) -> Reply {
    let s = crate::common::connect(env.proxy).await;
    request_on(s, target, method, path, extra, body).await
}

/// [`request`] on a connection the caller opened (FIX-177: one with a tight send buffer).
async fn request_on(
    mut s: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    target: SocketAddr,
    method: &str,
    path: &str,
    extra: &str,
    body: &[u8],
) -> Reply {
    let head = format!(
        "{method} http://{target}{path} HTTP/1.1\r\nHost: {target}\r\n{}{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        auth("agent"),
        body.len()
    );
    s.write_all(head.as_bytes()).await.unwrap();
    // Flushed: without it the end of a large body can stay in the TLS session (FIX-177).
    let queued = crate::common::write_flushed(&mut s, body).await;
    let mut out = Vec::new();
    // A multi-MiB body (the SigV4 signing limit is 8 MiB) is buffered, hashed and signed by a debug
    // build: on a loaded CI runner that alone can take longer than 5 s. The read ends when the
    // gateway closes, so the longer deadline costs nothing when it answers.
    let wait = if body.len() > 1 << 20 {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(5)
    };
    let ended = match tokio::time::timeout(wait, s.read_to_end(&mut out)).await {
        Err(_) => format!("timeout after {wait:?} ({} bytes)", out.len()),
        Ok(Ok(_)) => format!("closed ({} bytes)", out.len()),
        Ok(Err(e)) => format!("read error {e} ({} bytes)", out.len()),
    };
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Reply {
        status: head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        body: body.to_owned(),
        ended,
        queued,
    }
}

async fn msg(env: &Env, extra: &str, body: &Value) -> Reply {
    request(
        env,
        env.llm,
        "POST",
        "/v1/messages",
        extra,
        body.to_string().as_bytes(),
    )
    .await
}

async fn connect(env: &Env, target: SocketAddr) -> u16 {
    let mut s = crate::common::connect(env.proxy).await;
    s.write_all(
        format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{}\r\n",
            auth("open")
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = vec![0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&buf[..n])
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn has(r: &Reply, code: &str) -> bool {
    r.body.contains(&format!("\"{code}\""))
}

fn hello() -> Value {
    json!({"model": "claude-x", "messages": []})
}

async fn wait_writes(env: &Env, n: usize) -> Vec<(Uuid, Value, String)> {
    for _ in 0..100 {
        {
            let w = env.gsrc.writes.lock().unwrap();
            if w.len() >= n {
                return w.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("origin write-back did not happen");
}

// ---- FIX-180: request body idle limit ----

/// Sends the head of a `POST /v1/messages` with `Content-Length: len` and returns the client stream.
async fn open_post(
    env: &Env,
    len: usize,
) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
    let mut s = crate::common::connect(env.proxy).await;
    let head = format!(
        "POST http://{t}/v1/messages HTTP/1.1\r\nHost: {t}\r\n{}Content-Type: application/json\r\nContent-Length: {len}\r\n\r\n",
        auth("agent"),
        t = env.llm
    );
    s.write_all(head.as_bytes()).await.unwrap();
    s
}

/// (FIX-180, D-395, C-1040) A client that sends half a body and stops is answered 408
/// `request_body_timeout` after the idle limit, the connection is closed, and the log row says so.
#[tokio::test]
async fn a_stalled_request_body_is_answered_408_and_the_connection_closes() {
    let env = start(Opts::default()).await;
    env.px.set_body_idle_timeout(Duration::from_millis(300));
    let body = hello().to_string();
    let mut s = open_post(&env, body.len()).await;
    crate::common::write_flushed(&mut s, &body.as_bytes()[..body.len() / 2]).await;
    let mut out = Vec::new();
    // Bounded: before the fix nothing ever answered and this read hit the 10 s bound.
    let ended = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    assert!(
        ended.is_ok(),
        "the connection stayed open after the idle limit: {text}"
    );
    assert!(text.starts_with("HTTP/1.1 408"), "{text}");
    assert!(text.contains("request_body_timeout"), "{text}");
    assert!(text.to_ascii_lowercase().contains("connection: close"));
    env.log.flush().await;
    let rows = env.captured.0.lock().unwrap().clone();
    assert!(
        rows.iter()
            .any(|r| r.status == 408 && r.error_code.as_deref() == Some("request_body_timeout")),
        "{rows:?}"
    );
    assert_eq!(env.counters.messages.load(Ordering::SeqCst), 0);
}

/// (FIX-180) The limit is idle time, not total time: a body that keeps moving, slower in total than
/// the limit, is not cut.
#[tokio::test]
async fn a_slow_but_moving_request_body_is_not_cut() {
    let env = start(Opts::default()).await;
    env.px.set_body_idle_timeout(Duration::from_millis(600));
    let body = hello().to_string();
    let mut s = open_post(&env, body.len()).await;
    let started = std::time::Instant::now();
    for chunk in body.as_bytes().chunks(4) {
        crate::common::write_flushed(&mut s, chunk).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert!(
        started.elapsed() > Duration::from_millis(600),
        "the body must take longer than the idle limit"
    );
    let mut out = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut out))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&out[..n]).into_owned();
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
}

// ---- step 6a ----

#[tokio::test]
async fn compliant_key_is_injected_and_verified_once() {
    let env = start(Opts::default()).await;
    let r = msg(&env, "x-api-key: agent-own\r\n", &hello()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("key=sk-good"), "{}", r.body);
    assert_eq!(env.counters.verifies.load(Ordering::SeqCst), 1);
    // Second request: verification cached by (secret, sha256(value)).
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 200);
    assert_eq!(env.counters.verifies.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn endpoint_not_permitted_and_known_host_under_sanctioned_only() {
    let o = Opts {
        doc: json!({"llm": {"posture": "open", "endpoints": {"anthropic": {"allowed": false}}}}),
        ..Opts::default()
    };
    let env = start(o).await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_provider_not_permitted"), "{}", r.body);
    assert_eq!(env.counters.messages.load(Ordering::SeqCst), 0);

    // Known LLM host (no configured endpoint): refused only under sanctioned_only.
    let known: SocketAddr = "127.0.0.1:59871".parse().unwrap();
    let mk = |posture: &str| Opts {
        doc: json!({"llm": {"posture": posture, "endpoints": {"anthropic": {"allowed": true}}}}),
        known_hosts: vec![known.to_string()],
        secrets: vec![],
        ..Opts::default()
    };
    let env = start(mk("sanctioned_only")).await;
    let r = request(&env, known, "GET", "/x", "", b"").await;
    assert!(has(&r, "llm_provider_not_permitted"), "{}", r.body);
    let env = start(mk("open")).await;
    let r = request(&env, known, "GET", "/x", "", b"").await;
    assert!(!has(&r, "llm_provider_not_permitted"), "{}", r.body);
}

#[tokio::test]
async fn key_scope_not_permitted() {
    let o = Opts {
        doc: json!({"llm": {"endpoints": {"anthropic": {"allowed": true, "key_scopes": ["organization"]}}}}),
        ..Opts::default()
    };
    let env = start(o).await; // the key is workspace scoped
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_not_permitted"), "{}", r.body);
    assert!(r.body.contains(&env.secret_ids[0].to_string()));
}

#[tokio::test]
async fn stored_origin_must_comply() {
    for (origin, code) in [
        (Some(verified("org_bad")), "llm_key_wrong_organization"),
        (
            Some(json!({"status":"mismatch","value_version":1})),
            "llm_key_wrong_organization",
        ),
        (
            Some(json!({"status":"pending","value_version":1})),
            "llm_key_unverifiable",
        ),
        (None, "llm_key_unverifiable"),
        // verified, but for an older value
        (
            Some(json!({"status":"verified","organization_id":"org_ok","value_version":0})),
            "llm_key_unverifiable",
        ),
    ] {
        let env = start(Opts {
            secrets: vec![("sk-good", true, origin)],
            ..Opts::default()
        })
        .await;
        let r = msg(&env, "", &hello()).await;
        assert_eq!(r.status, 403, "{code}: {}", r.body);
        assert!(has(&r, code), "{code}: {}", r.body);
        // Never forwarded without the key instead.
        assert_eq!(env.counters.messages.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn sanctioned_only_requires_an_aurora_key() {
    let sanctioned = json!({"llm": {"posture": "sanctioned_only", "endpoints": {"anthropic": {"allowed": true}}}});
    let env = start(Opts {
        doc: sanctioned.clone(),
        secrets: vec![],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "x-api-key: agent-own\r\n", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_required"), "{}", r.body);
    assert!(r.body.contains("anthropic"));
    // Instance minimum raises an open org to sanctioned_only.
    let env = start(Opts {
        llm_min: aurora_config::LlmPosture::SanctionedOnly,
        secrets: vec![],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert!(has(&r, "llm_key_required"), "{}", r.body);
    // Open posture: an agent-supplied key passes through untouched.
    let env = start(Opts {
        secrets: vec![],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "x-api-key: agent-own\r\n", &hello()).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("key=agent-own"));
    // With a sanctioned key everything works.
    let env = start(Opts {
        doc: sanctioned,
        ..Opts::default()
    })
    .await;
    assert_eq!(msg(&env, "", &hello()).await.status, 200);
}

#[tokio::test]
async fn model_allow_list_fails_closed() {
    let env = start(Opts {
        doc: json!({"llm": {"endpoints": {"anthropic": {"allowed": true, "allowed_models": ["claude-a", "claude-b"]}}}}),
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &json!({"model": "claude-c"})).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    assert!(r.body.contains("claude-a") && r.body.contains("claude-c"));
    // Unparsable body.
    let r = request(&env, env.llm, "POST", "/v1/messages", "", b"{not json").await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    // Body without a model.
    let r = request(&env, env.llm, "POST", "/v1/messages", "", b"{}").await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    // Truncated body (larger than the condition buffer).
    let big = format!(
        "{{\"pad\":\"{}\",\"model\":\"claude-a\"}}",
        "a".repeat(300 * 1024)
    );
    let r = request(&env, env.llm, "POST", "/v1/messages", "", big.as_bytes()).await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    // Allowed model and model-less calls pass.
    assert_eq!(
        msg(&env, "", &json!({"model": "claude-b"})).await.status,
        200
    );
    assert_eq!(
        request(&env, env.llm, "GET", "/v1/models", "", b"")
            .await
            .status,
        200
    );
    // Instance list intersects with the org list.
    let env = start(Opts {
        instance_models: vec!["claude-a".into()],
        doc: json!({"llm": {"endpoints": {"anthropic": {"allowed": true, "allowed_models": ["claude-a", "claude-b"]}}}}),
        ..Opts::default()
    })
    .await;
    assert!(has(
        &msg(&env, "", &json!({"model": "claude-b"})).await,
        "llm_model_not_allowed"
    ));
    assert_eq!(
        msg(&env, "", &json!({"model": "claude-a"})).await.status,
        200
    );
}

#[tokio::test]
async fn refusals_are_logged_and_malformed_governance_fails_closed() {
    let env = start(Opts {
        secrets: vec![],
        doc: json!({"llm": {"posture": "sanctioned_only", "endpoints": {"anthropic": {"allowed": true}}}}),
        ..Opts::default()
    })
    .await;
    msg(&env, "", &hello()).await;
    env.log.flush().await;
    let rows = env.captured.0.lock().unwrap().clone();
    assert!(rows.iter().any(|r| r.decision == "governance_refused"
        && r.error_code.as_deref() == Some("llm_key_required")));
    let env = start(Opts {
        doc: json!({"llm": {"posture": 7}}),
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 503);
    assert!(has(&r, "policy_unavailable"), "{}", r.body);
}

#[tokio::test]
async fn effective_governance_is_cached_until_invalidated() {
    let env = start(Opts::default()).await;
    assert_eq!(msg(&env, "", &hello()).await.status, 200);
    *env.gsrc.doc.lock().unwrap() =
        json!({"llm": {"endpoints": {"anthropic": {"allowed": false}}}});
    assert_eq!(msg(&env, "", &hello()).await.status, 200, "cached");
    let ev = Invalidate {
        scope: Scope::Org(Uuid::nil()),
    };
    env.gov.invalidate(&Invalidate { scope: Scope::All });
    env.resolver.cache().invalidate(&ev);
    env.resolver.cache().flush_all();
    let r = msg(&env, "", &hello()).await;
    assert!(has(&r, "llm_provider_not_permitted"), "{}", r.body);
}

// ---- egress posture ----

#[tokio::test]
async fn egress_posture_blocks_uncredentialed_with_allow_list() {
    let blocked = |r: &Reply| r.status == 403 && has(r, "blocked_by_egress_policy");
    // Open: allowed.
    let env = start(Opts::default()).await;
    assert_eq!(
        request(&env, env.plain, "GET", "/x", "", b"").await.status,
        200
    );
    // credentialed_only: blocked, but credentialed LLM traffic is unaffected.
    let mut o = Opts::default();
    o.doc["egress"] = json!({"posture": "credentialed_only"});
    let env = start(o).await;
    let r = request(&env, env.plain, "GET", "/x", "", b"").await;
    assert!(blocked(&r), "{} {}", r.status, r.body);
    assert_eq!(msg(&env, "", &hello()).await.status, 200);
    // Org allow-list entry.
    let mut o = Opts::default();
    o.doc["egress"] = json!({"posture": "credentialed_only", "allowlist": [{"host": "127.0.0.1"}]});
    let env = start(o).await;
    assert_eq!(
        request(&env, env.plain, "GET", "/x", "", b"").await.status,
        200
    );
    // Instance minimum forces the posture on an org that says open.
    let env = start(Opts {
        egress_min: aurora_config::EgressPosture::CredentialedOnly,
        ..Opts::default()
    })
    .await;
    assert!(blocked(
        &request(&env, env.plain, "GET", "/x", "", b"").await
    ));
}

#[tokio::test]
async fn open_lane_under_both_postures() {
    // Egress: open reaches anything, credentialed_only only allow-listed hosts.
    let env = start(Opts::default()).await;
    assert_eq!(connect(&env, env.plain).await, 200);
    let mut o = Opts::default();
    o.doc["egress"] = json!({"posture": "credentialed_only"});
    let env = start(o).await;
    assert_eq!(connect(&env, env.plain).await, 403);
    let mut o = Opts::default();
    o.doc["egress"] = json!({"posture": "credentialed_only", "allowlist": [{"host": "127.0.0.1"}]});
    let env = start(o).await;
    assert_eq!(connect(&env, env.plain).await, 200);
    // LLM: governed hosts are refused under sanctioned_only (nothing can be injected).
    let env = start(Opts::default()).await;
    assert_eq!(connect(&env, env.llm).await, 200);
    let env = start(Opts {
        doc: json!({"llm": {"posture": "sanctioned_only", "endpoints": {"anthropic": {"allowed": true}}}}),
        ..Opts::default()
    })
    .await;
    assert_eq!(connect(&env, env.llm).await, 403);
    assert_eq!(connect(&env, env.plain).await, 200);
}

// ---- origin preflight and backstop ----

#[tokio::test]
async fn preflight_refuses_foreign_key_and_writes_back() {
    // Stored origin says verified, but the value is actually another account's key.
    let env = start(Opts {
        secrets: vec![("sk-foreign", true, Some(verified("org_ok")))],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_wrong_organization"), "{}", r.body);
    assert_eq!(
        env.counters.messages.load(Ordering::SeqCst),
        0,
        "never sent"
    );
    let w = wait_writes(&env, 1).await;
    assert_eq!(w[0].0, env.secret_ids[0]);
    assert_eq!(w[0].2, "mismatch");
    assert_eq!(w[0].1["status"], "mismatch");
    assert_eq!(w[0].1["organization_id"], "org_bad");
}

#[tokio::test]
async fn reference_secret_value_change_is_caught() {
    let env = start(Opts::default()).await;
    assert_eq!(msg(&env, "", &hello()).await.status, 200);
    // The external value changes to a foreign key outside Aurora.
    env.mem.preload("k0", None, b"sk-foreign");
    tokio::time::sleep(Duration::from_millis(10)).await; // plaintext cache TTL
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_wrong_organization"), "{}", r.body);
    assert_eq!(env.counters.messages.load(Ordering::SeqCst), 1);
    wait_writes(&env, 1).await;
}

#[tokio::test]
async fn preflight_fails_closed_when_provider_unreachable() {
    let env = start(Opts {
        secrets: vec![("sk-down", true, Some(verified("org_ok")))],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 503);
    assert!(has(&r, "llm_origin_check_unavailable"), "{}", r.body);
    assert!(r.body.contains("anthropic"));
    assert_eq!(env.counters.messages.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn backstop_replaces_response_from_foreign_account() {
    // Passes preflight (the models call reports a good account) but the response reports a
    // foreign one.
    let env = start(Opts {
        secrets: vec![("sk-sneaky", true, Some(verified("org_ok")))],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_wrong_organization"), "{}", r.body);
    assert_eq!(env.counters.messages.load(Ordering::SeqCst), 1);
    let w = wait_writes(&env, 1).await;
    assert_eq!(w[0].2, "mismatch");
}

#[tokio::test]
async fn backstop_counts_missing_headers_without_refusing() {
    let env = start(Opts {
        secrets: vec![("sk-nohdr", true, Some(verified("org_ok")))],
        ..Opts::default()
    })
    .await;
    assert_eq!(env.gov.header_missing_total(), 0);
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 200);
    assert_eq!(env.gov.header_missing_total(), 1);
}

/// (P4-LLM-1, D-260(c), D-268(d), SEC-65) The Anthropic key is injected on the inference routes only (on by
/// default): Files, Batches and the other APIs are refused before policy and before any credential is opened;
/// `allowed_paths` adds what the operator lists.
#[tokio::test]
async fn anthropic_key_is_injected_only_on_inference_paths() {
    let env = start(Opts {
        secrets: vec![("sk-good", true, Some(verified("org_ok")))],
        anthropic_paths: vec!["/v1/files".into()],
        ..Opts::default()
    })
    .await;
    for (method, path) in [
        ("POST", "/v1/messages"),
        ("POST", "/v1/messages/count_tokens"),
        ("GET", "/v1/models"),
        ("GET", "/v1/files"),
    ] {
        let r = request(&env, env.llm, method, path, "", b"{\"model\":\"m\"}").await;
        assert_eq!(r.status, 200, "{method} {path}: {}", r.body);
    }
    env.mem.set_unavailable(true);
    let other_before = env.counters.other.load(Ordering::SeqCst);
    for (method, path) in [
        ("GET", "/v1/files/file_1"),
        ("POST", "/v1/messages/batches"),
        ("GET", "/v1/messages/batches/b1"),
        ("POST", "/v1/skills"),
        ("GET", "/v1/organizations/me"),
        ("GET", "/v1/messages"),
        // (D-367) `GET /model/info` is a `litellm` default only: another kind refuses it.
        ("GET", "/model/info"),
    ] {
        let r = request(&env, env.llm, method, path, "", b"{}").await;
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
        assert!(
            has(&r, "llm_path_not_permitted"),
            "{method} {path}: {}",
            r.body
        );
        assert!(!r.body.contains("sk-good"), "{}", r.body);
    }
    assert_eq!(
        env.counters.other.load(Ordering::SeqCst),
        other_before,
        "nothing reached the endpoint"
    );
}

/// (D-177, SEC-65, FIX-105) A sanctioned LiteLLM key is injected on the inference routes only:
/// the management API and the other OpenAI routes are refused before policy and before any
/// credential is opened; an operator-listed path is admitted.
#[tokio::test]
async fn litellm_key_is_injected_only_on_inference_paths() {
    let env = start(Opts {
        doc: json!({"llm": {"endpoints": {"central": {"allowed": true}}}}),
        litellm: Some(vec!["/health/*".into(), "/model/group/info".into()]),
        secrets: vec![("sk-good", true, None)],
        ..Opts::default()
    })
    .await;
    // The six inference routes and the model list pass, with the key injected.
    for (method, path) in [
        ("POST", "/v1/chat/completions"),
        ("POST", "/v1/completions"),
        ("POST", "/v1/responses"),
        ("POST", "/v1/embeddings"),
        ("POST", "/v1/messages"),
        ("POST", "/v1/messages/count_tokens"),
        ("POST", "/openai/v1/chat/completions"),
        ("GET", "/v1/models"),
        ("GET", "/v1/models/gpt-x"),
        ("GET", "/health/liveliness"),
        ("GET", "/model/group/info"),
    ] {
        let r = request(&env, env.llm, method, path, "", b"{\"model\":\"m\"}").await;
        assert_eq!(r.status, 200, "{method} {path}: {}", r.body);
        // (`/v1/models` answers the model list, not the key.)
        assert!(
            path == "/v1/models" || r.body.contains("key=sk-good"),
            "{method} {path}: {}",
            r.body
        );
    }
    // The store goes down: a refused path must not even try to open the credential.
    env.mem.set_unavailable(true);
    let other_before = env.counters.other.load(Ordering::SeqCst);
    for (method, path) in [
        ("POST", "/key/generate"),
        ("POST", "/user/new"),
        ("GET", "/team/list"),
        ("POST", "/v1/images/generations"),
        ("GET", "/v1/files"),
        ("POST", "/v1/fine_tuning/jobs"),
        ("GET", "/v1/chat/completions"),
        ("GET", "/model/group/info/extra"),
    ] {
        let r = request(&env, env.llm, method, path, "", b"{}").await;
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
        assert!(
            has(&r, "llm_path_not_permitted"),
            "{method} {path}: {}",
            r.body
        );
        assert!(!r.body.contains("sk-good"), "{}", r.body);
    }
    assert_eq!(
        env.counters.other.load(Ordering::SeqCst),
        other_before,
        "nothing reached the endpoint"
    );
}

/// (D-367, SEC-65; goose's `litellm` provider asks `GET /model/info` at every start) With **no**
/// `allowed_paths`, a LiteLLM endpoint forwards `GET /model/info` with the key injected like an
/// inference path; `POST` to it, any other metadata or management route, and a path below it stay
/// refused before policy and before the credential is opened.
#[tokio::test]
async fn litellm_endpoint_forwards_get_model_info_by_default_and_nothing_else_of_its_metadata() {
    let env = start(Opts {
        doc: json!({"llm": {"endpoints": {"central": {"allowed": true}}}}),
        litellm: Some(vec![]),
        secrets: vec![("sk-good", true, None)],
        ..Opts::default()
    })
    .await;
    let r = request(&env, env.llm, "GET", "/model/info", "", b"").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("key=sk-good"),
        "the key is injected: {}",
        r.body
    );
    // The store goes down: a refused path must not even try to open the credential.
    env.mem.set_unavailable(true);
    let other_before = env.counters.other.load(Ordering::SeqCst);
    for (method, path) in [
        ("POST", "/model/info"),
        ("PUT", "/model/info"),
        ("GET", "/model/group/info"),
        ("GET", "/model/info/extra"),
        ("POST", "/model/new"),
        ("POST", "/model/delete"),
        ("GET", "/key/info"),
        ("GET", "/v1/model/info"),
    ] {
        let r = request(&env, env.llm, method, path, "", b"{}").await;
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
        assert!(
            has(&r, "llm_path_not_permitted"),
            "{method} {path}: {}",
            r.body
        );
        assert!(!r.body.contains("sk-good"), "{}", r.body);
    }
    assert_eq!(
        env.counters.other.load(Ordering::SeqCst),
        other_before,
        "nothing reached the endpoint"
    );
}

fn header_of<'a>(
    seen: &'a [(String, Vec<(String, String)>)],
    path: &str,
    name: &str,
) -> Vec<&'a str> {
    seen.iter()
        .filter(|(p, _)| p == path)
        .flat_map(|(_, h)| h.iter())
        .filter(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
        .collect()
}

/// (P4-LLM-1, C-806, C-809, C-808, acceptance 3) `openai` through the whole pipeline: the agent's own credential and
/// `OpenAI-*` headers are replaced by the key and the configured organization and project; an SSE answer passes byte
/// for byte; non-inference paths are refused before the value is opened; the model allow-list fails closed;
/// an upstream 401/429/500 reaches the client unchanged and is classified in the request log by status alone.
#[tokio::test]
async fn openai_key_pipeline_headers_paths_models_sse_and_classification() {
    let env = start(Opts {
        openai: true,
        doc: json!({"llm": {"posture": "open", "endpoints": {"openai": {"allowed": true}}}}),
        secrets: vec![("sk-good", true, None)],
        instance_models: vec!["gpt-a".into()],
        ..Opts::default()
    })
    .await;
    let body = json!({"model": "gpt-a"}).to_string();
    let r = request(
        &env,
        env.llm,
        "POST",
        "/v1/chat/completions",
        "Authorization: Bearer agent-own\r\nx-api-key: agent-own\r\napi-key: agent-own\r\nOpenAI-Organization: org-agent\r\nOpenAI-Project: proj-agent\r\nOpenAI-Beta: assistants=v2\r\n",
        body.as_bytes(),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    {
        let seen = env.counters.seen.lock().unwrap();
        let p = "/v1/chat/completions";
        assert_eq!(header_of(&seen, p, "authorization"), ["Bearer sk-good"]);
        assert!(header_of(&seen, p, "x-api-key").is_empty());
        assert!(header_of(&seen, p, "api-key").is_empty());
        assert_eq!(header_of(&seen, p, "openai-organization"), ["org-cfg"]);
        assert_eq!(header_of(&seen, p, "openai-project"), ["proj-cfg"]);
        assert!(
            !format!("{seen:?}").contains("agent-own") && !format!("{seen:?}").contains("agent"),
            "no agent credential reaches the upstream: {seen:?}"
        );
    }
    // SSE passthrough, byte for byte.
    let r = request(
        &env,
        env.llm,
        "POST",
        "/v1/responses",
        "",
        json!({"model": "gpt-a", "stream": true})
            .to_string()
            .as_bytes(),
    )
    .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, SSE);
    // The model allow-list fails closed: another model, an unparsable and a truncated body.
    let r = request(
        &env,
        env.llm,
        "POST",
        "/v1/chat/completions",
        "",
        b"{\"model\":\"gpt-z\"}",
    )
    .await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    let big = format!(
        "{{\"pad\":\"{}\",\"model\":\"gpt-a\"}}",
        "a".repeat(300 * 1024)
    );
    let r = request(
        &env,
        env.llm,
        "POST",
        "/v1/chat/completions",
        "",
        big.as_bytes(),
    )
    .await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    // Files, fine-tuning, images, assistants and the rest: refused before policy and before the value is opened.
    env.mem.set_unavailable(true);
    let other_before = env.counters.other.load(Ordering::SeqCst);
    for (method, path) in [
        ("GET", "/v1/files"),
        ("POST", "/v1/files"),
        ("POST", "/v1/fine_tuning/jobs"),
        ("POST", "/v1/images/generations"),
        ("GET", "/v1/assistants"),
        ("GET", "/v1/organization/admin_api_keys"),
        ("GET", "/v1/chat/completions"),
        // (D-367) `GET /model/info` is a `litellm` default only: another kind refuses it.
        ("GET", "/model/info"),
    ] {
        let r = request(&env, env.llm, method, path, "", b"{}").await;
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
        assert!(
            has(&r, "llm_path_not_permitted"),
            "{method} {path}: {}",
            r.body
        );
        assert!(!r.body.contains("sk-good"), "{}", r.body);
    }
    assert_eq!(env.counters.other.load(Ordering::SeqCst), other_before);
    env.mem.set_unavailable(false);
    // Classification (C-808): the status and body are the upstream's; the log row carries the class.
    for (key, status, code) in [
        ("sk-e401", 401u16, "llm_auth"),
        ("sk-e429", 429, "llm_rate_limited"),
        ("sk-e500", 500, "llm_overloaded"),
    ] {
        let env = start(Opts {
            openai: true,
            doc: json!({"llm": {"posture": "open", "endpoints": {"openai": {"allowed": true}}}}),
            secrets: vec![(key, true, None)],
            ..Opts::default()
        })
        .await;
        let r = request(
            &env,
            env.llm,
            "POST",
            "/v1/chat/completions",
            "",
            b"{\"model\":\"m\"}",
        )
        .await;
        assert_eq!(r.status, status, "{key}: {}", r.body);
        assert!(
            r.body.contains("insufficient_quota") && r.body.contains("upstream"),
            "{}",
            r.body
        );
        env.log.flush().await;
        let rows = env.captured.0.lock().unwrap().clone();
        assert!(
            rows.iter()
                .any(|row| row.error_code.as_deref() == Some(code)
                    && row.upstream_status == Some(i32::from(status))),
            "{key}: {rows:?}"
        );
    }
}

const VERTEX_DOC: &str =
    r#"{"llm": {"posture": "open", "endpoints": {"vertex": {"allowed": true}}}}"#;
const VERTEX_PATH: &str =
    "/v1/projects/proj-1/locations/us-east5/publishers/anthropic/models/claude-x";

async fn vertex_env(models: &[&str]) -> Env {
    start(Opts {
        vertex: true,
        doc: serde_json::from_str(VERTEX_DOC).unwrap(),
        secrets: vec![("unused", true, None)],
        instance_models: models.iter().map(|m| (*m).to_owned()).collect(),
        ..Opts::default()
    })
    .await
}

/// (P4-LLM-2, C-806, C-805, SEC-97, acceptance 3) `vertex` through the whole pipeline: the gateway mints a token from
/// the stored key file and injects it as Bearer, the agent's own credentials and `x-goog-*` are removed, SSE passes
/// byte for byte, other projects, locations and non-prediction routes are refused before policy and before the key
/// is opened (so nothing is minted), the model allow-list reads the path model.
#[tokio::test]
async fn vertex_key_is_minted_injected_and_confined_to_its_project_and_location() {
    let env = vertex_env(&["claude-x"]).await;
    let rig = env.vertex.as_ref().unwrap();
    let raw = format!("{VERTEX_PATH}:rawPredict");
    let r = request(
        &env,
        env.llm,
        "POST",
        &raw,
        "Authorization: Bearer agent-own\r\nx-api-key: agent-own\r\napi-key: agent-own\r\nx-goog-api-key: agent-own\r\nx-goog-user-project: agent-proj\r\n",
        b"{\"anthropic_version\":\"vertex-2023-10-16\"}",
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    {
        let seen = rig.upstream.requests();
        let one = &seen[0];
        assert_eq!(
            one.header("authorization"),
            Some("Bearer ya29.mock-token-1")
        );
        assert!(
            one.headers.iter().all(|(k, v)| !k.starts_with("x-goog-")
                && k != "x-api-key"
                && k != "api-key"
                && !v.contains("agent")),
            "{one:?}"
        );
    }
    assert_eq!(rig.google.requests(), 1);
    assert!(rig.google.refused().is_empty());
    // SSE passthrough, and the token is reused.
    let r = request(
        &env,
        env.llm,
        "POST",
        &format!("{VERTEX_PATH}:streamRawPredict"),
        "",
        b"{}",
    )
    .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, aurora_testkit::llm::vertex::SSE);
    assert_eq!(rig.google.requests(), 1);
    // The model allow-list reads the model from the path.
    let r = request(
        &env,
        env.llm,
        "POST",
        &raw.replace("claude-x", "claude-z"),
        "",
        b"{}",
    )
    .await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    // Refused before policy and before the value is opened or minted.
    env.mem.set_unavailable(true);
    let (up_before, mints_before) = (rig.upstream.requests().len(), rig.google.requests());
    for (method, path) in [
        ("POST", raw.replace("proj-1", "other-proj")),
        ("POST", raw.replace("us-east5", "europe-west4")),
        ("GET", raw.clone()),
        (
            "GET",
            "/v1/projects/proj-1/locations/us-east5/publishers/anthropic/models".to_owned(),
        ),
        (
            "POST",
            "/v1/projects/proj-1/locations/us-east5/endpoints/9:predict".to_owned(),
        ),
        (
            "POST",
            "/v1/projects/proj-1/locations/us-east5/endpoints/openapi/chat/completions".to_owned(),
        ),
        ("GET", "/v1/projects/proj-1".to_owned()),
    ] {
        let r = request(&env, env.llm, method, &path, "", b"{}").await;
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
        assert!(
            has(&r, "llm_path_not_permitted"),
            "{method} {path}: {}",
            r.body
        );
    }
    assert_eq!(rig.upstream.requests().len(), up_before);
    assert_eq!(rig.google.requests(), mints_before);
}

/// (C-807, SEC-97) A failed mint forwards nothing: 502 `credential_unavailable` `{ kind: "llm_mint", reason }`, request-log
/// `llm_auth` for a rejected key and `credential_unavailable` for an unavailable endpoint; neither the key nor a token
/// is in the answer or the log.
#[tokio::test]
async fn vertex_mint_failures_answer_502_and_forward_nothing() {
    use aurora_testkit::llm::google_token::Behaviour;
    for (b, reason, code) in [
        (Behaviour::InvalidGrant, "mint_rejected", "llm_auth"),
        (
            Behaviour::Unavailable,
            "mint_unavailable",
            "credential_unavailable",
        ),
    ] {
        let env = vertex_env(&[]).await;
        let rig = env.vertex.as_ref().unwrap();
        rig.google.set_behaviour(b);
        let body = r#"{"anthropic_version":"vertex-2023-10-16"}"#;
        let path = format!("{VERTEX_PATH}:rawPredict");
        let r = request(&env, env.llm, "POST", &path, "", body.as_bytes()).await;
        assert_eq!(r.status, 502, "{}", r.body);
        assert!(has(&r, "credential_unavailable"), "{}", r.body);
        assert!(
            r.body.contains("llm_mint") && r.body.contains(reason),
            "{}",
            r.body
        );
        assert!(!r.body.contains("BEGIN") && !r.body.contains("ya29"));
        assert!(rig.upstream.requests().is_empty(), "nothing is forwarded");
        // Held: a burst makes one call to the token endpoint.
        for _ in 0..5 {
            request(&env, env.llm, "POST", &path, "", body.as_bytes()).await;
        }
        assert_eq!(rig.google.requests(), 1);
        env.log.flush().await;
        let rows = env.captured.0.lock().unwrap().clone();
        assert!(
            rows.iter()
                .any(|row| row.error_code.as_deref() == Some(code) && row.status == 502),
            "{code}: {rows:?}"
        );
        let dump = format!("{rows:?}");
        assert!(!dump.contains("BEGIN") && !dump.contains("ya29"), "{dump}");
    }
}

const AZURE_DOC: &str =
    r#"{"llm": {"posture": "open", "endpoints": {"azure": {"allowed": true}}}}"#;

async fn azure_env(entra: bool, models: &[&str]) -> Env {
    start(Opts {
        azure: Some(entra),
        doc: serde_json::from_str(AZURE_DOC).unwrap(),
        secrets: vec![("unused", true, None)],
        instance_models: models.iter().map(|m| (*m).to_owned()).collect(),
        ..Opts::default()
    })
    .await
}

const AGENT_AUTH: &str =
    "Authorization: Bearer agent-own\r\nx-api-key: agent-own\r\napi-key: agent-own\r\n";

/// (P4-LLM-3, C-806, C-805, SEC-65, acceptance 2) An `azure_openai` resource key: the gateway sets `api-key` and removes
/// the agent's `Authorization`, `x-api-key` and `api-key`; SSE passes byte for byte; the deployment-name allow-list
/// reads body `/model`; the classic `api-version` routes and the v1 management routes are refused before policy and
/// before the key is opened; the authority is never called.
#[tokio::test]
async fn azure_key_is_injected_as_api_key_and_confined_to_the_v1_inference_routes() {
    let env = azure_env(false, &["gpt-4o"]).await;
    let rig = env.azure.as_ref().unwrap();
    let r = request(
        &env,
        env.llm,
        "POST",
        "/openai/v1/chat/completions",
        AGENT_AUTH,
        br#"{"model":"gpt-4o","messages":[]}"#,
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    {
        let seen = rig.upstream.requests();
        let one = &seen[0];
        assert_eq!(one.header("api-key"), Some(AZURE_KEY), "{one:?}");
        assert!(
            one.header("authorization").is_none()
                && one.header("x-api-key").is_none()
                && one.headers.iter().all(|(_, v)| !v.contains("agent-own")),
            "{one:?}"
        );
    }
    let r = request(
        &env,
        env.llm,
        "POST",
        "/openai/v1/responses",
        "",
        br#"{"model":"gpt-4o","stream":true}"#,
    )
    .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, aurora_testkit::llm::azure::SSE);
    // The allow-list holds deployment names.
    let r = request(
        &env,
        env.llm,
        "POST",
        "/openai/v1/chat/completions",
        "",
        br#"{"model":"gpt-4o-mini"}"#,
    )
    .await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    // Refused before policy and before the value is opened.
    env.mem.set_unavailable(true);
    let before = rig.upstream.requests().len();
    for (method, path) in [
        (
            "POST",
            "/openai/deployments/gpt-4o/chat/completions?api-version=2024-10-21",
        ),
        ("POST", "/openai/v1/files"),
        ("GET", "/openai/v1/files"),
        ("POST", "/openai/v1/fine_tuning/jobs"),
        ("DELETE", "/openai/v1/files/file_1"),
        ("POST", "/openai/v1/images/generations"),
        ("GET", "/openai/models?api-version=2024-10-21"),
        ("GET", "/v1/models"),
    ] {
        let r = request(&env, env.llm, method, path, "", b"{}").await;
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
        assert!(
            has(&r, "llm_path_not_permitted"),
            "{method} {path}: {}",
            r.body
        );
    }
    assert_eq!(rig.upstream.requests().len(), before);
    assert_eq!(rig.entra.requests(), 0, "a key is never minted");
}

/// (P4-LLM-3, C-806, C-807, SEC-97, acceptance 2) An Entra application: the gateway mints one token (single flight
/// across concurrent requests, reused afterwards) and injects it as Bearer; the agent's `api-key` and `Authorization`
/// are removed; no client secret reaches the upstream.
#[tokio::test]
async fn azure_entra_token_is_minted_once_and_injected_as_bearer() {
    let env = azure_env(true, &[]).await;
    let rig = env.azure.as_ref().unwrap();
    let go = || {
        request(
            &env,
            env.llm,
            "POST",
            "/openai/v1/chat/completions",
            AGENT_AUTH,
            br#"{"model":"gpt-4o"}"#,
        )
    };
    let replies = futures::future::join_all((0..12).map(|_| go())).await;
    assert!(replies.iter().all(|r| r.status == 200), "{replies:?}");
    assert_eq!(rig.entra.requests(), 1, "single flight");
    assert!(rig.entra.refused().is_empty(), "{:?}", rig.entra.refused());
    let token = &rig.entra.issued()[0];
    let seen = rig.upstream.requests();
    assert_eq!(seen.len(), 12);
    for one in &seen {
        assert_eq!(
            one.header("authorization"),
            Some(format!("Bearer {token}").as_str()),
            "{one:?}"
        );
        assert!(
            one.header("api-key").is_none()
                && one.header("x-api-key").is_none()
                && one.headers.iter().all(|(_, v)| {
                    !v.contains("agent-own") && !v.contains(&rig.app.client_secret)
                }),
            "{one:?}"
        );
    }
}

/// (C-807, SEC-97) A refused Entra application: 502 `credential_unavailable` `llm_mint` `mint_rejected`, request-log
/// `llm_auth`, nothing forwarded; the client secret and the token are in neither the answer nor the log.
#[tokio::test]
async fn azure_entra_mint_failures_answer_502_and_forward_nothing() {
    use aurora_testkit::llm::entra::Behaviour;
    for (b, reason, code) in [
        (Behaviour::InvalidClient, "mint_rejected", "llm_auth"),
        (
            Behaviour::Unavailable,
            "mint_unavailable",
            "credential_unavailable",
        ),
    ] {
        let env = azure_env(true, &[]).await;
        let rig = env.azure.as_ref().unwrap();
        rig.entra.set_behaviour(b);
        let body = br#"{"model":"gpt-4o"}"#;
        let path = "/openai/v1/chat/completions";
        let r = request(&env, env.llm, "POST", path, "", body).await;
        assert_eq!(r.status, 502, "{}", r.body);
        assert!(
            has(&r, "credential_unavailable")
                && r.body.contains("llm_mint")
                && r.body.contains(reason),
            "{}",
            r.body
        );
        assert!(rig.upstream.requests().is_empty(), "nothing is forwarded");
        for _ in 0..5 {
            request(&env, env.llm, "POST", path, "", body).await;
        }
        assert_eq!(rig.entra.requests(), 1, "held");
        env.log.flush().await;
        let rows = env.captured.0.lock().unwrap().clone();
        assert!(
            rows.iter()
                .any(|row| row.error_code.as_deref() == Some(code) && row.status == 502),
            "{code}: {rows:?}"
        );
        let dump = format!("{rows:?}{}", r.body);
        assert!(!dump.contains(&rig.app.client_secret), "{dump}");
    }
}

/// (P5-MODELS-2, D-304, C-880) The `llm_model` of the request-log row written for the next request: waits for
/// the row (a deadline, no sleeping for its own sake).
async fn next_llm_model(env: &Env, before: usize) -> (usize, Option<String>) {
    for _ in 0..200 {
        env.log.flush().await;
        {
            let rows = env.captured.0.lock().unwrap();
            if rows.len() > before {
                return (rows.len(), rows.last().unwrap().llm_model.clone());
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no request-log row was written");
}

fn logged(env: &Env) -> usize {
    env.captured.0.lock().unwrap().len()
}

/// The request log records the model each LLM request asked for, with and without an allow-list; a request whose
/// model cannot be told is logged with null and is not refused when no allow-list applies.
#[tokio::test]
async fn request_log_records_the_requested_llm_model_with_and_without_an_allow_list() {
    // No allow-list: nothing is refused, the model is still recorded.
    let env = start(Opts::default()).await;
    let r = msg(&env, "", &json!({"model": "claude-x", "messages": []})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (n, m) = next_llm_model(&env, 0).await;
    assert_eq!(m.as_deref(), Some("claude-x"));
    // A body without a model, an unparsable one and a truncated one: null, and no refusal.
    for body in [b"{}".to_vec(), b"{not json".to_vec()] {
        let before = logged(&env);
        let r = request(&env, env.llm, "POST", "/v1/messages", "", &body).await;
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(next_llm_model(&env, before).await.1, None);
    }
    let before = logged(&env);
    let big = format!(
        "{{\"pad\":\"{}\",\"model\":\"claude-a\"}}",
        "a".repeat(300 * 1024)
    );
    let r = request(&env, env.llm, "POST", "/v1/messages", "", big.as_bytes()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(
        next_llm_model(&env, before).await.1,
        None,
        "truncated: unknown"
    );
    assert!(n >= 1);

    // With an allow-list: the allowed model is recorded, and so is the refused one (audit).
    let env = start(Opts {
        doc: json!({"llm": {"endpoints": {"anthropic": {"allowed": true, "allowed_models": ["claude-a", "claude-b"]}}}}),
        ..Opts::default()
    })
    .await;
    assert_eq!(
        msg(&env, "", &json!({"model": "claude-b"})).await.status,
        200
    );
    let (n, m) = next_llm_model(&env, 0).await;
    assert_eq!(m.as_deref(), Some("claude-b"));
    let r = msg(&env, "", &json!({"model": "claude-c"})).await;
    assert_eq!(r.status, 403);
    assert_eq!(next_llm_model(&env, n).await.1.as_deref(), Some("claude-c"));
}

/// The model of a Vertex request is in its path.
#[tokio::test]
async fn request_log_records_the_llm_model_of_a_vertex_path() {
    for models in [vec!["claude-x"], vec![]] {
        let env = vertex_env(&models).await;
        let r = request(
            &env,
            env.llm,
            "POST",
            &format!("{VERTEX_PATH}:rawPredict"),
            "",
            b"{}",
        )
        .await;
        assert_eq!(r.status, 200, "{models:?}: {}", r.body);
        assert_eq!(
            next_llm_model(&env, 0).await.1.as_deref(),
            Some("claude-x"),
            "allow-list {models:?}"
        );
    }
}

// (P4-LLM-4) Bedrock signing tests share this file's harness.
#[path = "llm_bedrock.rs"]
mod llm_bedrock;
