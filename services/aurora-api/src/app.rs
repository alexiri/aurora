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
use axum::response::IntoResponse;
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
    /// Management-API limiter (phase 2, RFE-017).
    pub api_limiter: Arc<web::apilimit::Limiter>,
    /// (phase 4, P4-CNT, D-119) shared rate-limit counters (all classes local under `memory`).
    pub counters: Arc<web::counters::ApiCounters>,
    pub session_cache: Arc<AuthCache>,
    pub net: Arc<NetPolicy>,
    /// The mailer; unless a test replaced it, a [`LiveMailer`](domains::settings::live::LiveMailer)
    /// whose transport follows the `email` settings (SET-4).
    pub mailer: Arc<dyn Mailer>,
    /// The live mailer behind `mailer` (the settings swap writes it).
    pub mail_live: Arc<domains::settings::live::LiveMailer>,
    /// Sign-in settings in force, replaced when the `sign_in` or `email` section changes (SET-4);
    /// read them with [`AppState::auth`].
    pub auth_live: Arc<arc_swap::ArcSwap<domains::settings::live::LiveAuth>>,
    /// Secret value store registry (built from `[secrets]`).
    pub vault: Arc<aurora_secrets::SecretVault>,
    /// Core LLM provider registry (built from `[llm]`), replaced live when the `llm` settings
    /// change (SET-3); read it with [`AppState::llm`].
    pub llm_live: Arc<arc_swap::ArcSwap<aurora_llm::ProviderRegistry>>,
    /// Instance governance ceiling (`governance` settings over the registry), replaced live when
    /// the `governance` section changes (SET-2); read it with [`AppState::gov_instance`].
    pub gov_live: Arc<arc_swap::ArcSwap<aurora_llm::governance::Instance>>,
    /// HTTP client for provider key validation / origin checks.
    pub llm_http: reqwest::Client,
    /// Shared `aurora_approval` fan-out (SSE, long-poll).
    pub approval_bus: Arc<crate::domains::approvals::bus::ApprovalBus>,
    /// (phase 2, RFE-011a) Open approvals streams / long-polls of this replica (watchers).
    pub approval_watch: Arc<crate::domains::approvals::notify::WatchRegistry>,
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
    /// (phase 4, P4-GSL) git skill library: the gateway client and the browse cache.
    pub library: Arc<crate::domains::skill_libraries::LibraryEnv>,
    /// (phase 5, SET-1, D-236) Instance settings: the deployment configuration's presence and the
    /// snapshot every replica serves from. Nothing reads it yet except the Admin › Settings routes.
    pub settings: Arc<domains::settings::handle::SettingsHandle>,
    /// (phase 5, FIX-146 Part D, C-732 (6)) Browser span ingestion: `None` without
    /// `telemetry.otlp_endpoint`, then `POST /v1/telemetry/traces` answers 404.
    pub browser_spans: Option<Arc<domains::telemetry::BrowserSpans>>,
    /// SSE keepalive interval.
    pub sse_keepalive: std::time::Duration,
    /// How often open SSE streams re-validate their principal (besides revocation NOTIFYs).
    pub stream_recheck: std::time::Duration,
}

/// The encryption keyring of the configuration.
pub(crate) fn build_keyring(config: &ApiConfig) -> Result<Arc<aurora_crypto::Keyring>, String> {
    let keys = config
        .keyring
        .keys
        .iter()
        .map(|k| {
            format!(
                "{}:{}",
                k.id,
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, k.key.expose())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    aurora_crypto::Keyring::parse(&keys, &config.keyring.active)
        .map(Arc::new)
        .map_err(|e| format!("keyring: {e}"))
}

impl AppState {
    /// Build the state; fails on invalid `[secrets]` / `[llm]` configuration.
    pub fn try_new(pool: PgPool, config: ApiConfig) -> Result<Self, String> {
        let net = Arc::new(NetPolicy::from_config(&config));
        let mail_live = Arc::new(domains::settings::live::LiveMailer::new(
            crate::domains::auth::mail::from_config(&config),
        ));
        let mailer: Arc<dyn Mailer> = mail_live.clone();
        let auth_live = Arc::new(arc_swap::ArcSwap::from_pointee(
            domains::settings::live::LiveAuth::from_config(&config),
        ));
        let keyring = build_keyring(&config)?;
        let vault = Arc::new(aurora_secrets::SecretVault::new(
            aurora_secrets::StoreRegistry::from_config_with_tls(
                &config.secrets,
                keyring,
                config.tls.extra_ca_bundle.as_deref(),
            )
            .map_err(|e| format!("secrets: {e}"))?,
        ));
        let llm = Arc::new(
            aurora_llm::ProviderRegistry::from_config(&config.llm)
                .map_err(|e| format!("llm: {e}"))?,
        );
        let gov_live = Arc::new(arc_swap::ArcSwap::from_pointee(
            domains::secrets::gov::instance_from_config(&config.governance, &llm),
        ));
        let llm_http = aurora_secrets::tls::with_extra_roots(
            reqwest::Client::builder().timeout(aurora_core::limits::LLM_ORIGIN_CHECK_TIMEOUT),
            config.tls.extra_ca_bundle.as_deref(),
        )?
        .build()
        .map_err(|e| format!("http client: {e}"))?;
        let derived = Arc::new(
            aurora_crypto::DerivedKeys::from_ring(&config.signing_ring)
                .map_err(|e| format!("derived keys: {e}"))?,
        );
        let oidc = Arc::new(crate::domains::oidc::OidcClients::new(&config)?);
        let connect = Arc::new(crate::domains::connect::Env::new(
            &config.hookhost_url,
            &config.internal_ring,
            &config.gateway_allowed_private,
            config.tls.extra_ca_bundle.as_deref(),
        )?);
        let card_requests: Arc<dyn crate::domains::channels::card_requests::CardRequests> =
            Arc::new(crate::domains::channels::cards::requests::CardsImpl::new(
                derived.clone(),
                &config.external_url,
            ));
        let api_limiter = Arc::new(web::apilimit::Limiter::new(
            web::apilimit::Limits::from_config(&config.ratelimit.api),
        ));
        let conversation_bus = Arc::new(
            crate::domains::conversations::bus::ConversationBus::from_config(
                &config.events,
                &pool,
            )?,
        );
        let counters = Arc::new(web::counters::ApiCounters::build(&config.counters, &pool)?);
        let library = Arc::new(crate::domains::skill_libraries::LibraryEnv::new(Arc::new(
            crate::domains::skill_libraries::gateway::HttpGateway::new(
                &config.gateway_internal_url,
                &config.internal_ring,
                config.tls.extra_ca_bundle.as_deref(),
            )?,
        )));
        let browser_spans = domains::telemetry::BrowserSpans::from_config(&config.telemetry)
            .map_err(|e| format!("telemetry.otlp_endpoint: {e}"))?
            .map(Arc::new);
        let state = Self {
            pool,
            browser_spans,
            config: Arc::new(config),
            rate_limiter: Arc::new(RateLimiter::new()),
            api_limiter,
            counters,
            session_cache: Arc::new(AuthCache::default()),
            net,
            mailer,
            mail_live,
            auth_live,
            vault,
            llm_live: Arc::new(arc_swap::ArcSwap::new(llm)),
            gov_live,
            llm_http,
            approval_bus: Arc::default(),
            approval_watch: Arc::default(),
            conversation_bus,
            connector_bus: Arc::default(),
            derived,
            oidc,
            connect,
            card_requests,
            library,
            settings: Arc::new(domains::settings::handle::SettingsHandle::new(None)),
            sse_keepalive: aurora_core::limits::SSE_KEEPALIVE,
            stream_recheck: crate::http::auth::STREAM_RECHECK,
        };
        state.install_live_secrets();
        state.install_live_governance();
        state.install_live_sign_in();
        Ok(state)
    }

    /// The sign-in settings now in force (SET-4): sign-up mode, session lifetimes, providers. One
    /// load per request path, so a swap mid-request never mixes two sets.
    #[must_use]
    pub fn auth(&self) -> Arc<domains::settings::live::LiveAuth> {
        self.auth_live.load_full()
    }

    /// Replace the sign-in settings and the mailer whenever a `sign_in` or `email` setting changes
    /// (SET-4). The mailer is rebuilt in the background (its password is opened from the store);
    /// mail in flight finishes with the old transport.
    fn install_live_sign_in(&self) {
        let auth_cell = self.auth_live.clone();
        let mail = self.mail_live.clone();
        let vault = self.vault.clone();
        let sessions = self.session_cache.clone();
        self.settings.on_swap(Arc::new(move |old, new| {
            let changed = |prefix: &str| {
                new.effective
                    .keys
                    .keys()
                    .chain(old.effective.keys.keys())
                    .filter(|k| k.starts_with(prefix))
                    .any(|k| {
                        old.effective.keys.get(k).map(|e| &e.value)
                            != new.effective.keys.get(k).map(|e| &e.value)
                    })
            };
            let auth_changed = changed("auth.");
            let mail_changed = changed("email.")
                || old.effective.raw.email.smtp_password_sealed
                    != new.effective.raw.email.smtp_password_sealed;
            if !auth_changed && !mail_changed {
                return;
            }
            let cfg = match new.effective.raw.api_config() {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "sign-in settings did not resolve; keeping the previous ones");
                    return;
                }
            };
            if auth_changed {
                auth_cell.store(Arc::new(domains::settings::live::LiveAuth::from_config(
                    &cfg,
                )));
                // Cached sessions were judged under the old lifetimes: a shorter one must apply at
                // the next request (and open streams re-check), so the cache is dropped.
                if changed("auth.session_") {
                    sessions.clear();
                }
            }
            if mail_changed {
                let (vault, mail, email) = (vault.clone(), mail.clone(), cfg.email.clone());
                tokio::spawn(async move {
                    mail.set(domains::settings::live::build_mailer(&vault, &email).await);
                });
            }
        }));
    }

    /// The instance governance ceiling now in force (SET-2): one load per request path, so a
    /// swap mid-request never mixes two ceilings.
    #[must_use]
    pub fn gov_instance(&self) -> Arc<aurora_llm::governance::Instance> {
        self.gov_live.load_full()
    }

    /// The LLM provider registry now in force (SET-3): one load per request path, so a swap
    /// mid-request never mixes two tables.
    #[must_use]
    pub fn llm(&self) -> Arc<aurora_llm::ProviderRegistry> {
        self.llm_live.load_full()
    }

    /// Rebuild the provider registry when an `llm.*` setting changes (SET-3) and the instance
    /// ceiling when a `governance.*`, `byo.*` or `llm.*` setting changes (SET-2, `08 §11.1`: the
    /// ceiling carries the endpoints), in that order, then run the `08 §11.2` re-evaluation of every
    /// organization (it also covers secrets on a removed endpoint and model overrides that
    /// the narrowed allow-lists no longer permit).
    fn install_live_governance(&self) {
        let cell = self.gov_live.clone();
        let llm_cell = self.llm_live.clone();
        let pool = self.pool.clone();
        self.settings.on_swap(Arc::new(move |old, new| {
            let changed = |prefixes: &[&str]| {
                new.effective
                    .keys
                    .keys()
                    .chain(old.effective.keys.keys())
                    .filter(|k| prefixes.iter().any(|p| k.starts_with(p)))
                    .any(|k| {
                        old.effective.keys.get(k).map(|e| &e.value)
                            != new.effective.keys.get(k).map(|e| &e.value)
                    })
            };
            let llm_changed = changed(&["llm."]);
            if !llm_changed && !changed(&["governance.", "byo."]) {
                return;
            }
            if llm_changed {
                match aurora_llm::ProviderRegistry::from_config(&new.effective.raw.llm) {
                    Ok(r) => llm_cell.store(Arc::new(r)),
                    Err(e) => tracing::error!(error = %e, "llm settings did not resolve; keeping the previous provider registry"),
                }
            }
            match new.effective.raw.governance_config() {
                Ok(g) => {
                    let inst = Arc::new(domains::secrets::gov::instance_from_config(
                        &g,
                        &llm_cell.load(),
                    ));
                    cell.store(inst.clone());
                    // The `08 §11.2` re-evaluation of every organization (idempotent job).
                    if let Ok(rt) = tokio::runtime::Handle::try_current() {
                        let pool = pool.clone();
                        rt.spawn(async move {
                            match domains::governance::reeval::run(&pool, &inst).await {
                                Ok(Some(o)) => tracing::info!(?o, "governance re-evaluation done"),
                                Ok(None) => {}
                                Err(e) => tracing::warn!(error = %e.message(), "governance re-evaluation failed; it runs again on the next change or start"),
                            }
                        });
                    }
                }
                Err(e) => tracing::error!(error = %e, "governance settings did not resolve; keeping the previous instance ceiling"),
            }
        }));
    }

    /// Like [`Self::try_new`]; panics on invalid configuration (tests, fixed configs).
    #[allow(clippy::expect_used)]
    pub fn new(pool: PgPool, config: ApiConfig) -> Self {
        Self::try_new(pool, config).expect("valid secrets/llm configuration")
    }

    /// Attach the deployment configuration the settings framework locks against (D-236(b)).
    /// `main` always calls this; states built without it (tests, tools) lock nothing.
    #[must_use]
    pub fn with_config_source(mut self, source: aurora_config::ConfigSource) -> Self {
        self.settings = Arc::new(domains::settings::handle::SettingsHandle::new(Some(source)));
        self.vault
            .set_allowed_private(self.config.gateway_allowed_private.clone());
        self.install_live_secrets();
        self.install_live_governance();
        self.install_live_sign_in();
        // Sign-in as the configuration defines it (in production the same as the boot values).
        if let Ok(c) = self.settings.snapshot().effective.raw.api_config() {
            self.auth_live
                .store(Arc::new(domains::settings::live::LiveAuth::from_config(&c)));
        }
        // The merged view (the configuration's stores, nothing stored yet) is the first registry.
        let snap = self.settings.snapshot();
        let configured = snap
            .effective
            .keys
            .get("secrets.stores")
            .and_then(|k| k.entries.as_ref())
            .is_some_and(|e| !e.is_empty());
        if configured && let Ok(keyring) = build_keyring(&self.config) {
            self.vault.rebuild_from_effective(
                &snap.effective,
                keyring,
                self.config.tls.extra_ca_bundle.as_deref(),
            );
        }
        self
    }

    /// Rebuild the vault's store registry whenever the `secrets` settings change (FIX-124): the
    /// API and the gateway pick a dashboard-defined store or default up without a restart.
    fn install_live_secrets(&self) {
        let vault = self.vault.clone();
        let ca = self.config.tls.extra_ca_bundle.clone();
        let Ok(keyring) = build_keyring(&self.config) else {
            return;
        };
        self.settings.on_swap(Arc::new(move |old, new| {
            const KEYS: [&str; 2] = ["secrets.stores", "secrets.default_store"];
            let differs = KEYS.iter().any(|k| {
                old.effective.keys.get(*k).map(|e| &e.value)
                    != new.effective.keys.get(*k).map(|e| &e.value)
            });
            if differs {
                vault.rebuild_from_effective(&new.effective, keyring.clone(), ca.as_deref());
            }
        }));
    }

    /// Replace the shared counters (tests: a shared in-process store, fault injection).
    #[must_use]
    pub fn with_counters(mut self, counters: web::counters::ApiCounters) -> Self {
        self.counters = Arc::new(counters);
        self
    }

    /// Replace the secret vault (tests: preloaded memory store).
    #[must_use]
    pub fn with_vault(mut self, vault: Arc<aurora_secrets::SecretVault>) -> Self {
        self.vault = vault;
        self.install_live_secrets();
        self.install_live_sign_in();
        self
    }

    /// Replace the provider registry and rebuild the governance instance (tests).
    #[must_use]
    pub fn with_llm(mut self, llm: Arc<aurora_llm::ProviderRegistry>) -> Self {
        // The settings in force (not only the configuration) keep applying.
        let g = self
            .settings
            .snapshot()
            .effective
            .raw
            .governance_config()
            .unwrap_or_else(|_| self.config.governance.clone());
        self.gov_live = Arc::new(arc_swap::ArcSwap::from_pointee(
            domains::secrets::gov::instance_from_config(&g, &llm),
        ));
        self.llm_live = Arc::new(arc_swap::ArcSwap::new(llm));
        self.install_live_governance();
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
            &self.config.internal_ring,
            &self.config.gateway_allowed_private,
            self.config.tls.extra_ca_bundle.as_deref(),
        ) {
            let mut env = env;
            env.override_origin = reqwest::Url::parse(origin).ok();
            self.connect = Arc::new(env);
        }
        self
    }

    /// Shorten the registration probe's timeout (tests).
    #[must_use]
    pub fn with_probe_timeout(self, d: std::time::Duration) -> Self {
        self.connect.set_probe_timeout(d);
        self
    }

    /// Replace the library gateway client (tests: a fake that reads a fake git host).
    #[must_use]
    pub fn with_library_gateway(
        mut self,
        gw: Arc<dyn crate::domains::skill_libraries::gateway::LibraryGateway>,
    ) -> Self {
        self.library = Arc::new(crate::domains::skill_libraries::LibraryEnv::new(gw));
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
        .merge(domains::freeze::router())
        .merge(domains::approvals::router())
        .merge(domains::activity::router())
        .merge(domains::secrets::router())
        .merge(domains::grants::router())
        .merge(domains::policy::router())
        .merge(domains::governance::router())
        .merge(domains::me::router())
        .merge(domains::admin::router())
        .merge(domains::onboarding::router())
        .merge(domains::placement::router())
        .merge(domains::settings::router())
        .merge(domains::previews::router())
        .merge(domains::terminal::router())
        .merge(domains::files::router())
        .merge(domains::telemetry::router())
        .merge(domains::relay::router())
        .merge(domains::groups::router())
        // (P4-GRP-2, FIX-081) The SCIM management routes are ordinary owner `/v1` routes and
        // OpenAPI paths (C-471/C-472); only the `/scim/v2` plane stays outside the document.
        .merge(domains::scim::management_router())
        .merge(domains::runners_admin::router())
        .merge(domains::runner_plane::router())
        .merge(domains::plugins::router())
        .merge(domains::addon_sources::router())
        .merge(domains::apps::router())
        .merge(domains::connections::router())
        .merge(domains::conversations::router())
        .merge(domains::attachments::router())
        .merge(domains::hosted::router())
        .merge(domains::memory::router())
        .merge(domains::skills::router())
        .merge(domains::skill_libraries::router())
        .merge(domains::webhooks::router())
        .merge(domains::schedules::router())
        .merge(domains::automations::router())
        .merge(domains::action_approvals::router())
        .merge(domains::agent_links::router())
        .merge(domains::peer_tasks::router())
        .merge(domains::connect::router())
        .merge(domains::oauth_clients::router())
        .merge(domains::app_instances::router())
        .merge(domains::availability::router())
        .merge(domains::audit::router())
        .merge(domains::keys::router())
        .merge(domains::invitations::router())
        .merge(domains::cli_login::router())
        .merge(domains::oidc::router())
        .merge(domains::connectors::router())
        .merge(domains::channels::router())
}

/// Public API router (without ops endpoints) and its OpenAPI document. The SCIM plane
/// (`/scim/v2`, P4-GRP-2) is nested here — a plain axum router outside the OpenAPI
/// document — and answers **404 for the whole surface while the instance switch
/// `scim.enabled` is off** (checked by a middleware before auth). `state` is `None` only
/// for the OpenAPI document build (a layer never changes the document; the nested SCIM
/// rate-limit middleware needs the state at router-build time).
pub fn api_router(state: Option<&AppState>) -> (Router<AppState>, utoipa::openapi::OpenApi) {
    let (router, doc) = OpenApiRouter::with_openapi(openapi::base())
        .nest("/v1", v1_router())
        .split_for_parts();
    let router = router.route(
        "/v1/openapi.json",
        axum::routing::get(openapi::serve_document),
    );
    // The SCIM plane never appears in the OpenAPI document and its nested rate-limit
    // middleware needs the real state at build time (`from_fn_with_state`), so it is only
    // wired when actually serving (`Some`); the document build passes `None`.
    let router = match state {
        Some(s) => {
            let scim_enabled = s.config.scim.enabled;
            router
                .nest("/scim/v2", domains::scim::router(s))
                .layer(middleware::from_fn(move |req, next| {
                    scim_gate(scim_enabled, req, next)
                }))
        }
        None => router,
    };
    (router, doc)
}

/// (P4-GRP-2, SEC-72) While the instance switch is off, the whole `/scim/v2` surface is 404
/// before anything else runs (no auth oracle); the management routes stay reachable so the
/// owners see `available: false`.
async fn scim_gate(
    scim_enabled: bool,
    req: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    if !scim_enabled && req.uri().path().starts_with("/scim/v2") {
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": {"code": "not_found", "message": "Not found."}
            })),
        )
            .into_response();
    }
    next.run(req).await
}

/// Cross-cutting middleware: client IP (outer), CSRF, then the API rate limiter (RFE-017).
pub fn with_middleware(router: Router<AppState>, state: &AppState) -> Router<AppState> {
    router
        .layer(middleware::from_fn_with_state(
            state.clone(),
            web::apilimit::middleware,
        ))
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
    let (api, _) = api_router(Some(&state));
    with_middleware(api, &state)
        .method_not_allowed_fallback(web::error::method_not_allowed_fallback)
        .fallback_service(fallback)
        .with_state(state)
}
