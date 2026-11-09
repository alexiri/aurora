//! `notify` DeltaBus throughput and latency (docs/15 §Scaling, RFE-016, FIX-015).
//!
//! A plain `harness = false` benchmark; `scripts/perf/delta_bus.sh` drives it. It creates a
//! throwaway database next to `DATABASE_URL` (NOTIFY channels are per database, so the shared
//! tables and the shared `aurora_delta` channel are never touched), measures, and always drops
//! the database again. Prints `RESULT key=value ...` lines that the script compares with the
//! planning numbers of docs/15.
//!
//! Phases:
//! - `paced`: 1 000 notifications/s (the "comfortable region") for `PERF_SECS`, 2 000-char payloads.
//! - `sat`: closed-loop saturation with `PERF_PUBLISHERS` publishers for `PERF_SECS`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

use aurora_db::bus::{Delta, DeltaBus, DeltaKind, NotifyDeltaBus};
use futures::StreamExt;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use uuid::Uuid;

const PAYLOAD_CHARS: usize = 2_000;

fn env_u64(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let i = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[i]
}

struct Phase {
    sent: u64,
    received: u64,
    secs: f64,
    publish_ms: Vec<f64>,
    deliver_ms: Vec<f64>,
}

/// Run one phase. `rate` is the total target notifications/s (`None` = closed loop).
async fn phase(url: &str, publishers: usize, secs: u64, rate: Option<u64>) -> Phase {
    let pool = PgPoolOptions::new()
        .max_connections(publishers as u32 + 4)
        .connect(url)
        .await
        .expect("pool");
    let bus = Arc::new(NotifyDeltaBus::new(pool.clone()));
    let mut sub = bus.subscribe().await.expect("subscribe");
    let epoch = Instant::now();
    let received = Arc::new(AtomicU64::new(0));
    let deliver = Arc::new(std::sync::Mutex::new(Vec::<f64>::new()));
    let rx = {
        let (received, deliver) = (received.clone(), deliver.clone());
        tokio::spawn(async move {
            while let Some(d) = sub.next().await {
                let now = epoch.elapsed().as_secs_f64() * 1000.0;
                if let Some(sent_ms) = d.data.split(':').next().and_then(|s| s.parse::<f64>().ok())
                {
                    deliver.lock().unwrap().push(now - sent_ms);
                }
                received.fetch_add(1, Ordering::Relaxed);
            }
        })
    };
    let stop = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicU64::new(0));
    let publish = Arc::new(std::sync::Mutex::new(Vec::<f64>::new()));
    let started = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..publishers {
        let (bus, stop, sent, publish) = (bus.clone(), stop.clone(), sent.clone(), publish.clone());
        tasks.push(tokio::spawn(async move {
            let (c, t) = (Uuid::now_v7(), Uuid::now_v7());
            let interval = rate.map(|r| Duration::from_secs_f64(publishers as f64 / r as f64));
            let mut next = Instant::now();
            let mut mine = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                if let Some(iv) = interval {
                    next += iv;
                    tokio::time::sleep_until(next.into()).await;
                }
                let stamp = format!("{:.3}:", epoch.elapsed().as_secs_f64() * 1000.0);
                let data = format!("{stamp}{}", "x".repeat(PAYLOAD_CHARS - stamp.len()));
                let delta = Delta {
                    conversation_id: c,
                    turn_id: t,
                    kind: DeltaKind::Text,
                    message_id: Some("perf".into()),
                    reply_to: None,
                    data,
                };
                let t0 = Instant::now();
                if bus.publish(delta).await.is_ok() {
                    sent.fetch_add(1, Ordering::Relaxed);
                    mine.push(t0.elapsed().as_secs_f64() * 1000.0);
                }
            }
            publish.lock().unwrap().extend(mine);
        }));
    }
    tokio::time::sleep(Duration::from_secs(secs)).await;
    stop.store(true, Ordering::Relaxed);
    for t in tasks {
        let _ = t.await;
    }
    let secs_run = started.elapsed().as_secs_f64();
    // Let in-flight notifications land.
    let sent_n = sent.load(Ordering::Relaxed);
    let drain = Instant::now();
    while received.load(Ordering::Relaxed) < sent_n && drain.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    rx.abort();
    pool.close().await;
    let mut publish_ms = std::mem::take(&mut *publish.lock().unwrap());
    let mut deliver_ms = std::mem::take(&mut *deliver.lock().unwrap());
    publish_ms.sort_by(f64::total_cmp);
    deliver_ms.sort_by(f64::total_cmp);
    Phase {
        sent: sent_n,
        received: received.load(Ordering::Relaxed),
        secs: secs_run,
        publish_ms,
        deliver_ms,
    }
}

fn report(name: &str, p: &Phase) {
    let delivered_rate = p.received as f64 / p.secs;
    println!(
        "{name}: sent={} received={} lost={} in {:.1}s -> publish {:.0}/s, delivered {:.0}/s; publish p50 {:.2} ms p99 {:.2} ms; publish->receive p50 {:.2} ms p99 {:.2} ms",
        p.sent,
        p.received,
        p.sent.saturating_sub(p.received),
        p.secs,
        p.sent as f64 / p.secs,
        delivered_rate,
        pct(&p.publish_ms, 0.5),
        pct(&p.publish_ms, 0.99),
        pct(&p.deliver_ms, 0.5),
        pct(&p.deliver_ms, 0.99),
    );
    println!(
        "RESULT {name}_publish_per_s={:.0} {name}_delivered_per_s={:.0} {name}_lost={} {name}_publish_p50_ms={:.3} {name}_publish_p99_ms={:.3} {name}_deliver_p50_ms={:.3} {name}_deliver_p99_ms={:.3}",
        p.sent as f64 / p.secs,
        delivered_rate,
        p.sent.saturating_sub(p.received),
        pct(&p.publish_ms, 0.5),
        pct(&p.publish_ms, 0.99),
        pct(&p.deliver_ms, 0.5),
        pct(&p.deliver_ms, 0.99),
    );
}

/// `postgres://u:p@h:5432/db?x=y` -> (same with `/<name>`, the original database name).
fn with_database(url: &str, name: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, format!("?{q}")),
        None => (url, String::new()),
    };
    let cut = base.rfind('/').expect("database url has a database name");
    format!("{}/{name}{query}", &base[..cut])
}

#[tokio::main]
async fn main() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let secs = env_u64("PERF_SECS", 5);
    let publishers = env_u64("PERF_PUBLISHERS", 16) as usize;
    let db = format!(
        "aurora_perf_delta_{}",
        &Uuid::now_v7().simple().to_string()[20..]
    );
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("admin connect");
    sqlx::query(&format!("CREATE DATABASE {db}"))
        .execute(&admin)
        .await
        .expect("create throwaway database (role needs CREATEDB)");
    println!("throwaway database: {db}");
    let url = with_database(&url, &db);
    let paced = phase(&url, 20, secs, Some(1000)).await;
    report("paced", &paced);
    let sat = phase(&url, publishers, secs, None).await;
    report("sat", &sat);
    let dropped = sqlx::query(&format!("DROP DATABASE {db} WITH (FORCE)"))
        .execute(&admin)
        .await;
    match dropped {
        Ok(_) => println!("dropped {db}"),
        Err(e) => {
            println!("could not drop {db}: {e}");
            std::process::exit(1);
        }
    }
}
