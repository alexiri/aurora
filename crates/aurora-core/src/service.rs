//! Shared service bootstrap (decision I-09): JSON logging with `service` and
//! `instance`, Prometheus `/metrics`, `/healthz` + `/readyz` router, graceful
//! shutdown and the EX_CONFIG exit helper.

use crate::ApiError;
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::fmt::{self, Display};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{FmtContext, FormatEvent, FormatFields, format::Writer},
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
};

/// Process exit code for invalid configuration (`EX_CONFIG`).
pub const EX_CONFIG: i32 = 78;

/// Print `msg` to stderr and exit with [`EX_CONFIG`].
pub fn exit_config(msg: impl Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(EX_CONFIG)
}

/// Wraps the JSON event formatter so every line starts with `service` and `instance`.
struct WithIdentity<F> {
    inner: F,
    prefix: String,
}

impl<S, N, F> FormatEvent<S, N> for WithIdentity<F>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut buf = String::new();
        self.inner.format_event(ctx, Writer::new(&mut buf), event)?;
        // FIX-146 (D-232 (a)): `request_id` = the W3C trace id of the nearest enclosing span
        // that carries one, so `grep <x-request-id>` follows a request across services.
        let request_id = ctx.event_scope().and_then(|mut scope| {
            scope.find_map(|s| {
                s.extensions()
                    .get::<crate::telemetry::RequestId>()
                    .map(|r| r.0.clone())
            })
        });
        let prefix = match request_id {
            Some(id) => format!("{}\"request_id\":\"{id}\",", self.prefix),
            None => self.prefix.clone(),
        };
        match buf.strip_prefix('{') {
            Some(rest) if rest.starts_with('}') => {
                write!(writer, "{}{rest}", prefix.trim_end_matches(','))
            }
            Some(rest) => write!(writer, "{prefix}{rest}"),
            None => writer.write_str(&buf),
        }
    }
}

/// The log-level filter for events. Every span stays visible to the formatter whatever the
/// level, so an event's `request_id` (taken from its span scope) does not depend on the level.
fn log_filter<S>(level: EnvFilter) -> impl tracing_subscriber::layer::Filter<S> + 'static
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    use tracing_subscriber::filter::{FilterExt, filter_fn};
    filter_fn(|m| m.is_span()).or(level)
}

/// Initialise JSON logging on stdout. Returns the generated instance id
/// (`HOSTNAME` if set, otherwise a random UUID).
///
/// `level` is an `EnvFilter` directive (e.g. `info`); `RUST_LOG` overrides it.
/// An invalid directive falls back to `info`.
pub fn init_tracing(service: &str, level: &str) -> String {
    init_telemetry(service, level, None, 1.0)
}

/// [`init_tracing`] plus tracing (docs/15 §Observability, D-232). The W3C propagator and a tracer
/// provider with the sampling rule (`sample_ratio` = `telemetry.sample_ratio`) are always
/// installed, so every request has a trace id (the `request_id` of its log lines) and
/// propagates it; the OTLP exporter is added only when `otlp_endpoint`
/// (`telemetry.otlp_endpoint`) is set. An exporter that cannot be built is reported on stderr
/// and logging continues.
pub fn init_telemetry(
    service: &str,
    level: &str,
    otlp_endpoint: Option<&str>,
    sample_ratio: f64,
) -> String {
    let instance = std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let prefix = format!(
        "{{\"service\":{},\"instance\":{},",
        serde_json::Value::from(service),
        serde_json::Value::from(instance.as_str())
    );
    let formatter = WithIdentity {
        inner: tracing_subscriber::fmt::format().json().flatten_event(true),
        prefix,
    };
    let tracer = crate::telemetry::install(service, otlp_endpoint, sample_ratio);
    let db_spans = crate::telemetry::DbSpanLayer::new(tracer.clone());
    let otel = tracing_opentelemetry::layer().with_tracer(tracer);
    // Ignore the error: a subscriber may already be installed (tests).
    // The OTel layer sees spans only (never log events, which would become span events and could
    // carry message text, SEC-88); its own filter keeps the operator's log level out of it.
    let spans_only = tracing_subscriber::filter::filter_fn(|m| m.is_span());
    let _ = tracing_subscriber::registry()
        .with(otel.with_filter(spans_only))
        // Database spans (D-232 (6)): its own filter, so SQL never reaches stdout.
        .with(db_spans.with_filter(crate::telemetry::DbSpanLayer::filter()))
        .with(
            tracing_subscriber::fmt::layer()
                // Span fields must be JSON too: the JSON event formatter parses them back and
                // panics (debug builds) on the default `key=value` text.
                .fmt_fields(tracing_subscriber::fmt::format::JsonFields::new())
                .event_format(formatter)
                .with_filter(log_filter(filter)),
        )
        .try_init();
    instance
}

/// Install the global Prometheus recorder and mark the service as up.
/// Returns `None` if a recorder is already installed in this process.
pub fn install_metrics(service: &'static str) -> Option<PrometheusHandle> {
    let handle = PrometheusBuilder::new().install_recorder().ok()?;
    metrics::gauge!("aurora_service_up", "service" => service).set(1.0);
    Some(handle)
}

/// Future returned by a readiness check; `Err` carries a short reason.
pub type CheckFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
/// A named, pluggable readiness check.
pub type ReadyCheck = Arc<dyn Fn() -> CheckFuture + Send + Sync>;

/// Build a [`ReadyCheck`] from an async closure.
pub fn ready_check<F, Fut>(f: F) -> ReadyCheck
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    Arc::new(move || Box::pin(f()))
}

const CHECK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone)]
struct OpsState {
    metrics: Option<PrometheusHandle>,
    checks: Arc<Vec<(&'static str, ReadyCheck)>>,
}

/// Router with `/healthz` (always 200), `/readyz` (200 iff every check passes,
/// else 503 listing failing check names) and `/metrics`. Unknown paths get the
/// 404 error envelope.
pub fn ops_router(
    metrics: Option<PrometheusHandle>,
    checks: Vec<(&'static str, ReadyCheck)>,
) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_handler))
        .fallback(not_found)
        .with_state(OpsState {
            metrics,
            checks: Arc::new(checks),
        })
}

/// Flush pending OTLP spans before exit (no-op when export is not configured).
pub fn telemetry_shutdown() {
    crate::telemetry::shutdown();
}

pub use crate::telemetry::Trust;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Which trace-context trust applies to which request path (SEC-88): the first matching prefix of
/// `overrides` wins, otherwise `default`. Routers are built with `const` values so the table can
/// be pinned by a test.
#[derive(Debug, Clone, Copy)]
pub struct TraceTrust {
    pub default: Trust,
    pub overrides: &'static [(&'static str, Trust)],
}

impl TraceTrust {
    /// One trust for every path.
    pub const fn fixed(default: Trust) -> Self {
        Self {
            default,
            overrides: &[],
        }
    }

    /// The trust for a request `path`.
    pub fn resolve(&self, path: &str) -> Trust {
        self.overrides
            .iter()
            .find(|(prefix, _)| path.starts_with(prefix))
            .map_or(self.default, |(_, t)| *t)
    }
}

/// Wrap `router` in [`trace_requests`] with `trust`. Apply it last, so it is the outermost layer.
pub fn traced(router: Router, trust: TraceTrust) -> Router {
    router.layer(axum::middleware::from_fn_with_state(trust, trace_requests))
}

/// Trust for internal listeners (ops, internal APIs of Aurora's own services).
pub const INTERNAL_TRUST: TraceTrust = TraceTrust::fixed(Trust::Parent);
/// Trust for listeners reached by third parties (provider callbacks, webhooks): never adopted.
pub const UNTRUSTED_TRUST: TraceTrust = TraceTrust::fixed(Trust::Ignore);

/// Middleware (D-232 (b)/(c), SEC-88): run each request in a server span named
/// `"{METHOD} {route}"` (the route template, never the raw path) that applies `TraceTrust` to
/// the incoming `traceparent`, tag its log lines with the request id (the trace id), emit one
/// `info` access-log line and return `x-request-id`. `/healthz`, `/readyz` and `/metrics` get no
/// span and no line. Use as `from_fn_with_state(trust, trace_requests)` on the outermost layer.
pub async fn trace_requests(
    State(trust): State<TraceTrust>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use futures::FutureExt;
    use tracing::Instrument;
    if matches!(req.uri().path(), "/healthz" | "/readyz" | "/metrics") {
        return next.run(req).await;
    }
    let method = req.method().as_str().to_owned();
    let route = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map_or("unmatched", |m| m.as_str())
        .to_owned();
    let span = tracing::info_span!(
        parent: None,
        "http.request",
        otel.name = %format!("{method} {route}"),
        otel.kind = "server",
        http.request.method = %method,
        http.route = %route,
    );
    crate::telemetry::adopt(&span, req.headers(), trust.resolve(req.uri().path()));
    let request_id = crate::telemetry::tag_request_id(&span);
    let started = std::time::Instant::now();
    let outcome = std::panic::AssertUnwindSafe(next.run(req).instrument(span.clone()))
        .catch_unwind()
        .await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut res = match outcome {
        Ok(res) => res,
        Err(panic) => {
            span.set_status(opentelemetry::trace::Status::error("panic"));
            span.in_scope(|| {
                tracing::error!(%method, %route, duration_ms = elapsed_ms, "request panicked");
            });
            std::panic::resume_unwind(panic);
        }
    };
    let status = res.status().as_u16();
    span.set_attribute("http.response.status_code", i64::from(status));
    if status >= 400
        && let Some(code) = res.extensions().get::<crate::error::ErrorCode>()
    {
        span.set_attribute("aurora.error_code", code.0.clone());
    }
    if status >= 500 {
        span.set_status(opentelemetry::trace::Status::error("server error"));
    }
    span.in_scope(|| {
        tracing::info!(%method, %route, status, duration_ms = elapsed_ms, "request");
    });
    if let Some(id) = request_id
        && let Ok(v) = axum::http::HeaderValue::from_str(&id)
    {
        res.headers_mut().insert("x-request-id", v);
    }
    res
}

/// 404 error-envelope handler for use as a router fallback.
pub async fn not_found() -> Response {
    ApiError::not_found("No such route.").into_response()
}

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz(State(st): State<OpsState>) -> Response {
    let mut failing = Vec::new();
    for (name, check) in st.checks.iter() {
        match tokio::time::timeout(CHECK_TIMEOUT, check()).await {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => {
                tracing::warn!(check = *name, %reason, "readiness check failed");
                failing.push(*name);
            }
            Err(_) => {
                tracing::warn!(check = *name, "readiness check timed out");
                failing.push(*name);
            }
        }
    }
    if failing.is_empty() {
        (StatusCode::OK, "ready").into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("not ready: {}", failing.join(",")),
        )
            .into_response()
    }
}

async fn metrics_handler(State(st): State<OpsState>) -> Response {
    let body = st.metrics.as_ref().map(PrometheusHandle::render);
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body.unwrap_or_default(),
    )
        .into_response()
}

/// Resolves on SIGINT or SIGTERM.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("shutdown signal received");
}

/// Bind `bind:port` (`bind` is an IP address, validated by config).
pub async fn bind(bind: &str, port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let ip: std::net::IpAddr = bind
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    tokio::net::TcpListener::bind(SocketAddr::new(ip, port)).await
}

/// Serve every `(listener, router)` pair until SIGINT/SIGTERM, then drain all
/// of them. Returns the first server error, if any.
pub async fn serve_all(servers: Vec<(tokio::net::TcpListener, Router)>) -> std::io::Result<()> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = tx.send(true);
    });
    let mut set = tokio::task::JoinSet::new();
    for (listener, app) in servers {
        if let Ok(addr) = listener.local_addr() {
            tracing::info!(%addr, "listening");
        }
        let mut rx = rx.clone();
        set.spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.wait_for(|stop| *stop).await;
                })
                .await
        });
    }
    let mut result = Ok(());
    while let Some(joined) = set.join_next().await {
        let r = joined.unwrap_or_else(|e| Err(std::io::Error::other(e)));
        if r.is_err() && result.is_ok() {
            result = r;
        }
    }
    tracing::info!("stopped");
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::telemetry;
    use axum::body::Body;
    use axum::http::Request;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
    use std::sync::Mutex;
    use tower::ServiceExt;

    // Ratio 0.5: ids whose bytes 9..16 start below 0x80 are sampled by the rule.
    const PASS_ID: &str = "aaaaaaaaaaaaaaaaaa7fffffffffffff";
    const FAIL_ID: &str = "aaaaaaaaaaaaaaaaaa80000000000000";
    const PARENT_SPAN: &str = "00f067aa0ba902b7";

    const TABLE: TraceTrust = TraceTrust {
        default: Trust::Browser,
        overrides: &[
            ("/v1/runner/", Trust::Parent),
            ("/scim/", Trust::Ignore),
            ("/v1/connector/", Trust::Link),
        ],
    };

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct Harness {
        spans: InMemorySpanExporter,
        logs: Buf,
        _guard: tracing::subscriber::DefaultGuard,
    }

    fn harness() -> Harness {
        harness_at("info")
    }

    fn harness_at(level: &str) -> Harness {
        telemetry::set_sample_ratio(0.5);
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        let spans = InMemorySpanExporter::default();
        let provider = telemetry::provider_builder("test", 0.5)
            .with_simple_exporter(spans.clone())
            .build();
        let logs = Buf::default();
        let w = logs.clone();
        let formatter = WithIdentity {
            inner: tracing_subscriber::fmt::format().json().flatten_event(true),
            prefix: "{\"service\":\"test\",".into(),
        };
        let sub = tracing_subscriber::registry()
            .with(
                tracing_opentelemetry::layer()
                    .with_tracer(provider.tracer("t"))
                    .with_filter(tracing_subscriber::filter::filter_fn(|m| m.is_span())),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .fmt_fields(tracing_subscriber::fmt::format::JsonFields::new())
                    .event_format(formatter)
                    .with_writer(move || w.clone())
                    // A deliberately strict log level: request ids must not depend on it.
                    .with_filter(log_filter(EnvFilter::new(level))),
            );
        Harness {
            spans,
            logs,
            _guard: tracing::subscriber::set_default(sub),
        }
    }

    impl Harness {
        fn lines(&self) -> Vec<serde_json::Value> {
            let raw = String::from_utf8(self.logs.0.lock().unwrap().clone()).unwrap();
            raw.lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
        fn spans(&self) -> Vec<SpanData> {
            self.spans.get_finished_spans().unwrap()
        }
    }

    fn app(trust: TraceTrust) -> Router {
        Router::new()
            .route("/v1/things/{id}", get(|| async { "ok" }))
            .route("/v1/runner/work", get(|| async { "ok" }))
            .route("/v1/connector/events", get(|| async { "ok" }))
            .route("/scim/v2/Users", get(|| async { "ok" }))
            .route(
                "/v1/boom",
                get(|| async { Err::<&str, _>(ApiError::conflict("handle_taken", "Taken.")) }),
            )
            .route(
                "/v1/fail",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .merge(ops_router(None, Vec::new()))
            .fallback(not_found)
            .layer(axum::middleware::from_fn_with_state(trust, trace_requests))
    }

    async fn call(app: &Router, path: &str, traceparent: Option<String>) -> Response {
        let mut req = Request::builder().uri(path);
        if let Some(tp) = traceparent {
            req = req
                .header("traceparent", tp)
                .header("tracestate", "vendor=x");
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn rid(res: &Response) -> String {
        res.headers()["x-request-id"].to_str().unwrap().to_owned()
    }

    fn tp(id: &str, flag: &str) -> String {
        format!("00-{id}-{PARENT_SPAN}-{flag}")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_id_header_log_line_and_route_template() {
        let h = harness();
        let res = call(&app(TABLE), "/v1/things/42-secret-id?q=canary", None).await;
        let id = rid(&res);
        assert_eq!(id.len(), 32);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        let lines = h.lines();
        let line = lines.iter().find(|l| l["message"] == "request").unwrap();
        assert_eq!(line["request_id"], id.as_str());
        assert_eq!(line["service"], "test");
        assert_eq!(line["method"], "GET");
        assert_eq!(line["route"], "/v1/things/{id}");
        assert_eq!(line["status"], 200);
        assert!(line["duration_ms"].is_number());
        // No raw path or query string anywhere in the line.
        let raw = line.to_string();
        assert!(
            !raw.contains("42-secret-id") && !raw.contains("canary"),
            "{raw}"
        );
        let spans = h.spans();
        let s = spans
            .iter()
            .find(|s| s.name == "GET /v1/things/{id}")
            .unwrap();
        assert_eq!(format!("{:032x}", s.span_context.trace_id()), id);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn browser_adopts_the_trace_id_but_the_rule_decides() {
        let h = harness();
        let app = app(TABLE);
        // Fails the rule although the browser says "sampled": not exported, id still adopted.
        let res = call(&app, "/v1/things/1", Some(tp(FAIL_ID, "01"))).await;
        assert_eq!(rid(&res), FAIL_ID);
        assert!(h.spans().is_empty(), "a browser must not force sampling");
        // Passes the rule although the browser says "not sampled": exported as the child.
        let res = call(&app, "/v1/things/1", Some(tp(PASS_ID, "00"))).await;
        assert_eq!(rid(&res), PASS_ID);
        let spans = h.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(format!("{:016x}", spans[0].parent_span_id), PARENT_SPAN);
        assert!(spans[0].span_context.trace_state().header().is_empty());
        // The request id is in the log even for the unsampled trace.
        let lines = h.lines();
        assert!(lines.iter().any(|l| l["request_id"] == FAIL_ID));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn machine_route_is_parent_based() {
        let h = harness();
        let app = app(TABLE);
        let res = call(&app, "/v1/runner/work", Some(tp(FAIL_ID, "01"))).await;
        assert_eq!(rid(&res), FAIL_ID);
        assert_eq!(
            h.spans().len(),
            1,
            "parent sampled => sampled, whatever the rule says"
        );
        let res = call(&app, "/v1/runner/work", Some(tp(PASS_ID, "00"))).await;
        assert_eq!(rid(&res), PASS_ID);
        assert_eq!(h.spans().len(), 1, "parent unsampled => unsampled");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ignored_and_linked_routes_start_a_new_trace() {
        let h = harness();
        let app = app(TABLE);
        let res = call(&app, "/scim/v2/Users", Some(tp(PASS_ID, "01"))).await;
        assert_ne!(rid(&res), PASS_ID);
        let res = call(&app, "/v1/connector/events", Some(tp(PASS_ID, "01"))).await;
        assert_ne!(rid(&res), PASS_ID);
        for s in h.spans() {
            assert_ne!(format!("{:032x}", s.span_context.trace_id()), PASS_ID);
            assert_eq!(s.parent_span_id, opentelemetry::trace::SpanId::INVALID);
        }
        let spans = h.spans();
        let linked = spans.iter().find(|s| s.name == "GET /v1/connector/events");
        if let Some(s) = linked {
            let ids: Vec<_> = s.links.iter().map(|l| l.span_context.trace_id()).collect();
            assert!(
                ids.iter().any(|t| format!("{t:032x}") == PASS_ID),
                "{ids:?}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn health_probes_have_no_span_line_or_request_id() {
        let h = harness();
        for p in ["/healthz", "/readyz", "/metrics"] {
            let res = call(&app(TABLE), p, None).await;
            assert!(res.headers().get("x-request-id").is_none(), "{p}");
        }
        assert!(h.spans().is_empty());
        assert!(h.lines().iter().all(|l| l["message"] != "request"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn errors_unmatched_routes_and_5xx_are_recorded() {
        let h = harness();
        let app = app(TABLE);
        let res = call(&app, "/v1/nope/abc123", Some(tp(PASS_ID, "00"))).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(rid(&res), PASS_ID);
        let res = call(&app, "/v1/boom", Some(tp(PASS_ID, "00"))).await;
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let res = call(&app, "/v1/fail", Some(tp(PASS_ID, "00"))).await;
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let spans = h.spans();
        let attr = |s: &SpanData, k: &str| {
            s.attributes
                .iter()
                .find(|kv| kv.key.as_str() == k)
                .map(|kv| kv.value.to_string())
        };
        let unmatched = spans.iter().find(|s| s.name == "GET unmatched").unwrap();
        assert_eq!(
            attr(unmatched, "aurora.error_code").as_deref(),
            Some("not_found")
        );
        assert_eq!(attr(unmatched, "http.route").as_deref(), Some("unmatched"));
        let boom = spans.iter().find(|s| s.name == "GET /v1/boom").unwrap();
        assert_eq!(
            attr(boom, "aurora.error_code").as_deref(),
            Some("handle_taken")
        );
        assert_eq!(
            attr(boom, "http.response.status_code").as_deref(),
            Some("409")
        );
        let fail = spans.iter().find(|s| s.name == "GET /v1/fail").unwrap();
        assert!(matches!(
            fail.status,
            opentelemetry::trace::Status::Error { .. }
        ));
        assert!(!matches!(
            boom.status,
            opentelemetry::trace::Status::Error { .. }
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_id_does_not_depend_on_the_log_level() {
        let h = harness_at("warn");
        let app = Router::new()
            .route(
                "/v1/w",
                get(|| async {
                    tracing::warn!("something odd");
                    "ok"
                }),
            )
            .layer(axum::middleware::from_fn_with_state(TABLE, trace_requests));
        let res = call(&app, "/v1/w", None).await;
        let lines = h.lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["message"], "something odd");
        assert_eq!(lines[0]["request_id"], rid(&res).as_str());
    }

    #[test]
    fn trust_table_resolution() {
        assert_eq!(TABLE.resolve("/v1/runner/work"), Trust::Parent);
        assert_eq!(TABLE.resolve("/scim/v2/Users"), Trust::Ignore);
        assert_eq!(TABLE.resolve("/v1/connector/events"), Trust::Link);
        assert_eq!(TABLE.resolve("/v1/connectors/x/icon"), Trust::Browser);
        assert_eq!(
            TraceTrust::fixed(Trust::Parent).resolve("/x"),
            Trust::Parent
        );
    }
}
