//! OTLP trace export and W3C `traceparent` propagation (docs/15 §Observability).
//!
//! Everything here is inert unless `telemetry.otlp_endpoint` is configured: without
//! [`install`] no provider or propagator is registered, so [`inject`] and [`extract`] do
//! nothing and no network connection is ever made.

use http::{HeaderMap, HeaderName, HeaderValue};
use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::{
    Link, SpanContext, SpanKind, TraceContextExt, TraceFlags, TraceId, TraceState,
    TracerProvider as _,
};
use opentelemetry::{Context, KeyValue, global};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{
    SamplingDecision, SamplingResult, SdkTracer, SdkTracerProvider, ShouldSample,
    TracerProviderBuilder,
};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing_opentelemetry::OpenTelemetrySpanExt;

static PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

/// 2^56: the sampling threshold of ratio 1.0 (every 56-bit value is below it).
const SAMPLE_ONE: u64 = 1 << 56;
static THRESHOLD: AtomicU64 = AtomicU64::new(SAMPLE_ONE);

/// `floor(ratio × 2^56)` (C-732 (7)); ratios outside 0.0..=1.0 clamp.
pub fn sample_threshold(ratio: f64) -> u64 {
    if ratio.is_nan() || ratio <= 0.0 {
        0
    } else if ratio >= 1.0 {
        SAMPLE_ONE
    } else {
        (ratio * SAMPLE_ONE as f64).floor() as u64
    }
}

/// The sampling rule (C-732 (7)): the trace id's rightmost 7 bytes (bytes 9..16, big-endian)
/// are below `threshold`. The same formula runs in the dashboard (`web/src/lib/telemetry`).
pub fn rule_sampled(trace_id: TraceId, threshold: u64) -> bool {
    let b = trace_id.to_bytes();
    let mut v = 0u64;
    for byte in &b[9..16] {
        v = (v << 8) | u64::from(*byte);
    }
    v < threshold
}

/// Set the process-wide threshold from `telemetry.sample_ratio` (done by [`install`]).
pub fn set_sample_ratio(ratio: f64) {
    THRESHOLD.store(sample_threshold(ratio), Ordering::Relaxed);
}

/// [`rule_sampled`] with the process-wide threshold.
pub fn sampled_by_rule(trace_id: TraceId) -> bool {
    rule_sampled(trace_id, THRESHOLD.load(Ordering::Relaxed))
}

/// Root spans follow the sampling rule; a span with a parent follows the parent's flag.
#[derive(Debug, Clone)]
pub struct RuleSampler {
    threshold: u64,
}

impl RuleSampler {
    pub fn new(ratio: f64) -> Self {
        Self {
            threshold: sample_threshold(ratio),
        }
    }
}

impl ShouldSample for RuleSampler {
    fn should_sample(
        &self,
        parent_context: Option<&Context>,
        trace_id: TraceId,
        _name: &str,
        _span_kind: &SpanKind,
        _attributes: &[KeyValue],
        _links: &[Link],
    ) -> SamplingResult {
        let parent = parent_context
            .map(|c| c.span().span_context().clone())
            .filter(SpanContext::is_valid);
        let (sampled, trace_state) = match parent {
            Some(sc) => (sc.is_sampled(), sc.trace_state().clone()),
            None => (
                rule_sampled(trace_id, self.threshold),
                TraceState::default(),
            ),
        };
        SamplingResult {
            decision: if sampled {
                SamplingDecision::RecordAndSample
            } else {
                SamplingDecision::Drop
            },
            attributes: Vec::new(),
            trace_state,
        }
    }
}

/// Full OTLP/HTTP traces URL: a bare collector base URL gets `/v1/traces` appended.
pub fn traces_url(endpoint: &str) -> String {
    let e = endpoint.trim_end_matches('/');
    if e.ends_with("/v1/traces") {
        e.to_owned()
    } else {
        format!("{e}/v1/traces")
    }
}

/// Provider builder with the resource and the rule sampler; the caller adds an exporter.
pub fn provider_builder(service: &str, ratio: f64) -> TracerProviderBuilder {
    let resource = Resource::builder()
        .with_service_name(service.to_owned())
        .with_attribute(KeyValue::new("service.namespace", "aurora"))
        .build();
    SdkTracerProvider::builder()
        .with_resource(resource)
        .with_sampler(RuleSampler::new(ratio))
}

/// Build an OTLP/HTTP protobuf span exporter. It connects lazily, so no collector is needed.
pub fn build_exporter(endpoint: &str) -> Result<SpanExporter, String> {
    let url = traces_url(endpoint);
    // The blocking HTTP client must not be created inside an async runtime thread.
    std::thread::spawn(move || {
        SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(url)
            .build()
    })
    .join()
    .map_err(|_| "exporter thread panicked".to_owned())?
    .map_err(|e| e.to_string())
}

/// Build (but do not register) a tracer provider exporting to `endpoint`.
pub fn build_provider(service: &str, endpoint: &str) -> Result<SdkTracerProvider, String> {
    let exporter = build_exporter(endpoint)?;
    Ok(provider_builder(service, 1.0)
        .with_batch_exporter(exporter)
        .build())
}

/// Always register the W3C propagator and a tracer provider with the sampling rule (D-232 (a)),
/// so every request has a trace id and propagates it. An exporter is added only with an
/// `endpoint`; one that cannot be built is reported on stderr and the provider stays
/// export-less. Returns the tracer for the `tracing` layer.
pub fn install(service: &str, endpoint: Option<&str>, ratio: f64) -> SdkTracer {
    let mut builder = provider_builder(service, ratio);
    if let Some(ep) = endpoint {
        match build_exporter(ep) {
            Ok(exporter) => builder = builder.with_batch_exporter(exporter),
            Err(e) => eprintln!("telemetry.otlp_endpoint: exporter not started: {e}"),
        }
    }
    let provider = builder.build();
    let tracer = provider.tracer("aurora");
    global::set_text_map_propagator(TraceContextPropagator::new());
    global::set_tracer_provider(provider.clone());
    set_sample_ratio(ratio);
    let _ = PROVIDER.set(provider);
    tracer
}

/// Flush pending spans (best effort). A no-op when export was never configured.
pub fn shutdown() {
    if let Some(p) = PROVIDER.get() {
        let _ = p.shutdown();
    }
}

/// Where the API hands the spans it rebuilt from a browser's batch (FIX-146 Part D, D-232 (j)):
/// the resource of the batch and the finished spans. It never blocks the caller.
pub type SpanSink = std::sync::Arc<
    dyn Fn(opentelemetry_sdk::Resource, Vec<opentelemetry_sdk::trace::SpanData>) + Send + Sync,
>;

/// Pending batches the OTLP sink worker holds; more are dropped (telemetry is best effort).
const SINK_QUEUE: usize = 64;

/// A [`SpanSink`] exporting to `endpoint` with its own OTLP exporter on a worker thread. The blocking
/// HTTP client of the exporter must not run on an async runtime thread, so the batches cross a bounded
/// channel; when the collector is slow and the queue is full, the new batch is dropped.
pub fn otlp_span_sink(endpoint: &str) -> Result<SpanSink, String> {
    use opentelemetry_sdk::trace::SpanExporter as _;
    let mut exporter = build_exporter(endpoint)?;
    let (tx, rx) = std::sync::mpsc::sync_channel::<(
        opentelemetry_sdk::Resource,
        Vec<opentelemetry_sdk::trace::SpanData>,
    )>(SINK_QUEUE);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("browser-spans".into())
        .spawn(move || {
            while let Ok((resource, batch)) = rx.recv() {
                // The resource of a batch carries its user, so it is set per export.
                exporter.set_resource(&resource);
                let _ = rt.block_on(exporter.export(batch));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(std::sync::Arc::new(move |resource, batch| {
        let _ = tx.try_send((resource, batch));
    }))
}

struct HeaderCarrier<'a>(&'a mut HeaderMap);

impl Injector for HeaderCarrier<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(k), Ok(v)) = (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            self.0.insert(k, v);
        }
    }
}

struct HeaderReader<'a>(&'a HeaderMap);

impl Extractor for HeaderReader<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(HeaderName::as_str).collect()
    }
}

/// Write `cx` as `traceparent`/`tracestate` into `headers` with `propagator`.
pub fn inject_context(propagator: &impl TextMapPropagator, cx: &Context, headers: &mut HeaderMap) {
    propagator.inject_context(cx, &mut HeaderCarrier(headers));
}

/// Read a remote context from `headers` with `propagator`.
pub fn extract_context(propagator: &impl TextMapPropagator, headers: &HeaderMap) -> Context {
    propagator.extract(&HeaderReader(headers))
}

/// Propagate the current span to an outgoing request (global propagator; no-op when export is
/// not configured).
pub fn inject(headers: &mut HeaderMap) {
    let cx = tracing::Span::current().context();
    global::get_text_map_propagator(|p| p.inject_context(&cx, &mut HeaderCarrier(headers)));
}

/// `traceparent` headers for an outgoing request (empty when export is not configured).
pub fn headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    inject(&mut h);
    h
}

/// How an incoming `traceparent`/`tracestate` is treated (SEC-88, D-232 (c)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Internal listeners and token-authenticated machine routes: adopted, parent-based.
    Parent,
    /// Browser-facing routes: trace id and parent span id kept, `tracestate` dropped, the
    /// sampled flag ignored and the sampling rule decides.
    Browser,
    /// Not adopted: a new trace starts.
    Ignore,
    /// Not adopted, but the incoming context is recorded as a span link (external connectors).
    Link,
}

/// Apply `trust` to a not yet started request `span` (a no-op for [`Trust::Ignore`] and when the
/// headers carry no valid `traceparent`).
pub fn adopt(span: &tracing::Span, headers: &HeaderMap, trust: Trust) {
    if trust == Trust::Ignore {
        return;
    }
    let incoming = global::get_text_map_propagator(|p| p.extract(&HeaderReader(headers)));
    let sc = incoming.span().span_context().clone();
    if !sc.is_valid() {
        return;
    }
    match trust {
        Trust::Parent => {
            let _ = span.set_parent(incoming);
        }
        Trust::Browser => {
            let flags = if sampled_by_rule(sc.trace_id()) {
                TraceFlags::SAMPLED
            } else {
                TraceFlags::NOT_SAMPLED
            };
            let cx = Context::new().with_remote_span_context(SpanContext::new(
                sc.trace_id(),
                sc.span_id(),
                flags,
                true,
                TraceState::default(),
            ));
            let _ = span.set_parent(cx);
        }
        Trust::Link => span.add_link(sc),
        Trust::Ignore => {}
    }
}

/// The request id of a log line: the W3C trace id of the span it was emitted in (32 hex chars).
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// The trace id of `span`, if it has a valid OTel context (starts the span's OTel side).
pub fn span_trace_id(span: &tracing::Span) -> Option<TraceId> {
    let cx = span.context();
    let sc = cx.span().span_context().clone();
    sc.is_valid().then(|| sc.trace_id())
}

/// Attach `request_id` to `span` so every JSON log line emitted inside it (or a child) carries
/// it. Needs the `tracing` registry subscriber `init_telemetry` installs; a no-op otherwise.
pub fn set_request_id(span: &tracing::Span, request_id: &str) {
    use tracing_subscriber::registry::LookupSpan;
    span.with_subscriber(|(id, dispatch)| {
        if let Some(reg) = dispatch.downcast_ref::<tracing_subscriber::Registry>()
            && let Some(data) = reg.span(id)
        {
            data.extensions_mut()
                .replace(RequestId(request_id.to_owned()));
        }
    });
}

/// Give `span` its request id (its trace id, lowercase hex) and return it.
pub fn tag_request_id(span: &tracing::Span) -> Option<String> {
    let id = format!("{:032x}", span_trace_id(span)?);
    set_request_id(span, &id);
    Some(id)
}

/// The W3C `traceparent` (`00-<32 hex>-<16 hex>-<2 hex>`) of the current span, if it has a valid
/// context. Stored in `turns.trace_context` (D-232 (7)). Not usable inside a tracing layer callback.
pub fn current_traceparent() -> Option<String> {
    let cx = tracing::Span::current().context();
    let sc = cx.span().span_context().clone();
    sc.is_valid().then(|| {
        format!(
            "00-{:032x}-{:016x}-{:02x}",
            sc.trace_id(),
            sc.span_id(),
            sc.trace_flags().to_u8()
        )
    })
}

/// Parse a stored/transported `traceparent` into a remote span context; `None` when malformed.
pub fn parse_traceparent(tp: &str) -> Option<SpanContext> {
    let b = tp.as_bytes();
    if b.len() != 55 || !tp.starts_with("00-") || b[35] != b'-' || b[52] != b'-' {
        return None;
    }
    let lower_hex = |s: &str| {
        s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    };
    let (t, p, f) = (&tp[3..35], &tp[36..52], &tp[53..55]);
    if !(lower_hex(t) && lower_hex(p) && lower_hex(f)) {
        return None;
    }
    let sc = SpanContext::new(
        TraceId::from_hex(t).ok()?,
        opentelemetry::trace::SpanId::from_hex(p).ok()?,
        TraceFlags::new(u8::from_str_radix(f, 16).ok()?),
        true,
        TraceState::default(),
    );
    sc.is_valid().then_some(sc)
}

/// Make the not yet started `span` a child of a stored turn context (`turns.trace_context`,
/// work-item `trace_context`, `gateway_credentials.trace_context`) and tag its request id.
/// Returns false (and changes nothing) when `tp` is absent or malformed. The value must come from
/// the database or a trusted Aurora peer, never from a sandbox (SEC-88).
pub fn adopt_turn_context(span: &tracing::Span, tp: Option<&str>) -> bool {
    let Some(sc) = tp.and_then(parse_traceparent) else {
        return false;
    };
    let _ = span.set_parent(Context::new().with_remote_span_context(sc));
    tag_request_id(span);
    true
}

/// `builder.send_internal("aurora-api")`: [`send`] as a method, a drop-in for `.send()` on a call
/// to another Aurora service.
pub trait InternalRequest {
    fn send_internal(
        self,
        peer: &'static str,
    ) -> impl std::future::Future<Output = Result<reqwest::Response, reqwest::Error>> + Send;
}

impl InternalRequest for reqwest::RequestBuilder {
    fn send_internal(
        self,
        peer: &'static str,
    ) -> impl std::future::Future<Output = Result<reqwest::Response, reqwest::Error>> + Send {
        send(self, peer)
    }
}

/// Send a request to **another Aurora service** (D-232 (5)): records a client span named
/// `"{METHOD} {peer}"` (`peer` is a fixed service name, never a URL) and injects the context into
/// the request. Never use it for upstream apps, LLM providers, IdPs, push services or chat
/// platforms: those must not receive an Aurora-generated `traceparent`.
pub async fn send(
    builder: reqwest::RequestBuilder,
    peer: &'static str,
) -> Result<reqwest::Response, reqwest::Error> {
    use tracing::Instrument;
    let (client, request) = builder.build_split();
    let mut request = request?;
    let method = request.method().as_str().to_owned();
    let span = tracing::info_span!(
        "http.client",
        otel.name = %format!("{method} {peer}"),
        otel.kind = "client",
        peer.service = peer,
        http.request.method = %method,
        http.response.status_code = tracing::field::Empty,
    );
    inject_context(
        &TraceContextPropagator::new(),
        &span.context(),
        request.headers_mut(),
    );
    let result = client.execute(request).instrument(span.clone()).await;
    match &result {
        Ok(res) => {
            span.set_attribute(
                "http.response.status_code",
                i64::from(res.status().as_u16()),
            );
            if res.status().is_server_error() {
                span.set_status(opentelemetry::trace::Status::error("server error"));
            }
        }
        Err(_) => span.set_status(opentelemetry::trace::Status::error("request failed")),
    }
    result
}

/// Root span of a background job iteration that claimed work (`job {name}`, D-232 (6)): a new
/// trace, tagged with its `request_id`, so the iteration's queries become its children. An
/// iteration that finds nothing must not call this (no span, no trace).
pub fn job_span(name: &str) -> tracing::Span {
    let span = tracing::info_span!(
        parent: None,
        "job",
        otel.name = %format!("job {name}"),
        otel.kind = "internal",
        job.name = name,
    );
    tag_request_id(&span);
    span
}

/// Fields of one `sqlx::query` event.
#[derive(Default)]
struct DbEvent {
    summary: String,
    statement: String,
    rows_affected: Option<u64>,
    rows_returned: Option<u64>,
    elapsed_secs: f64,
    slow: bool,
}

impl tracing::field::Visit for DbEvent {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "summary" => self.summary = value.to_owned(),
            "db.statement" => self.statement = value.to_owned(),
            _ => {}
        }
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        match field.name() {
            "rows_affected" => self.rows_affected = Some(value),
            "rows_returned" => self.rows_returned = Some(value),
            _ => {}
        }
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        if let Ok(v) = u64::try_from(value) {
            self.record_u64(field, v);
        }
    }
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.elapsed_secs = value;
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {
        // sqlx adds `slow_threshold` to a statement that exceeded the alert threshold.
        if field.name() == "slow_threshold" {
            self.slow = true;
        }
    }
}

/// `(operation, table)` parsed from parameterised SQL: the first keyword and the first table
/// after `FROM` / `INTO` / `UPDATE` (the operation's own keyword first). Identifiers only.
pub fn parse_sql_name(sql: &str) -> (String, Option<String>) {
    let tokens: Vec<&str> = sql.split_whitespace().collect();
    let Some(first) = tokens.first() else {
        return ("unknown".to_owned(), None);
    };
    let op = first
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_ascii_uppercase();
    let want: &[&str] = match op.as_str() {
        "SELECT" | "DELETE" => &["FROM"],
        "INSERT" => &["INTO"],
        "UPDATE" => &["UPDATE"],
        _ => &["FROM", "INTO", "UPDATE"],
    };
    let mut table = None;
    for (i, t) in tokens.iter().enumerate() {
        if want.iter().any(|w| t.eq_ignore_ascii_case(w)) {
            let mut j = i + 1;
            if tokens
                .get(j)
                .is_some_and(|n| n.eq_ignore_ascii_case("ONLY"))
            {
                j += 1;
            }
            if let Some(n) = tokens.get(j) {
                let name: String = n
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '"'))
                    .filter(|c| *c != '"')
                    .collect();
                if !name.is_empty() {
                    table = Some(name);
                }
            }
            break;
        }
    }
    (
        if op.is_empty() {
            "unknown".to_owned()
        } else {
            op
        },
        table,
    )
}

/// Cap `s` to `max` bytes at a char boundary.
fn cap(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Turns every `sqlx::query` event into an OTel client span (D-232 (6)): the span ends at the
/// event time and started `elapsed` before. Its parent is the current span; with no sampled
/// current span nothing is recorded (listener polls, empty job polls). Install it with a
/// per-layer filter of `sqlx::query=debug` so SQL never reaches stdout at the operator's level.
/// The event never carries bind values (sqlx logs the parameterised statement).
pub struct DbSpanLayer {
    tracer: SdkTracer,
}

impl DbSpanLayer {
    pub fn new(tracer: SdkTracer) -> Self {
        Self { tracer }
    }

    /// The per-layer filter: only `sqlx::query` events at debug and above.
    pub fn filter<S>() -> impl tracing_subscriber::layer::Filter<S> + 'static
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        tracing_subscriber::filter::Targets::new().with_target("sqlx::query", tracing::Level::DEBUG)
    }
}

impl<S> tracing_subscriber::Layer<S> for DbSpanLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        use opentelemetry::trace::{Span as _, SpanBuilder, Tracer as _};
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        // `Span::current()` is unusable inside a layer callback (tracing's re-entrancy guard
        // hides the dispatcher), but the OTel layer activates the entered span's context.
        let cx = Context::current();
        let parent = cx.span().span_context().clone();
        if !parent.is_valid() || !parent.is_sampled() {
            return;
        }
        let mut e = DbEvent::default();
        event.record(&mut e);
        let text = if e.statement.trim().is_empty() {
            e.summary.trim()
        } else {
            e.statement.trim()
        };
        let (op, table) = parse_sql_name(text);
        let name = match &table {
            Some(t) => format!("{op} {t}"),
            None => op.clone(),
        };
        let end = std::time::SystemTime::now();
        let start = end
            .checked_sub(std::time::Duration::from_secs_f64(e.elapsed_secs.max(0.0)))
            .unwrap_or(end);
        let mut attrs = vec![
            KeyValue::new("db.system.name", "postgresql"),
            KeyValue::new("db.operation.name", op),
            KeyValue::new(
                "db.query.text",
                cap(text, crate::limits::DB_SPAN_STATEMENT_MAX).to_owned(),
            ),
        ];
        if let Some(t) = table {
            attrs.push(KeyValue::new("db.collection.name", t));
        }
        if let Some(n) = e.rows_returned {
            attrs.push(KeyValue::new(
                "db.response.returned_rows",
                i64::try_from(n).unwrap_or(i64::MAX),
            ));
        }
        if let Some(n) = e.rows_affected {
            attrs.push(KeyValue::new(
                "aurora.db.rows_affected",
                i64::try_from(n).unwrap_or(i64::MAX),
            ));
        }
        if e.slow {
            attrs.push(KeyValue::new("aurora.db.slow", true));
        }
        let builder = SpanBuilder::from_name(name)
            .with_kind(SpanKind::Client)
            .with_start_time(start)
            .with_attributes(attrs);
        let mut span = self.tracer.build_with_context(builder, &cx);
        span.end_with_timestamp(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{
        SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
    };

    /// Shared with the dashboard (`web/src/lib/telemetry`): one fixture, tested on both sides.
    #[test]
    fn sampling_rule_matches_the_shared_vectors() {
        let fx: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/sampling-vectors.json")).unwrap();
        for t in fx["thresholds"].as_array().unwrap() {
            let want: u64 = t["threshold"].as_str().unwrap().parse().unwrap();
            assert_eq!(sample_threshold(t["ratio"].as_f64().unwrap()), want, "{t}");
        }
        for v in fx["vectors"].as_array().unwrap() {
            let id = TraceId::from_hex(v["trace_id"].as_str().unwrap()).unwrap();
            let threshold = sample_threshold(v["ratio"].as_f64().unwrap());
            assert_eq!(
                rule_sampled(id, threshold),
                v["sampled"].as_bool().unwrap(),
                "{v}"
            );
        }
    }

    // ---- database spans (FIX-146 A2) ----

    use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
    use tracing_subscriber::{Layer as _, layer::SubscriberExt as _};

    fn db_harness(ratio: f64) -> (InMemorySpanExporter, tracing::subscriber::DefaultGuard) {
        let exporter = InMemorySpanExporter::default();
        let provider = provider_builder("test", ratio)
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("t");
        let sub = tracing_subscriber::registry()
            .with(
                tracing_opentelemetry::layer()
                    .with_tracer(tracer.clone())
                    .with_filter(tracing_subscriber::filter::filter_fn(|m| m.is_span())),
            )
            .with(DbSpanLayer::new(tracer).with_filter(DbSpanLayer::filter()));
        (exporter, tracing::subscriber::set_default(sub))
    }

    fn attr<'a>(s: &'a SpanData, key: &str) -> Option<&'a opentelemetry::Value> {
        s.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| &kv.value)
    }

    fn query_event(statement: &str, elapsed: f64) {
        tracing::debug!(
            target: "sqlx::query",
            summary = "SELECT id FROM turns WHERE …",
            db.statement = statement,
            rows_affected = 0_u64,
            rows_returned = 3_u64,
            elapsed_secs = elapsed,
        );
    }

    #[test]
    fn query_event_becomes_a_child_client_span_ending_at_the_event() {
        let (exp, _g) = db_harness(1.0);
        let req = tracing::info_span!("request");
        let parent = req.context().span().span_context().span_id();
        req.in_scope(|| {
            query_event(
                "\n\nSELECT id FROM turns WHERE conversation_id = $1\n",
                0.25,
            )
        });
        drop(req);
        let spans = exp.get_finished_spans().unwrap();
        let db = spans.iter().find(|s| s.name == "SELECT turns").unwrap();
        assert_eq!(db.parent_span_id, parent);
        assert_eq!(db.span_kind, SpanKind::Client);
        assert_eq!(
            attr(db, "db.system.name").unwrap().as_str().as_ref(),
            "postgresql"
        );
        assert_eq!(
            attr(db, "db.operation.name").unwrap().as_str().as_ref(),
            "SELECT"
        );
        assert_eq!(
            attr(db, "db.collection.name").unwrap().as_str().as_ref(),
            "turns"
        );
        assert_eq!(
            attr(db, "db.query.text").unwrap().as_str().as_ref(),
            "SELECT id FROM turns WHERE conversation_id = $1"
        );
        assert_eq!(
            attr(db, "db.response.returned_rows"),
            Some(&opentelemetry::Value::I64(3))
        );
        assert_eq!(
            attr(db, "aurora.db.rows_affected"),
            Some(&opentelemetry::Value::I64(0))
        );
        assert!(attr(db, "aurora.db.slow").is_none());
        let took = db.end_time.duration_since(db.start_time).unwrap();
        assert!(
            (took.as_secs_f64() - 0.25).abs() < 0.001,
            "start = end - elapsed, got {took:?}"
        );
    }

    #[test]
    fn slow_statements_are_flagged() {
        let (exp, _g) = db_harness(1.0);
        let req = tracing::info_span!("request");
        req.in_scope(|| {
            tracing::warn!(
                target: "sqlx::query",
                summary = "UPDATE turns SET …",
                db.statement = "UPDATE turns SET state = $1",
                rows_affected = 1_u64,
                rows_returned = 0_u64,
                elapsed_secs = 2.0_f64,
                slow_threshold = ?std::time::Duration::from_secs(1),
                "slow statement: execution time exceeded alert threshold"
            );
        });
        drop(req);
        let spans = exp.get_finished_spans().unwrap();
        let db = spans.iter().find(|s| s.name == "UPDATE turns").unwrap();
        assert_eq!(
            attr(db, "aurora.db.slow"),
            Some(&opentelemetry::Value::Bool(true))
        );
    }

    #[test]
    fn no_span_without_a_sampled_parent() {
        // No current span at all (a listener poll, an empty job poll).
        let (exp, g) = db_harness(1.0);
        query_event("SELECT 1 FROM turns", 0.01);
        assert!(exp.get_finished_spans().unwrap().is_empty());
        drop(g);
        // A current span the sampling rule dropped (ratio 0).
        let (exp, _g) = db_harness(0.0);
        let req = tracing::info_span!("request");
        req.in_scope(|| query_event("SELECT 1 FROM turns", 0.01));
        drop(req);
        assert!(exp.get_finished_spans().unwrap().is_empty());
    }

    #[test]
    fn statement_is_capped_and_other_targets_are_ignored() {
        let (exp, _g) = db_harness(1.0);
        let long = format!("SELECT id FROM turns WHERE a IN ({})", "$1, ".repeat(5000));
        let req = tracing::info_span!("request");
        req.in_scope(|| {
            query_event(&long, 0.01);
            tracing::debug!(target: "other", summary = "SELECT 1 FROM nope");
        });
        drop(req);
        let spans = exp.get_finished_spans().unwrap();
        assert!(spans.iter().all(|s| s.name != "SELECT nope"));
        let db = spans.iter().find(|s| s.name == "SELECT turns").unwrap();
        let text = attr(db, "db.query.text").unwrap().as_str().to_string();
        assert_eq!(text.len(), crate::limits::DB_SPAN_STATEMENT_MAX);
    }

    #[test]
    fn job_span_is_a_root_with_a_request_id() {
        let (exp, _g) = db_harness(1.0);
        let job = job_span("sweep");
        job.in_scope(|| query_event("DELETE FROM sessions WHERE x < $1", 0.01));
        drop(job);
        let spans = exp.get_finished_spans().unwrap();
        let root = spans.iter().find(|s| s.name == "job sweep").unwrap();
        assert_eq!(root.parent_span_id, SpanId::INVALID);
        let db = spans.iter().find(|s| s.name == "DELETE sessions").unwrap();
        assert_eq!(db.parent_span_id, root.span_context.span_id());
        assert_eq!(db.span_context.trace_id(), root.span_context.trace_id());
    }

    #[test]
    fn turn_context_is_parsed_strictly_and_parents_a_span() {
        let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let sc = parse_traceparent(tp).unwrap();
        assert_eq!(format!("{:032x}", sc.trace_id()), &tp[3..35]);
        assert!(sc.is_sampled() && sc.is_remote());
        for bad in [
            "",
            "garbage",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
        ] {
            assert!(parse_traceparent(bad).is_none(), "{bad}");
        }

        let (exp, _g) = db_harness(1.0);
        let span = tracing::info_span!("work_item");
        assert!(adopt_turn_context(&span, Some(tp)));
        assert!(!adopt_turn_context(&span, Some("nope")));
        assert!(!adopt_turn_context(&span, None));
        span.in_scope(|| {
            // The adopted span is in the turn's trace; its traceparent round-trips.
            let cur = current_traceparent().unwrap();
            assert_eq!(&cur[3..35], &tp[3..35]);
            query_event("SELECT 1 FROM turns", 0.01);
        });
        drop(span);
        let spans = exp.get_finished_spans().unwrap();
        let w = spans.iter().find(|s| s.name == "work_item").unwrap();
        assert_eq!(format!("{:032x}", w.span_context.trace_id()), &tp[3..35]);
        assert_eq!(format!("{:016x}", w.parent_span_id), &tp[36..52]);
    }

    #[test]
    fn sql_names_are_parsed_from_the_statement() {
        let n = |s: &str| parse_sql_name(s);
        assert_eq!(
            n("SELECT a FROM public.turns t WHERE x"),
            ("SELECT".into(), Some("public.turns".into()))
        );
        assert_eq!(
            n("insert into \"agents\" (a) values ($1)"),
            ("INSERT".into(), Some("agents".into()))
        );
        assert_eq!(
            n("UPDATE ONLY turns SET a = 1"),
            ("UPDATE".into(), Some("turns".into()))
        );
        assert_eq!(
            n("DELETE FROM runs"),
            ("DELETE".into(), Some("runs".into()))
        );
        assert_eq!(n("SELECT 1"), ("SELECT".into(), None));
        assert_eq!(n("BEGIN"), ("BEGIN".into(), None));
        assert_eq!(n(""), ("unknown".into(), None));
    }

    #[test]
    fn url_gets_traces_path() {
        assert_eq!(traces_url("http://c:4318"), "http://c:4318/v1/traces");
        assert_eq!(traces_url("http://c:4318/"), "http://c:4318/v1/traces");
        assert_eq!(
            traces_url("http://c:4318/v1/traces"),
            "http://c:4318/v1/traces"
        );
    }

    #[test]
    fn provider_builds_without_a_collector() {
        let p = build_provider("aurora-test", "http://127.0.0.1:1").unwrap();
        let _ = p.tracer("t");
        let _ = p.shutdown();
    }

    #[test]
    fn inject_is_a_noop_when_not_configured() {
        let mut h = HeaderMap::new();
        inject(&mut h);
        assert!(h.get("traceparent").is_none());
    }

    #[test]
    fn traceparent_round_trips() {
        let prop = TraceContextPropagator::new();
        let sc = SpanContext::new(
            TraceId::from_bytes([7; 16]),
            SpanId::from_bytes([9; 8]),
            TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        );
        let cx = Context::new().with_remote_span_context(sc);
        let mut h = HeaderMap::new();
        inject_context(&prop, &cx, &mut h);
        let tp = h.get("traceparent").unwrap().to_str().unwrap();
        assert_eq!(
            tp,
            "00-07070707070707070707070707070707-0909090909090909-01"
        );
        let back = extract_context(&prop, &h);
        assert_eq!(
            back.span().span_context().trace_id(),
            TraceId::from_bytes([7; 16])
        );
        assert!(back.span().span_context().is_remote());
    }
}
