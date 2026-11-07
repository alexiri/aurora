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
    EnvFilter,
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
        match buf.strip_prefix('{') {
            Some(rest) if rest.starts_with('}') => {
                write!(writer, "{}{rest}", self.prefix.trim_end_matches(','))
            }
            Some(rest) => write!(writer, "{}{rest}", self.prefix),
            None => writer.write_str(&buf),
        }
    }
}

/// Initialise JSON logging on stdout. Returns the generated instance id
/// (`HOSTNAME` if set, otherwise a random UUID).
///
/// `level` is an `EnvFilter` directive (e.g. `info`); `RUST_LOG` overrides it.
/// An invalid directive falls back to `info`.
pub fn init_tracing(service: &str, level: &str) -> String {
    init_telemetry(service, level, None)
}

/// [`init_tracing`] plus OTLP trace export when `otlp_endpoint` (`telemetry.otlp_endpoint`) is
/// set (docs/15 §Observability). Without an endpoint no exporter, provider or propagator is
/// installed. An exporter that cannot be built is reported on stderr and logging continues.
pub fn init_telemetry(service: &str, level: &str, otlp_endpoint: Option<&str>) -> String {
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
    let otel = otlp_endpoint.and_then(|ep| match crate::telemetry::install(service, ep) {
        Ok(tracer) => Some(tracing_opentelemetry::layer().with_tracer(tracer)),
        Err(e) => {
            eprintln!("telemetry.otlp_endpoint: exporter not started: {e}");
            None
        }
    });
    // Ignore the error: a subscriber may already be installed (tests).
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(otel)
        .with(tracing_subscriber::fmt::layer().event_format(formatter))
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

/// Middleware: run each request inside a span that continues the caller's `traceparent`.
/// Only the method is recorded (paths can carry identifiers). Inert without OTLP export.
pub async fn trace_context(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    use tracing::Instrument;
    let span = tracing::info_span!("http.request", method = %req.method());
    crate::telemetry::adopt_remote_parent(&span, req.headers());
    next.run(req).instrument(span).await
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
