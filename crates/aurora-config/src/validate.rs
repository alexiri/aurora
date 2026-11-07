//! Per-service validation. Every check appends to one list so a single run
//! reports all problems, each naming its key.

use crate::error::{ConfigError, ConfigErrors, Result};
use crate::raw::*;
use crate::redacted::Redacted;
use crate::urlcheck::{host_of, is_bind_addr, is_cidr, parse_url};
use aurora_crypto::{RingKind, SecretRing};
use base64::Engine;
use base64::alphabet::STANDARD as B64_ALPHABET;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use std::collections::{BTreeMap, BTreeSet};

/// Port of the gateway's proxy listener, used to derive `advertised_proxy_url`.
const GATEWAY_PROXY_PORT: u16 = 4102;

pub const BUILTIN_KNOWN_LLM_HOSTS: &[&str] = &[
    "api.openai.com",
    "chatgpt.com",
    "generativelanguage.googleapis.com",
    "*-aiplatform.googleapis.com",
    "*.openai.azure.com",
    "*.services.ai.azure.com",
    "bedrock-runtime.*.amazonaws.com",
    "api.mistral.ai",
    "api.cohere.com",
    "api.cohere.ai",
    "api.groq.com",
    "api.together.xyz",
    "api.fireworks.ai",
    "api.deepseek.com",
    "api.x.ai",
    "api.perplexity.ai",
    "openrouter.ai",
    "api.anthropic.com",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Api,
    Gateway,
    Hookhost,
    Runner,
    Connector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignupMode {
    Open,
    InviteOnly,
    SsoJit,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgCreation {
    InstanceAdmins,
    Anyone,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmPosture {
    Open,
    SanctionedOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressPosture {
    Open,
    CredentialedOnly,
}
/// Open-lane posture, ordered by strictness (RFE-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OpenLanePosture {
    Enabled,
    AllowlistOnly,
    Disabled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerBackend {
    Podman,
    Kubernetes,
    Fake,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanReap {
    Reap,
    LogOnly,
}

#[derive(Debug, Clone)]
pub struct KeyringEntry {
    pub id: String,
    pub key: Redacted<[u8; 32]>,
}

#[derive(Debug, Clone)]
pub struct Keyring {
    pub keys: Vec<KeyringEntry>,
    pub active: String,
}

/// Governance with enums parsed and `known_llm_hosts` resolved.
#[derive(Debug, Clone)]
pub struct Governance {
    pub allowed_key_scopes: Vec<String>,
    pub llm_minimum_posture: LlmPosture,
    pub require_key_origin: bool,
    pub known_llm_hosts: Vec<String>,
    pub origin_recheck_secs: u64,
    pub egress_minimum_posture: EgressPosture,
    pub baseline_allowlist: Vec<EgressEntry>,
    pub open_lane_minimum: OpenLanePosture,
}

#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub server: ServerSection,
    pub external_url: String,
    pub database_url: Redacted<String>,
    pub max_connections: u32,
    pub gateway_role: Option<String>,
    pub keyring: Keyring,
    /// Signing-key ring (RFE-007): producers use the active entry, receivers accept any.
    pub signing_ring: SecretRing,
    pub internal_ring: SecretRing,
    pub runner_session_key: Redacted<[u8; 32]>,
    pub bootstrap_runner_token: Option<Redacted<String>>,
    /// `(connector id, token)` pairs.
    pub bootstrap_connector_tokens: Vec<(String, Redacted<String>)>,
    pub auth: AuthSection,
    pub signup_mode: SignupMode,
    pub org_creation: OrgCreation,
    pub oidc_client_secrets: BTreeMap<String, Redacted<String>>,
    pub email: EmailSection,
    pub secrets: SecretsSection,
    pub llm: LlmSection,
    pub governance: Governance,
    pub agents: AgentsSection,
    pub hosted: HostedSection,
    pub advertised_proxy_url: String,
    /// `gateway.ca_dir`: directory holding the gateway's `ca.pem`.
    pub gateway_ca_dir: String,
    /// (phase 1, RFE-002) `true` when `gateway.proxy_tls.cert_file` is set: the proxy listener
    /// presents an operator certificate, so BYO clients verify the outer leg with system roots.
    pub gateway_proxy_operator_cert: bool,
    pub hookhost_url: String,
    pub plugins: PluginsSection,
    pub retention: RetentionSection,
    pub ratelimit: RatelimitSection,
    pub tls: TlsSection,
    pub telemetry: TelemetrySection,
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub server: ServerSection,
    pub external_url: String,
    pub database_url: Redacted<String>,
    pub max_connections: u32,
    pub keyring: Keyring,
    pub internal_ring: SecretRing,
    pub secrets: SecretsSection,
    pub llm: LlmSection,
    pub governance: Governance,
    pub gateway: GatewaySection,
    /// Resolved `gateway.internal_bind` (defaults to `server.bind`).
    pub internal_bind: String,
    pub advertised_proxy_url: String,
    pub approvals: ApprovalsSection,
    pub tls: TlsSection,
    pub telemetry: TelemetrySection,
}

#[derive(Debug, Clone)]
pub struct HookhostConfig {
    pub server: ServerSection,
    pub internal_ring: SecretRing,
    pub hookhost: HookhostSection,
    pub tls: TlsSection,
    pub telemetry: TelemetrySection,
}

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub server: ServerSection,
    /// Resolved `runner.ops_bind` (defaults to `server.bind`).
    pub ops_bind: String,
    pub api_url: String,
    pub backend: RunnerBackend,
    pub token: Redacted<String>,
    pub session_key: Redacted<[u8; 32]>,
    pub orphan_reap: OrphanReap,
    pub held_awake_ceiling: u32,
    pub runner: RunnerSection,
    pub tls: TlsSection,
    pub telemetry: TelemetrySection,
}

#[derive(Debug, Clone)]
pub struct ConnectorConfig {
    pub token: Redacted<String>,
    pub tls: TlsSection,
    pub telemetry: TelemetrySection,
}

#[derive(Debug, Clone)]
pub enum ServiceConfig {
    Api(Box<ApiConfig>),
    Gateway(Box<GatewayConfig>),
    Hookhost(Box<HookhostConfig>),
    Runner(Box<RunnerConfig>),
    Connector(Box<ConnectorConfig>),
}

impl RawConfig {
    /// Validate exactly what `service` needs; all errors are collected.
    pub fn validate_for(&self, service: Service) -> Result<ServiceConfig> {
        Ok(match service {
            Service::Api => ServiceConfig::Api(Box::new(self.api_config()?)),
            Service::Gateway => ServiceConfig::Gateway(Box::new(self.gateway_config()?)),
            Service::Hookhost => ServiceConfig::Hookhost(Box::new(self.hookhost_config()?)),
            Service::Runner => ServiceConfig::Runner(Box::new(self.runner_config()?)),
            Service::Connector => ServiceConfig::Connector(Box::new(self.connector_config()?)),
        })
    }

    /// Validate only the signing-key and internal-secret rings (`aurora-api keys status`).
    pub fn rings(&self) -> Result<(SecretRing, SecretRing)> {
        let mut c = Ctx::default();
        let s = &self.env_secrets;
        let signing = ring(
            &mut c,
            "AURORA_SIGNING_KEY",
            "AURORA_SIGNING_ACTIVE_KEY",
            &s.signing_key,
            &s.signing_active_key,
            RingKind::Signing,
        );
        let internal = ring(
            &mut c,
            "AURORA_INTERNAL_SECRET",
            "AURORA_INTERNAL_ACTIVE_KEY",
            &s.internal_secret,
            &s.internal_active_key,
            RingKind::Internal,
        );
        c.finish()?;
        Ok((signing.unwrap_or_default(), internal.unwrap_or_default()))
    }

    pub fn api_config(&self) -> Result<ApiConfig> {
        let mut c = Ctx::default();
        let s = &self.env_secrets;
        let external_url = check_server(&mut c, &self.server, true);
        let database_url = check_database_url(&mut c, &s.database_url);
        let keyring = check_keyring(&mut c, s);
        let signing_ring = ring(
            &mut c,
            "AURORA_SIGNING_KEY",
            "AURORA_SIGNING_ACTIVE_KEY",
            &s.signing_key,
            &s.signing_active_key,
            RingKind::Signing,
        );
        let internal_ring = ring(
            &mut c,
            "AURORA_INTERNAL_SECRET",
            "AURORA_INTERNAL_ACTIVE_KEY",
            &s.internal_secret,
            &s.internal_active_key,
            RingKind::Internal,
        );
        let runner_session_key = key32(&mut c, "AURORA_RUNNER_SESSION_KEY", &s.runner_session_key);
        if let Some(t) = &s.bootstrap_runner_token {
            token_prefix(&mut c, "AURORA_BOOTSTRAP_RUNNER_TOKEN", t, "aur_");
        }
        let bootstrap_connector_tokens =
            bootstrap_connectors(&mut c, &s.bootstrap_connector_tokens);
        let (signup_mode, org_creation) = check_auth(&mut c, &self.auth, s);
        check_email(&mut c, &self.email);
        check_secret_stores(&mut c, &self.secrets);
        check_llm(&mut c, &self.llm, &self.governance);
        let governance = check_governance(&mut c, self);
        check_hosted(&mut c, &self.hosted);
        check_pos(
            &mut c,
            "database.max_connections",
            self.database.max_connections,
        );
        check_proxy_url(
            &mut c,
            "gateway.advertised_proxy_url",
            self.gateway.advertised_proxy_url.as_deref(),
        );
        check_url(&mut c, "api.hookhost_url", Some(&self.api.hookhost_url));
        if self.plugins.dir.is_empty() {
            c.err("plugins.dir", "must not be empty");
        }
        check_pos(
            &mut c,
            "retention.request_log_days",
            Some(self.retention.request_log_days),
        );
        check_ratelimit(&mut c, &self.ratelimit.api);
        check_common(&mut c, &self.tls, &self.telemetry);
        let advertised_proxy_url = advertised_proxy_url(&self.gateway, external_url.as_deref());
        c.finish()?;
        Ok(ApiConfig {
            server: self.server.clone(),
            external_url: external_url.unwrap_or_default(),
            database_url: database_url.unwrap_or_default(),
            max_connections: self.database.max_connections.unwrap_or(20),
            gateway_role: self.database.gateway_role.clone(),
            keyring: keyring.unwrap_or_else(empty_keyring),
            signing_ring: signing_ring.unwrap_or_default(),
            internal_ring: internal_ring.unwrap_or_default(),
            runner_session_key: runner_session_key.unwrap_or_default(),
            bootstrap_runner_token: s.bootstrap_runner_token.clone(),
            bootstrap_connector_tokens,
            auth: self.auth.clone(),
            signup_mode: signup_mode.unwrap_or(SignupMode::InviteOnly),
            org_creation: org_creation.unwrap_or(OrgCreation::InstanceAdmins),
            oidc_client_secrets: s.oidc_client_secrets.clone(),
            email: self.email.clone(),
            secrets: self.secrets.clone(),
            llm: self.llm.clone(),
            governance: governance.unwrap_or_else(|| resolve_governance_lossy(self)),
            agents: self.agents.clone(),
            hosted: self.hosted.clone(),
            advertised_proxy_url,
            gateway_ca_dir: self.gateway.ca_dir.clone(),
            gateway_proxy_operator_cert: self.gateway.proxy_tls.cert_file.is_some(),
            hookhost_url: self.api.hookhost_url.clone(),
            plugins: self.plugins.clone(),
            retention: self.retention.clone(),
            ratelimit: self.ratelimit.clone(),
            tls: self.tls.clone(),
            telemetry: self.telemetry.clone(),
        })
    }

    pub fn gateway_config(&self) -> Result<GatewayConfig> {
        let mut c = Ctx::default();
        let s = &self.env_secrets;
        let external_url = check_server(&mut c, &self.server, true);
        let database_url = check_database_url(&mut c, &s.database_url);
        let keyring = check_keyring(&mut c, s);
        let internal_ring = ring(
            &mut c,
            "AURORA_INTERNAL_SECRET",
            "AURORA_INTERNAL_ACTIVE_KEY",
            &s.internal_secret,
            &s.internal_active_key,
            RingKind::Internal,
        );
        check_pos(
            &mut c,
            "database.max_connections",
            self.database.max_connections,
        );
        check_secret_stores(&mut c, &self.secrets);
        check_llm(&mut c, &self.llm, &self.governance);
        let governance = check_governance(&mut c, self);
        let g = &self.gateway;
        let internal_bind = resolve_bind(
            &mut c,
            "gateway.internal_bind",
            g.internal_bind.as_deref(),
            &self.server.bind,
        );
        if !(4096..=8 * 1024 * 1024).contains(&g.condition_body_bytes) {
            c.err(
                "gateway.condition_body_bytes",
                "must be between 4096 (4 KiB) and 8388608 (8 MiB)",
            );
        }
        check_pos(
            &mut c,
            "gateway.upstream_header_timeout_secs",
            Some(g.upstream_header_timeout_secs),
        );
        check_pos(
            &mut c,
            "gateway.resolve_cache_ttl_secs",
            Some(g.resolve_cache_ttl_secs),
        );
        check_proxy_url(
            &mut c,
            "gateway.advertised_proxy_url",
            g.advertised_proxy_url.as_deref(),
        );
        check_proxy_tls(&mut c, &g.proxy_tls);
        check_url(&mut c, "gateway.hookhost_url", Some(&g.hookhost_url));
        if let Some(p) = &g.egress_proxy {
            check_url(&mut c, "gateway.egress_proxy", Some(p));
        }
        if g.ca_dir.is_empty() {
            c.err("gateway.ca_dir", "must not be empty");
        }
        check_pos(
            &mut c,
            "approvals.timeout_secs",
            Some(self.approvals.timeout_secs),
        );
        check_common(&mut c, &self.tls, &self.telemetry);
        let advertised_proxy_url = advertised_proxy_url(g, external_url.as_deref());
        c.finish()?;
        Ok(GatewayConfig {
            server: self.server.clone(),
            external_url: external_url.unwrap_or_default(),
            database_url: database_url.unwrap_or_default(),
            max_connections: self.database.max_connections.unwrap_or(10),
            keyring: keyring.unwrap_or_else(empty_keyring),
            internal_ring: internal_ring.unwrap_or_default(),
            secrets: self.secrets.clone(),
            llm: self.llm.clone(),
            governance: governance.unwrap_or_else(|| resolve_governance_lossy(self)),
            gateway: g.clone(),
            internal_bind,
            advertised_proxy_url,
            approvals: self.approvals.clone(),
            tls: self.tls.clone(),
            telemetry: self.telemetry.clone(),
        })
    }

    pub fn hookhost_config(&self) -> Result<HookhostConfig> {
        let mut c = Ctx::default();
        check_server(&mut c, &self.server, false);
        let internal = ring(
            &mut c,
            "AURORA_INTERNAL_SECRET",
            "AURORA_INTERNAL_ACTIVE_KEY",
            &self.env_secrets.internal_secret,
            &self.env_secrets.internal_active_key,
            RingKind::Internal,
        );
        check_url(
            &mut c,
            "hookhost.api_internal_url",
            Some(&self.hookhost.api_internal_url),
        );
        check_common(&mut c, &self.tls, &self.telemetry);
        c.finish()?;
        Ok(HookhostConfig {
            server: self.server.clone(),
            internal_ring: internal.unwrap_or_default(),
            hookhost: self.hookhost.clone(),
            tls: self.tls.clone(),
            telemetry: self.telemetry.clone(),
        })
    }

    pub fn runner_config(&self) -> Result<RunnerConfig> {
        let mut c = Ctx::default();
        let s = &self.env_secrets;
        let r = &self.runner;
        check_server(&mut c, &self.server, false);
        let token = match &s.runner_token {
            Some(t) => {
                token_prefix(&mut c, "AURORA_RUNNER_TOKEN", t, "aur_");
                Some(t.clone())
            }
            None => {
                c.err("AURORA_RUNNER_TOKEN", "required");
                None
            }
        };
        let session_key = key32(&mut c, "AURORA_RUNNER_SESSION_KEY", &s.runner_session_key);
        let backend = match r.backend.as_deref() {
            None => {
                c.err("runner.backend", "required (podman, kubernetes or fake)");
                None
            }
            Some("podman") => Some(RunnerBackend::Podman),
            Some("kubernetes") => Some(RunnerBackend::Kubernetes),
            Some("fake") => Some(RunnerBackend::Fake),
            Some(other) => {
                c.err(
                    "runner.backend",
                    format!("invalid value `{other}`; expected podman, kubernetes or fake"),
                );
                None
            }
        };
        let api_url = match &r.api_url {
            Some(u) => {
                check_url(&mut c, "runner.api_url", Some(u));
                Some(u.clone())
            }
            None => {
                c.err("runner.api_url", "required");
                None
            }
        };
        let orphan_reap = match r.orphan_reap.as_str() {
            "reap" => Some(OrphanReap::Reap),
            "log_only" => Some(OrphanReap::LogOnly),
            other => {
                c.err(
                    "runner.orphan_reap",
                    format!("invalid value `{other}`; expected reap or log_only"),
                );
                None
            }
        };
        for (k, v) in [
            ("runner.max_sandboxes", r.max_sandboxes),
            ("runner.sandbox_memory_mib", r.sandbox_memory_mib),
            ("runner.sandbox_cpus", r.sandbox_cpus),
            ("runner.sandbox_pids", r.sandbox_pids),
            ("runner.home_gib", r.home_gib),
        ] {
            check_pos(&mut c, k, Some(v));
        }
        check_pos(&mut c, "runner.held_awake_ceiling", r.held_awake_ceiling);
        check_pos(
            &mut c,
            "runner.reconcile_interval_secs",
            Some(r.reconcile_interval_secs),
        );
        if r.image.is_empty() {
            c.err("runner.image", "must not be empty");
        }
        if backend == Some(RunnerBackend::Podman) && r.sandbox_network.is_empty() {
            c.err("runner.sandbox_network", "must not be empty");
        }
        check_common(&mut c, &self.tls, &self.telemetry);
        let ops_bind = resolve_bind(
            &mut c,
            "runner.ops_bind",
            r.ops_bind.as_deref(),
            &self.server.bind,
        );
        c.finish()?;
        Ok(RunnerConfig {
            ops_bind,
            server: self.server.clone(),
            api_url: api_url.unwrap_or_default(),
            backend: backend.unwrap_or(RunnerBackend::Fake),
            token: token.unwrap_or_default(),
            session_key: session_key.unwrap_or_default(),
            orphan_reap: orphan_reap.unwrap_or(OrphanReap::Reap),
            held_awake_ceiling: r
                .held_awake_ceiling
                .unwrap_or_else(|| r.max_sandboxes.saturating_sub(1).max(1)),
            runner: r.clone(),
            tls: self.tls.clone(),
            telemetry: self.telemetry.clone(),
        })
    }

    pub fn connector_config(&self) -> Result<ConnectorConfig> {
        let mut c = Ctx::default();
        let token = match &self.env_secrets.connector_token {
            Some(t) => {
                token_prefix(&mut c, "AURORA_CONNECTOR_TOKEN", t, "aun_");
                Some(t.clone())
            }
            None => {
                c.err("AURORA_CONNECTOR_TOKEN", "required");
                None
            }
        };
        check_common(&mut c, &self.tls, &self.telemetry);
        c.finish()?;
        Ok(ConnectorConfig {
            token: token.unwrap_or_default(),
            tls: self.tls.clone(),
            telemetry: self.telemetry.clone(),
        })
    }

    /// Effective `known_llm_hosts`: configured list (or built-in), plus
    /// `known_llm_hosts_extra`, minus hosts of configured endpoints.
    pub fn known_llm_hosts(&self) -> Vec<String> {
        let g = &self.governance.llm;
        let mut hosts: Vec<String> = match &g.known_llm_hosts {
            Some(list) => list.clone(),
            None => {
                let has_anthropic = self.llm.endpoints.iter().any(|e| e.provider == "anthropic");
                BUILTIN_KNOWN_LLM_HOSTS
                    .iter()
                    .filter(|h| !(has_anthropic && **h == "api.anthropic.com"))
                    .map(|h| (*h).to_string())
                    .collect()
            }
        };
        hosts.extend(g.known_llm_hosts_extra.iter().cloned());
        let endpoint_hosts: BTreeSet<String> = self
            .llm
            .endpoints
            .iter()
            .filter_map(|e| e.base_url.as_deref().and_then(host_of))
            .collect();
        let mut seen = BTreeSet::new();
        hosts.retain(|h| {
            let l = h.to_ascii_lowercase();
            !endpoint_hosts.contains(&l) && seen.insert(l)
        });
        hosts
    }
}

// ---------------------------------------------------------------- helpers

#[derive(Default)]
struct Ctx {
    errors: Vec<ConfigError>,
}

impl Ctx {
    fn err(&mut self, key: impl Into<String>, msg: impl Into<String>) {
        self.errors.push(ConfigError::new(key, msg));
    }
    fn finish(self) -> std::result::Result<(), ConfigErrors> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigErrors(self.errors))
        }
    }
}

fn empty_keyring() -> Keyring {
    Keyring {
        keys: vec![],
        active: String::new(),
    }
}

fn b64() -> GeneralPurpose {
    GeneralPurpose::new(
        &B64_ALPHABET,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    )
}

fn decode32(s: &str) -> std::result::Result<[u8; 32], String> {
    let bytes = b64()
        .decode(s.trim())
        .map_err(|_| "not valid base64".to_string())?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| format!("must decode to exactly 32 bytes (got {})", bytes.len()))
}

fn key32(c: &mut Ctx, name: &str, v: &Option<Redacted<String>>) -> Option<Redacted<[u8; 32]>> {
    let Some(v) = v else {
        c.err(name, "required");
        return None;
    };
    match decode32(v.expose()) {
        Ok(k) => Some(Redacted::new(k)),
        Err(m) => {
            c.err(name, m);
            None
        }
    }
}

/// Parse a signing-key / internal-secret ring (`08 §4`, RFE-007). A bare value is ring `v0`;
/// errors name the variable and never include values.
fn ring(
    c: &mut Ctx,
    name: &str,
    active_name: &str,
    v: &Option<Redacted<String>>,
    active: &Option<Redacted<String>>,
    kind: RingKind,
) -> Option<SecretRing> {
    let Some(v) = v else {
        c.err(name, "required");
        return None;
    };
    match SecretRing::parse(
        v.expose(),
        active.as_ref().map(|a| a.expose().as_str()),
        kind,
    ) {
        Ok(r) => Some(r),
        Err(e) => {
            let msg = e.to_string();
            let key = if msg.contains("active key id") {
                active_name
            } else {
                name
            };
            c.err(key, msg.trim_start_matches("invalid keyring: "));
            None
        }
    }
}

fn token_prefix(c: &mut Ctx, name: &str, v: &Redacted<String>, prefix: &str) {
    let t = v.expose();
    if !t.starts_with(prefix) || t.len() <= prefix.len() || t.chars().any(char::is_whitespace) {
        c.err(
            name,
            format!("must start with `{prefix}` and contain no whitespace"),
        );
    }
}

fn bootstrap_connectors(
    c: &mut Ctx,
    v: &Option<Redacted<String>>,
) -> Vec<(String, Redacted<String>)> {
    const NAME: &str = "AURORA_BOOTSTRAP_CONNECTOR_TOKENS";
    let mut out = Vec::new();
    let Some(v) = v else {
        return out;
    };
    for item in v
        .expose()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match item.split_once(':') {
            Some((id, tok))
                if valid_connector_id(id) && tok.starts_with("aun_") && tok.len() > 4 =>
            {
                out.push((id.to_string(), Redacted::new(tok.to_string())));
            }
            _ => c.err(
                NAME,
                "entries must be `id:aun_…` (id: [a-z][a-z0-9-]{1,31})",
            ),
        }
    }
    out
}

/// Connector ids as stored (`connectors.id` check): `[a-z][a-z0-9-]{1,31}`.
fn valid_connector_id(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn check_database_url(c: &mut Ctx, v: &Option<Redacted<String>>) -> Option<Redacted<String>> {
    const NAME: &str = "AURORA_DATABASE_URL";
    let Some(v) = v else {
        c.err(NAME, "required");
        return None;
    };
    let u = v.expose();
    if !(u.starts_with("postgres://") || u.starts_with("postgresql://")) {
        c.err(NAME, "must be a postgres:// or postgresql:// URL");
        return None;
    }
    Some(v.clone())
}

fn check_keyring(c: &mut Ctx, s: &EnvSecrets) -> Option<Keyring> {
    const KEYS: &str = "AURORA_ENCRYPTION_KEYS";
    const ACTIVE: &str = "AURORA_ENCRYPTION_ACTIVE_KEY";
    let mut keys: Vec<KeyringEntry> = Vec::new();
    let mut ok = true;
    match &s.encryption_keys {
        None => {
            c.err(KEYS, "required");
            ok = false;
        }
        Some(v) => {
            for item in v
                .expose()
                .split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
            {
                let Some((id, b)) = item.split_once(':') else {
                    c.err(KEYS, "entries must be `<key_id>:<base64 32 bytes>`");
                    ok = false;
                    continue;
                };
                if !(1..=16).contains(&id.len())
                    || !id
                        .chars()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
                {
                    c.err(KEYS, "key ids must match [a-z0-9]{1,16}");
                    ok = false;
                    continue;
                }
                if keys.iter().any(|k| k.id == id) {
                    c.err(KEYS, format!("duplicate key id `{id}`"));
                    ok = false;
                    continue;
                }
                match decode32(b) {
                    Ok(k) => keys.push(KeyringEntry {
                        id: id.to_string(),
                        key: Redacted::new(k),
                    }),
                    Err(m) => {
                        c.err(KEYS, format!("key `{id}`: {m}"));
                        ok = false;
                    }
                }
            }
            if ok && keys.is_empty() {
                c.err(KEYS, "must contain at least one key");
                ok = false;
            }
        }
    }
    let active = match &s.encryption_active_key {
        None => {
            c.err(ACTIVE, "required");
            return None;
        }
        Some(a) => a.expose().trim().to_string(),
    };
    if ok && !keys.iter().any(|k| k.id == active) {
        c.err(
            ACTIVE,
            format!("key id `{active}` is not present in {KEYS}"),
        );
        ok = false;
    }
    ok.then_some(Keyring { keys, active })
}

/// Class names accepted under `ratelimit.api.classes` (docs/05 §Conventions).
pub const API_RATE_CLASSES: [&str; 7] = [
    "general", "write", "message", "upload", "simulate", "search", "export",
];

fn check_ratelimit(c: &mut Ctx, r: &ApiRateSection) {
    if !(0.1..=100.0).contains(&r.multiplier) {
        c.err("ratelimit.api.multiplier", "must be between 0.1 and 100");
    }
    if r.org_per_minute == 0 {
        c.err("ratelimit.api.org_per_minute", "must be at least 1");
    }
    for (name, o) in &r.classes {
        let key = format!("ratelimit.api.classes.{name}");
        if !API_RATE_CLASSES.contains(&name.as_str()) {
            c.err(
                &key,
                format!(
                    "unknown class (expected one of {})",
                    API_RATE_CLASSES.join(", ")
                ),
            );
            continue;
        }
        if o.per_minute.is_some() && o.per_hour.is_some() {
            c.err(&key, "set either per_minute or per_hour, not both");
        }
        for (field, v) in [
            ("per_minute", o.per_minute),
            ("per_hour", o.per_hour),
            ("burst", o.burst),
        ] {
            if v == Some(0) {
                c.err(format!("{key}.{field}"), "must be at least 1");
            }
        }
    }
}

fn check_pos<T: Into<u64> + Copy>(c: &mut Ctx, key: &str, v: Option<T>) {
    if v.is_some_and(|v| v.into() == 0) {
        c.err(key, "must be greater than 0");
    }
}

fn check_url(c: &mut Ctx, key: &str, v: Option<&str>) {
    if let Some(v) = v
        && let Err(m) = parse_url(v, &["http", "https"])
    {
        c.err(key, m);
    }
}

/// (phase 1, RFE-002) The proxy listener is TLS-only, so the advertised URL must be `https://`.
fn check_proxy_url(c: &mut Ctx, key: &str, v: Option<&str>) {
    let Some(v) = v else { return };
    match parse_url(v, &["http", "https"]) {
        Err(m) => c.err(key, m),
        Ok(_) if !v.trim_start().to_ascii_lowercase().starts_with("https://") => c.err(
            key,
            "must be an https:// URL: the gateway proxy listener is TLS-only (docs/06 §Proxy listener TLS)",
        ),
        Ok(_) => {}
    }
}

/// `gateway.proxy_tls.*`: certificate and key come as a pair; names must be DNS names or IPs.
fn check_proxy_tls(c: &mut Ctx, t: &crate::raw::ProxyTlsSection) {
    match (&t.cert_file, &t.key_file) {
        (Some(a), Some(b)) => {
            if a.is_empty() {
                c.err("gateway.proxy_tls.cert_file", "must not be empty");
            }
            if b.is_empty() {
                c.err("gateway.proxy_tls.key_file", "must not be empty");
            }
        }
        (Some(_), None) => c.err(
            "gateway.proxy_tls.key_file",
            "required when gateway.proxy_tls.cert_file is set (both or neither)",
        ),
        (None, Some(_)) => c.err(
            "gateway.proxy_tls.cert_file",
            "required when gateway.proxy_tls.key_file is set (both or neither)",
        ),
        (None, None) => {}
    }
    for n in &t.names {
        let ok = !n.is_empty()
            && n.len() <= 253
            && n.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':' | '*'));
        if !ok {
            c.err(
                "gateway.proxy_tls.names",
                format!("`{n}` is not a DNS name or IP address"),
            );
        }
    }
}

/// Resolve an optional secondary bind address, validating it when present.
fn resolve_bind(c: &mut Ctx, key: &str, v: Option<&str>, fallback: &str) -> String {
    match v {
        None => fallback.to_string(),
        Some(b) => {
            if !is_bind_addr(b) {
                c.err(key, format!("`{b}` is not an IP address"));
            }
            b.trim_start_matches('[').trim_end_matches(']').to_string()
        }
    }
}

/// SEC-09 defence in depth: a message when the internal/ops listener can be
/// reached through the same address as the sandbox-facing listener.
pub fn bind_overlap_warning(sandbox_facing: &str, internal: &str) -> Option<&'static str> {
    let strip = |s: &str| s.trim_start_matches('[').trim_end_matches(']').to_string();
    let (a, b) = (
        strip(sandbox_facing).parse::<std::net::IpAddr>().ok()?,
        strip(internal).parse::<std::net::IpAddr>().ok()?,
    );
    // An unspecified internal bind covers every address, including the sandbox-facing one.
    if a == b || b.is_unspecified() {
        Some(
            "SEC-09: internal listener shares an address with the sandbox-facing listener; \
             bind it to a core-network-only address or isolate it with NetworkPolicy",
        )
    } else {
        None
    }
}

/// Returns the normalized external URL (no trailing slash) when valid/required.
fn check_server(c: &mut Ctx, s: &ServerSection, need_external: bool) -> Option<String> {
    if !is_bind_addr(&s.bind) {
        c.err("server.bind", format!("`{}` is not an IP address", s.bind));
    }
    for (i, p) in s.trusted_proxies.iter().enumerate() {
        if !is_cidr(p) {
            c.err(
                format!("server.trusted_proxies[{i}]"),
                format!("`{p}` is not a CIDR"),
            );
        }
    }
    match s.external_url.as_deref() {
        None | Some("") => {
            if need_external {
                c.err("server.external_url", "required");
            }
            None
        }
        Some(u) => match parse_url(u, &["http", "https"]) {
            Ok(p)
                if (p.path.is_empty() || p.path == "/")
                    && !p.has_query_or_fragment
                    && !p.has_userinfo =>
            {
                Some(u.trim_end_matches('/').to_string())
            }
            Ok(_) => {
                c.err("server.external_url", "must be an origin (scheme://host[:port]) without credentials, path, query or fragment");
                None
            }
            Err(m) => {
                c.err("server.external_url", m);
                None
            }
        },
    }
}

fn advertised_proxy_url(g: &GatewaySection, external: Option<&str>) -> String {
    if let Some(u) = &g.advertised_proxy_url {
        return u.clone();
    }
    match external.and_then(host_of) {
        Some(h) if h.contains(':') => format!("https://[{h}]:{GATEWAY_PROXY_PORT}"),
        Some(h) => format!("https://{h}:{GATEWAY_PROXY_PORT}"),
        None => String::new(),
    }
}

fn check_common(c: &mut Ctx, tls: &TlsSection, t: &TelemetrySection) {
    if tls.extra_ca_bundle.as_deref() == Some("") {
        c.err("tls.extra_ca_bundle", "must not be empty when set");
    }
    if t.log_level.trim().is_empty() {
        c.err("telemetry.log_level", "must not be empty");
    }
    check_url(c, "telemetry.otlp_endpoint", t.otlp_endpoint.as_deref());
}

fn check_auth(
    c: &mut Ctx,
    a: &AuthSection,
    s: &EnvSecrets,
) -> (Option<SignupMode>, Option<OrgCreation>) {
    let signup = match a.signup_mode.as_str() {
        "open" => Some(SignupMode::Open),
        "invite_only" => Some(SignupMode::InviteOnly),
        "sso_jit" => Some(SignupMode::SsoJit),
        o => {
            c.err(
                "auth.signup_mode",
                format!("invalid value `{o}`; expected open, invite_only or sso_jit"),
            );
            None
        }
    };
    let org = match a.org_creation.as_str() {
        "instance_admins" => Some(OrgCreation::InstanceAdmins),
        "anyone" => Some(OrgCreation::Anyone),
        o => {
            c.err(
                "auth.org_creation",
                format!("invalid value `{o}`; expected instance_admins or anyone"),
            );
            None
        }
    };
    for (i, e) in a.instance_admins.iter().enumerate() {
        if !e.contains('@') {
            c.err(
                format!("auth.instance_admins[{i}]"),
                "must be an email address",
            );
        }
    }
    for (i, o) in a.trusted_origins.iter().enumerate() {
        check_url(c, &format!("auth.trusted_origins[{i}]"), Some(o));
    }
    check_pos(c, "auth.session_idle_secs", Some(a.session_idle_secs));
    check_pos(
        c,
        "auth.session_absolute_secs",
        Some(a.session_absolute_secs),
    );
    if a.session_idle_secs > a.session_absolute_secs {
        c.err(
            "auth.session_idle_secs",
            "must not exceed auth.session_absolute_secs",
        );
    }
    let mut ids = BTreeSet::new();
    for (i, p) in a.oidc_providers.iter().enumerate() {
        let k = |f: &str| format!("auth.oidc_providers[{i}].{f}");
        if !valid_id(&p.id) {
            c.err(k("id"), "required; lowercase letters, digits, `_` or `-`");
        } else if !ids.insert(p.id.as_str()) {
            c.err(k("id"), format!("duplicate provider id `{}`", p.id));
        }
        if p.issuer.is_empty() {
            c.err(k("issuer"), "required");
        } else {
            check_url(c, &k("issuer"), Some(&p.issuer));
        }
        if p.client_id.is_empty() {
            c.err(k("client_id"), "required");
        }
        if p.client_secret_env.is_empty() {
            c.err(k("client_secret_env"), "required");
        } else if !s.oidc_client_secrets.contains_key(&p.id) {
            c.err(
                k("client_secret_env"),
                format!(
                    "environment variable {} (or {0}_FILE) is not set",
                    p.client_secret_env
                ),
            );
        }
        if p.allowed_email_domains
            .iter()
            .any(|d| d.trim().trim_start_matches('@').is_empty() || d.contains(char::is_whitespace))
        {
            c.err(
                k("allowed_email_domains"),
                "entries must be non-empty domains such as `example.com`",
            );
        }
        if let Some(j) = &p.jit {
            if j.org_slug.is_empty() {
                c.err(k("jit.org_slug"), "required");
            }
            if j.role != "member" && j.role != "admin" {
                c.err(k("jit.role"), "must be member or admin");
            }
            if !matches!(j.workspace_role.as_str(), "" | "member" | "approver") {
                c.err(k("jit.workspace_role"), "must be member or approver");
            }
        }
    }
    if signup == Some(SignupMode::SsoJit) && a.oidc_providers.is_empty() {
        c.err(
            "auth.oidc_providers",
            "at least one provider is required when auth.signup_mode = sso_jit",
        );
    }
    (signup, org)
}

fn check_email(c: &mut Ctx, e: &EmailSection) {
    if let Some(u) = &e.smtp_url {
        if let Err(m) = parse_url(u.expose(), &["smtp", "smtps"]) {
            c.err("email.smtp_url", m);
        }
        if e.from.as_deref().is_none_or(|f| !f.contains('@')) {
            c.err(
                "email.from",
                "required (an email address) when email.smtp_url is set",
            );
        }
    }
}

fn check_secret_stores(c: &mut Ctx, s: &SecretsSection) {
    let mut ids = BTreeSet::new();
    for (i, st) in s.stores.iter().enumerate() {
        if !valid_id(&st.id) {
            c.err(
                format!("secrets.stores[{i}].id"),
                "required; lowercase letters, digits, `_` or `-`",
            );
        } else if !ids.insert(st.id.as_str()) {
            c.err(
                format!("secrets.stores[{i}].id"),
                format!("duplicate store id `{}`", st.id),
            );
        }
        if st.kind.is_empty() {
            c.err(format!("secrets.stores[{i}].kind"), "required");
        }
    }
    if s.default_store != "inline" && !ids.contains(s.default_store.as_str()) {
        c.err(
            "secrets.default_store",
            format!(
                "`{}` is neither `inline` nor a configured store id",
                s.default_store
            ),
        );
    }
}

fn check_llm(c: &mut Ctx, l: &LlmSection, g: &GovernanceSection) {
    let mut ids = BTreeSet::new();
    for (i, e) in l.endpoints.iter().enumerate() {
        let k = |f: &str| format!("llm.endpoints[{i}].{f}");
        if !valid_id(&e.id) {
            c.err(k("id"), "required; lowercase letters, digits, `_` or `-`");
        } else if !ids.insert(e.id.as_str()) {
            c.err(k("id"), format!("duplicate endpoint id `{}`", e.id));
        }
        if e.provider.is_empty() {
            c.err(k("provider"), "required");
        }
        check_url(c, &k("base_url"), e.base_url.as_deref());
        match &e.key_origin {
            Some(o) => {
                if !matches!(
                    o.method.as_str(),
                    "endpoint" | "litellm_team" | "provider_account"
                ) {
                    c.err(
                        k("key_origin.method"),
                        "must be endpoint, litellm_team or provider_account",
                    );
                }
            }
            None if g.llm.require_key_origin => {
                c.err(
                    k("key_origin"),
                    "required because governance.llm.require_key_origin = true",
                );
            }
            None => {}
        }
    }
}

fn check_governance(c: &mut Ctx, raw: &RawConfig) -> Option<Governance> {
    let g = &raw.governance;
    let before = c.errors.len();
    let llm_posture = match g.llm.minimum_posture.as_str() {
        "open" => Some(LlmPosture::Open),
        "sanctioned_only" => Some(LlmPosture::SanctionedOnly),
        o => {
            c.err(
                "governance.llm.minimum_posture",
                format!("invalid value `{o}`; expected open or sanctioned_only"),
            );
            None
        }
    };
    let egress_posture = match g.egress.minimum_posture.as_str() {
        "open" => Some(EgressPosture::Open),
        "credentialed_only" => Some(EgressPosture::CredentialedOnly),
        o => {
            c.err(
                "governance.egress.minimum_posture",
                format!("invalid value `{o}`; expected open or credentialed_only"),
            );
            None
        }
    };
    let open_lane_minimum = match g.egress.open_lane_minimum.as_str() {
        "enabled" => Some(OpenLanePosture::Enabled),
        "allowlist_only" => Some(OpenLanePosture::AllowlistOnly),
        "disabled" => Some(OpenLanePosture::Disabled),
        o => {
            c.err(
                "governance.egress.open_lane_minimum",
                format!("invalid value `{o}`; expected enabled, allowlist_only or disabled"),
            );
            None
        }
    };
    for (i, s) in g.llm.allowed_key_scopes.iter().enumerate() {
        if !matches!(s.as_str(), "organization" | "workspace") {
            c.err(
                format!("governance.llm.allowed_key_scopes[{i}]"),
                format!("invalid scope `{s}`; expected organization or workspace"),
            );
        }
    }
    check_pos(
        c,
        "governance.llm.origin_recheck_secs",
        Some(g.llm.origin_recheck_secs),
    );
    for (i, h) in g.llm.known_llm_hosts.iter().flatten().enumerate() {
        check_host_pattern(c, &format!("governance.llm.known_llm_hosts[{i}]"), h);
    }
    for (i, h) in g.llm.known_llm_hosts_extra.iter().enumerate() {
        check_host_pattern(c, &format!("governance.llm.known_llm_hosts_extra[{i}]"), h);
    }
    for (i, e) in g.egress.baseline_allowlist.iter().enumerate() {
        check_host_pattern(
            c,
            &format!("governance.egress.baseline_allowlist[{i}].host"),
            &e.host,
        );
        if e.path.as_deref().is_some_and(|p| !p.starts_with('/')) {
            c.err(
                format!("governance.egress.baseline_allowlist[{i}].path"),
                "must start with `/`",
            );
        }
    }
    if c.errors.len() > before {
        return None;
    }
    Some(Governance {
        allowed_key_scopes: g.llm.allowed_key_scopes.clone(),
        llm_minimum_posture: llm_posture?,
        require_key_origin: g.llm.require_key_origin,
        known_llm_hosts: raw.known_llm_hosts(),
        origin_recheck_secs: g.llm.origin_recheck_secs,
        egress_minimum_posture: egress_posture?,
        baseline_allowlist: g.egress.baseline_allowlist.clone(),
        open_lane_minimum: open_lane_minimum?,
    })
}

/// Only used on paths where validation already failed and the value is discarded.
fn resolve_governance_lossy(raw: &RawConfig) -> Governance {
    let g = &raw.governance;
    Governance {
        allowed_key_scopes: g.llm.allowed_key_scopes.clone(),
        llm_minimum_posture: LlmPosture::Open,
        require_key_origin: g.llm.require_key_origin,
        known_llm_hosts: raw.known_llm_hosts(),
        origin_recheck_secs: g.llm.origin_recheck_secs,
        egress_minimum_posture: EgressPosture::Open,
        baseline_allowlist: g.egress.baseline_allowlist.clone(),
        open_lane_minimum: OpenLanePosture::Enabled,
    }
}

fn check_host_pattern(c: &mut Ctx, key: &str, h: &str) {
    if h.is_empty()
        || h.chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '/' | ':' | '@'))
    {
        c.err(
            key,
            "must be a host name or wildcard pattern without scheme, port or path",
        );
    }
}

fn check_hosted(c: &mut Ctx, h: &HostedSection) {
    for (k, v) in [
        ("hosted.idle_stop_secs", h.idle_stop_secs),
        ("hosted.background_cap_secs", h.background_cap_secs),
        ("hosted.turn_ceiling_secs", h.turn_ceiling_secs),
        ("hosted.turn_stall_secs", h.turn_stall_secs),
    ] {
        check_pos(c, k, Some(v));
    }
    if h.default_harness.is_empty() {
        c.err("hosted.default_harness", "must not be empty");
    }
    if h.default_timezone.is_empty() {
        c.err("hosted.default_timezone", "must not be empty");
    }
}
