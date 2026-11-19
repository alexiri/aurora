//! (phase 5, P5-ICS, D-273, C-843, SEC-107) The gateway secret-link lane through the whole
//! pipeline: a request on `<connection_id>.link.aurora.internal` is classified, granted and logged
//! like any tool request, then answered by one `GET` of the stored URL against a fake HTTPS
//! server (private CA) — and the agent controls no part of that request. The fake records every
//! request it receives; the canary tokens in the stored URL's path and query must reach nothing
//! the agent, the log or an error can see.
//!
//! A child module of `apps` (it reuses that file's pipeline harness).
use super::*;
use aurora_gateway::ca::Ca;
use aurora_gateway::invalidation::{Invalidatable, Invalidate, Scope};
use tokio_rustls::TlsAcceptor;

/// In the stored URL's path and in its query: neither may appear anywhere the agent can look.
const PATH_CANARY: &str = "PATHCANARY0xA1B2C3";
const QUERY_CANARY: &str = "QUERYCANARY0xD4E5F6";
const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// `(method, path and query, headers)` of one request the fake received.
type Received = (String, String, Vec<(String, String)>);

#[derive(Default)]
struct Seen {
    requests: Vec<Received>,
}

fn pq() -> String {
    format!("/ics/{PATH_CANARY}/basic.ics?key={QUERY_CANARY}")
}

async fn fake(
    seen: Arc<Mutex<Seen>>,
    port: Arc<std::sync::atomic::AtomicU16>,
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let (parts, _) = req.into_parts();
    let target = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    seen.lock().unwrap().requests.push((
        parts.method.to_string(),
        target.clone(),
        parts
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
            .collect(),
    ));
    let redirect = |to: &str| {
        Response::builder()
            .status(302)
            .header("location", to)
            .body(Full::new(Bytes::new()))
            .unwrap()
    };
    let path = parts.uri.path();
    let me = port.load(std::sync::atomic::Ordering::Relaxed);
    Ok(match path {
        p if p.starts_with("/ics/") => Response::builder()
            .status(200)
            .header("content-type", "text/calendar; charset=utf-8")
            .header("set-cookie", "sid=upstream-cookie")
            .header("cache-control", "no-store")
            .body(Full::new(Bytes::from(ICS)))
            .unwrap(),
        "/r1" => redirect("/r2"),
        "/r2" => redirect(&format!("https://localhost:{me}/r3")),
        "/r3" => redirect(&pq()),
        "/r0" => redirect("/r1"),
        "/to-http" => redirect("http://localhost/plain"),
        "/to-private" => redirect("https://169.254.169.254/latest/meta-data"),
        "/gone" => Response::builder()
            .status(404)
            .body(Full::new(Bytes::new()))
            .unwrap(),
        "/removed" => Response::builder()
            .status(410)
            .body(Full::new(Bytes::new()))
            .unwrap(),
        "/err" => Response::builder()
            .status(500)
            .body(Full::new(Bytes::from("boom")))
            .unwrap(),
        // Echoes its own path: the whole path-with-query of the stored URL is in the body.
        p if p.starts_with("/echo/") => Response::builder()
            .status(200)
            .body(Full::new(Bytes::from(format!("you asked for {target}"))))
            .unwrap(),
        "/big" => Response::builder()
            .status(200)
            .body(Full::new(Bytes::from(vec![b'x'; 21 * 1024 * 1024])))
            .unwrap(),
        _ => Response::builder()
            .status(418)
            .body(Full::new(Bytes::new()))
            .unwrap(),
    })
}

/// A fake HTTPS server with a private CA; returns its port, the CA certificate and what it saw.
async fn server() -> (u16, String, Arc<Mutex<Seen>>) {
    let gen_ca = ca::generate("link-private-ca").unwrap();
    let ca = Ca::from_pem(&gen_ca.cert_pem, &gen_ca.key_pem).unwrap();
    let cfg = ca.server_config("localhost").unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let shared = Arc::new(std::sync::atomic::AtomicU16::new(port));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s2 = seen.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = l.accept().await {
            let (cfg, seen, shared) = (cfg.clone(), s2.clone(), shared.clone());
            tokio::spawn(async move {
                let Ok(tls) = TlsAcceptor::from(cfg).accept(tcp).await else {
                    return;
                };
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(
                        TokioIo::new(tls),
                        service_fn(move |r| fake(seen.clone(), shared.clone(), r)),
                    )
                    .await;
            });
        }
    });
    (port, gen_ca.cert_pem, seen)
}

fn link_snapshot() -> Value {
    let app = json!({
        "app_id": "ics-calendar", "package_id": "pkg",
        "about": {"id": "ics-calendar", "name": "Calendar link (ICS)"},
        "credentials": [{"name": "url", "secret": true, "optional": false}],
        "metadata": [{"name": "host"}], "connect": [], "token": null,
        "tools": {"read": {"items": [
            {"id": "read_calendar", "name": "Read calendar", "host": "*.link.aurora.internal",
             "path": "/", "methods": ["GET"]}
        ]}},
        "approvals": {}, "agent": null, "database": null, "grants": {},
        "secret_link": {"credential": "url", "expect_prefix": "BEGIN:VCALENDAR"}
    });
    let mut snap = snapshot(vec![app], vec![]);
    snap["reserved_hosts"] = json!(["*.aurora.internal"]);
    snap
}

struct Lane {
    env: Env,
    src: Arc<Src>,
    seen: Arc<Mutex<Seen>>,
    /// The connection ids, in the order of the paths given to [`lane`].
    ids: Vec<Uuid>,
}

impl Lane {
    fn host(&self, i: usize) -> String {
        format!("{}.link.aurora.internal", self.ids[i])
    }

    fn upstream_count(&self) -> usize {
        self.seen.lock().unwrap().requests.len()
    }

    async fn get(&self, i: usize, path: &str, extra: &str) -> Reply {
        self.call(i, "GET", path, extra).await
    }

    async fn call(&self, i: usize, method: &str, path: &str, extra: &str) -> Reply {
        request(&self.env, &self.host(i), method, path, extra).await
    }

    /// Request-log rows written so far.
    async fn rows(&self) -> Vec<aurora_db::repo::gateway::RequestLogRow> {
        self.env.log.flush().await;
        self.env.rows.lock().unwrap().clone()
    }
}

/// One stored link per entry of `paths` (path and query on the fake server).
async fn lane(paths: &[String]) -> Lane {
    lane_in(paths, None).await
}

/// [`lane`] whose grants all `require_approval`, with a real hold store on `pool`.
async fn lane_with_approval(paths: &[String], pool: aurora_db::PgPool) -> Lane {
    lane_in(paths, Some(pool)).await
}

async fn lane_in(paths: &[String], approval_pool: Option<aurora_db::PgPool>) -> Lane {
    let (port, ca_pem, seen) = server().await;
    let approval = approval_pool.is_some();
    let ids: Arc<Mutex<Vec<Uuid>>> = Arc::default();
    let ids2 = ids.clone();
    let urls: Vec<String> = paths
        .iter()
        .map(|p| format!("https://localhost:{port}{p}"))
        .collect();
    let creds: Vec<(String, String)> = urls
        .iter()
        .enumerate()
        .map(|(i, u)| (format!("c{i}"), json!({ "url": u }).to_string()))
        .collect();
    let cred_refs: Vec<(&str, &str)> = creds
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let n = paths.len();
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    let (env, src, _) = start_in(
        w,
        link_snapshot(),
        &cred_refs,
        move |w| {
            let mut r = Rows::default();
            for i in 0..n {
                let mut c = conn(w, "ics-calendar", &format!("c{i}"), None);
                c.metadata = json!({"host": "localhost"});
                c.label = format!("Team calendar {i}");
                r.ws.rules.push(if approval {
                    rule(
                        "grant",
                        "allow",
                        json!([{"type": "connection", "id": c.id, "tools": null}]),
                        true,
                    )
                } else {
                    allow_conn(&c, None)
                });
                ids2.lock().unwrap().push(c.id);
                r.conns.push(c);
            }
            r
        },
        Some(ca_pem),
        approval_pool,
        None,
        approval,
    )
    .await;
    let ids = ids.lock().unwrap().clone();
    Lane {
        env,
        src,
        seen,
        ids,
    }
}

/// Like an agent: CONNECT to the synthetic host, TLS with the gateway's leaf inside the tunnel.
async fn request(env: &Env, host: &str, method: &str, path: &str, extra: &str) -> Reply {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
    let target = format!("{host}:443");
    s.write_all(
        format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert!(s.read(&mut b).await.unwrap() > 0, "eof in CONNECT head");
        head.push(b[0]);
    }
    assert!(String::from_utf8_lossy(&head).contains(" 200"));
    let cfg = crate::common::client_config(&crate::common::trust_pem(env.proxy), &[b"http/1.1"]);
    let mut t = tokio_rustls::TlsConnector::from(cfg)
        .connect(
            rustls::pki_types::ServerName::try_from(host.to_owned()).unwrap(),
            s,
        )
        .await
        .unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    t.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(20), t.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Reply {
        status: head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        head: head.to_owned(),
        body: dechunk(body),
    }
}

fn header(r: &Reply, name: &str) -> Option<String> {
    r.head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_owned())
    })
}

/// No canary anywhere in what an agent (or the log) can see.
fn clean(what: &str, text: &str) {
    assert!(
        !text.contains(PATH_CANARY) && !text.contains(QUERY_CANARY),
        "{what} leaked the stored URL: {text}"
    );
}

#[tokio::test]
async fn the_stored_url_is_fetched_exactly_and_the_agent_steers_nothing() {
    let l = lane(&[pq()]).await;
    let r = l
        .get(
            0,
            "/other/path?x=1&key=evil",
            "Accept: text/calendar\r\nCookie: agent=1\r\nAuthorization: Bearer agent-token\r\nRange: bytes=0-9\r\nIf-Modified-Since: Mon, 01 Jan 2001 00:00:00 GMT\r\nX-Forwarded-For: 1.2.3.4\r\n",
        )
        .await;
    assert_eq!(r.status, 200, "{} {}", r.head, r.body);
    assert_eq!(r.body, ICS);
    assert_eq!(
        header(&r, "content-type").as_deref(),
        Some("text/calendar; charset=utf-8")
    );
    assert_eq!(header(&r, "age").as_deref(), Some("0"));
    assert!(header(&r, "etag").is_some(), "{}", r.head);
    // The upstream's cookie and no-store never reach the agent.
    assert!(header(&r, "set-cookie").is_none(), "{}", r.head);
    assert!(header(&r, "cache-control").is_none(), "{}", r.head);
    clean("response head", &r.head);

    assert_eq!(l.upstream_count(), 1);
    let (method, target, headers) = l.seen.lock().unwrap().requests[0].clone();
    assert_eq!(method, "GET");
    assert_eq!(target, pq(), "the stored URL, exactly");
    let names: Vec<_> = headers.iter().map(|(k, _)| k.as_str()).collect();
    for bad in [
        "cookie",
        "authorization",
        "range",
        "if-modified-since",
        "if-none-match",
        "x-forwarded-for",
        "proxy-authorization",
    ] {
        assert!(
            !names.contains(&bad),
            "{bad} reached the upstream: {names:?}"
        );
    }
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "accept" && v == "text/calendar"),
        "{headers:?}"
    );

    // The request-log row: the synthetic host and path, the connection id, never the real path.
    let rows = l.rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].host, l.host(0));
    assert_eq!(rows[0].path, "/");
    assert_eq!(rows[0].connection_id, Some(l.ids[0]));
    assert_eq!(rows[0].app_id.as_deref(), Some("ics-calendar"));
    assert_eq!(rows[0].tool_id.as_deref(), Some("read_calendar"));
    clean("log row", &format!("{:?}", rows[0]));
}

#[tokio::test]
async fn head_works_post_is_405_and_a_read_only_connection_reads() {
    let l = lane(&[pq()]).await;
    l.src.0.lock().unwrap().conns[0].access = "read".into();
    let h = l.call(0, "HEAD", "/", "").await;
    assert_eq!(h.status, 200, "{}", h.head);
    assert!(h.body.is_empty(), "{}", h.body);
    assert!(header(&h, "content-length").is_some());
    for m in ["POST", "PUT", "DELETE", "PATCH"] {
        let r = l.call(0, m, "/", "").await;
        assert_eq!(r.status, 405, "{m}: {}", r.body);
        assert_eq!(r.code(), "method_not_allowed");
    }
    assert_eq!(l.upstream_count(), 1, "only the HEAD reached the upstream");
}

#[tokio::test]
async fn an_unknown_ungranted_or_foreign_host_is_refused_and_nothing_is_fetched() {
    let l = lane(&[pq()]).await;
    let none = format!("{}.link.aurora.internal", Uuid::now_v7());
    let r = request(&l.env, &none, "GET", "/", "").await;
    assert_eq!((r.status, r.code().as_str()), (403, "access_restricted"));
    let r = request(&l.env, "plainname.link.aurora.internal", "GET", "/", "").await;
    assert_eq!((r.status, r.code().as_str()), (403, "access_restricted"));
    // A connection no rule grants.
    l.src.0.lock().unwrap().ws.rules.clear();
    l.env.resolve_cache.flush_all();
    let r = l.get(0, "/", "").await;
    assert_eq!((r.status, r.code().as_str()), (403, "access_restricted"));
    assert_eq!(l.upstream_count(), 0);
}

#[tokio::test]
async fn the_cache_answers_after_admission_and_never_outlives_the_connection() {
    let l = lane(&[pq()]).await;
    let first = l.get(0, "/", "").await;
    assert_eq!(first.status, 200);
    let etag = header(&first, "etag").unwrap();
    // Within the TTL: no second upstream call, logged as a hit, with an Age.
    let second = l.get(0, "/", "").await;
    assert_eq!(second.status, 200);
    assert_eq!(second.body, ICS);
    assert_eq!(header(&second, "etag").as_deref(), Some(etag.as_str()));
    assert!(header(&second, "age").is_some());
    assert_eq!(l.upstream_count(), 1);
    let rows = l.rows().await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].error_code, None);
    assert_eq!(rows[0].upstream_status, Some(200));
    assert_eq!(rows[1].error_code, None);
    assert_eq!(rows[1].upstream_status, None);
    assert_eq!(rows[1].status, 200);
    // If-None-Match with the gateway's ETag: 304, still no upstream call.
    let nm = l.get(0, "/", &format!("If-None-Match: {etag}\r\n")).await;
    assert_eq!(nm.status, 304, "{}", nm.head);
    assert!(nm.body.is_empty());
    assert_eq!(l.upstream_count(), 1);
    // After the TTL: a new upstream call.
    l.env.cache.advance(Duration::from_secs(301));
    assert_eq!(l.get(0, "/", "").await.status, 200);
    assert_eq!(l.upstream_count(), 2);
    assert_eq!(l.env.cache.len(), 1);

    // Revocation of the grant while the body is cached: refused, not answered from memory.
    l.src.0.lock().unwrap().ws.rules.clear();
    l.env.resolve_cache.flush_all();
    let r = l.get(0, "/", "").await;
    assert_eq!((r.status, r.code().as_str()), (403, "access_restricted"));
    assert!(!r.body.contains("BEGIN:VCALENDAR"));
    assert_eq!(l.upstream_count(), 2);
}

#[tokio::test]
async fn a_connection_change_drops_the_entry_and_a_reconnect_never_serves_the_old_body() {
    let l = lane(&[pq()]).await;
    assert_eq!(l.get(0, "/", "").await.status, 200);
    assert_eq!(l.env.cache.len(), 1);
    // The invalidation hub's event for a reconnect / disable / delete of the connection.
    l.env.cache.invalidate(&Invalidate {
        scope: Scope::Connection(l.ids[0]),
    });
    assert!(l.env.cache.is_empty());
    assert_eq!(l.get(0, "/", "").await.status, 200);
    assert_eq!(
        l.upstream_count(),
        2,
        "the next request went upstream again"
    );
    // A new credential version is a new key even if no event arrived.
    l.src.0.lock().unwrap().conns[0].credentials_version = 2;
    l.env.resolve_cache.flush_all();
    assert_eq!(l.get(0, "/", "").await.status, 200);
    assert_eq!(l.upstream_count(), 3);
    // Delete: the connection is gone, so the request is refused.
    l.src.0.lock().unwrap().conns.clear();
    l.env.resolve_cache.flush_all();
    l.env.cache.invalidate(&Invalidate {
        scope: Scope::Connection(l.ids[0]),
    });
    let r = l.get(0, "/", "").await;
    assert_eq!((r.status, r.code().as_str()), (403, "access_restricted"));
    assert_eq!(l.upstream_count(), 3);
}

#[tokio::test]
async fn errors_and_cut_bodies_are_not_cached() {
    let l = lane(&["/err".into(), "/gone".into(), "/big".into()]).await;
    for _ in 0..2 {
        let r = l.get(0, "/", "").await;
        assert_eq!((r.status, r.code().as_str()), (502, "link_upstream_error"));
        let r = l.get(1, "/", "").await;
        assert_eq!((r.status, r.code().as_str()), (404, "link_not_found"));
    }
    assert_eq!(l.upstream_count(), 4, "every error went upstream again");
    let r = l.get(2, "/", "").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (502, "link_response_too_large")
    );
    assert!(l.env.cache.is_empty());
}

#[tokio::test]
async fn gone_is_link_not_found_too() {
    let l = lane(&["/removed".into()]).await;
    let r = l.get(0, "/", "").await;
    assert_eq!((r.status, r.code().as_str()), (404, "link_not_found"));
}

#[tokio::test]
async fn redirects_are_followed_up_to_three_and_never_shown() {
    // /r1 -> /r2 -> /r3 -> the calendar: three hops.
    let l = lane(&["/r1".into(), "/r0".into()]).await;
    let r = l.get(0, "/", "").await;
    assert_eq!(r.status, 200, "{} {}", r.head, r.body);
    assert_eq!(r.body, ICS);
    assert!(header(&r, "location").is_none());
    clean("redirected response", &r.head);
    // /r0 needs a fourth.
    let r = l.get(1, "/", "").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (502, "link_too_many_redirects")
    );
    assert!(header(&r, "location").is_none());
    clean("too many redirects", &format!("{}{}", r.head, r.body));
}

#[tokio::test]
async fn a_redirect_to_http_or_a_private_address_is_refused() {
    let l = lane(&["/to-http".into(), "/to-private".into()]).await;
    for i in 0..2 {
        let r = l.get(i, "/", "").await;
        assert_eq!(
            (r.status, r.code().as_str()),
            (403, "link_destination_refused"),
            "{}",
            r.body
        );
        assert!(header(&r, "location").is_none());
    }
}

#[tokio::test]
async fn an_org_block_rule_on_the_real_host_blocks_the_fetch() {
    let l = lane(&[pq()]).await;
    {
        let mut rows = l.src.0.lock().unwrap();
        rows.org.rules.push(rule(
            "no localhost",
            "block",
            json!([{"type": "network", "host": "localhost"}]),
            false,
        ));
    }
    l.env.resolve_cache.flush_all();
    let r = l.get(0, "/", "").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (403, "link_destination_refused"),
        "{}",
        r.body
    );
    assert_eq!(
        l.upstream_count(),
        0,
        "the blocked host was never contacted"
    );
}

#[tokio::test]
async fn an_echoed_url_is_withheld() {
    let l = lane(&[format!("/echo/{PATH_CANARY}?key={QUERY_CANARY}")]).await;
    let r = l.get(0, "/", "").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (502, "upstream_echoed_credential")
    );
    clean("echo refusal", &format!("{}{}", r.head, r.body));
    assert!(l.env.cache.is_empty());
}

#[tokio::test]
async fn the_canaries_appear_in_no_response_error_or_log_row() {
    let l = lane(&[
        pq(),
        "/gone".into(),
        "/err".into(),
        format!("/echo/{PATH_CANARY}?key={QUERY_CANARY}"),
        "/r0".into(),
        "/to-private".into(),
    ])
    .await;
    let mut seen = String::new();
    for i in 0..6 {
        let r = l
            .get(i, &format!("/{PATH_CANARY}?{QUERY_CANARY}"), "")
            .await;
        seen.push_str(&format!("{}\n{}\n", r.head, r.body));
    }
    // The agent's own path and query are ignored and never logged either.
    clean("responses", &seen);
    let rows = l.rows().await;
    assert_eq!(rows.len(), 6);
    for r in &rows {
        assert_eq!(r.path, "/");
        clean("log row", &format!("{r:?}"));
    }
}

/// The id, workspace and summary of the one pending approval of connection `conn`.
pub(super) async fn pending_for(pool: &sqlx::PgPool, conn: Uuid) -> (Uuid, Uuid, Value) {
    for _ in 0..100 {
        if let Some(row) = sqlx::query_as::<_, (Uuid, Uuid, Value)>(
            "SELECT id, workspace_id, summary FROM approvals WHERE connection_id=$1 AND status='pending'",
        )
        .bind(conn)
        .fetch_optional(pool)
        .await
        .unwrap()
        {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no pending approval");
}

/// Applies a decision the way the API does: first decision wins, then NOTIFY.
pub(super) async fn decide(pool: &sqlx::PgPool, id: Uuid, status: &str, ws: Uuid) {
    sqlx::query(
        "UPDATE approvals SET status=$2, decided_at=now() WHERE id=$1 AND status='pending'",
    )
    .bind(id)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("SELECT pg_notify('aurora_approval', $1)")
        .bind(json!({"id": id, "workspace_id": ws, "event": "decided"}).to_string())
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_approval_hold_names_the_connection_and_host_never_the_url_and_fetches_only_once_approved()
 {
    let db = aurora_testkit::TestDb::new().await;
    let pool = db.pool().clone();
    let l = Arc::new(lane_with_approval(&[pq()], pool.clone()).await);

    // Approve: held first (nothing fetched), then exactly one fetch.
    let l2 = l.clone();
    let t = tokio::spawn(async move { l2.get(0, "/", "").await });
    let (id, ws, summary) = pending_for(&pool, l.ids[0]).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!t.is_finished(), "the request stays held while pending");
    assert_eq!(l.upstream_count(), 0, "nothing is fetched while held");
    // The summary shows the connection's name and the real host, never the URL.
    let text = summary.to_string();
    assert!(text.contains("Team calendar 0"), "{text}");
    assert!(text.contains("localhost"), "{text}");
    clean("approval summary", &text);
    let (host, path): (String, String) =
        sqlx::query_as("SELECT host, path FROM approvals WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((host.as_str(), path.as_str()), (l.host(0).as_str(), "/"));
    // (The held row records the app, the tool and the account.)
    let (app, tool): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT app_id, tool_id FROM approvals WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(app.as_deref(), Some("ics-calendar"));
    assert_eq!(tool.as_deref(), Some("read_calendar"));
    decide(&pool, id, "approved", ws).await;
    let r = t.await.unwrap();
    assert_eq!(r.status, 200, "{} {}", r.head, r.body);
    assert_eq!(r.body, ICS);
    clean("approved response", &format!("{} {}", r.head, r.body));
    assert_eq!(l.upstream_count(), 1, "one fetch after the approval");

    // Approved, but the grant is withdrawn while held: the second pass re-reads it and refuses.
    let l2 = l.clone();
    let t = tokio::spawn(async move { l2.get(0, "/", "").await });
    let (id2, ws2, _) = pending_for(&pool, l.ids[0]).await;
    assert_ne!(id, id2);
    l.src.0.lock().unwrap().ws.rules.clear();
    l.env.resolve_cache.flush_all();
    decide(&pool, id2, "approved", ws2).await;
    let r = t.await.unwrap();
    assert_eq!(
        (r.status, r.code().as_str()),
        (403, "access_restricted"),
        "{} {}",
        r.head,
        r.body
    );
    assert_eq!(
        l.upstream_count(),
        1,
        "a withdrawn grant is not fetched after approval"
    );
}

#[tokio::test]
async fn a_denied_link_approval_fetches_nothing_and_leaks_nothing() {
    let db = aurora_testkit::TestDb::new().await;
    let pool = db.pool().clone();
    let l = Arc::new(lane_with_approval(&[pq()], pool.clone()).await);
    let l2 = l.clone();
    let t = tokio::spawn(async move { l2.get(0, "/", "").await });
    let (id, ws, _) = pending_for(&pool, l.ids[0]).await;
    decide(&pool, id, "denied", ws).await;
    let r = t.await.unwrap();
    assert_eq!(r.status, 403, "{} {}", r.head, r.body);
    assert_eq!(r.code(), "manual_approval_denied");
    assert!(r.body.contains(&id.to_string()), "{}", r.body);
    clean("denied response", &format!("{} {}", r.head, r.body));
    assert_eq!(l.upstream_count(), 0, "nothing is fetched after a denial");
    for row in l.rows().await {
        clean("log row", &format!("{row:?}"));
    }
}
