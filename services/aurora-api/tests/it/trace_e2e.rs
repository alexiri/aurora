#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-146 (D-232, C-732, SEC-88): a request's trace crosses the turn queue, and nothing a user,
//! a runner or a model sent ends up in a span or a log line.
//!
//! One real API (real router wrapped in `service::traced` with the public trust table, real
//! database, the real runner plane) with an in-memory exporter:
//! 1. a browser POST with a `traceparent` → the server span is a child of the browser span, the
//!    turn's database spans are its descendants, and `turns.trace_context` is a context of that
//!    trace whose span descends from the server span;
//! 2. the runner's work item carries exactly that context (the runner and the gateway adopt it
//!    with `adopt_turn_context`; their own tests pin that: `services/aurora-runner`
//!    `executor::trace_tests` and `services/aurora-gateway/tests/it/proxy.rs`
//!    `gateway_span_is_a_child_of_the_turn_context_and_carries_no_canary`);
//! 3. a report about the turn (sent with a runner-side `traceparent`) is handled in a `report
//!    turn` span that is a child of the turn's **stored** context, whatever the header says, and
//!    its queries are its descendants;
//! 4. canaries (message text, header, query value, cookie, runner token, a report body text)
//!    appear in no span (name, attribute, event, link) and in no log line.
//!
//! Own subscriber per test (`set_default`): the tests run on a current-thread runtime so every
//! task of the request stays on the thread that owns the subscriber.

use crate::hosted::rig;
use aurora_api::http::trace_trust::PUBLIC_TRUST;
use aurora_core::service::traced;
use aurora_core::telemetry::{DbSpanLayer, parse_traceparent, provider_builder};
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _};

const BROWSER_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const BROWSER_SPAN: &str = "00f067aa0ba902b7";
const RUNNER_SPAN: &str = "b7ad6b7169203331";

const CANARY_TEXT: &str = "canary-message-text-91c2e7";
const CANARY_HEADER: &str = "canary-header-value-5d10aa";
const CANARY_QUERY: &str = "canary-query-value-3be8f4";
const CANARY_REPORT: &str = "canary-report-text-0a77d9";

/// Collects formatted log lines.
#[derive(Clone, Default)]
pub(crate) struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> LogBuf {
        self.clone()
    }
}

pub(crate) fn install() -> (
    InMemorySpanExporter,
    LogBuf,
    tracing::subscriber::DefaultGuard,
) {
    // What `telemetry::install` sets in a service: the propagator `adopt` extracts with.
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );
    let exporter = InMemorySpanExporter::default();
    let provider = provider_builder("test", 1.0)
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("t");
    let logs = LogBuf::default();
    let sub = tracing_subscriber::registry()
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer.clone())
                .with_filter(tracing_subscriber::filter::filter_fn(|m| m.is_span())),
        )
        .with(DbSpanLayer::new(tracer).with_filter(DbSpanLayer::filter()))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(logs.clone())
                .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
        );
    // FIX-161: a callsite caches its interest when it is first hit. While only ONE dispatcher is
    // registered in the process, tracing computes that interest from the hitting thread's own
    // dispatcher; a parallel test's thread (it has none) hits the `job` / sqlx callsites first
    // and caches "never", and this test's exporter stays empty (only in parallel runs). With a
    // second, process-long dispatcher registered, interest is the union of all live dispatchers,
    // whichever thread hits the callsite.
    static KEEP: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
    KEEP.get_or_init(|| tracing::Dispatch::new(tracing_subscriber::registry()));
    let guard = tracing::subscriber::set_default(sub);
    // Callsites already cached by earlier hits are recomputed with this subscriber in the set.
    tracing::callsite::rebuild_interest_cache();
    (exporter, logs, guard)
}

fn tp(trace: &str, span: &str) -> String {
    format!("00-{trace}-{span}-01")
}

/// Is `ancestor` an ancestor (or the same span) of `s` in `by_id`?
pub(crate) fn descends(
    by_id: &HashMap<String, &SpanData>,
    s: &SpanData,
    ancestor: &SpanData,
) -> bool {
    let target = ancestor.span_context.span_id();
    let mut cur = s;
    for _ in 0..64 {
        if cur.span_context.span_id() == target {
            return true;
        }
        match by_id.get(&format!("{:016x}", cur.parent_span_id)) {
            Some(p) => cur = p,
            None => return false,
        }
    }
    false
}

pub(crate) fn index(spans: &[SpanData]) -> HashMap<String, &SpanData> {
    spans
        .iter()
        .map(|s| (format!("{:016x}", s.span_context.span_id()), s))
        .collect()
}

async fn call(app: &axum::Router, req: Request<Body>) -> (u16, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn one_trace_from_the_browser_post_through_the_queue_to_the_report_and_no_canary_in_any_span_or_log()
 {
    let r = rig().await;
    let lv = r.boot("tracee").await;
    // The boot turn is delivered and finished so the next message makes a fresh turn.
    let d = r.deliver().await;
    let boot_turn = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": boot_turn}))
        .await;
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": boot_turn, "status": "done"}),
    )
    .await;

    let (exporter, logs, _guard) = install();
    let app = traced(r.h.app.clone(), PUBLIC_TRUST);

    // 1. The browser POST, with a sampled browser traceparent.
    let post = Request::builder()
        .method(Method::POST)
        .uri(format!("/v1/conversations/{}/messages", lv.conv))
        .header("cookie", &r.cookie)
        .header("origin", crate::common::ORIGIN)
        .header("content-type", "application/json")
        .header("traceparent", tp(BROWSER_TRACE, BROWSER_SPAN))
        .header("x-canary", CANARY_HEADER)
        .body(Body::from(json!({"text": CANARY_TEXT}).to_string()))
        .unwrap();
    let (status, body) = call(&app, post).await;
    assert_eq!(status, 202, "{body}");
    // A read with a canary query value (an unknown parameter is ignored by the route).
    let get = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/v1/conversations/{}/messages?canary={CANARY_QUERY}",
            lv.conv
        ))
        .header("cookie", &r.cookie)
        .body(Body::empty())
        .unwrap();
    let _ = call(&app, get).await;

    let stored: String = sqlx::query_scalar(
        "SELECT trace_context FROM turns WHERE conversation_id = $1 AND role = 'turn' \
          AND status = 'queued' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(lv.conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    let sc = parse_traceparent(&stored).expect("a valid stored context");
    assert_eq!(format!("{:032x}", sc.trace_id()), BROWSER_TRACE);

    let spans = exporter.get_finished_spans().unwrap();
    let by_id = index(&spans);
    let server = spans
        .iter()
        .find(|s| s.name == "POST /v1/conversations/{id}/messages")
        .unwrap_or_else(|| {
            panic!(
                "server span missing: {:?}",
                spans.iter().map(|s| s.name.to_string()).collect::<Vec<_>>()
            )
        });
    assert_eq!(format!("{:016x}", server.parent_span_id), BROWSER_SPAN);
    assert_eq!(
        format!("{:032x}", server.span_context.trace_id()),
        BROWSER_TRACE
    );
    // The stored context is a span of this trace under the server span.
    let stored_span = by_id
        .get(&format!("{:016x}", sc.span_id()))
        .expect("the stored context names an exported span");
    assert!(descends(&by_id, stored_span, server), "{stored}");
    // The turn's database writes are descendants of the server span in the same trace.
    let db: Vec<&SpanData> = spans.iter().filter(|s| s.name == "INSERT turns").collect();
    assert!(!db.is_empty(), "no INSERT turns span");
    for s in &db {
        assert!(descends(&by_id, s, server));
        assert_eq!(s.span_context.trace_id(), server.span_context.trace_id());
    }

    // 2. The runner's work item carries exactly the stored context.
    let item = r.deliver().await;
    assert_eq!(item["trace_context"], json!(stored), "{item}");
    let turn = item["turn_id"].as_str().unwrap().to_owned();

    // 3. A report about the turn, sent with the runner's own span (a child of the stored context
    // in the real runner) as `traceparent`: handled in `report turn`, a child of the STORED one.
    let before = exporter.get_finished_spans().unwrap().len();
    let report = Request::builder()
        .method(Method::POST)
        .uri("/v1/runner/reports")
        .header("authorization", format!("Bearer {}", r.tok))
        .header("x-aurora-replica", r.rep.to_string())
        .header("content-type", "application/json")
        .header("traceparent", tp(BROWSER_TRACE, RUNNER_SPAN))
        .body(Body::from(
            json!({"reports": [{
                "report_id": uuid::Uuid::now_v7(), "sandbox_id": lv.sb, "boot_id": lv.boot,
                "kind": "turn.started", "turn_id": turn,
                "note": CANARY_REPORT,
            }]})
            .to_string(),
        ))
        .unwrap();
    let (status, body) = call(&app, report).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["results"][0]["status"], "applied", "{body}");

    let spans = exporter.get_finished_spans().unwrap();
    assert!(spans.len() > before);
    let by_id = index(&spans);
    let report_span = spans
        .iter()
        .find(|s| s.name == "report turn")
        .expect("report turn span");
    assert_eq!(
        format!("{:016x}", report_span.parent_span_id),
        format!("{:016x}", sc.span_id()),
        "the report span must hang under the stored context, not under the header"
    );
    assert_eq!(
        format!("{:032x}", report_span.span_context.trace_id()),
        BROWSER_TRACE
    );
    let under: Vec<&SpanData> = spans
        .iter()
        .filter(|s| {
            s.name != "report turn"
                && s.span_context.span_id() != report_span.span_context.span_id()
                && descends(&by_id, s, report_span)
        })
        .collect();
    assert!(
        under.iter().any(|s| s.name.starts_with("UPDATE ")
            || s.name.starts_with("SELECT ")
            || s.name.starts_with("INSERT ")),
        "the report's queries are database spans under it: {:?}",
        under.iter().map(|s| s.name.to_string()).collect::<Vec<_>>()
    );

    // 4. The allowlist: no canary and no credential anywhere in a span or a log line.
    let dump = spans
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    let log = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("\"request\""),
        "an access line was logged: {log}"
    );
    let cookie_value = r
        .cookie
        .split_once('=')
        .map_or(r.cookie.as_str(), |(_, v)| v);
    for secret in [
        CANARY_TEXT,
        CANARY_HEADER,
        CANARY_QUERY,
        CANARY_REPORT,
        cookie_value,
        r.tok.as_str(),
    ] {
        assert!(!dump.contains(secret), "{secret} leaked into a span");
        assert!(!log.contains(secret), "{secret} leaked into a log line");
    }
}
