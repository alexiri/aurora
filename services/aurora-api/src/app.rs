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
}

impl AppState {
    pub fn new(pool: PgPool, config: ApiConfig) -> Self {
        let net = Arc::new(NetPolicy::from_config(&config));
        let mailer = crate::domains::auth::mail::from_config(&config);
        Self {
            pool,
            config: Arc::new(config),
            rate_limiter: Arc::new(RateLimiter::new()),
            session_cache: Arc::new(AuthCache::default()),
            net,
            mailer,
        }
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
