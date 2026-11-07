use aurora_testkit::{MockUpstream, TestDb};
use sqlx::Connection;

#[tokio::test]
async fn dbs_are_isolated_and_migrated() {
    let a = TestDb::new().await;
    let b = TestDb::new().await;
    assert_ne!(a.name(), b.name());

    // Migrations applied: `users` comes from the identity migration.
    let (exists,): (bool,) = sqlx::query_as("SELECT to_regclass('public.users') IS NOT NULL")
        .fetch_one(a.pool())
        .await
        .expect("query");
    assert!(exists, "migrated table `users` missing");

    sqlx::query("CREATE TABLE testkit_probe (x int)")
        .execute(a.pool())
        .await
        .expect("create probe");
    sqlx::query("INSERT INTO testkit_probe VALUES (1)")
        .execute(a.pool())
        .await
        .expect("insert");
    let (in_b,): (bool,) = sqlx::query_as("SELECT to_regclass('public.testkit_probe') IS NOT NULL")
        .fetch_one(b.pool())
        .await
        .expect("query b");
    assert!(!in_b, "probe table leaked into second TestDb");

    a.cleanup().await.expect("cleanup a");
    b.cleanup().await.expect("cleanup b");
}

#[tokio::test]
async fn cleanup_drops_database() {
    let db = TestDb::new().await;
    let name = db.name().to_owned();
    let server = db.server_url().to_owned();
    db.cleanup().await.expect("cleanup");

    let mut conn = sqlx::PgConnection::connect(&server).await.expect("connect");
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM pg_database WHERE datname = $1")
        .bind(&name)
        .fetch_one(&mut conn)
        .await
        .expect("query");
    assert_eq!(count, 0, "database still exists after cleanup");
}

#[tokio::test]
async fn drop_drops_database() {
    let db = TestDb::new().await;
    let name = db.name().to_owned();
    let server = db.server_url().to_owned();
    drop(db);

    let mut conn = sqlx::PgConnection::connect(&server).await.expect("connect");
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM pg_database WHERE datname = $1")
        .bind(&name)
        .fetch_one(&mut conn)
        .await
        .expect("query");
    assert_eq!(count, 0, "database still exists after drop");
}

#[tokio::test]
async fn mock_upstream_serves_json_and_records() {
    let up = MockUpstream::start().await;
    up.json("GET", "/v1/ping", 200, serde_json::json!({"ok": true}))
        .await;

    let uri = up.uri();
    let addr = uri.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(b"GET /v1/ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.expect("read");
    assert!(resp.starts_with("HTTP/1.1 200"), "resp: {resp}");
    assert!(resp.contains(r#""ok":true"#), "resp: {resp}");

    let received = up.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].url.path(), "/v1/ping");
}
