//! Raw (deserialized, not yet validated) configuration sections.
//!
//! Defaults follow `docs/15-deployment-operations.md` "Configuration reference".
//! Enum-like values are kept as strings here and validated (with all errors
//! collected) by `RawConfig::validate_for`.

use crate::redacted::Redacted;
use serde::Deserialize;
use std::collections::BTreeMap;

pub const DEFAULT_AGENT_BRIEF: &str = "You are a general-purpose assistant in this workspace. Be concise and direct. Ask before acting on anything irreversible, and say which tools or apps you used.";

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

/// Secrets supplied through environment variables (or `<NAME>_FILE`).
/// Never read from the TOML file.
#[derive(Debug, Clone, Default)]
pub struct EnvSecrets {
    pub database_url: Option<Redacted<String>>,
    pub encryption_keys: Option<Redacted<String>>,
    pub encryption_active_key: Option<Redacted<String>>,
    /// Ring list or bare value (RFE-007, `08 §4`).
    pub signing_key: Option<Redacted<String>>,
    pub signing_active_key: Option<Redacted<String>>,
    /// Ring list or bare value (RFE-007, `08 §4`).
    pub internal_secret: Option<Redacted<String>>,
    pub internal_active_key: Option<Redacted<String>>,
    pub runner_session_key: Option<Redacted<String>>,
    pub runner_token: Option<Redacted<String>>,
    pub connector_token: Option<Redacted<String>>,
    /// (phase 4, P4-MCON) the connector host's anchor token `auh_…`.
    pub connector_host_token: Option<Redacted<String>>,
    pub bootstrap_runner_token: Option<Redacted<String>>,
    pub bootstrap_connector_tokens: Option<Redacted<String>>,
    pub bootstrap_connector_host_token: Option<Redacted<String>>,
    /// OIDC client secrets keyed by provider id (from each `client_secret_env`).
    pub oidc_client_secrets: BTreeMap<String, Redacted<String>>,
    /// (phase 4, P4-CNT) values of `ratelimit.counters.redis.username_env` / `password_env`.
    pub redis_username: Option<Redacted<String>>,
    pub redis_password: Option<Redacted<String>>,
}

/// The whole configuration file, merged with env overrides.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RawConfig {
    pub server: ServerSection,
    pub database: DatabaseSection,
    pub auth: AuthSection,
    pub email: EmailSection,
    pub secrets: SecretsSection,
    pub llm: LlmSection,
    pub governance: GovernanceSection,
    /// (phase 2, RFE-025) BYO agent controls (docs/08 §11.9).
    pub byo: ByoSection,
    pub agents: AgentsSection,
    pub hosted: HostedSection,
    pub api: ApiSection,
    pub gateway: GatewaySection,
    pub approvals: ApprovalsSection,
    /// (phase 1, RFE-016) ephemeral delta fan-out.
    pub events: EventsSection,
    pub plugins: PluginsSection,
    pub hookhost: HookhostSection,
    pub runner: RunnerSection,
    /// (phase 4, P4-MCON) the connector host (docs/15).
    pub connector_host: ConnectorHostSection,
    pub tls: TlsSection,
    pub retention: RetentionSection,
    pub attachments: AttachmentsSection,
    pub ratelimit: RatelimitSection,
    /// (phase 4, P4-GSL) git skill library switches and limits.
    pub skills: SkillsSection,
    pub telemetry: TelemetrySection,
    #[serde(skip)]
    pub env_secrets: EnvSecrets,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSection {
    pub external_url: Option<String>,
    pub bind: String,
    pub trusted_proxies: Vec<String>,
}
impl Default for ServerSection {
    fn default() -> Self {
        Self {
            external_url: None,
            bind: "0.0.0.0".into(),
            trusted_proxies: vec![],
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseSection {
    /// Default: 20 (api), 10 (gateway).
    pub max_connections: Option<u32>,
    pub gateway_role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthSection {
    pub signup_mode: String,
    pub org_creation: String,
    pub instance_admins: Vec<String>,
    pub trusted_origins: Vec<String>,
    pub session_idle_secs: u64,
    pub session_absolute_secs: u64,
    pub oidc_providers: Vec<OidcProvider>,
    /// (phase 1, RFE-004) The development IdP, `docs/04 §Development and end-to-end sign-in`.
    pub dev_idp: DevIdp,
}

/// Fixed development client of the `dev` provider (`aurora-dev-idp`, `aurora_testkit::oidc`).
pub const DEV_IDP_PROVIDER_ID: &str = "dev";
pub const DEV_IDP_CLIENT_ID: &str = "aurora-dev-client";
pub const DEV_IDP_CLIENT_SECRET: &str = "aurora-dev-secret";

/// `auth.dev_idp`: registers the extra provider `dev` served by `aurora-dev-idp`. Development
/// and tests only: refused on a non-loopback `server.external_url` (`validate::check_auth`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DevIdp {
    pub enabled: bool,
    /// Issuer URL as the *browser* reaches it (also the `iss` claim).
    pub issuer: Option<String>,
    /// Where the API itself reaches the IdP (discovery, token, JWKS) when that differs from
    /// `issuer` (e.g. the in-cluster Service while the browser uses the ingress). Default: `issuer`.
    pub internal_url: Option<String>,
    /// (P4-IDP-1) Role claim settings of the `dev` provider, as for `auth.oidc_providers[]`, so
    /// e2e runs can drive roles through the dev IdP's configurable claims (`PUT /dev/claims/…`).
    pub role_claim: Option<String>,
    pub role_claim_source: String,
    pub require_role: bool,
    pub role_mappings: Vec<OidcRoleMapping>,
}
impl Default for AuthSection {
    fn default() -> Self {
        Self {
            signup_mode: "invite_only".into(),
            org_creation: "instance_admins".into(),
            instance_admins: vec![],
            trusted_origins: vec![],
            session_idle_secs: 1_209_600,
            session_absolute_secs: 2_592_000,
            oidc_providers: vec![],
            dev_idp: DevIdp::default(),
        }
    }
}

/// `docs/04 §OIDC`. `issuer_url` is accepted as an alias of `issuer`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OidcProvider {
    pub id: String,
    pub display_name: Option<String>,
    #[serde(alias = "issuer_url")]
    pub issuer: String,
    pub client_id: String,
    /// Name of the environment variable holding the client secret.
    pub client_secret_env: String,
    pub scopes: Vec<String>,
    /// Removed in phase 1 (RFE-004): an unverified e-mail is always refused. `false` fails
    /// startup; `true` is accepted and ignored.
    pub require_email_verified: Option<bool>,
    pub allowed_email_domains: Vec<String>,
    /// (D-118) Domains this provider is authoritative for: an `email` in one of them (exact
    /// domain, case-insensitive, no subdomains) counts as verified even when the ID token has no
    /// `email_verified = true`. For IdPs that never send the claim. Empty = claim required.
    pub trusted_email_domains: Vec<String>,
    pub jit: Option<OidcJit>,
    /// (phase 4, P4-IDP-1, D-139) Dot path into the validated ID token (or userinfo response)
    /// holding the IdP's role values, e.g. `cern_roles` or `resource_access.aurora.roles`.
    pub role_claim: Option<String>,
    /// `id_token` (default, also when empty) or `userinfo`.
    pub role_claim_source: String,
    /// A sign-in whose claim maps to nothing is refused (`no_mapped_role`, no user row).
    pub require_role: bool,
    pub role_mappings: Vec<OidcRoleMapping>,
}
impl Default for OidcProvider {
    fn default() -> Self {
        Self {
            id: String::new(),
            display_name: None,
            issuer: String::new(),
            client_id: String::new(),
            client_secret_env: String::new(),
            scopes: strs(&["openid", "email", "profile"]),
            require_email_verified: None,
            allowed_email_domains: vec![],
            trusted_email_domains: vec![],
            jit: None,
            role_claim: None,
            role_claim_source: String::new(),
            require_role: false,
            role_mappings: vec![],
        }
    }
}

/// (D-139) One exact, case-sensitive claim value and what it grants.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OidcRoleMapping {
    pub value: String,
    pub instance_admin: bool,
    pub org: Option<OidcOrgRole>,
    /// Workspaces of `org` (required when set).
    pub workspaces: Vec<OidcWorkspaceRole>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OidcOrgRole {
    pub slug: String,
    /// `owner` | `admin` | `member`.
    pub role: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OidcWorkspaceRole {
    pub slug: String,
    /// `owner` | `approver` | `member`.
    pub role: String,
}

impl OidcProvider {
    /// Whether the role claim is read from the userinfo response instead of the ID token.
    pub fn role_claim_from_userinfo(&self) -> bool {
        self.role_claim_source == "userinfo"
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OidcJit {
    pub org_slug: String,
    /// `member` or `admin`.
    pub role: String,
    pub workspace_slugs: Vec<String>,
    /// Phase 2, RFE-012: `member` (default, also when empty) or `approver`.
    pub workspace_role: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmailSection {
    /// May embed a password, hence redacted. Unset => log-only mode.
    pub smtp_url: Option<Redacted<String>>,
    pub from: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecretsSection {
    pub default_store: String,
    pub stores: Vec<SecretStoreConfig>,
}
impl Default for SecretsSection {
    fn default() -> Self {
        Self {
            default_store: "inline".into(),
            stores: vec![],
        }
    }
}

/// Opaque store table: only `id` and `kind` are interpreted here; the rest is
/// kept (redacted in `Debug`) for `aurora-secrets`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SecretStoreConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(flatten)]
    pub settings: Redacted<toml::Table>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LlmSection {
    pub enabled_providers: Vec<String>,
    pub provider_preference: Vec<String>,
    pub endpoints: Vec<LlmEndpoint>,
}
impl Default for LlmSection {
    fn default() -> Self {
        Self {
            enabled_providers: strs(&["litellm", "anthropic"]),
            provider_preference: strs(&["litellm", "anthropic"]),
            endpoints: vec![],
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LlmEndpoint {
    pub id: String,
    pub provider: String,
    pub display_name: Option<String>,
    pub base_url: Option<String>,
    /// `anthropic_messages`, `openai_chat`, `openai_responses`; empty = provider default.
    pub dialects: Vec<String>,
    pub default_model: Option<String>,
    /// Optional static fallback model list.
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub key_origin: Option<KeyOrigin>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeyOrigin {
    /// `endpoint`, `litellm_team` or `provider_account`.
    pub method: String,
    pub allowed_organization_ids: Vec<String>,
    pub allowed_workspace_ids: Vec<String>,
    pub allowed_team_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GovernanceSection {
    pub llm: GovernanceLlm,
    pub egress: GovernanceEgress,
    pub agent_tokens: GovernanceAgentTokens,
}

/// `[governance.agent_tokens]` (phase 3, RFE-013, C-317).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GovernanceAgentTokens {
    /// Maximum age of BYO agent tokens in days (1..=3650); unset = none.
    pub max_age_days: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GovernanceLlm {
    pub allowed_key_scopes: Vec<String>,
    /// `open` or `sanctioned_only`.
    pub minimum_posture: String,
    pub require_key_origin: bool,
    /// `None` => built-in list.
    pub known_llm_hosts: Option<Vec<String>>,
    pub known_llm_hosts_extra: Vec<String>,
    pub origin_recheck_secs: u64,
}
impl Default for GovernanceLlm {
    fn default() -> Self {
        Self {
            allowed_key_scopes: strs(&["organization", "workspace"]),
            minimum_posture: "open".into(),
            require_key_origin: false,
            known_llm_hosts: None,
            known_llm_hosts_extra: vec![],
            origin_recheck_secs: 3600,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GovernanceEgress {
    /// `open` or `credentialed_only`.
    pub minimum_posture: String,
    pub baseline_allowlist: Vec<EgressEntry>,
    /// (RFE-003) `enabled`, `allowlist_only` or `disabled`: floor of the open-lane
    /// posture for every org and workspace.
    pub open_lane_minimum: String,
}
impl Default for GovernanceEgress {
    fn default() -> Self {
        Self {
            minimum_posture: "open".into(),
            baseline_allowlist: vec![],
            open_lane_minimum: "enabled".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct EgressEntry {
    pub host: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsSection {
    pub default_name: String,
    pub default_brief: String,
}
impl Default for AgentsSection {
    fn default() -> Self {
        Self {
            default_name: "Daneel".into(),
            default_brief: DEFAULT_AGENT_BRIEF.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostedSection {
    pub enabled: bool,
    pub default_harness: String,
    /// (phase 4, P4-HAR, D-122) Harness ids agents may use; `fake` only with
    /// `allow_fake_harness`. Default `["claude-code"]`: Codex is opt-in.
    pub enabled_harnesses: Vec<String>,
    pub allow_fake_harness: bool,
    pub default_timezone: String,
    pub idle_stop_secs: u64,
    pub background_cap_secs: u64,
    pub turn_ceiling_secs: u64,
    pub turn_stall_secs: u64,
    /// (FIX-025, D-158) End-to-end bound from "a turn needs a sandbox" to "the supervisor is
    /// ready" (the runner's capacity wait counts inside it). 60–1800, default 300.
    pub sandbox_ready_timeout_secs: u64,
}
impl Default for HostedSection {
    fn default() -> Self {
        Self {
            enabled: true,
            default_harness: "claude-code".into(),
            enabled_harnesses: vec!["claude-code".into()],
            allow_fake_harness: false,
            default_timezone: "UTC".into(),
            idle_stop_secs: 30 * 60,
            background_cap_secs: 12 * 3600,
            turn_ceiling_secs: 6 * 3600,
            turn_stall_secs: 10 * 60,
            sandbox_ready_timeout_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApiSection {
    pub hookhost_url: String,
    /// (phase 4, P4-GSL, D-154) The gateway's internal listener (4103) as the API reaches it for
    /// `POST /internal/v1/library-fetch` (docs/06 §Library fetch); the internal secret ring
    /// authenticates the call.
    pub gateway_internal_url: String,
}
impl Default for ApiSection {
    fn default() -> Self {
        Self {
            hookhost_url: "http://aurora-hookhost:4104".into(),
            gateway_internal_url: "http://aurora-gateway:4103".into(),
        }
    }
}

/// `[skills]` (phase 4, P4-GSL).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillsSection {
    pub library: SkillsLibrarySection,
}

/// `[skills.library]` (phase 4, P4-GSL, D-149; docs/15 §Git skill library).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillsLibrarySection {
    /// Kill-switch: `false` hides the feature and stops the refresh job; stored skills keep
    /// working.
    pub enabled: bool,
    /// `LIBRARY_REFRESH_INTERVAL` in seconds (at least 300).
    pub refresh_interval_secs: u64,
    /// `LIBRARY_BYTES_PER_AGENT` (an operator may only lower it).
    pub bytes_per_agent: u64,
}
impl Default for SkillsLibrarySection {
    fn default() -> Self {
        Self {
            enabled: true,
            refresh_interval_secs: 900,
            bytes_per_agent: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GatewaySection {
    /// Default: derived from the external host plus `:4102`.
    pub advertised_proxy_url: Option<String>,
    /// (FIX-010, D-87) The proxy address hosted sandboxes dial (the boot document's
    /// `gateway.proxy` / `server_name`), `https://host[:port]` only. Default by `runner.backend`:
    /// `kubernetes` → `https://aurora-gateway:4102`; otherwise the advertised proxy URL.
    pub sandbox_proxy_url: Option<String>,
    /// Address for the internal listener (4103: health, metrics, status).
    /// Must NOT be reachable from sandboxes (SEC-09). Default: `server.bind`
    /// (startup logs a warning when it overlaps the proxy listener).
    pub internal_bind: Option<String>,
    pub allowed_private: Vec<String>,
    pub egress_proxy: Option<String>,
    pub condition_body_bytes: u64,
    pub upstream_header_timeout_secs: u64,
    pub resolve_cache_ttl_secs: u64,
    pub ca_dir: String,
    pub hookhost_url: String,
    /// (phase 1, RFE-002) Proxy listener certificate settings (docs/06 §Proxy listener TLS).
    pub proxy_tls: ProxyTlsSection,
    /// (phase 2, RFE-025) CIDRs of layer-4 load balancers that send a PROXY protocol header on
    /// the proxy port (docs/06 §BYO admission). Not `server.trusted_proxies`.
    pub trusted_proxies: Vec<String>,
}

/// `[byo]` (phase 2, RFE-025, D-59).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ByoSection {
    /// Master switch: `false` = BYO disabled everywhere.
    pub enabled: bool,
    /// `enabled` | `owners_only` | `disabled`: floor of the BYO posture.
    pub minimum_posture: String,
    /// Default source CIDR list of every BYO agent without its own or an org list.
    pub default_source_cidrs: Vec<String>,
    pub require_context_credentials: bool,
}
impl Default for ByoSection {
    fn default() -> Self {
        Self {
            enabled: true,
            minimum_posture: "owners_only".into(),
            default_source_cidrs: vec![],
            require_context_credentials: false,
        }
    }
}

/// `[gateway.proxy_tls]` (phase 1, RFE-002).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyTlsSection {
    /// Extra DNS names / IPs for the CA-issued proxy certificate.
    pub names: Vec<String>,
    /// Operator-supplied certificate chain (PEM); replaces the CA-issued leaf. Both or neither.
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
}
impl Default for GatewaySection {
    fn default() -> Self {
        Self {
            advertised_proxy_url: None,
            sandbox_proxy_url: None,
            internal_bind: None,
            allowed_private: vec![],
            egress_proxy: None,
            condition_body_bytes: 262_144,
            upstream_header_timeout_secs: 300,
            resolve_cache_ttl_secs: 60,
            ca_dir: "/var/lib/aurora/ca".into(),
            hookhost_url: "http://aurora-hookhost:4104".into(),
            proxy_tls: ProxyTlsSection::default(),
            trusted_proxies: vec![],
        }
    }
}

/// Id of the only `DeltaBus` implementation (Postgres `LISTEN`/`NOTIFY`).
pub const DELTA_BUS_NOTIFY: &str = "notify";
/// Bounds of `events.delta_coalesce_ms`.
pub const DELTA_COALESCE_MS_MIN: u32 = 100;
pub const DELTA_COALESCE_MS_MAX: u32 = 500;

/// (phase 1, RFE-016) `[events]`: which `DeltaBus` carries streaming deltas and the producer-side
/// coalescing window (`docs/15 §Configuration`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EventsSection {
    /// `DeltaBus` implementation id; `notify` is the only one. Unknown ids fail startup.
    pub delta_bus: String,
    /// Delta coalescing window in milliseconds, 100..=500.
    pub delta_coalesce_ms: u32,
}
impl Default for EventsSection {
    fn default() -> Self {
        Self {
            delta_bus: DELTA_BUS_NOTIFY.to_owned(),
            delta_coalesce_ms: DELTA_COALESCE_MS_MIN,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalsSection {
    pub timeout_secs: u64,
    /// (RFE-011a) Upper bound of per-rule / per-grant timeouts (300..=14400, below the turn ceiling).
    pub max_timeout_secs: u64,
    /// (RFE-011a) E-mail deciders when an approval has no live surface.
    pub email_notifications: bool,
}
impl Default for ApprovalsSection {
    fn default() -> Self {
        Self {
            timeout_secs: 300,
            max_timeout_secs: 3600,
            email_notifications: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginsSection {
    pub dir: String,
    /// Built-in catalogue (09 §10): only archives signed by the project key are loaded from here.
    pub builtin_dir: String,
    pub allow_unsigned: bool,
}
impl Default for PluginsSection {
    fn default() -> Self {
        Self {
            dir: "/var/lib/aurora/plugins".into(),
            builtin_dir: "/usr/share/aurora/plugins".into(),
            allow_unsigned: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HookhostSection {
    pub api_internal_url: String,
}
impl Default for HookhostSection {
    fn default() -> Self {
        Self {
            api_internal_url: "http://aurora-api:4111".into(),
        }
    }
}

/// `connector_host.*` (docs/15). The token comes from `AURORA_CONNECTOR_HOST_TOKEN`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectorHostSection {
    /// The API the host registers with.
    pub api_url: String,
    /// Platform callbacks (`/connectors/<id>/…`).
    pub listen: String,
    /// Health and metrics.
    pub ops_port: u16,
    /// Also announce the `fake` type (development and e2e only).
    pub test_types: bool,
}
impl Default for ConnectorHostSection {
    fn default() -> Self {
        Self {
            api_url: "http://127.0.0.1:4101".into(),
            listen: "0.0.0.0:4110".into(),
            ops_port: 4192,
            test_types: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerSection {
    pub api_url: Option<String>,
    /// Address for the ops listener (4106: health, metrics). Must NOT be
    /// reachable from sandboxes (SEC-09). Default: `server.bind` (startup logs
    /// a warning when it overlaps the sandbox channel listener).
    pub ops_bind: Option<String>,
    /// `podman`, `kubernetes` or `fake`; required for the runner.
    pub backend: Option<String>,
    pub image: String,
    /// (phase 4, P4-HAR, D-153) Harness runtimes the sandbox image contains, for backends that
    /// cannot read the image label `org.aurora.harnesses` (Kubernetes). Empty = the backend
    /// default (`claude-code`, `fake`). Podman reads the label and ignores this.
    pub image_harnesses: Vec<String>,
    pub advertised_host: Option<String>,
    pub max_sandboxes: u32,
    /// Default: `max(1, max_sandboxes - 1)`.
    pub held_awake_ceiling: Option<u32>,
    pub sandbox_memory_mib: u32,
    pub sandbox_cpus: u32,
    pub sandbox_pids: u32,
    pub home_gib: u32,
    pub reconcile_interval_secs: u64,
    pub orphan_grace_secs: u64,
    /// `reap` or `log_only`.
    pub orphan_reap: String,
    pub capacity_wait_secs: u64,
    pub podman_socket: Option<String>,
    pub sandbox_network: String,
    pub sandbox_hosts: BTreeMap<String, String>,
    /// Dev escape hatch: run the podman backend on a non-internal sandbox network.
    pub dev_allow_external_network: bool,
    /// OPS-018, dev only: absolute host path of a host-built `aurora-supervisor`, bind-mounted
    /// read-only over the image's supervisor in new podman sandboxes. Refused unless the
    /// instance is in dev mode (loopback bind and external URL, podman backend).
    pub dev_supervisor_binary: Option<String>,
    pub k8s: K8sSection,
}
impl Default for RunnerSection {
    fn default() -> Self {
        Self {
            api_url: None,
            ops_bind: None,
            backend: None,
            image: concat!("aurora-sandbox:", env!("CARGO_PKG_VERSION")).into(),
            image_harnesses: Vec::new(),
            advertised_host: None,
            max_sandboxes: 4,
            held_awake_ceiling: None,
            sandbox_memory_mib: 2048,
            sandbox_cpus: 1,
            sandbox_pids: 512,
            home_gib: 10,
            reconcile_interval_secs: 60,
            orphan_grace_secs: 3600,
            orphan_reap: "reap".into(),
            capacity_wait_secs: 300,
            podman_socket: None,
            sandbox_network: "aurora-sandbox".into(),
            sandbox_hosts: BTreeMap::new(),
            dev_allow_external_network: false,
            dev_supervisor_binary: None,
            k8s: K8sSection::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct K8sSection {
    /// Sandbox namespace (default `aurora-sandboxes`).
    pub namespace: Option<String>,
    /// Namespace of the core services (gateway/runner Services are looked up here).
    /// Default: the runner pod's own namespace (service-account namespace file), else `default`.
    pub core_namespace: Option<String>,
    pub storage_class: Option<String>,
    pub runtime_class: Option<String>,
    pub node_selector: BTreeMap<String, String>,
    pub tolerations: Vec<toml::Table>,
    pub gateway_service: Option<String>,
    pub runner_service: Option<String>,
    /// Names of `kubernetes.io/dockerconfigjson` Secrets in the sandbox namespace, set as the
    /// sandbox Pods' `imagePullSecrets` (private sandbox image registry). Never read or created
    /// by the runner. Each must be a DNS-1123 subdomain name. Default: none.
    pub image_pull_secrets: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TlsSection {
    pub extra_ca_bundle: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetentionSection {
    pub request_log_days: u32,
    /// (phase 2, RFE-010) Instance maximum of an org's conversation retention window, 7..=3650
    /// days. `None` = no ceiling. The instance can only tighten what orgs choose.
    pub conversation_days_max: Option<u32>,
}
impl Default for RetentionSection {
    fn default() -> Self {
        Self {
            request_log_days: 30,
            conversation_days_max: None,
        }
    }
}

/// `[attachments]` (phase 2, RFE-015): per-org attachment storage limits.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AttachmentsSection {
    /// Quota of an org that set none, MiB (`0` = unlimited).
    pub default_org_quota_mib: u64,
    /// Instance ceiling of an org's quota, MiB (`0` = no ceiling).
    pub org_quota_mib_max: u64,
    /// Instance maximum of an org's attachment retention window, 7..=3650 days; `None` = none.
    pub days_max: Option<u32>,
}
impl Default for AttachmentsSection {
    fn default() -> Self {
        Self {
            default_org_quota_mib: 10 * 1024,
            org_quota_mib_max: 50 * 1024,
            days_max: None,
        }
    }
}

/// `[ratelimit]` (phase 2, RFE-017).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RatelimitSection {
    pub api: ApiRateSection,
    /// (phase 4, P4-CNT, D-119) shared counters.
    pub counters: CountersSection,
}

/// `[ratelimit.counters]` (phase 4, P4-CNT, D-119, SEC-62; docs/15). Enum-like values stay
/// strings and are validated with every error collected.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CountersSection {
    /// `memory` (default) | `postgres` | `redis`.
    pub backend: String,
    /// `shared` | `memory`: whether the API limiter uses the backend.
    pub api: String,
    /// `shared` | `memory`: whether the gateway rule quotas use the backend.
    pub gateway: String,
    pub on_unavailable: OnUnavailableSection,
    /// `COUNTER_TIMEOUT` per store call, 10..=2000 ms.
    pub timeout_ms: u64,
    pub postgres: CountersPostgresSection,
    pub redis: CountersRedisSection,
}
impl Default for CountersSection {
    fn default() -> Self {
        Self {
            backend: "memory".into(),
            api: "shared".into(),
            gateway: "shared".into(),
            on_unavailable: OnUnavailableSection::default(),
            timeout_ms: 50,
            postgres: CountersPostgresSection::default(),
            redis: CountersRedisSection::default(),
        }
    }
}

/// `local` | `closed` per class; there is no `open` (SEC-62).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OnUnavailableSection {
    pub security: String,
    pub management: String,
    pub gateway: String,
}
impl Default for OnUnavailableSection {
    fn default() -> Self {
        Self {
            security: "local".into(),
            management: "local".into(),
            gateway: "local".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CountersPostgresSection {
    /// Dedicated pool size, 1..=32.
    pub pool_size: u32,
}
impl Default for CountersPostgresSection {
    fn default() -> Self {
        Self { pool_size: 4 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CountersRedisSection {
    /// `rediss://host[:port]`; required for the `redis` backend.
    pub address: Option<String>,
    pub allow_plaintext: bool,
    /// Names of the environment variables holding the ACL user / password (never the values).
    pub username_env: Option<String>,
    pub password_env: Option<String>,
    pub ca_file: Option<String>,
    pub db: i64,
    pub key_prefix: String,
}
impl Default for CountersRedisSection {
    fn default() -> Self {
        Self {
            address: None,
            allow_plaintext: false,
            username_env: None,
            password_env: None,
            ca_file: None,
            db: 0,
            key_prefix: "aurora:rl:".into(),
        }
    }
}

/// `[ratelimit.api]`: management-API token buckets (docs/05 §Conventions).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApiRateSection {
    pub enabled: bool,
    /// Scales every bucket (0.1-100).
    pub multiplier: f64,
    /// Per-organization bucket refill per minute (burst = one fifth of it).
    pub org_per_minute: u32,
    /// Per-class overrides keyed by class name.
    pub classes: BTreeMap<String, RateClassOverride>,
}
impl Default for ApiRateSection {
    fn default() -> Self {
        Self {
            enabled: true,
            multiplier: 1.0,
            org_per_minute: 3000,
            classes: BTreeMap::new(),
        }
    }
}

/// `{ per_minute | per_hour, burst }`; omitted fields keep the class default.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RateClassOverride {
    pub per_minute: Option<u32>,
    pub per_hour: Option<u32>,
    pub burst: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetrySection {
    pub otlp_endpoint: Option<String>,
    pub log_level: String,
}
impl Default for TelemetrySection {
    fn default() -> Self {
        Self {
            otlp_endpoint: None,
            log_level: "info".into(),
        }
    }
}
