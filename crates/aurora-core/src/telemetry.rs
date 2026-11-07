//! OTLP trace export and W3C `traceparent` propagation (docs/15 §Observability).
//!
//! Everything here is inert unless `telemetry.otlp_endpoint` is configured: without
//! [`install`] no provider or propagator is registered, so [`inject`] and [`extract`] do
//! nothing and no network connection is ever made.

use http::{HeaderMap, HeaderName, HeaderValue};
use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{Context, KeyValue, global};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use std::sync::OnceLock;
use tracing_opentelemetry::OpenTelemetrySpanExt;

static PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

/// Full OTLP/HTTP traces URL: a bare collector base URL gets `/v1/traces` appended.
pub fn traces_url(endpoint: &str) -> String {
    let e = endpoint.trim_end_matches('/');
    if e.ends_with("/v1/traces") {
        e.to_owned()
    } else {
        format!("{e}/v1/traces")
    }
}

/// Build (but do not register) a tracer provider exporting to `endpoint` over OTLP/HTTP
/// protobuf. The exporter connects lazily, so no collector is needed to build it.
pub fn build_provider(service: &str, endpoint: &str) -> Result<SdkTracerProvider, String> {
    let url = traces_url(endpoint);
    let service = service.to_owned();
    // The blocking HTTP client must not be created inside an async runtime thread.
    let exporter = std::thread::spawn(move || {
        SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(url)
            .build()
    })
    .join()
    .map_err(|_| "exporter thread panicked".to_owned())?
    .map_err(|e| e.to_string())?;
    let resource = Resource::builder()
        .with_service_name(service.clone())
        .with_attribute(KeyValue::new("service.namespace", "aurora"))
        .build();
    Ok(SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build())
}

/// Build the provider, register it and the W3C propagator globally, return its tracer.
pub fn install(service: &str, endpoint: &str) -> Result<SdkTracer, String> {
    let provider = build_provider(service, endpoint)?;
    let tracer = provider.tracer("aurora");
    global::set_text_map_propagator(TraceContextPropagator::new());
    global::set_tracer_provider(provider.clone());
    let _ = PROVIDER.set(provider);
    Ok(tracer)
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

/// Make `span` a child of the remote context carried by `headers` (no-op without a propagator).
pub fn adopt_remote_parent(span: &tracing::Span, headers: &HeaderMap) {
    let cx = global::get_text_map_propagator(|p| p.extract(&HeaderReader(headers)));
    let _ = span.set_parent(cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{
        SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
    };

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
