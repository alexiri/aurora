//! Wire types of `POST /v1/telemetry/traces` (FIX-146 Part D, C-732 (6)): the OTLP/HTTP JSON
//! `ExportTraceServiceRequest`, reduced to the fields the API reads. Everything else a client
//! sends (resource, scope, links, event contents, `tracestate`, unknown attributes) is ignored by
//! the deserialiser and never reaches an exporter: [`super::sanitize`] rebuilds each span.

use serde::{Deserialize, Deserializer};
use utoipa::ToSchema;

/// OTLP/HTTP JSON `ExportTraceServiceRequest` (the dashboard's spans).
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OtlpExportRequest {
    #[serde(default)]
    pub resource_spans: Vec<OtlpResourceSpans>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OtlpResourceSpans {
    #[serde(default)]
    pub scope_spans: Vec<OtlpScopeSpans>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OtlpScopeSpans {
    #[serde(default)]
    pub spans: Vec<OtlpSpan>,
}

/// One span as the browser sends it. The server rebuilds it (name, attributes, times) before export.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OtlpSpan {
    /// 32 hex characters.
    pub trace_id: String,
    /// 16 hex characters.
    pub span_id: String,
    /// 16 hex characters; absent or empty for a root span.
    #[serde(default)]
    pub parent_span_id: Option<String>,
    #[serde(default)]
    pub name: String,
    /// Unix nanoseconds, as a decimal string (OTLP JSON) or a number.
    #[serde(deserialize_with = "nanos")]
    #[schema(value_type = String)]
    pub start_time_unix_nano: u64,
    #[serde(deserialize_with = "nanos")]
    #[schema(value_type = String)]
    pub end_time_unix_nano: u64,
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
    /// Only counted (at most `TELEMETRY_SPAN_EVENTS_MAX`); events are never exported.
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub events: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OtlpKeyValue {
    pub key: String,
    #[serde(default)]
    pub value: OtlpAnyValue,
}

/// An attribute value: only `stringValue` and `intValue` are read.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OtlpAnyValue {
    #[serde(default)]
    pub string_value: Option<String>,
    /// A decimal string (OTLP JSON) or a number.
    #[serde(default, deserialize_with = "opt_int")]
    #[schema(value_type = Option<String>)]
    pub int_value: Option<i64>,
}

fn nanos<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    use serde::de::Error;
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => s.parse().map_err(|_| D::Error::custom("not a number")),
        serde_json::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| D::Error::custom("not an unsigned integer")),
        _ => Err(D::Error::custom("expected a decimal string or number")),
    }
}

fn opt_int<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    use serde::de::Error;
    match Option::<serde_json::Value>::deserialize(d)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => s
            .parse()
            .map(Some)
            .map_err(|_| D::Error::custom("not an integer")),
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| D::Error::custom("not an integer")),
        Some(_) => Err(D::Error::custom("expected a decimal string or number")),
    }
}
