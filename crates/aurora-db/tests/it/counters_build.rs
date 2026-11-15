//! FIX-087: a service that does not share a counter class builds no counters store at all
//! (the gateway with `backend = postgres` but `gateway = memory` opened a pool it never used).

use aurora_config::{CounterBackend, CountersConfig};
use aurora_db::PgPool;

fn postgres_cfg() -> CountersConfig {
    CountersConfig {
        backend: CounterBackend::Postgres,
        api_shared: true,
        gateway_shared: false,
        ..CountersConfig::default()
    }
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn unshared_class_builds_no_store(pool: PgPool) {
    let cfg = postgres_cfg();
    let none = aurora_db::counters::build_store_if(cfg.gateway_shared, &cfg, &pool).expect("ok");
    assert!(none.is_none(), "gateway = memory builds no counters pool");
    let some = aurora_db::counters::build_store_if(cfg.api_shared, &cfg, &pool).expect("ok");
    assert!(some.is_some(), "a shared class still gets its store");
}
