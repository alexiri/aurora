//! Shared rate-limit counters of the API (phase 4, P4-CNT, D-119, SEC-62; docs/05 §Conventions
//! *Shared counters*, docs/15 §Shared rate-limit counters).
//!
//! `memory` (the default) changes nothing: the limiters keep their per-replica state. With a
//! shared backend each limiter first checks its own in-memory bucket (the *local pre-filter*:
//! full configured limit, so a refusal there is final), then takes from the shared store; a store
//! refusal refunds the local token. An unreachable store applies the class policy (`security`,
//! `management`): `local` keeps the local count (today's behaviour), `closed` refuses with 503
//! `counter_unavailable`. There is no fail-open path.

use crate::app::AppState;
use crate::http::apilimit::{Charge, Refusal, Spec};
use crate::http::ratelimit::{Bucket, Quota};
use aurora_config::CountersConfig;
use aurora_core::ApiError;
use aurora_counters::{
    BreakerConfig, BucketTake, ClassCounters, ClassPolicy, CounterStore, Fallback, Guarded, State,
    Verdict, WindowUnit,
};
use aurora_db::PgPool;
use std::sync::Arc;
use std::time::Duration;

/// The API's shared-counter classes. A class is `None` when it stays per replica (`memory`
/// backend, or `ratelimit.counters.api = "memory"`).
pub struct ApiCounters {
    backend: &'static str,
    security: Option<ClassCounters>,
    management: Option<ClassCounters>,
}

impl ApiCounters {
    /// The default: everything per replica.
    pub fn local() -> Self {
        Self {
            backend: "memory",
            security: None,
            management: None,
        }
    }

    /// Build from the validated configuration.
    ///
    /// Each class gets its own store: its own breaker and its own connection budget, so an
    /// unauthenticated flood that saturates the `security` pool never turns authenticated
    /// `management` calls into outages (FIX-087, docs/05 §Shared counters). Nothing is built
    /// (no pool, no client) unless the API limiter is shared.
    pub fn build(cfg: &CountersConfig, pool: &PgPool) -> Result<Self, String> {
        if !cfg.api_shared {
            return Ok(Self::local());
        }
        let class = |name: &'static str, p: ClassPolicy| {
            Ok::<_, String>(
                aurora_db::counters::build_store(cfg, pool)?
                    .map(|store| ClassCounters::new(store, name, p)),
            )
        };
        let security = class("security", cfg.security)?;
        let management = class("management", cfg.management)?;
        if security.is_none() {
            return Ok(Self::local());
        }
        Ok(Self {
            backend: cfg.backend.as_str(),
            security,
            management,
        })
    }

    /// Tests: share `store` with the given policies; each class wraps it in its own breaker.
    pub fn for_store(
        store: Arc<dyn CounterStore>,
        security: ClassPolicy,
        management: ClassPolicy,
        breaker: BreakerConfig,
    ) -> Self {
        let guard = |s: &Arc<dyn CounterStore>| -> Arc<dyn CounterStore> {
            Arc::new(Guarded::new(s.clone(), breaker))
        };
        Self {
            backend: "test",
            security: Some(ClassCounters::new(guard(&store), "security", security)),
            management: Some(ClassCounters::new(guard(&store), "management", management)),
        }
    }

    pub fn security(&self) -> Option<&ClassCounters> {
        self.security.as_ref()
    }

    pub fn management(&self) -> Option<&ClassCounters> {
        self.management.as_ref()
    }

    /// `GET /v1/admin/overview` `counters` (C-402): `local` for `memory`, else `shared` or
    /// `fallback` (any class currently counting locally).
    pub fn overview(&self) -> (&'static str, &'static str) {
        let states = [self.security.as_ref(), self.management.as_ref()];
        let mut seen = false;
        for c in states.into_iter().flatten() {
            seen = true;
            if c.state() == State::Fallback {
                return (self.backend, "fallback");
            }
        }
        (self.backend, if seen { "shared" } else { "local" })
    }
}

fn secs_ceil(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

/// Emission interval of a bucket (`per` seconds for `limit` tokens).
fn interval(per_secs: u32, limit: u32) -> Duration {
    Duration::from_secs_f64(f64::from(per_secs) / f64::from(limit.max(1)))
}

/// Store key of a limiter charge (docs/05: ids and keyed hashes only, never an e-mail or IP).
fn shared_key(s: &AppState, c: &Charge) -> String {
    if c.class == "unauth" {
        // `ip-get:<ip>` / `ip-rejected:<ip>`: keyed hash of the address.
        let (kind, ip) = c.key.split_once(':').unwrap_or(("ip", &c.key));
        return format!("api:{kind}:{}", s.derived.ratelimit_hash(ip));
    }
    match c.scope {
        "org" => format!("api:{}", c.key),
        "class" => {
            let (pid, name) = c.key.rsplit_once('|').unwrap_or((&c.key, ""));
            format!("api:class:{name}:{pid}")
        }
        _ => {
            let pid = c.key.rsplit_once('|').map_or(c.key.as_str(), |(p, _)| p);
            format!("api:general:{pid}")
        }
    }
}

/// Take one token from every charge in the shared store (after the local pre-filter passed).
/// `Ok(())` = proceed (shared allowed, or counted locally under policy `local`).
pub async fn take_shared(
    s: &AppState,
    class: &ClassCounters,
    charges: &[Charge],
) -> Result<(), Result<Refusal, ApiError>> {
    let keys: Vec<String> = charges.iter().map(|c| shared_key(s, c)).collect();
    let takes: Vec<BucketTake> = charges
        .iter()
        .zip(&keys)
        .map(|(c, k)| BucketTake {
            key: k.clone(),
            emission_interval: interval(c.spec.per_secs, c.spec.limit),
            burst: c.spec.burst,
        })
        .collect();
    match class.take(&takes).await {
        Ok(Verdict::Allowed) => Ok(()),
        Ok(Verdict::Refused { key, retry_after }) => {
            s.api_limiter.refund(charges);
            let blocking = keys
                .iter()
                .position(|k| *k == key)
                .and_then(|i| charges.get(i))
                .or_else(|| charges.first());
            let spec = blocking.map_or(
                Spec {
                    burst: 1,
                    limit: 1,
                    per_secs: 60,
                },
                |c| c.spec,
            );
            Err(Ok(Refusal {
                scope: blocking.map_or("general", |c| c.scope),
                class: blocking.map_or("general", |c| c.class),
                spec,
                retry_after_secs: secs_ceil(retry_after).max(1),
                remaining: 0,
            }))
        }
        Err(Fallback::Local) => Ok(()),
        Err(Fallback::Closed) => {
            s.api_limiter.refund(charges);
            Err(Err(ApiError::counter_unavailable(match class.class() {
                "security" => "api_security",
                _ => "api_management",
            })))
        }
    }
}

/// Take from the OIDC start/callback per-IP bucket (`security` class). `ip` is the client's
/// address; the shared key holds only its keyed hash.
pub async fn check_security_bucket(s: &AppState, bucket: Bucket, ip: &str) -> Result<(), ApiError> {
    s.rate_limiter.check(bucket, ip)?;
    let Some(class) = s.counters.security() else {
        return Ok(());
    };
    let q: Quota = bucket.quota();
    let key = format!("api:{}:{}", bucket.name(), s.derived.ratelimit_hash(ip));
    let take = BucketTake {
        key,
        emission_interval: Duration::from_secs_f64(q.per.as_secs_f64() / f64::from(q.capacity)),
        burst: q.capacity,
    };
    match class.take(&[take]).await {
        Ok(Verdict::Allowed) | Err(Fallback::Local) => Ok(()),
        Ok(Verdict::Refused { retry_after, .. }) => {
            s.rate_limiter.refund(bucket, ip);
            Err(ApiError::rate_limited(
                secs_ceil(retry_after).max(1),
                "Too many requests. Try again later.",
            ))
        }
        Err(Fallback::Closed) => {
            s.rate_limiter.refund(bucket, ip);
            Err(ApiError::counter_unavailable("api_security"))
        }
    }
}

/// The per-webhook limit (`POST /v1/hooks/{token}`, 60 per minute; `management` class). The
/// row-based check inside the delivery transaction stays authoritative; this adds the shared
/// store so a flood is refused before the body is read.
pub async fn check_webhook(
    s: &AppState,
    webhook: uuid::Uuid,
    per_minute: u64,
) -> Result<(), ApiError> {
    let Some(class) = s.counters.management() else {
        return Ok(());
    };
    let unit = WindowUnit {
        key: format!("api:hook:{webhook}"),
        period: aurora_counters::Period::Minute,
        limit: per_minute,
        cost: 1,
    };
    match class.consume(&[unit]).await {
        Ok(Verdict::Allowed) | Err(Fallback::Local) => Ok(()),
        Ok(Verdict::Refused { retry_after, .. }) => Err(ApiError::rate_limited(
            secs_ceil(retry_after).max(1),
            "Too many deliveries for this webhook.",
        )),
        Err(Fallback::Closed) => Err(ApiError::counter_unavailable("api_webhook")),
    }
}
