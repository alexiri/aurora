//! The `postgres` backend of `aurora_counters::CounterStore` (phase 4, P4-CNT, D-119; `03
//! §rate_limit_windows`, `§rate_limit_buckets`). Window and bucket arithmetic uses the database
//! clock; the tables are `UNLOGGED`; the pool is dedicated and small (`ratelimit.counters.postgres
//! .pool_size`) so a slow counter query cannot starve the data path.

use async_trait::async_trait;
use aurora_counters::{
    BucketTake, CounterError, CounterStore, Result, Verdict, WindowProbe, WindowUnit, check_key,
    gcra_new_tat, gcra_wait_ms, interval_ms, normalize_takes, normalize_units,
};
use sqlx::PgPool;
use std::collections::HashMap;
use std::time::Duration;

/// Postgres-backed counters over `pool`.
pub struct PgCounterStore {
    pool: PgPool,
}

impl PgCounterStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn unavailable(e: &sqlx::Error) -> CounterError {
    // Only the error class, never the statement or any connection detail.
    let kind = match e {
        sqlx::Error::Io(_) => "io",
        sqlx::Error::PoolTimedOut => "pool timeout",
        sqlx::Error::PoolClosed => "pool closed",
        sqlx::Error::Database(_) => "database",
        _ => "error",
    };
    CounterError::Unavailable(format!("postgres: {kind}"))
}

fn i64_of(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

const CONSUME_ONE: &str = "\
INSERT INTO rate_limit_windows AS w (key, window_start, count, expires_at) \
SELECT $1, s.ws, $3, s.ws + make_interval(secs => $2::float8 + 60) \
FROM (SELECT date_bin(make_interval(secs => $2::float8), clock_timestamp(), 'epoch'::timestamptz) AS ws) s \
ON CONFLICT (key, window_start) DO UPDATE SET count = w.count + $3 WHERE w.count + $3 <= $4 \
RETURNING count";

const NOW_MS: &str = "SELECT (floor(extract(epoch from clock_timestamp()) * 1000))::bigint";

#[async_trait]
impl CounterStore for PgCounterStore {
    fn backend(&self) -> &'static str {
        "postgres"
    }

    async fn fixed_window_peek(&self, probes: &[WindowProbe]) -> Result<Vec<u64>> {
        for p in probes {
            check_key(&p.key)?;
        }
        if probes.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<&str> = probes.iter().map(|p| p.key.as_str()).collect();
        let secs: Vec<f64> = probes.iter().map(|p| p.period.seconds() as f64).collect();
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "WITH t AS (SELECT clock_timestamp() AS now), \
                  p AS (SELECT k, per, ord FROM unnest($1::text[], $2::float8[]) \
                        WITH ORDINALITY AS u(k, per, ord)) \
             SELECT p.ord, COALESCE(w.count, 0) \
             FROM p CROSS JOIN t \
             LEFT JOIN rate_limit_windows w ON w.key = p.k \
              AND w.window_start = date_bin(make_interval(secs => p.per), t.now, 'epoch'::timestamptz) \
             ORDER BY p.ord",
        )
        .bind(&keys)
        .bind(&secs)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| unavailable(&e))?;
        if rows.len() != probes.len() {
            return Err(CounterError::Invalid("bad peek reply".into()));
        }
        Ok(rows
            .into_iter()
            .map(|(_, n)| u64::try_from(n).unwrap_or(0))
            .collect())
    }

    async fn fixed_window_consume(&self, units: &[WindowUnit]) -> Result<Verdict> {
        for u in units {
            check_key(&u.key)?;
        }
        let units = normalize_units(units);
        let mut tx = self.pool.begin().await.map_err(|e| unavailable(&e))?;
        let mut refusing = Vec::new();
        for u in &units {
            let refused = if u.cost > u.limit {
                true
            } else {
                let row: Option<(i64,)> = sqlx::query_as(CONSUME_ONE)
                    .bind(&u.key)
                    .bind(u.period.seconds() as f64)
                    .bind(i64_of(u.cost))
                    .bind(i64_of(u.limit))
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|e| unavailable(&e))?;
                row.is_none()
            };
            if refused {
                refusing.push(u);
            }
        }
        if !refusing.is_empty() {
            let (now,): (i64,) = sqlx::query_as(NOW_MS)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| unavailable(&e))?;
            // Nothing of this call is kept (units seen before and after the refusal included).
            tx.rollback().await.map_err(|e| unavailable(&e))?;
            // Report the unit with the longest wait (the first one on a tie), like the memory
            // and Redis stores: a client retrying after it is not refused by another unit.
            let mut worst: Option<(&str, i64)> = None;
            for u in refusing {
                let per = i64_of(u.period.millis());
                let ms = (per - now.rem_euclid(per)).max(1);
                if worst.is_none_or(|(_, r)| ms > r) {
                    worst = Some((u.key.as_str(), ms));
                }
            }
            if let Some((key, ms)) = worst {
                return Ok(Verdict::Refused {
                    key: key.to_owned(),
                    retry_after: Duration::from_millis(u64::try_from(ms).unwrap_or(1)),
                });
            }
            return Err(CounterError::Invalid("refusal without a unit".into()));
        }
        tx.commit().await.map_err(|e| unavailable(&e))?;
        Ok(Verdict::Allowed)
    }

    async fn bucket_take(&self, takes: &[BucketTake]) -> Result<Verdict> {
        for t in takes {
            check_key(&t.key)?;
        }
        if takes.is_empty() {
            return Ok(Verdict::Allowed);
        }
        let takes = normalize_takes(takes);
        let mut keys: Vec<&str> = takes.iter().map(|t| t.key.as_str()).collect();
        keys.dedup();
        let mut tx = self.pool.begin().await.map_err(|e| unavailable(&e))?;
        // Make sure every row exists (a new bucket starts full) and lock it, **one statement per
        // key in the sorted order of `keys`**: the order in which a multi-row `FOR UPDATE` takes
        // its row locks is not guaranteed, so overlapping takes could deadlock. Locking key by
        // key in one global order cannot.
        let mut rows: Vec<(String, i64, i64)> = Vec::with_capacity(keys.len());
        for key in &keys {
            sqlx::query(
                "INSERT INTO rate_limit_buckets (key, tat, expires_at) \
                 VALUES ($1, clock_timestamp(), clock_timestamp() + interval '1 minute') \
                 ON CONFLICT (key) DO NOTHING",
            )
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(|e| unavailable(&e))?;
            let row: (String, i64, i64) = sqlx::query_as(
                "SELECT key, (floor(extract(epoch from tat) * 1000))::bigint, \
                        (floor(extract(epoch from clock_timestamp()) * 1000))::bigint \
                 FROM rate_limit_buckets WHERE key = $1 FOR UPDATE",
            )
            .bind(key)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| unavailable(&e))?;
            rows.push(row);
        }
        // The clock of the last statement: a bucket created above has `tat` = its insert time,
        // which must not be later than `now` (a new bucket starts full).
        let Some(now) = rows.last().map(|r| r.2) else {
            return Err(CounterError::Invalid("bucket rows missing".into()));
        };
        let mut tat: HashMap<String, i64> = rows.into_iter().map(|(k, t, _)| (k, t)).collect();
        let mut worst: Option<(String, u64)> = None;
        for t in &takes {
            let tms = interval_ms(t.emission_interval);
            let cur = tat.get(&t.key).copied().unwrap_or(now);
            if let Some(w) = gcra_wait_ms(cur, now, tms, t.burst) {
                if worst.as_ref().is_none_or(|(_, r)| w > *r) {
                    worst = Some((t.key.clone(), w));
                }
            } else {
                tat.insert(t.key.clone(), gcra_new_tat(cur, now, tms));
            }
        }
        if let Some((key, ms)) = worst {
            tx.rollback().await.map_err(|e| unavailable(&e))?;
            return Ok(Verdict::Refused {
                key,
                retry_after: Duration::from_millis(ms),
            });
        }
        let ks: Vec<&str> = tat.keys().map(String::as_str).collect();
        let vs: Vec<f64> = ks.iter().map(|k| tat[*k] as f64).collect();
        sqlx::query(
            "UPDATE rate_limit_buckets b \
             SET tat = to_timestamp(u.tat / 1000.0), \
                 expires_at = to_timestamp(u.tat / 1000.0) + interval '60 seconds' \
             FROM unnest($1::text[], $2::float8[]) AS u(key, tat) WHERE b.key = u.key",
        )
        .bind(&ks)
        .bind(&vs)
        .execute(&mut *tx)
        .await
        .map_err(|e| unavailable(&e))?;
        tx.commit().await.map_err(|e| unavailable(&e))?;
        Ok(Verdict::Allowed)
    }

    async fn health(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|e| unavailable(&e))
    }
}

/// Build the configured shared store, wrapped in the timeout and breaker (`None` for `memory`:
/// the default needs nothing). The `postgres` backend gets a **dedicated, lazily connecting**
/// pool of `pool_size` connections to the same database and role as `pool`, so a slow counter
/// query cannot starve the data path. Redis needs no connection until the first call.
pub fn build_store(
    cfg: &aurora_config::CountersConfig,
    pool: &PgPool,
) -> std::result::Result<Option<std::sync::Arc<aurora_counters::Guarded>>, String> {
    use aurora_config::CounterBackend;
    let inner: std::sync::Arc<dyn CounterStore> = match cfg.backend {
        CounterBackend::Memory => return Ok(None),
        CounterBackend::Postgres => {
            let own = sqlx::postgres::PgPoolOptions::new()
                .max_connections(cfg.pg_pool_size)
                // The per-call budget is the `Guarded` timeout; this only bounds a stuck connect.
                .acquire_timeout(std::time::Duration::from_secs(2))
                .connect_lazy_with((*pool.connect_options()).clone());
            // Open the first connection now so the first limited request does not pay (and
            // time out on) the handshake of a cold pool.
            if let Ok(h) = tokio::runtime::Handle::try_current() {
                let warm = own.clone();
                h.spawn(async move {
                    let _ = sqlx::query("SELECT 1").execute(&warm).await;
                });
            }
            std::sync::Arc::new(PgCounterStore::new(own))
        }
        CounterBackend::Redis => {
            let r = cfg
                .redis
                .as_ref()
                .ok_or("ratelimit.counters.redis.address: required when backend = \"redis\"")?;
            if r.plaintext_remote {
                tracing::warn!(
                    "ratelimit.counters.redis.address is plaintext redis:// to a non-loopback host \
                     (allow_plaintext = true): counter traffic is not encrypted"
                );
            }
            std::sync::Arc::new(
                aurora_counters::redis_store::RedisStore::new(
                    &aurora_counters::redis_store::RedisOptions {
                        address: r.address.clone(),
                        allow_plaintext: r.allow_plaintext,
                        username: r.username.as_ref().map(|v| v.expose().clone()),
                        password: r.password.as_ref().map(|v| v.expose().clone()),
                        ca_file: r.ca_file.as_ref().map(std::path::PathBuf::from),
                        db: r.db,
                        key_prefix: r.key_prefix.clone(),
                    },
                )
                .map_err(|e| format!("ratelimit.counters.redis: {e}"))?,
            )
        }
    };
    Ok(Some(std::sync::Arc::new(aurora_counters::Guarded::new(
        inner,
        aurora_counters::BreakerConfig {
            timeout: cfg.timeout,
            ..aurora_counters::BreakerConfig::default()
        },
    ))))
}

/// The API's `counters_prune` arm: delete dead rows (`expires_at < now()`) of both tables, in
/// batches, skipping rows another replica is pruning. Returns the number of rows deleted.
pub async fn prune(pool: &PgPool, batch: i64) -> std::result::Result<u64, sqlx::Error> {
    let mut total = 0;
    for table in ["rate_limit_windows", "rate_limit_buckets"] {
        let sql = format!(
            "DELETE FROM {table} WHERE ctid IN (SELECT ctid FROM {table} \
             WHERE expires_at < now() ORDER BY expires_at LIMIT $1 FOR UPDATE SKIP LOCKED)"
        );
        total += sqlx::query(&sql)
            .bind(batch)
            .execute(pool)
            .await?
            .rows_affected();
    }
    Ok(total)
}
