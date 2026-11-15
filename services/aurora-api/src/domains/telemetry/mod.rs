//! Browser span ingestion (phase 5, FIX-146 Part D, D-232 (j), C-732 (6)): the dashboard posts its
//! spans to the same origin; the API rebuilds each one (`sanitize`) and hands it to its exporter.
pub mod dto;
mod routes;
pub mod sanitize;

use aurora_core::telemetry::{SpanSink, otlp_span_sink, sample_threshold};
pub use routes::router;

/// The ingestion endpoint's runtime state: present only when `telemetry.otlp_endpoint` is set
/// (without it the route answers 404 like an unknown route).
pub struct BrowserSpans {
    pub sink: SpanSink,
    /// C-732 (7): `floor(sample_ratio × 2^56)`.
    pub threshold: u64,
}

impl BrowserSpans {
    /// Build from the configuration: `None` without an OTLP endpoint.
    pub fn from_config(t: &aurora_config::TelemetrySection) -> Result<Option<Self>, String> {
        let Some(ep) = t.otlp_endpoint.as_deref() else {
            return Ok(None);
        };
        Ok(Some(Self {
            sink: otlp_span_sink(ep)?,
            threshold: sample_threshold(t.sample_ratio),
        }))
    }
}
