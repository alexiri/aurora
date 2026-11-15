#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-146 Part D, C-732 (6): `POST /v1/telemetry/traces` (browser span ingestion).

use crate::common::*;
use aurora_api::domains::telemetry::BrowserSpans;
use aurora_core::limits::{
    TELEMETRY_INGEST_BODY_MAX, TELEMETRY_INGEST_SPANS_MAX, TELEMETRY_SPAN_ATTRIBUTES_MAX,
    TELEMETRY_SPAN_EVENTS_MAX,
};
use aurora_core::telemetry::{SpanSink, sample_threshold};
use axum::body::Body;
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SpanData;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

type Batches = Arc<Mutex<Vec<(Resource, Vec<SpanData>)>>>;

const URI: &str = "/v1/telemetry/traces";
/// The seven rightmost bytes are zero: sampled at every ratio above 0.
const SAMPLED: &str = "0af7651916cd43dd8400000000000000";
/// The seven rightmost bytes are 0xff: sampled only at ratio 1.0.
const UNSAMPLED_AT_HALF: &str = "0af7651916cd43ddffffffffffffffff";

fn enable(h: &mut Harness, ratio: f64) -> Batches {
    let batches: Batches = Arc::default();
    let sink: SpanSink = {
        let b = batches.clone();
        Arc::new(move |r, spans| b.lock().unwrap().push((r, spans)))
    };
    h.state.browser_spans = Some(Arc::new(BrowserSpans {
        sink,
        threshold: sample_threshold(ratio),
    }));
    h.app = test_router(h.state.clone());
    batches
}

fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn span(trace: &str, name: &str) -> Value {
    let n = now_ns();
    json!({
        "traceId": trace,
        "spanId": "b7ad6b7169203331",
        "name": name,
        "startTimeUnixNano": (n - 2_000_000_000).to_string(),
        "endTimeUnixNano": (n - 1_000_000_000).to_string(),
    })
}

fn export(spans: Vec<Value>) -> Value {
    json!({"resourceSpans": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "evil"}}]},
        "scopeSpans": [{"spans": spans}]
    }]})
}

async fn signed_in(h: &Harness) -> (String, uuid::Uuid) {
    let user = h.user("t@x.io", false).await;
    let (_, tok) = h.good_session(user).await;
    (cookie(&tok), user)
}

async fn post(h: &Harness, cookie: &str, body: Value) -> (StatusCode, Value) {
    let (s, _, v) = h
        .send(
            Method::POST,
            URI,
            &[("cookie", cookie), ("origin", ORIGIN)],
            Some(body),
        )
        .await;
    (s, v)
}

#[tokio::test]
async fn without_an_endpoint_the_route_is_a_404_even_unauthenticated() {
    let h = Harness::new().await;
    let (c, _) = signed_in(&h).await;
    let (s, v) = post(&h, &c, export(vec![span(SAMPLED, "action send_message")])).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "not_found");
    let (s, _, v) = h
        .send(
            Method::POST,
            URI,
            &[("origin", ORIGIN)],
            Some(export(vec![])),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
}

#[tokio::test]
async fn authentication_and_csrf_are_enforced() {
    let mut h = Harness::new().await;
    let batches = enable(&mut h, 1.0);
    let (c, user) = signed_in(&h).await;
    let body = export(vec![span(SAMPLED, "action send_message")]);
    // no session
    let (s, _, _) = h
        .send(Method::POST, URI, &[("origin", ORIGIN)], Some(body.clone()))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // cross-origin, and no origin at all, with a valid session cookie
    let (s, _, v) = h
        .send(
            Method::POST,
            URI,
            &[("cookie", &c), ("origin", "http://evil.test")],
            Some(body.clone()),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("csrf_rejected"))
    );
    let (s, _, v) = h
        .send(Method::POST, URI, &[("cookie", &c)], Some(body.clone()))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("csrf_rejected"))
    );
    // an API key is not a session
    let org = h.org().await;
    h.member(org, user, "owner", "active").await;
    let (_, key) = h.api_key(user, org, None, false).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            URI,
            &[("authorization", &bearer(&key))],
            Some(body),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("session_required"))
    );
    assert!(
        batches.lock().unwrap().is_empty(),
        "nothing exported for a refused request"
    );
}

#[tokio::test]
async fn a_valid_batch_is_rebuilt_and_exported() {
    let mut h = Harness::new().await;
    let batches = enable(&mut h, 1.0);
    let (c, user) = signed_in(&h).await;
    let mut s1 = span(SAMPLED, "GET /v1/agents/{agent}/files");
    s1["attributes"] = json!([
        {"key": "http.request.method", "value": {"stringValue": "GET"}},
        {"key": "http.route", "value": {"stringValue": "/v1/agents/{agent}/files"}},
        {"key": "http.response.status_code", "value": {"intValue": 200}},
        {"key": "exception.message", "value": {"stringValue": "CANARY-MESSAGE"}},
        {"key": "http.url", "value": {"stringValue": "/v1/x?token=CANARY-QUERY"}},
    ]);
    s1["links"] = json!([{"traceId": SAMPLED, "spanId": "0000000000000001"}]);
    s1["traceState"] = "vendor=CANARY-STATE".into();
    s1["events"] = json!([{"name": "CANARY-EVENT", "timeUnixNano": "1"}]);
    let mut s2 = span(
        SAMPLED,
        "navigate /agents/6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    );
    s2["spanId"] = "b7ad6b7169203332".into();
    s2["startTimeUnixNano"] = "1".into(); // 1970: clamped
    s2["endTimeUnixNano"] = "2".into();
    let (st, _) = post(&h, &c, export(vec![s1, s2])).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let got = batches.lock().unwrap();
    assert_eq!(got.len(), 1);
    let (res, spans) = &got[0];
    // the resource is the server's, whatever the client sent
    let get = |k: &str| {
        res.iter()
            .find(|(key, _)| key.as_str() == k)
            .map(|(_, v)| v.to_string())
    };
    assert_eq!(get("service.name").as_deref(), Some("aurora-web"));
    assert_eq!(get("service.namespace").as_deref(), Some("aurora"));
    assert_eq!(get("aurora.user_id"), Some(user.to_string()));
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].name, "GET /v1/agents/{agent}/files");
    assert_eq!(
        spans[1].name, "browser",
        "a name carrying an id becomes `browser`"
    );
    let dump = format!("{spans:?}");
    for canary in [
        "CANARY-MESSAGE",
        "CANARY-QUERY",
        "CANARY-STATE",
        "CANARY-EVENT",
    ] {
        assert!(!dump.contains(canary), "{canary} reached the exporter");
    }
    assert!(spans[0].links.links.is_empty() && spans[0].events.events.is_empty());
    assert_eq!(spans[0].attributes.len(), 3);
    // clamped to the last 15 minutes
    let floor = SystemTime::now() - std::time::Duration::from_secs(16 * 60);
    assert!(spans[1].start_time > floor);
}

#[tokio::test]
async fn the_sampling_rule_drops_spans_and_zero_exports_nothing() {
    let mut h = Harness::new().await;
    let batches = enable(&mut h, 0.5);
    let (c, _) = signed_in(&h).await;
    let (s, _) = post(
        &h,
        &c,
        export(vec![
            span(UNSAMPLED_AT_HALF, "action a"),
            span(SAMPLED, "action b"),
        ]),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    {
        let got = batches.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1.len(), 1);
        assert_eq!(got[0].1[0].name, "action b");
    }
    // a "sampled" flag sent by the client changes nothing: the rule is the server's
    let mut flagged = span(UNSAMPLED_AT_HALF, "action c");
    flagged["flags"] = 1.into();
    let (s, _) = post(&h, &c, export(vec![flagged])).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(batches.lock().unwrap().len(), 1);
    // ratio 0.0: 204 and nothing exported
    let batches = enable(&mut h, 0.0);
    let (s, _) = post(&h, &c, export(vec![span(SAMPLED, "action d")])).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(batches.lock().unwrap().is_empty());
}

#[tokio::test]
async fn limits_and_malformed_input() {
    let mut h = Harness::new().await;
    let batches = enable(&mut h, 1.0);
    let (c, _) = signed_in(&h).await;
    let code = |v: &Value| v["error"]["code"].as_str().unwrap_or("").to_owned();
    // too many spans: 400 naming the first one over
    let many = (0..=TELEMETRY_INGEST_SPANS_MAX)
        .map(|_| span(SAMPLED, "action x"))
        .collect();
    let (s, v) = post(&h, &c, export(many)).await;
    assert_eq!(
        (s, code(&v).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"],
        format!("/resourceSpans/0/scopeSpans/0/spans/{TELEMETRY_INGEST_SPANS_MAX}")
    );
    // too many attributes / events
    let mut s1 = span(SAMPLED, "action x");
    s1["attributes"] = (0..=TELEMETRY_SPAN_ATTRIBUTES_MAX)
        .map(|i| json!({"key": format!("k{i}"), "value": {"stringValue": "v"}}))
        .collect::<Vec<_>>()
        .into();
    let (s, v) = post(&h, &c, export(vec![s1])).await;
    assert_eq!(
        (s, code(&v).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"],
        "/resourceSpans/0/scopeSpans/0/spans/0/attributes"
    );
    let mut s1 = span(SAMPLED, "action x");
    s1["events"] = vec![json!({}); TELEMETRY_SPAN_EVENTS_MAX + 1].into();
    let (s, v) = post(&h, &c, export(vec![s1])).await;
    assert_eq!(
        (s, code(&v).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"],
        "/resourceSpans/0/scopeSpans/0/spans/0/events"
    );
    // malformed ids, timestamps and JSON
    let mut s1 = span("nothex", "action x");
    let (s, v) = post(&h, &c, export(vec![s1.clone()])).await;
    assert_eq!(
        (s, code(&v).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    s1["traceId"] = SAMPLED.into();
    s1["startTimeUnixNano"] = "yesterday".into();
    let (s, _) = post(&h, &c, export(vec![s1])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let raw = |ct: &str, body: Vec<u8>| {
        let req = Request::builder()
            .method(Method::POST)
            .uri(URI)
            .header("cookie", &c)
            .header("origin", ORIGIN)
            .header("content-type", ct.to_owned())
            .body(Body::from(body))
            .unwrap();
        let app = h.app.clone();
        async move {
            let res = app.oneshot(req).await.unwrap();
            let s = res.status();
            let b = res.into_body().collect().await.unwrap().to_bytes();
            (
                s,
                serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null),
            )
        }
    };
    assert_eq!(
        raw("application/json", b"{not json".to_vec()).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw("text/plain", b"{}".to_vec()).await.0,
        StatusCode::BAD_REQUEST
    );
    // the body cap: 413 above it (the Content-Length is known and the stream is also bounded)
    let big = vec![b' '; TELEMETRY_INGEST_BODY_MAX + 1];
    let (s, v) = raw("application/json", big).await;
    assert_eq!(
        (s, code(&v).as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
    );
    // an empty export is fine
    assert_eq!(
        raw("application/json", b"{}".to_vec()).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(
        batches.lock().unwrap().is_empty(),
        "no refused request exported anything"
    );
}

#[tokio::test]
async fn telemetry_has_its_own_rate_class_and_never_spends_write() {
    use aurora_config::RateClassOverride;
    let mut h = Harness::with_config(|c| {
        c.ratelimit.api.multiplier = 1.0;
        c.ratelimit.api.classes.insert(
            "telemetry".into(),
            RateClassOverride {
                per_hour: Some(1),
                burst: Some(2),
                ..Default::default()
            },
        );
    })
    .await;
    let _ = enable(&mut h, 1.0);
    let (c, _) = signed_in(&h).await;
    for i in 0..2 {
        let (s, _) = post(&h, &c, export(vec![span(SAMPLED, "action x")])).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "request {i}");
    }
    let (s, v) = post(&h, &c, export(vec![span(SAMPLED, "action x")])).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(v["error"]["details"]["class"], "telemetry");
    // the write class of the same user is untouched (the default burst is 30)
    let (s, _, _) = h
        .send(
            Method::PATCH,
            "/v1/me",
            &[("cookie", &c), ("origin", ORIGIN)],
            Some(json!({"display_name": "T"})),
        )
        .await;
    assert_ne!(s, StatusCode::TOO_MANY_REQUESTS);
}
