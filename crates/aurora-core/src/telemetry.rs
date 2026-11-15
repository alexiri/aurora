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
