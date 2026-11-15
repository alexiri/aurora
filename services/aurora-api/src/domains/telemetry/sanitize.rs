//! Rebuilding a browser's spans (FIX-146 Part D, D-232 (j), C-732 (6)). The API never trusts the
//! payload: it reads ids, a name, two timestamps and a few allow-listed attributes, and builds new
//! SDK `SpanData` from them. Resource, scope, links, events, `tracestate` and every attribute not on
//! the allowlist are never carried over.

use super::dto::{OtlpExportRequest, OtlpKeyValue, OtlpSpan};
use aurora_core::FieldError;
use aurora_core::limits::{
    TELEMETRY_CLOCK_FUTURE, TELEMETRY_CLOCK_PAST, TELEMETRY_INGEST_SPANS_MAX,
    TELEMETRY_SPAN_ATTRIBUTES_MAX, TELEMETRY_SPAN_DURATION_MAX, TELEMETRY_SPAN_EVENTS_MAX,
    TELEMETRY_STACKTRACE_MAX,
};
use aurora_core::telemetry::rule_sampled;
use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{SpanData, SpanEvents, SpanLinks};
use std::borrow::Cow;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// The resource of every browser batch (`service.name`, `service.namespace`, the signed-in user).
pub fn resource(user: Uuid) -> Resource {
    Resource::builder_empty()
        .with_attributes([
            KeyValue::new("service.name", "aurora-web"),
            KeyValue::new("service.namespace", "aurora"),
            KeyValue::new("aurora.user_id", user.to_string()),
        ])
        .build()
}

/// What [`rebuild`] made of a request.
#[derive(Debug)]
pub struct Rebuilt {
    pub spans: Vec<SpanData>,
    /// Spans refused by the sampling rule (C-732 (7)).
    pub dropped: usize,
}

fn field(path: String, message: &str) -> FieldError {
    FieldError::new(path, message)
}

/// Validate and rebuild every span of `req`. `Err` names the first offending path (400). A span
/// failing the sampling rule (`threshold`, C-732 (7)) is dropped silently.
pub fn rebuild(
    req: &OtlpExportRequest,
    now: SystemTime,
    threshold: u64,
) -> Result<Rebuilt, FieldError> {
    // Count limits first, in document order, so the first offending path is the one named.
    let mut total = 0usize;
    for (ri, rs) in req.resource_spans.iter().enumerate() {
        for (si, ss) in rs.scope_spans.iter().enumerate() {
            for (pi, sp) in ss.spans.iter().enumerate() {
                let base = format!("/resourceSpans/{ri}/scopeSpans/{si}/spans/{pi}");
                total += 1;
                if total > TELEMETRY_INGEST_SPANS_MAX {
                    return Err(field(
                        base,
                        &format!("at most {TELEMETRY_INGEST_SPANS_MAX} spans per request"),
                    ));
                }
                if sp.attributes.len() > TELEMETRY_SPAN_ATTRIBUTES_MAX {
                    return Err(field(
                        format!("{base}/attributes"),
                        &format!("at most {TELEMETRY_SPAN_ATTRIBUTES_MAX} attributes per span"),
                    ));
                }
                if sp.events.len() > TELEMETRY_SPAN_EVENTS_MAX {
                    return Err(field(
                        format!("{base}/events"),
                        &format!("at most {TELEMETRY_SPAN_EVENTS_MAX} events per span"),
                    ));
                }
            }
        }
    }
    let mut out = Rebuilt {
        spans: Vec::new(),
        dropped: 0,
    };
    for (ri, rs) in req.resource_spans.iter().enumerate() {
        for (si, ss) in rs.scope_spans.iter().enumerate() {
            for (pi, sp) in ss.spans.iter().enumerate() {
                let base = format!("/resourceSpans/{ri}/scopeSpans/{si}/spans/{pi}");
                match build_span(sp, &base, now, threshold)? {
                    Some(d) => out.spans.push(d),
                    None => out.dropped += 1,
                }
            }
        }
    }
    Ok(out)
}

fn hex_id<const N: usize>(s: &str, path: &str, what: &str) -> Result<[u8; N], FieldError> {
    let bad = || {
        field(
            path.to_owned(),
            &format!("{what} must be {} hex characters", N * 2),
        )
    };
    if s.len() != N * 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| bad())?;
    }
    if out.iter().all(|b| *b == 0) {
        return Err(bad());
    }
    Ok(out)
}

fn build_span(
    sp: &OtlpSpan,
    base: &str,
    now: SystemTime,
    threshold: u64,
) -> Result<Option<SpanData>, FieldError> {
    let trace_id = TraceId::from_bytes(hex_id::<16>(
        &sp.trace_id,
        &format!("{base}/traceId"),
        "traceId",
    )?);
    let span_id = SpanId::from_bytes(hex_id::<8>(
        &sp.span_id,
        &format!("{base}/spanId"),
        "spanId",
    )?);
    let parent = match sp.parent_span_id.as_deref() {
        None | Some("") => SpanId::INVALID,
        Some(p) => SpanId::from_bytes(hex_id::<8>(
            p,
            &format!("{base}/parentSpanId"),
            "parentSpanId",
        )?),
    };
    if !rule_sampled(trace_id, threshold) {
        return Ok(None);
    }
    let (start, end) = clamp_times(sp.start_time_unix_nano, sp.end_time_unix_nano, now);
    let name = shape_name(&sp.name);
    let error = name == "browser.error";
    let kind = if is_http_name(&name) {
        SpanKind::Client
    } else {
        SpanKind::Internal
    };
    Ok(Some(SpanData {
        span_context: SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        ),
        parent_span_id: parent,
        parent_span_is_remote: false,
        span_kind: kind,
        name: Cow::Owned(name.into_owned()),
        start_time: start,
        end_time: end,
        attributes: sp.attributes.iter().filter_map(keep_attribute).collect(),
        dropped_attributes_count: 0,
        events: SpanEvents::default(),
        links: SpanLinks::default(),
        status: if error {
            Status::error("")
        } else {
            Status::Unset
        },
        instrumentation_scope: InstrumentationScope::builder("aurora-web").build(),
    }))
}

/// Timestamps clamped to `[now - PAST, now + FUTURE]`, the end never before the start and the
/// duration at most `TELEMETRY_SPAN_DURATION_MAX`.
fn clamp_times(start_ns: u64, end_ns: u64, now: SystemTime) -> (SystemTime, SystemTime) {
    let lo = now - TELEMETRY_CLOCK_PAST;
    let hi = now + TELEMETRY_CLOCK_FUTURE;
    let at = |ns: u64| UNIX_EPOCH + Duration::from_nanos(ns);
    let clamp = |t: SystemTime| t.clamp(lo, hi);
    let start = clamp(at(start_ns));
    let mut end = clamp(at(end_ns)).max(start);
    end = end.min(start + TELEMETRY_SPAN_DURATION_MAX).min(hi);
    (start, end)
}

// ---- names ----

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

fn is_http_name(name: &str) -> bool {
    name.split_once(' ')
        .is_some_and(|(m, _)| METHODS.contains(&m))
}

/// The name when it has one of the shapes of C-732 (6), else `browser`.
fn shape_name(name: &str) -> Cow<'_, str> {
    let ok = if name == "browser.error" {
        true
    } else if let Some(t) = name.strip_prefix("navigate ") {
        valid_template(t)
    } else if let Some(t) = name.strip_prefix("stream connect ") {
        valid_template(t)
    } else if let Some(a) = name.strip_prefix("action ") {
        valid_action(a)
    } else if let Some((m, t)) = name.split_once(' ') {
        METHODS.contains(&m) && t.starts_with("/v1/") && valid_template(t)
    } else {
        false
    };
    if ok {
        Cow::Borrowed(name)
    } else {
        Cow::Borrowed("browser")
    }
}

fn valid_action(a: &str) -> bool {
    let mut b = a.bytes();
    a.len() <= 64
        && b.next().is_some_and(|c| c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

/// Crockford base32 without I, L, O, U, as a 26 character ULID.
fn is_ulid(s: &str) -> bool {
    s.len() == 26
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_alphabetic() && !b"ILOUilou".contains(&b)))
}

/// A path segment that looks like an id: UUID, ULID, `aun_…`, all digits, or a long hex string.
pub fn id_like(seg: &str) -> bool {
    Uuid::parse_str(seg).is_ok()
        || is_ulid(seg)
        || seg.starts_with("aun_")
        || (!seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()))
        || (seg.len() >= 16 && seg.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A route template: `/`, then segments that are `[param]`, `{param}` or an id-free literal;
/// at most 256 bytes and 16 segments, no query string, no fragment.
pub fn valid_template(t: &str) -> bool {
    if t.len() > 256 || !t.starts_with('/') {
        return false;
    }
    let segs: Vec<&str> = t.split('/').skip(1).collect();
    if segs.len() > 16 {
        return false;
    }
    segs.iter().enumerate().all(|(i, s)| {
        if s.is_empty() {
            // only the trailing empty segment of `/` (and `/v1/x/`) is fine
            return i + 1 == segs.len();
        }
        let param =
            (s.starts_with('[') && s.ends_with(']')) || (s.starts_with('{') && s.ends_with('}'));
        if param {
            let inner = &s[1..s.len() - 1];
            return !inner.is_empty()
                && inner.len() <= 64
                && inner
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
        }
        s.len() <= 64
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
            && !id_like(s)
    })
}

// ---- attributes ----

fn keep_attribute(kv: &OtlpKeyValue) -> Option<KeyValue> {
    let s = kv.value.string_value.as_deref();
    match kv.key.as_str() {
        "http.request.method" => {
            let m = s?;
            METHODS
                .contains(&m)
                .then(|| KeyValue::new("http.request.method", m.to_owned()))
        }
        "http.route" | "aurora.page" => {
            let r = s?;
            valid_template(r).then(|| KeyValue::new(attr_key(&kv.key), r.to_owned()))
        }
        "http.response.status_code" => {
            let n = kv.value.int_value?;
            (100..=599)
                .contains(&n)
                .then(|| KeyValue::new("http.response.status_code", n))
        }
        "aurora.error_code" => {
            let c = s?;
            let mut b = c.bytes();
            (c.len() <= 64
                && b.next().is_some_and(|x| x.is_ascii_lowercase())
                && b.all(|x| {
                    x.is_ascii_lowercase() || x.is_ascii_digit() || matches!(x, b'_' | b'.' | b'-')
                }))
            .then(|| KeyValue::new("aurora.error_code", c.to_owned()))
        }
        "exception.type" => {
            let t = s?;
            let mut b = t.bytes();
            (t.len() <= 128
                && b.next()
                    .is_some_and(|x| x.is_ascii_alphabetic() || x == b'_' || x == b'$')
                && b.all(|x| x.is_ascii_alphanumeric() || matches!(x, b'_' | b'$' | b'.')))
            .then(|| KeyValue::new("exception.type", t.to_owned()))
        }
        "exception.stacktrace" => Some(KeyValue::new(
            "exception.stacktrace",
            cap(s?, TELEMETRY_STACKTRACE_MAX).to_owned(),
        )),
        _ => None,
    }
}

/// The static key for an allow-listed dynamic match arm.
fn attr_key(k: &str) -> &'static str {
    if k == "http.route" {
        "http.route"
    } else {
        "aurora.page"
    }
}

/// `s` cut to at most `max` bytes on a character boundary.
fn cap(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut n = max;
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_core::telemetry::sample_threshold;

    fn span(json: serde_json::Value) -> OtlpExportRequest {
        serde_json::from_value(serde_json::json!({
            "resourceSpans": [{"scopeSpans": [{"spans": [json]}]}]
        }))
        .unwrap()
    }

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn ns(t: SystemTime) -> String {
        t.duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string()
    }

    fn base(name: &str) -> serde_json::Value {
        serde_json::json!({
            "traceId": "0af7651916cd43dd8448eb211c80319c",
            "spanId": "b7ad6b7169203331",
            "name": name,
            "startTimeUnixNano": ns(now() - Duration::from_secs(2)),
            "endTimeUnixNano": ns(now() - Duration::from_secs(1)),
        })
    }

    fn one(json: serde_json::Value) -> SpanData {
        let mut r = rebuild(&span(json), now(), sample_threshold(1.0)).unwrap();
        assert_eq!(r.spans.len(), 1);
        r.spans.remove(0)
    }

    #[test]
    fn id_like_segments() {
        for s in [
            "0af7651916cd43dd8448eb211c80319c",
            "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "aun_abc123",
            "12345",
        ] {
            assert!(id_like(s), "{s}");
        }
        for s in ["agents", "files", "v1", "send_message", "abcdef"] {
            assert!(!id_like(s), "{s}");
        }
    }

    #[test]
    fn templates() {
        for t in [
            "/",
            "/agents/[agent]/chat",
            "/v1/agents/{agent}/files",
            "/a/",
        ] {
            assert!(valid_template(t), "{t}");
        }
        for t in [
            "agents",
            "/agents/6ba7b810-9dad-11d1-80b4-00c04fd430c8",
            "/agents/42",
            "/a?x=1",
            "/a#f",
            "/a b",
            "/[]/x",
        ] {
            assert!(!valid_template(t), "{t}");
        }
    }

    #[test]
    fn names_have_the_documented_shapes() {
        for n in [
            "navigate /agents/[agent]",
            "GET /v1/agents/{agent}/files",
            "stream connect /c/[conversation]",
            "action send_message",
            "browser.error",
        ] {
            assert_eq!(one(base(n)).name, n);
        }
        for n in [
            "navigate /agents/6ba7b810-9dad-11d1-80b4-00c04fd430c8",
            "GET /agents/[a]",
            "FETCH /v1/x",
            "action Send Message",
            "something else",
            "",
        ] {
            assert_eq!(one(base(n)).name, "browser", "{n}");
        }
    }

    #[test]
    fn only_allowlisted_attributes_survive() {
        let mut s = base("browser.error");
        s["attributes"] = serde_json::json!([
            {"key": "http.request.method", "value": {"stringValue": "GET"}},
            {"key": "http.route", "value": {"stringValue": "/v1/agents/{agent}"}},
            {"key": "http.response.status_code", "value": {"intValue": "502"}},
            {"key": "aurora.error_code", "value": {"stringValue": "upstream_unavailable"}},
            {"key": "aurora.page", "value": {"stringValue": "/agents/[agent]"}},
            {"key": "exception.type", "value": {"stringValue": "TypeError"}},
            {"key": "exception.stacktrace", "value": {"stringValue": "at f (/a.js:1:2)"}},
            // all of these are dropped
            {"key": "exception.message", "value": {"stringValue": "SECRET-CANARY"}},
            {"key": "user.email", "value": {"stringValue": "a@b.c"}},
            {"key": "http.url", "value": {"stringValue": "/v1/x?token=SECRET-CANARY"}},
            {"key": "http.route", "value": {"stringValue": "/v1/agents/6ba7b810-9dad-11d1-80b4-00c04fd430c8"}},
            {"key": "aurora.page", "value": {"stringValue": "/a?q=SECRET-CANARY"}},
            {"key": "http.request.method", "value": {"stringValue": "get SECRET-CANARY"}},
            {"key": "http.response.status_code", "value": {"intValue": "99999"}},
            {"key": "aurora.error_code", "value": {"stringValue": "Has Spaces SECRET-CANARY"}},
            {"key": "exception.type", "value": {"stringValue": "X: SECRET-CANARY"}},
        ]);
        let d = one(s);
        let keys: Vec<_> = d.attributes.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "http.request.method",
                "http.route",
                "http.response.status_code",
                "aurora.error_code",
                "aurora.page",
                "exception.type",
                "exception.stacktrace"
            ]
        );
        assert!(!format!("{:?}", d.attributes).contains("SECRET-CANARY"));
        assert_eq!(d.status, Status::error(""));
    }

    #[test]
    fn stacktrace_is_capped_on_a_char_boundary() {
        let mut s = base("browser.error");
        let big = "é".repeat(TELEMETRY_STACKTRACE_MAX);
        s["attributes"] = serde_json::json!([
            {"key": "exception.stacktrace", "value": {"stringValue": big}}
        ]);
        let d = one(s);
        let v = d.attributes[0].value.as_str().to_string();
        assert!(v.len() <= TELEMETRY_STACKTRACE_MAX && v.len() > TELEMETRY_STACKTRACE_MAX - 2);
    }

    #[test]
    fn links_events_and_tracestate_are_never_carried() {
        let mut s = base("action send_message");
        s["links"] = serde_json::json!([{"traceId": "1", "spanId": "2"}]);
        s["traceState"] = "vendor=x".into();
        s["events"] = serde_json::json!([{"name": "e", "timeUnixNano": "1"}]);
        let d = one(s);
        assert!(d.links.links.is_empty() && d.events.events.is_empty());
        assert_eq!(d.span_context.trace_state().header(), "");
        assert_eq!(d.span_context.trace_flags(), TraceFlags::SAMPLED);
    }

    #[test]
    fn timestamps_are_clamped() {
        let mut s = base("action x");
        s["startTimeUnixNano"] = ns(now() - Duration::from_secs(3600)).into();
        s["endTimeUnixNano"] = ns(now() + Duration::from_secs(3600)).into();
        let d = one(s);
        assert_eq!(d.start_time, now() - TELEMETRY_CLOCK_PAST);
        // the end is clamped to now + 1 min, then the duration cap (15 min after the start) applies
        assert_eq!(d.end_time, d.start_time + TELEMETRY_SPAN_DURATION_MAX);
        // an end before the start becomes zero duration; a very long span is cut to 15 min
        let mut s = base("action x");
        s["endTimeUnixNano"] = ns(now() - Duration::from_secs(60)).into();
        s["startTimeUnixNano"] = ns(now() - Duration::from_secs(10)).into();
        let d = one(s);
        assert_eq!(d.end_time, d.start_time);
        let mut s = base("action x");
        s["startTimeUnixNano"] = ns(now() - Duration::from_secs(14 * 60)).into();
        s["endTimeUnixNano"] = ns(now() + Duration::from_secs(30)).into();
        let d = one(s);
        assert!(d.end_time.duration_since(d.start_time).unwrap() <= TELEMETRY_SPAN_DURATION_MAX);
    }

    #[test]
    fn the_sampling_rule_drops_spans() {
        // trace id whose 7 low bytes are ff…: sampled only at ratio 1.0's threshold edge cases
        let mut s = base("action x");
        s["traceId"] = "0af7651916cd43ddffffffffffffffff".into();
        let r = rebuild(&span(s.clone()), now(), sample_threshold(0.5)).unwrap();
        assert_eq!((r.spans.len(), r.dropped), (0, 1));
        let r = rebuild(&span(s.clone()), now(), sample_threshold(0.0)).unwrap();
        assert_eq!((r.spans.len(), r.dropped), (0, 1));
        let r = rebuild(&span(s), now(), sample_threshold(1.0)).unwrap();
        assert_eq!((r.spans.len(), r.dropped), (1, 0));
    }

    #[test]
    fn malformed_ids_and_counts_name_the_first_path() {
        let mut s = base("action x");
        s["traceId"] = "zz".into();
        let e = rebuild(&span(s), now(), sample_threshold(1.0)).unwrap_err();
        assert_eq!(e.path, "/resourceSpans/0/scopeSpans/0/spans/0/traceId");
        let mut s = base("action x");
        s["attributes"] = (0..=TELEMETRY_SPAN_ATTRIBUTES_MAX)
            .map(|i| serde_json::json!({"key": format!("k{i}"), "value": {}}))
            .collect::<Vec<_>>()
            .into();
        let e = rebuild(&span(s), now(), sample_threshold(1.0)).unwrap_err();
        assert_eq!(e.path, "/resourceSpans/0/scopeSpans/0/spans/0/attributes");
        let mut s = base("action x");
        s["events"] = vec![serde_json::json!({}); TELEMETRY_SPAN_EVENTS_MAX + 1].into();
        let e = rebuild(&span(s), now(), sample_threshold(1.0)).unwrap_err();
        assert_eq!(e.path, "/resourceSpans/0/scopeSpans/0/spans/0/events");
        let many = OtlpExportRequest {
            resource_spans: serde_json::from_value(serde_json::json!(
                [{"scopeSpans": [{"spans": vec![base("action x"); TELEMETRY_INGEST_SPANS_MAX + 1]}]}]
            ))
            .unwrap(),
        };
        let e = rebuild(&many, now(), sample_threshold(1.0)).unwrap_err();
        assert_eq!(
            e.path,
            format!(
                "/resourceSpans/0/scopeSpans/0/spans/{}",
                TELEMETRY_INGEST_SPANS_MAX
            )
        );
    }

    #[test]
    fn resource_is_replaced() {
        let u = Uuid::nil();
        let r = resource(u);
        let get = |k: &str| {
            r.iter()
                .find(|(key, _)| key.as_str() == k)
                .map(|(_, v)| v.to_string())
        };
        assert_eq!(get("service.name").as_deref(), Some("aurora-web"));
        assert_eq!(get("service.namespace").as_deref(), Some("aurora"));
        assert_eq!(get("aurora.user_id"), Some(u.to_string()));
        assert_eq!(r.len(), 3);
    }
}
