//! Application state and router assembly.

use crate::domains::auth::mail::Mailer;
use crate::http::auth::AuthCache;
use crate::http::client_ip::{self, Cidr};
use crate::http::csrf::origin_of;
use crate::http::ratelimit::RateLimiter;
use crate::{domains, http as web, openapi};
use aurora_config::ApiConfig;
use aurora_db::PgPool;
use axum::Router;
use axum::middleware;
use std::sync::Arc;
use utoipa_axum::router::OpenApiRouter;

/// Derived network/security policy (parsed once from config).
#[derive(Debug)]
pub struct NetPolicy {
    pub trusted_proxies: Vec<Cidr>,
    /// Normalised origins accepted on cookie-authenticated unsafe requests.
    pub allowed_origins: Vec<String>,
    pub cookie_secure: bool,
}

impl NetPolicy {
    pub fn from_config(cfg: &ApiConfig) -> Self {
        let trusted_proxies = cfg
            .server
            .trusted_proxies
            .iter()
            .filter_map(|s| Cidr::parse(s))
            .collect();
        let allowed_origins = std::iter::once(cfg.external_url.as_str())
            .chain(cfg.auth.trusted_origins.iter().map(String::as_str))
            .filter_map(origin_of)
            .collect();
        Self {
            trusted_proxies,
            allowed_origins,
            cookie_secure: cfg.external_url.starts_with("https://"),
        }
    }
}

/// Cheap to clone (all `Arc`s / pool handle).
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<ApiConfig>,
    pub rate_limiter: Arc<RateLimiter>,
    pub session_cache: Arc<AuthCache>,
    pub net: Arc<NetPolicy>,
    pub mailer: Arc<dyn Mailer>,
    /// Secret value store registry (built from `[secrets]`).
    pub vault: Arc<aurora_secrets::SecretVault>,
    /// Core LLM provider registry (built from `[llm]`).
    pub llm: Arc<aurora_llm::ProviderRegistry>,
    /// Instance governance ceiling (`[governance]` over the registry).
    pub gov_instance: Arc<aurora_llm::governance::Instance>,
    /// HTTP client for provider key validation / origin checks.
    pub llm_http: reqwest::Client,
    /// Shared `aurora_approval` fan-out (SSE, long-poll).
    pub approval_bus: Arc<crate::domains::approvals::bus::ApprovalBus>,
    /// Shared `aurora_conversation`/`aurora_delta` fan-out (transcript SSE) and delta coalescer.
    pub conversation_bus: Arc<crate::domains::conversations::bus::ConversationBus>,
    /// Shared `aurora_ops`/`aurora_op_result`/`aurora_inbox` fan-out (connector plane).
    pub connector_bus: Arc<crate::domains::connectors::bus::ConnectorBus>,
    /// HKDF-derived keys (OIDC state, ...).
    pub derived: Arc<aurora_crypto::DerivedKeys>,
    /// OIDC discovery cache and HTTP client.
    pub oidc: Arc<crate::domains::oidc::OidcClients>,
    /// Connect-flow outbound clients (guarded HTTP, hook host).
    pub connect: Arc<crate::domains::connect::Env>,
    /// Card requests from inbox processing (BE-073).
    pub card_requests: Arc<dyn crate::domains::channels::card_requests::CardRequests>,
    /// SSE keepalive interval.
    pub sse_keepalive: std::time::Duration,
    /// How often open SSE streams re-validate their principal (besides revocation NOTIFYs).
    pub stream_recheck: std::time::Duration,
}

impl AppState {
    /// Build the state; fails on invalid `[secrets]` / `[llm]` configuration.
    pub fn try_new(pool: PgPool, config: ApiConfig) -> Result<Self, String> {
        let net = Arc::new(NetPolicy::from_config(&config));
        let mailer = crate::domains::auth::mail::from_config(&config);
        let keys = config
            .keyring
            .keys
            .iter()
            .map(|k| {
                format!(
                    "{}:{}",
                    k.id,
                    base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        k.key.expose()
                    )
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let keyring = Arc::new(
            aurora_crypto::Keyring::parse(&keys, &config.keyring.active)
                .map_err(|e| format!("keyring: {e}"))?,
        );
        let vault = Arc::new(aurora_secrets::SecretVault::new(
            aurora_secrets::StoreRegistry::from_config(&config.secrets, keyring)
                .map_err(|e| format!("secrets: {e}"))?,
        ));
        let llm = Arc::new(
            aurora_llm::ProviderRegistry::from_config(&config.llm)
                .map_err(|e| format!("llm: {e}"))?,
        );
        let gov_instance = Arc::new(domains::secrets::gov::instance_from_config(
            &config.governance,
            &llm,
        ));
        let llm_http = reqwest::Client::builder()
            .timeout(aurora_core::limits::LLM_ORIGIN_CHECK_TIMEOUT)
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let derived = Arc::new(
            aurora_crypto::DerivedKeys::new(aurora_core::secret::Secret::new(
                *config.signing_key.expose(),
            ))
            .map_err(|e| format!("derived keys: {e}"))?,
        );
        let oidc = Arc::new(crate::domains::oidc::OidcClients::new()?);
        let connect = Arc::new(crate::domains::connect::Env::new(
            &config.hookhost_url,
            config.internal_secret.expose(),
        )?);
        let card_requests: Arc<dyn crate::domains::channels::card_requests::CardRequests> =
            Arc::new(crate::domains::channels::cards::requests::CardsImpl::new(
                derived.clone(),
                &config.external_url,
            ));
        Ok(Self {
            pool,
            config: Arc::new(config),
            rate_limiter: Arc::new(RateLimiter::new()),
            session_cache: Arc::new(AuthCache::default()),
            net,
            mailer,
            vault,
            llm,
            gov_instance,
            llm_http,
            approval_bus: Arc::default(),
            conversation_bus: Arc::default(),
            connector_bus: Arc::default(),
            derived,
            oidc,
            connect,
            card_requests,
            sse_keepalive: aurora_core::limits::SSE_KEEPALIVE,
            stream_recheck: crate::http::auth::STREAM_RECHECK,
        })
    }

    /// Like [`Self::try_new`]; panics on invalid configuration (tests, fixed configs).
    #[allow(clippy::expect_used)]
    pub fn new(pool: PgPool, config: ApiConfig) -> Self {
        Self::try_new(pool, config).expect("valid secrets/llm configuration")
    }

    /// Replace the secret vault (tests: preloaded memory store).
    #[must_use]
    pub fn with_vault(mut self, vault: Arc<aurora_secrets::SecretVault>) -> Self {
        self.vault = vault;
        self
    }

    /// Replace the provider registry and rebuild the governance instance (tests).
    #[must_use]
    pub fn with_llm(mut self, llm: Arc<aurora_llm::ProviderRegistry>) -> Self {
        self.gov_instance = Arc::new(domains::secrets::gov::instance_from_config(
            &self.config.governance,
            &llm,
        ));
        self.llm = llm;
        self
    }

    /// Override the SSE keepalive interval (tests).
    #[must_use]
    pub fn with_sse_keepalive(mut self, d: std::time::Duration) -> Self {
        self.sse_keepalive = d;
        self
    }

    /// Override the SSE principal re-check interval (tests).
    #[must_use]
    pub fn with_stream_recheck(mut self, d: std::time::Duration) -> Self {
        self.stream_recheck = d;
        self
    }

    /// Redirect guarded connect-flow requests to a test server (tests only).
    #[must_use]
    pub fn with_connect_override(mut self, origin: &str) -> Self {
        if let Ok(env) = crate::domains::connect::Env::new(
            &self.config.hookhost_url,
            self.config.internal_secret.expose(),
        ) {
            let mut env = env;
            env.override_origin = reqwest::Url::parse(origin).ok();
            self.connect = Arc::new(env);
        }
        self
    }

    /// Replace the mailer (tests).
    #[must_use]
    pub fn with_mailer(mut self, mailer: Arc<dyn Mailer>) -> Self {
        self.mailer = mailer;
        self
    }
}

/// All `/v1` domain routers merged. Add new domains here.
pub fn v1_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(domains::instance::router())
        .merge(domains::auth::router())
        .merge(domains::tenancy::router())
        .merge(domains::agents::router())
        .merge(domains::approvals::router())
        .merge(domains::activity::router())
        .merge(domains::secrets::router())
        .merge(domains::grants::router())
        .merge(domains::policy::router())
        .merge(domains::governance::router())
        .merge(domains::me::router())
        .merge(domains::admin::router())
        .merge(domains::runners_admin::router())
        .merge(domains::runner_plane::router())
        .merge(domains::plugins::router())
        .merge(domains::apps::router())
        .merge(domains::connections::router())
        .merge(domains::conversations::router())
        .merge(domains::attachments::router())
        .merge(domains::hosted::router())
        .merge(domains::memory::router())
        .merge(domains::skills::router())
        .merge(domains::webhooks::router())
        .merge(domains::schedules::router())
        .merge(domains::action_approvals::router())
        .merge(domains::agent_links::router())
        .merge(domains::peer_tasks::router())
        .merge(domains::connect::router())
        .merge(domains::oauth_clients::router())
        .merge(domains::availability::router())
        .merge(domains::audit::router())
        .merge(domains::keys::router())
        .merge(domains::invitations::router())
        .merge(domains::cli_login::router())
        .merge(domains::oidc::router())
        .merge(domains::connectors::router())
        .merge(domains::channels::router())
}

/// Public API router (without ops endpoints) and its OpenAPI document.
pub fn api_router() -> (Router<AppState>, utoipa::openapi::OpenApi) {
    let (router, doc) = OpenApiRouter::with_openapi(openapi::base())
        .nest("/v1", v1_router())
        .split_for_parts();
    let router = router.route(
        "/v1/openapi.json",
        axum::routing::get(openapi::serve_document),
    );
    (router, doc)
}

/// Cross-cutting middleware: client IP (outer) then CSRF.
pub fn with_middleware(router: Router<AppState>, state: &AppState) -> Router<AppState> {
    router
        .layer(middleware::from_fn_with_state(
            state.clone(),
            web::csrf::middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            client_ip::middleware,
        ))
}

/// Full public router. `fallback` serves anything unmatched (ops endpoints, 404 envelope).
pub fn router(state: AppState, fallback: Router) -> Router {
    let (api, _) = api_router();
    with_middleware(api, &state)
        .method_not_allowed_fallback(web::error::method_not_allowed_fallback)
        .fallback_service(fallback)
        .with_state(state)
}
