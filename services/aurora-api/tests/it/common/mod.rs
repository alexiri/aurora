#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Test support: app over a fresh migrated database, SQL seeding helpers, request helper.

use aurora_api::app::{self, AppState};
use aurora_api::http::auth::{Caller, InstanceAdmin, SessionCaller, require_own};
use aurora_api::http::json::{Json, Query};
use aurora_api::http::pagination::PageParams;
use aurora_api::http::ratelimit::Quota;
use aurora_core::token::{self, TokenPrefix};
use aurora_testkit::TestDb;
use aurora_testkit::oidc::MockIdp;

/// The whole text exposition of the process-wide recorder (installed on first use).
pub fn metrics_text() -> String {
    use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
    static H: std::sync::OnceLock<PrometheusHandle> = std::sync::OnceLock::new();
    H.get_or_init(|| PrometheusBuilder::new().install_recorder().unwrap())
        .render()
}

/// Value of one counter series (`name` plus its exact label text, e.g.
/// `aurora_scim_requests_total{outcome="ok"}`) in the process-wide Prometheus recorder, 0 when
/// the series does not exist yet. The recorder is global to the `it` binary and installed once
/// here; counters only grow, so tests assert on deltas.
pub fn counter_value(series: &str) -> f64 {
    metrics_text()
        .lines()
        .find(|l| l.starts_with(series) && l[series.len()..].starts_with(' '))
        .and_then(|l| l.split_whitespace().last()?.parse().ok())
        .unwrap_or(0.0)
}
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use base64::Engine;
use http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

pub const ORIGIN: &str = "http://localhost:4100";

pub struct Harness {
    pub db: TestDb,
    pub state: AppState,
    pub app: Router,
    pub mail: std::sync::Arc<aurora_api::domains::auth::mail::MemoryMailer>,
    /// In-process OIDC mock registered as the development provider `dev` (phase 1, RFE-004).
    pub idp: MockIdp,
}

/// A session created through the real OIDC start/callback flow.
pub struct SignedIn {
    pub user_id: Uuid,
    /// `aurora_session=aus_…` (a `Cookie` header value).
    pub cookie: String,
}

pub fn config(mutate: impl FnOnce(&mut aurora_config::ApiConfig)) -> aurora_config::ApiConfig {
    let k = base64::engine::general_purpose::STANDARD.encode([b'a'; 32]);
    let env: Vec<(String, String)> = [
        (
            "AURORA_DATABASE_URL",
            "postgres://x:x@127.0.0.1:1/x".to_owned(),
        ),
        ("AURORA_ENCRYPTION_KEYS", format!("k1:{k}")),
        ("AURORA_ENCRYPTION_ACTIVE_KEY", "k1".to_owned()),
        ("AURORA_SIGNING_KEY", k.clone()),
        ("AURORA_INTERNAL_SECRET", k.clone()),
        ("AURORA_RUNNER_SESSION_KEY", k),
        ("AURORA_RUNNER_TOKEN", "aur_devtoken".to_owned()),
        ("AURORA_CONNECTOR_TOKEN", "aun_devtoken".to_owned()),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_owned(), b))
    .collect();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/aurora.test.toml"
    );
    let mut cfg = aurora_config::load(Some(std::path::Path::new(path)), env.into_iter())
        .unwrap()
        .api_config()
        .unwrap();
    // Suites exercise routes far faster than any real client: scale the RFE-017 buckets up so
    // they never interfere. `api_ratelimit.rs` resets the multiplier to prove the mechanism.
    cfg.ratelimit.api.multiplier = 100.0;
    mutate(&mut cfg);
    cfg
}

impl Harness {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    pub async fn with_config(mutate: impl FnOnce(&mut aurora_config::ApiConfig)) -> Self {
        let db = TestDb::new().await;
        let idp = MockIdp::start().await;
        let mail = aurora_api::domains::auth::mail::MemoryMailer::new();
        let issuer = idp.issuer().to_owned();
        let state = AppState::new(
            db.pool().clone(),
            config(|c| {
                c.auth.dev_idp.issuer = Some(issuer.clone());
                for p in &mut c.auth.oidc_providers {
                    if p.id == aurora_config::DEV_IDP_PROVIDER_ID {
                        p.issuer.clone_from(&issuer);
                    }
                }
                mutate(c);
            }),
        )
        .with_mailer(mail.clone());
        let app = test_router(state.clone());
        let h = Harness {
            db,
            state,
            app,
            mail,
            idp,
        };
        // P4-ONB-1 (D-140): on a fresh `open` instance non-admins wait for the placement. The
        // suites that predate it expect today's per-user organizations, so an `open` harness
        // starts with `personal_org` saved; `placement.rs` removes it with `unsave_placement`.
        if h.state.config.signup_mode == aurora_config::SignupMode::Open {
            sqlx::query("INSERT INTO instance_placement (mode) VALUES ('personal_org')")
                .execute(h.pool())
                .await
                .unwrap();
        }
        h
    }

    /// Back to "nothing saved" (a fresh instance that behaves as before D-140).
    pub async fn unsave_placement(&self) {
        sqlx::query("DELETE FROM instance_placement")
            .execute(self.pool())
            .await
            .unwrap();
        // A fresh instance: the setup gate (FIX-116) is pending until a placement is saved.
        sqlx::query("UPDATE instance_setup SET placement_pending = true")
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// An instance that existed before the setup gate (FIX-116): nothing saved, not pending.
    pub async fn mark_upgraded_instance(&self) {
        sqlx::query("UPDATE instance_setup SET placement_pending = false")
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// Test helper `sign_in(email)` (docs/04 §Development and end-to-end sign-in): drives the
    /// **real** OIDC `start` and `callback` of the development provider `dev` against the
    /// in-process mock IdP (`login_hint = email`, verified e-mail) and returns the session cookie.
    /// Obeys `auth.signup_mode` exactly like a browser would; panics if the sign-in is refused.
    pub async fn sign_in(&self, email: &str) -> SignedIn {
        self.try_sign_in("dev", email)
            .await
            .unwrap_or_else(|e| panic!("sign_in({email}) refused: {e}"))
    }

    /// Like [`Harness::sign_in`] for any provider; `Err` carries the `/auth/login?error=` code.
    pub async fn try_sign_in(&self, provider: &str, email: &str) -> Result<SignedIn, String> {
        self.try_sign_in_hint(provider, Some(email)).await
    }

    /// Sign in with the IdP's *configured claims* (`h.idp.set(..)`), no `login_hint`.
    pub async fn try_sign_in_claims(&self, provider: &str) -> Result<SignedIn, String> {
        self.try_sign_in_hint(provider, None).await
    }

    async fn try_sign_in_hint(
        &self,
        provider: &str,
        email: Option<&str>,
    ) -> Result<SignedIn, String> {
        let (path, flow) = self.oidc_authorize(provider, "", email).await;
        let (st, hd, _) = self
            .send(Method::GET, &path, &[("cookie", &flow)], None)
            .await;
        assert_eq!(st, StatusCode::FOUND, "callback");
        let loc = hd["location"].to_str().unwrap().to_owned();
        let cookie = hd
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with("aurora_session=aus_"))
            .and_then(|c| c.split(';').next())
            .map(str::to_owned)
            .ok_or_else(|| loc.rsplit("error=").next().unwrap_or(&loc).to_owned())?;
        let (s, me) = self.get("/v1/me", &[("cookie", &cookie)]).await;
        assert_eq!(s, StatusCode::OK, "{me}");
        let user_id = me["user"]["id"].as_str().unwrap().parse().unwrap();
        Ok(SignedIn { user_id, cookie })
    }

    /// `start` (with `extra` query, e.g. `&reauth=1`) plus the provider's authorize step.
    /// Returns `(callback path+query, __Host-aurora_oidc cookie pair)`.
    pub async fn oidc_authorize(
        &self,
        provider: &str,
        extra: &str,
        login_hint: Option<&str>,
    ) -> (String, String) {
        self.oidc_authorize_with(provider, extra, login_hint, &[])
            .await
    }

    pub async fn oidc_authorize_with(
        &self,
        provider: &str,
        extra: &str,
        login_hint: Option<&str>,
        headers: &[(&str, &str)],
    ) -> (String, String) {
        let hint = login_hint.map_or(String::new(), |h| {
            format!(
                "&login_hint={}",
                percent_encoding::utf8_percent_encode(h, percent_encoding::NON_ALPHANUMERIC)
            )
        });
        let uri = format!("/v1/auth/oidc/{provider}/start?x=1{hint}{extra}");
        let (st, hd, body) = self.send(Method::GET, &uri, headers, None).await;
        assert_eq!(st, StatusCode::FOUND, "start: {body}");
        let flow = hd
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with("__Host-aurora_oidc="))
            .and_then(|c| c.split(';').next())
            .expect("flow cookie")
            .to_owned();
        let loc = hd["location"].to_str().unwrap().to_owned();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let res = http.get(&loc).send().await.unwrap();
        assert_eq!(res.status().as_u16(), 303, "authorize: {loc}");
        let back = res.headers()["location"].to_str().unwrap().to_owned();
        let i = back
            .find("/v1/auth/oidc/")
            .expect("redirect_uri on external url");
        (back[i..].to_owned(), flow)
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        self.db.pool()
    }

    pub async fn send(
        &self,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut b = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let req = if let Some(body) = body {
            b.header("content-type", "application/json")
                .body(Body::from(body.to_string()))
        } else {
            b.body(Body::empty())
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let (parts, body) = res.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (parts.status, parts.headers, v)
    }

    pub async fn get(&self, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, Value) {
        let (s, _, v) = self.send(Method::GET, uri, headers, None).await;
        (s, v)
    }

    // ---- seeding ----
    pub async fn user(&self, email: &str, admin: bool) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO users (id, email, display_name, is_instance_admin) VALUES ($1,$2,'U',$3)",
        )
        .bind(id)
        .bind(email)
        .bind(admin)
        .execute(self.pool())
        .await
        .unwrap();
        id
    }
    pub async fn disable_user(&self, id: Uuid) {
        sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await
            .unwrap();
    }
    pub async fn org(&self) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,$2,'o',$2)",
        )
        .bind(id)
        .bind(id.simple().to_string())
        .execute(self.pool())
        .await
        .unwrap();
        id
    }
    pub async fn member(&self, org: Uuid, user: Uuid, role: &str, status: &str) {
        sqlx::query("INSERT INTO org_members (org_id, user_id, role, status) VALUES ($1,$2,$3,$4)")
            .bind(org)
            .bind(user)
            .bind(role)
            .bind(status)
            .execute(self.pool())
            .await
            .unwrap();
    }
    pub async fn set_member_status(&self, org: Uuid, user: Uuid, status: &str) {
        sqlx::query("UPDATE org_members SET status=$3 WHERE org_id=$1 AND user_id=$2")
            .bind(org)
            .bind(user)
            .bind(status)
            .execute(self.pool())
            .await
            .unwrap();
    }
    pub async fn workspace(&self, org: Uuid) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,$3,'w')")
            .bind(id)
            .bind(org)
            .bind(id.simple().to_string())
            .execute(self.pool())
            .await
            .unwrap();
        id
    }
    pub async fn bind_ws(&self, ws: Uuid, user: Uuid, role: &str) {
        sqlx::query("INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1,$2,$3)")
            .bind(ws)
            .bind(user)
            .bind(role)
            .execute(self.pool())
            .await
            .unwrap();
    }

    /// Insert a session; offsets are seconds relative to now (negative = past).
    pub async fn session(
        &self,
        user: Uuid,
        last_seen_off: i64,
        expires_off: i64,
        revoked: bool,
    ) -> (Uuid, String) {
        let tok = token::generate(TokenPrefix::Session).expose().clone();
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO sessions (id, user_id, token_hash, last_seen_at, expires_at, revoked_at)
             VALUES ($1,$2,$3, now() + make_interval(secs => $4), now() + make_interval(secs => $5),
                     CASE WHEN $6 THEN now() END)",
        )
        .bind(id)
        .bind(user)
        .bind(&token::hash(&tok)[..])
        .bind(last_seen_off as f64)
        .bind(expires_off as f64)
        .bind(revoked)
        .execute(self.pool())
        .await
        .unwrap();
        (id, tok)
    }
    pub async fn good_session(&self, user: Uuid) -> (Uuid, String) {
        self.session(user, 0, 3600, false).await
    }

    pub async fn api_key(
        &self,
        user: Uuid,
        org: Uuid,
        ws: Option<Uuid>,
        revoked: bool,
    ) -> (Uuid, String) {
        let prefix = if ws.is_some() {
            TokenPrefix::WorkspaceKey
        } else {
            TokenPrefix::OrgKey
        };
        let tok = token::generate(prefix).expose().clone();
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO api_keys (id, org_id, workspace_id, user_id, kind, name, token_hash, display_prefix, revoked_at)
             VALUES ($1,$2,$3,$4,'user','k',$5,$6, CASE WHEN $7 THEN now() END)",
        )
        .bind(id)
        .bind(org)
        .bind(ws)
        .bind(user)
        .bind(&token::hash(&tok)[..])
        .bind(token::display_prefix(&tok))
        .bind(revoked)
        .execute(self.pool())
        .await
        .unwrap();
        (id, tok)
    }
}

pub fn cookie(tok: &str) -> String {
    format!("aurora_session={tok}")
}

pub fn bearer(tok: &str) -> String {
    format!("Bearer {tok}")
}

// ---- test routes exercising the extractors ----

#[derive(serde::Deserialize)]
struct JsonBody {
    #[allow(dead_code)]
    name: String,
}

pub fn test_router(state: AppState) -> Router {
    let (api, _) = app::api_router(Some(&state));
    let t = Router::new()
        .route(
            "/t/whoami",
            get(|c: Caller| async move {
                Json(json!({"user_id": c.user_id, "source": c.audit_source()}))
            }),
        )
        .route(
            "/t/post",
            post(|_c: Caller| async { Json(json!({"ok": true})) }),
        )
        .route(
            "/t/so",
            post(|_c: SessionCaller| async { Json(json!({"ok": true})) }),
        )
        .route(
            "/t/ia",
            get(|_c: InstanceAdmin| async { Json(json!({"ok": true})) }),
        )
        .route(
            "/t/ws/{id}",
            get(
                |c: Caller, State(s): State<AppState>, Path(id): Path<Uuid>| async move {
                    let a = c.workspace(&s, id).await?;
                    Ok::<_, aurora_core::ApiError>(Json(
                        json!({"role": a.effective_ws_role.as_str(), "own": a.can_own()}),
                    ))
                },
            ),
        )
        .route(
            "/t/ws/{id}/own",
            get(
                |c: Caller, State(s): State<AppState>, Path(id): Path<Uuid>| async move {
                    let a = c.workspace(&s, id).await?;
                    require_own(&a)?;
                    Ok::<_, aurora_core::ApiError>(Json(json!({"ok": true})))
                },
            ),
        )
        .route(
            "/t/org/{id}",
            get(
                |c: Caller, State(s): State<AppState>, Path(id): Path<Uuid>| async move {
                    let a = c.org(&s, id).await?;
                    a.require_admin()?;
                    Ok::<_, aurora_core::ApiError>(Json(json!({"role": a.role.as_str()})))
                },
            ),
        )
        .route(
            "/t/json",
            post(|Json(_b): Json<JsonBody>| async { Json(json!({"ok": true})) }),
        )
        .route(
            "/t/page",
            get(|Query(p): Query<PageParams>| async move {
                let p = p.resolve()?;
                Ok::<_, aurora_core::ApiError>(Json(
                    json!({"limit": p.limit, "after": p.after.map(|c| c.id)}),
                ))
            }),
        )
        .route(
            "/t/limited",
            get(|State(s): State<AppState>| async move {
                s.rate_limiter.check_quota(
                    "t",
                    "k",
                    Quota {
                        capacity: 2,
                        per: Duration::from_secs(60),
                    },
                )?;
                Ok::<_, aurora_core::ApiError>(Json(json!({"ok": true})))
            }),
        );
    let router = api
        .merge(t)
        .fallback(aurora_api::http::error::not_found_fallback);
    app::with_middleware(router, &state).with_state(state)
}
