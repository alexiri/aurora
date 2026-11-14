//! Conformance suite (docs/18 §Shared rate-limit counters): one body, run against `memory`,
//! `postgres` (the test database) and `redis` (a container pinned by digest, gated by
//! `AURORA_TEST_REDIS=1` and `AURORA_TEST_REDIS_ADDR`).

use aurora_counters::{
    BucketTake, CounterError, CounterStore, MemoryStore, Period, Verdict, WindowProbe, WindowUnit,
};
use aurora_db::counters::PgCounterStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A key namespace unique to one test run, so backends that persist (Redis) never see old state.
fn ns() -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("t:{n:x}{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn unit(key: &str, limit: u64, cost: u64) -> WindowUnit {
    WindowUnit {
        key: key.to_owned(),
        period: Period::Hour,
        limit,
        cost,
    }
}

fn probe(key: &str) -> WindowProbe {
    WindowProbe {
        key: key.to_owned(),
        period: Period::Hour,
    }
}

fn take(key: &str, interval_ms: u64, burst: u32) -> BucketTake {
    BucketTake {
        key: key.to_owned(),
        emission_interval: Duration::from_millis(interval_ms),
        burst,
    }
}

fn refused_key(v: &Verdict) -> Option<&str> {
    match v {
        Verdict::Refused { key, .. } => Some(key),
        Verdict::Allowed => None,
    }
}

/// The whole suite except the clock-dependent rollover case (see [`rollover_real_clock`]).
pub async fn suite(store: Arc<dyn CounterStore>) {
    concurrent_consume_never_exceeds_limit(store.clone()).await;
    all_or_nothing_consume(store.clone()).await;
    cost_and_duplicates(store.clone()).await;
    peek_has_no_side_effect(store.clone()).await;
    bucket_burst_refill_and_all_or_nothing(store.clone()).await;
    keys_large_and_unicode(store.clone()).await;
    refusal_reports_the_longest_wait(store.clone()).await;
    store.health().await.expect("health");
}

/// Two exhausted units with different time left: every backend reports the one with the
/// longest `retry_after`, whatever the order in the call (the key sorting first is the short one).
async fn refusal_reports_the_longest_wait(store: Arc<dyn CounterStore>) {
    let n = ns();
    let (short, long, fine) = (format!("{n}:a-min"), format!("{n}:b-day"), format!("{n}:c"));
    let minute = |key: &str| WindowUnit {
        period: Period::Minute,
        ..unit(key, 1, 1)
    };
    let day = |key: &str| WindowUnit {
        period: Period::Day,
        ..unit(key, 1, 1)
    };
    assert!(
        store
            .fixed_window_consume(&[minute(&short), day(&long)])
            .await
            .expect("fill")
            .is_allowed()
    );
    for units in [
        vec![minute(&short), day(&long), unit(&fine, 5, 1)],
        vec![unit(&fine, 5, 1), day(&long), minute(&short)],
    ] {
        let v = store.fixed_window_consume(&units).await.expect("refused");
        let Verdict::Refused { key, retry_after } = v else {
            panic!("both units are exhausted");
        };
        assert_eq!(key, long, "the unit that frees last is reported");
        assert!(
            retry_after > Duration::from_secs(60) && retry_after <= Duration::from_secs(86_400),
            "{retry_after:?}"
        );
    }
    assert_eq!(
        store
            .fixed_window_peek(&[probe(&fine)])
            .await
            .expect("peek"),
        vec![0],
        "the refusal consumed nothing"
    );
}

async fn concurrent_consume_never_exceeds_limit(store: Arc<dyn CounterStore>) {
    let key = format!("{}:conc", ns());
    let mut tasks = Vec::new();
    for _ in 0..64 {
        let s = store.clone();
        let k = key.clone();
        tasks.push(tokio::spawn(async move {
            s.fixed_window_consume(&[unit(&k, 10, 1)])
                .await
                .expect("consume")
        }));
    }
    let mut allowed = 0;
    for t in tasks {
        if t.await.expect("join").is_allowed() {
            allowed += 1;
        }
    }
    assert_eq!(
        allowed, 10,
        "exactly `limit` of 64 concurrent consumes succeed"
    );
    assert_eq!(
        store.fixed_window_peek(&[probe(&key)]).await.expect("peek"),
        vec![10]
    );
}

async fn all_or_nothing_consume(store: Arc<dyn CounterStore>) {
    let n = ns();
    let (a, b) = (format!("{n}:a"), format!("{n}:b"));
    assert!(
        store
            .fixed_window_consume(&[unit(&b, 1, 1)])
            .await
            .expect("b")
            .is_allowed()
    );
    // `a` would allow, `b` is full: nothing is consumed, `a` included (it sorts first).
    let v = store
        .fixed_window_consume(&[unit(&a, 5, 1), unit(&b, 1, 1)])
        .await
        .expect("both");
    assert_eq!(refused_key(&v), Some(b.as_str()));
    if let Verdict::Refused { retry_after, .. } = v {
        assert!(retry_after > Duration::ZERO && retry_after <= Duration::from_secs(3600));
    }
    assert_eq!(
        store
            .fixed_window_peek(&[probe(&a), probe(&b)])
            .await
            .expect("peek"),
        vec![0, 1],
        "a refusal consumes nothing, including units earlier in the call"
    );
    // Order in the call does not matter.
    let v = store
        .fixed_window_consume(&[unit(&b, 1, 1), unit(&a, 5, 1)])
        .await
        .expect("both, other order");
    assert_eq!(refused_key(&v), Some(b.as_str()));
    assert_eq!(
        store.fixed_window_peek(&[probe(&a)]).await.expect("peek"),
        vec![0]
    );
}

async fn cost_and_duplicates(store: Arc<dyn CounterStore>) {
    let k = format!("{}:cost", ns());
    assert!(
        store
            .fixed_window_consume(&[unit(&k, 5, 3)])
            .await
            .expect("3")
            .is_allowed()
    );
    assert!(
        !store
            .fixed_window_consume(&[unit(&k, 5, 3)])
            .await
            .expect("3 again")
            .is_allowed()
    );
    assert!(
        store
            .fixed_window_consume(&[unit(&k, 5, 2)])
            .await
            .expect("2")
            .is_allowed()
    );
    // Cost above the limit is refused on an empty window too.
    let big = format!("{}:big", ns());
    assert!(
        !store
            .fixed_window_consume(&[unit(&big, 2, 3)])
            .await
            .expect("big")
            .is_allowed()
    );
    assert_eq!(
        store.fixed_window_peek(&[probe(&big)]).await.expect("peek"),
        vec![0]
    );
    // A limit of zero refuses everything.
    let zero = format!("{}:zero", ns());
    assert!(
        !store
            .fixed_window_consume(&[unit(&zero, 0, 1)])
            .await
            .expect("zero")
            .is_allowed()
    );
    // The same window named twice in one call is charged twice against its limit.
    let d = format!("{}:dup", ns());
    assert!(
        !store
            .fixed_window_consume(&[unit(&d, 1, 1), unit(&d, 1, 1)])
            .await
            .expect("dup")
            .is_allowed()
    );
    assert_eq!(
        store.fixed_window_peek(&[probe(&d)]).await.expect("peek"),
        vec![0]
    );
    // Periods are separate windows of the same key.
    let p = format!("{}:per", ns());
    let minute = WindowUnit {
        key: p.clone(),
        period: Period::Minute,
        limit: 1,
        cost: 1,
    };
    let day = WindowUnit {
        key: p.clone(),
        period: Period::Day,
        limit: 1,
        cost: 1,
    };
    assert!(
        store
            .fixed_window_consume(std::slice::from_ref(&minute))
            .await
            .expect("m")
            .is_allowed()
    );
    assert!(
        store
            .fixed_window_consume(std::slice::from_ref(&day))
            .await
            .expect("d")
            .is_allowed()
    );
    assert!(
        !store
            .fixed_window_consume(&[minute])
            .await
            .expect("m2")
            .is_allowed()
    );
    assert!(
        !store
            .fixed_window_consume(&[day])
            .await
            .expect("d2")
            .is_allowed()
    );
}

async fn peek_has_no_side_effect(store: Arc<dyn CounterStore>) {
    let k = format!("{}:peek", ns());
    for _ in 0..5 {
        assert_eq!(
            store.fixed_window_peek(&[probe(&k)]).await.expect("peek"),
            vec![0]
        );
    }
    assert!(
        store
            .fixed_window_consume(&[unit(&k, 1, 1)])
            .await
            .expect("c")
            .is_allowed()
    );
    assert_eq!(
        store
            .fixed_window_peek(&[probe(&k), probe(&k)])
            .await
            .expect("peek"),
        vec![1, 1]
    );
    assert_eq!(
        store.fixed_window_peek(&[]).await.expect("empty"),
        Vec::<u64>::new()
    );
}

async fn bucket_burst_refill_and_all_or_nothing(store: Arc<dyn CounterStore>) {
    let n = ns();
    let a = format!("{n}:ba");
    // Burst of 3 at one token per second: three at once, the fourth waits.
    for i in 0..3 {
        assert!(
            store
                .bucket_take(&[take(&a, 1000, 3)])
                .await
                .expect("take")
                .is_allowed(),
            "burst token {i}"
        );
    }
    let v = store.bucket_take(&[take(&a, 1000, 3)]).await.expect("4th");
    let Verdict::Refused { key, retry_after } = v else {
        panic!("the fourth take must be refused");
    };
    assert_eq!(key, a);
    assert!(
        retry_after > Duration::ZERO && retry_after <= Duration::from_millis(1100),
        "retry_after {retry_after:?}"
    );
    // Steady refill.
    tokio::time::sleep(retry_after + Duration::from_millis(60)).await;
    assert!(
        store
            .bucket_take(&[take(&a, 1000, 3)])
            .await
            .expect("refilled")
            .is_allowed()
    );
    assert!(
        !store
            .bucket_take(&[take(&a, 1000, 3)])
            .await
            .expect("again")
            .is_allowed()
    );

    // All-or-nothing across buckets: x has a token, y is empty -> x keeps its token.
    let (x, y) = (format!("{n}:bx"), format!("{n}:by"));
    assert!(
        store
            .bucket_take(&[take(&y, 60_000, 1)])
            .await
            .expect("y")
            .is_allowed()
    );
    let v = store
        .bucket_take(&[take(&x, 60_000, 1), take(&y, 60_000, 1)])
        .await
        .expect("x+y");
    assert_eq!(refused_key(&v), Some(y.as_str()));
    assert!(
        store
            .bucket_take(&[take(&x, 60_000, 1)])
            .await
            .expect("x alone")
            .is_allowed(),
        "a refused call took nothing from the bucket that would have allowed it"
    );
    // The blocking bucket is the one that waits longest.
    let (s, l) = (format!("{n}:short"), format!("{n}:long"));
    assert!(
        store
            .bucket_take(&[take(&s, 1_000, 1), take(&l, 600_000, 1)])
            .await
            .expect("fill")
            .is_allowed()
    );
    let v = store
        .bucket_take(&[take(&s, 1_000, 1), take(&l, 600_000, 1)])
        .await
        .expect("both full");
    assert_eq!(refused_key(&v), Some(l.as_str()));
}

async fn keys_large_and_unicode(store: Arc<dyn CounterStore>) {
    let mut big = ns();
    while big.len() < 200 {
        big.push('k');
    }
    assert!(
        store
            .fixed_window_consume(&[unit(&big, 1, 1)])
            .await
            .expect("200 bytes")
            .is_allowed()
    );
    assert!(
        !store
            .fixed_window_consume(&[unit(&big, 1, 1)])
            .await
            .expect("200 bytes")
            .is_allowed()
    );
    let uni = format!("{}:ключ-🔑-鍵", ns());
    assert!(
        store
            .fixed_window_consume(&[unit(&uni, 1, 1)])
            .await
            .expect("unicode")
            .is_allowed()
    );
    assert!(
        !store
            .fixed_window_consume(&[unit(&uni, 1, 1)])
            .await
            .expect("unicode")
            .is_allowed()
    );
    assert!(
        store
            .bucket_take(&[take(&uni, 1000, 1)])
            .await
            .expect("unicode bucket")
            .is_allowed()
    );
    // Over-long and empty keys are a bug of the caller, never "allowed".
    let too_long = "z".repeat(201);
    assert!(matches!(
        store.fixed_window_consume(&[unit(&too_long, 1, 1)]).await,
        Err(CounterError::Invalid(_))
    ));
    assert!(matches!(
        store.bucket_take(&[take("", 1000, 1)]).await,
        Err(CounterError::Invalid(_))
    ));
}

/// Window rollover and `retry_after` against the **store's** clock, for backends that read the
/// real clock: wait out the minute window by the store's own `retry_after`.
pub async fn rollover_real_clock(store: Arc<dyn CounterStore>) {
    let key = format!("{}:roll", ns());
    let u = WindowUnit {
        key: key.clone(),
        period: Period::Minute,
        limit: 1,
        cost: 1,
    };
    assert!(
        store
            .fixed_window_consume(std::slice::from_ref(&u))
            .await
            .expect("first")
            .is_allowed()
    );
    let v = store
        .fixed_window_consume(std::slice::from_ref(&u))
        .await
        .expect("second");
    let Verdict::Refused { retry_after, .. } = v else {
        panic!("second consume in the same window must be refused");
    };
    assert!(
        retry_after > Duration::ZERO && retry_after <= Duration::from_secs(60),
        "{retry_after:?}"
    );
    tokio::time::sleep(retry_after + Duration::from_millis(150)).await;
    assert!(
        store
            .fixed_window_consume(&[u])
            .await
            .expect("next window")
            .is_allowed(),
        "a new window starts at the boundary of the store's clock"
    );
}

// ---- memory ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory() {
    suite(Arc::new(MemoryStore::new())).await;
}

#[tokio::test]
async fn memory_rollover_uses_the_injected_clock() {
    let now = Arc::new(AtomicI64::new(1_800_000_030_000)); // 30 s into a minute
    let c = now.clone();
    let store = MemoryStore::with_clock(Arc::new(move || c.load(Ordering::Relaxed)));
    let u = WindowUnit {
        key: "roll".into(),
        period: Period::Minute,
        limit: 2,
        cost: 1,
    };
    assert!(
        store
            .fixed_window_consume(std::slice::from_ref(&u))
            .await
            .expect("1")
            .is_allowed()
    );
    assert!(
        store
            .fixed_window_consume(std::slice::from_ref(&u))
            .await
            .expect("2")
            .is_allowed()
    );
    let v = store
        .fixed_window_consume(std::slice::from_ref(&u))
        .await
        .expect("3");
    assert!(
        matches!(v, Verdict::Refused { retry_after, .. } if retry_after == Duration::from_secs(30))
    );
    // 29.9 s later: still the same window; at the boundary: a new one.
    now.store(1_800_000_059_900, Ordering::Relaxed);
    assert!(
        !store
            .fixed_window_consume(std::slice::from_ref(&u))
            .await
            .expect("same")
            .is_allowed()
    );
    now.store(1_800_000_060_000, Ordering::Relaxed);
    assert!(
        store
            .fixed_window_consume(std::slice::from_ref(&u))
            .await
            .expect("rolled")
            .is_allowed()
    );
    assert_eq!(
        store
            .fixed_window_peek(&[WindowProbe {
                key: "roll".into(),
                period: Period::Minute
            }])
            .await
            .expect("peek"),
        vec![1]
    );
    rollover_real_clock(Arc::new(MemoryStore::new())).await;
}

// ---- postgres ------------------------------------------------------------------------

/// Overlapping multi-key takes in different orders must never deadlock (or error): each task
/// takes the same keys in its own rotation of the call order, with new keys each round too.
#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn postgres_overlapping_bucket_takes_never_deadlock(pool: aurora_db::PgPool) {
    let store: Arc<dyn CounterStore> = Arc::new(PgCounterStore::new(pool));
    let n = ns();
    let keys: Vec<String> = (0..6).map(|i| format!("{n}:k{i}")).collect();
    let mut tasks = Vec::new();
    for t in 0..24usize {
        let (s, keys) = (store.clone(), keys.clone());
        tasks.push(tokio::spawn(async move {
            for round in 0..15usize {
                let mut ks: Vec<&String> = keys.iter().collect();
                ks.rotate_left((t + round) % keys.len());
                if t % 2 == 1 {
                    ks.reverse();
                }
                let takes: Vec<BucketTake> =
                    ks.iter().take(4).map(|k| take(k, 1, 1_000_000)).collect();
                s.bucket_take(&takes).await.expect("no deadlock, no error");
            }
        }));
    }
    for t in tasks {
        t.await.expect("join");
    }
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn postgres(pool: aurora_db::PgPool) {
    let store: Arc<dyn CounterStore> = Arc::new(PgCounterStore::new(pool.clone()));
    suite(store.clone()).await;
    rollover_real_clock(store.clone()).await;

    // Expiry and pruning: only dead rows go.
    let live = format!("{}:live", ns());
    assert!(
        store
            .fixed_window_consume(&[unit(&live, 5, 1)])
            .await
            .expect("live")
            .is_allowed()
    );
    assert!(
        store
            .bucket_take(&[take(&live, 1000, 2)])
            .await
            .expect("live bucket")
            .is_allowed()
    );
    sqlx::query(
        "INSERT INTO rate_limit_windows (key, window_start, count, expires_at) \
         VALUES ('dead', now() - interval '2 hours', 3, now() - interval '1 hour')",
    )
    .execute(&pool)
    .await
    .expect("dead window");
    sqlx::query(
        "INSERT INTO rate_limit_buckets (key, tat, expires_at) \
         VALUES ('dead', now() - interval '2 hours', now() - interval '1 hour')",
    )
    .execute(&pool)
    .await
    .expect("dead bucket");
    let n = aurora_db::counters::prune(&pool, 5000)
        .await
        .expect("prune");
    // At least the two inserted dead rows. Rows the suite wrote earlier may have expired for real
    // meanwhile (a bucket lives 60 s past its TAT and `rollover_real_clock` sleeps up to a minute),
    // and pruning them is correct; the live keys below must survive.
    assert!(n >= 2, "{n}");
    let left: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rate_limit_windows WHERE key = $1), \
                (SELECT count(*) FROM rate_limit_buckets WHERE key = $1)",
    )
    .bind(&live)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(left, (1, 1), "live rows are kept");
    let dead: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM rate_limit_windows WHERE key = 'dead') \
              + (SELECT count(*) FROM rate_limit_buckets WHERE key = 'dead')",
    )
    .fetch_one(&pool)
    .await
    .expect("dead");
    assert_eq!(dead, 0);
    // Rows hold the expected expiry (`window_start + period + 1 minute`).
    let ok: bool = sqlx::query_scalar(
        "SELECT bool_and(expires_at = window_start + interval '1 hour 1 minute') \
         FROM rate_limit_windows WHERE key = $1",
    )
    .bind(&live)
    .fetch_one(&pool)
    .await
    .expect("expiry");
    assert!(ok);

    // Error mapping: a closed pool is `Unavailable`, never an answer.
    pool.close().await;
    for r in [
        store.fixed_window_peek(&[probe("x")]).await.map(|_| ()),
        store
            .fixed_window_consume(&[unit("x", 1, 1)])
            .await
            .map(|_| ()),
        store.bucket_take(&[take("x", 1000, 1)]).await.map(|_| ()),
        store.health().await,
    ] {
        assert!(matches!(r, Err(CounterError::Unavailable(_))), "{r:?}");
    }
}

// ---- redis ---------------------------------------------------------------------------

/// (FIX-101) `None` skips (gate unset outside CI); with `CI=true` and the gate unset `require_or_skip`
/// panics, so the Redis backend cannot silently drop out of the equivalence claim (SEC-62).
#[allow(clippy::expect_used)]
fn redis_addr() -> Option<String> {
    aurora_testkit::require_or_skip("AURORA_TEST_REDIS").then(|| {
        std::env::var("AURORA_TEST_REDIS_ADDR")
            .expect("AURORA_TEST_REDIS is set: AURORA_TEST_REDIS_ADDR (rediss:// or redis://host:port) must be too")
    })
}

fn redis_store(addr: &str, prefix: &str) -> aurora_counters::redis_store::RedisStore {
    aurora_counters::redis_store::RedisStore::new(&aurora_counters::redis_store::RedisOptions {
        address: addr.to_owned(),
        allow_plaintext: false,
        username: None,
        password: None,
        ca_file: None,
        db: 0,
        key_prefix: prefix.to_owned(),
    })
    .expect("redis store")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis() {
    let Some(addr) = redis_addr() else {
        eprintln!("skipped: set AURORA_TEST_REDIS=1 and AURORA_TEST_REDIS_ADDR (docs/18)");
        return;
    };
    let store: Arc<dyn CounterStore> = Arc::new(redis_store(&addr, &format!("{}:", ns())));
    suite(store.clone()).await;
    rollover_real_clock(store.clone()).await;
}

#[tokio::test]
async fn redis_keys_expire_and_errors_are_unavailable() {
    let Some(addr) = redis_addr() else {
        eprintln!("skipped: set AURORA_TEST_REDIS=1 and AURORA_TEST_REDIS_ADDR (docs/18)");
        return;
    };
    let prefix = format!("{}:", ns());
    let store = redis_store(&addr, &prefix);
    let k = "ttl";
    assert!(
        store
            .fixed_window_consume(&[unit(k, 1, 1)])
            .await
            .expect("c")
            .is_allowed()
    );
    assert!(
        store
            .bucket_take(&[take(k, 1000, 1)])
            .await
            .expect("b")
            .is_allowed()
    );
    // Every key under the prefix carries a TTL (a window's state expires with it).
    let client = redis::Client::open(addr.as_str()).expect("client");
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{prefix}*"))
        .query_async(&mut c)
        .await
        .expect("keys");
    assert_eq!(keys.len(), 2, "{keys:?}");
    for key in &keys {
        let ttl: i64 = redis::cmd("PTTL")
            .arg(key)
            .query_async(&mut c)
            .await
            .expect("pttl");
        assert!(ttl > 0 && ttl <= 3_600_000 + 60_000, "{key}: {ttl}");
        assert!(key.starts_with(&prefix));
    }
    // Nothing but ids and hashes in the keys: no scheme, no address.
    assert!(keys.iter().all(|k| !k.contains('@')));

    // A Redis that is not there is `Unavailable` for every operation.
    let dead = redis_store("redis://127.0.0.1:1", "x:");
    for r in [
        dead.fixed_window_peek(&[probe("x")]).await.map(|_| ()),
        dead.fixed_window_consume(&[unit("x", 1, 1)])
            .await
            .map(|_| ()),
        dead.bucket_take(&[take("x", 1000, 1)]).await.map(|_| ()),
        dead.health().await,
    ] {
        assert!(matches!(r, Err(CounterError::Unavailable(_))), "{r:?}");
    }
}

/// FIX-087 / SEC-62: a stored value that is not a number is `Unavailable`, never "under the
/// limit" (the scripts used to read it as zero or a full bucket).
#[tokio::test]
async fn redis_garbage_values_are_unavailable() {
    let Some(addr) = redis_addr() else {
        eprintln!("skipped: set AURORA_TEST_REDIS=1 and AURORA_TEST_REDIS_ADDR (docs/18)");
        return;
    };
    let prefix = format!("{}:", ns());
    let store = redis_store(&addr, &prefix);
    let client = redis::Client::open(addr.as_str()).expect("client");
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    // Window keys are `<prefix>w:<key>:<period s>:<window start ms>`; poison whatever the
    // store wrote for `x` by consuming first, then overwriting every key under the prefix.
    store
        .fixed_window_consume(&[unit("x", 5, 1)])
        .await
        .expect("seed window");
    store
        .bucket_take(&[take("x", 1000, 1)])
        .await
        .expect("seed bucket");
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{prefix}*"))
        .query_async(&mut c)
        .await
        .expect("keys");
    assert_eq!(keys.len(), 2, "{keys:?}");
    for k in &keys {
        let _: () = redis::cmd("SET")
            .arg(k)
            .arg("abc")
            .query_async(&mut c)
            .await
            .expect("poison");
    }
    for r in [
        store.fixed_window_peek(&[probe("x")]).await.map(|_| ()),
        store
            .fixed_window_consume(&[unit("x", 5, 1)])
            .await
            .map(|_| ()),
        store.bucket_take(&[take("x", 1000, 1)]).await.map(|_| ()),
    ] {
        assert!(matches!(r, Err(CounterError::Unavailable(_))), "{r:?}");
    }
}

/// FIX-087: a user limited to the ACL documented in `docs/15` runs the whole conformance body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_documented_acl_runs_the_scripts() {
    let Some(addr) = redis_addr() else {
        eprintln!("skipped: set AURORA_TEST_REDIS=1 and AURORA_TEST_REDIS_ADDR (docs/18)");
        return;
    };
    let prefix = format!("{}:", ns());
    let user = format!(
        "acl{}",
        ns().replace(|c: char| !c.is_ascii_alphanumeric(), "")
    );
    let client = redis::Client::open(addr.as_str()).expect("client");
    let mut admin = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&user)
        .arg("on")
        .arg(">pw-fix087")
        .arg(format!("~{prefix}*"))
        .arg("-@all")
        .arg("+evalsha")
        .arg("+script|load")
        .arg("+get")
        .arg("+set")
        .arg("+incrby")
        .arg("+pexpire")
        .arg("+ping")
        .arg("+time")
        .query_async(&mut admin)
        .await
        .expect("acl setuser");
    let store: Arc<dyn CounterStore> = Arc::new(
        aurora_counters::redis_store::RedisStore::new(
            &aurora_counters::redis_store::RedisOptions {
                address: addr.clone(),
                allow_plaintext: false,
                username: Some(user.clone()),
                password: Some("pw-fix087".to_owned()),
                ca_file: None,
                db: 0,
                key_prefix: prefix,
            },
        )
        .expect("redis store"),
    );
    suite(store).await;
    let _: () = redis::cmd("ACL")
        .arg("DELUSER")
        .arg(&user)
        .query_async(&mut admin)
        .await
        .expect("acl deluser");
}
