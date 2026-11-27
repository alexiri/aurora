#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-146 A2 (D-232 (6)): real sqlx queries become database spans, and bind values never reach
//! a span. Own test binary: it installs a scoped tracing subscriber, which must not race with
//! other tests' callsite registration.

use aurora_core::telemetry::{DbSpanLayer, provider_builder};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::InMemorySpanExporter;
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _};

const CANARY: &str = "canary-bind-value-7f3a9c";

#[tokio::test]
async fn query_spans_carry_parameterised_sql_never_bind_values() {
    // An empty database: the test creates its own table.
    let (_db, pool) = aurora_testkit::TestDb::unmigrated_with_pool().await;
    let exporter = InMemorySpanExporter::default();
    let provider = provider_builder("test", 1.0)
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
    let _guard = tracing::subscriber::set_default(sub);

    // Outside any span: no span, no trace.
    sqlx::query("CREATE TABLE canary_t (v text)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        exporter.get_finished_spans().unwrap().is_empty(),
        "a query outside a request or job must not create a span"
    );

    let req = tracing::info_span!("request");
    {
        use tracing::Instrument as _;
        async {
            sqlx::query("INSERT INTO canary_t (v) VALUES ($1)")
                .bind(CANARY)
                .execute(&pool)
                .await
                .unwrap();
            let rows: Vec<(String,)> = sqlx::query_as("SELECT v FROM canary_t WHERE v = $1")
                .bind(CANARY)
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
        }
        .instrument(req.clone())
        .await;
    }
    drop(req);

    let spans = exporter.get_finished_spans().unwrap();
    let names: Vec<&str> = spans.iter().map(|s| s.name.as_ref()).collect();
    assert!(names.contains(&"INSERT canary_t"), "{names:?}");
    assert!(names.contains(&"SELECT canary_t"), "{names:?}");
    let root = spans.iter().find(|s| s.name == "request").unwrap();
    for s in &spans {
        assert!(
            !format!("{s:?}").contains(CANARY),
            "bind value leaked into span {}: {s:?}",
            s.name
        );
        if s.name.ends_with("canary_t") {
            assert_eq!(s.parent_span_id, root.span_context.span_id());
            assert_eq!(s.span_context.trace_id(), root.span_context.trace_id());
            let text = s
                .attributes
                .iter()
                .find(|kv| kv.key.as_str() == "db.query.text")
                .unwrap()
                .value
                .to_string();
            assert!(text.contains("$1"), "parameterised: {text}");
        }
    }
}
