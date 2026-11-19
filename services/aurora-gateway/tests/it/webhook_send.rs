//! (phase 5, P5-WEBHOOK-2, D-320, C-910, SEC-127) The outgoing-webhook send path through the whole
//! pipeline: a request on `<connection_id>.link.aurora.internal` of a send-mode app is admitted,
//! validated, optionally approved, rate-taken and rendered, then posted to the stored URL on a fake
//! HTTPS server (private CA). The fake records every request; the canaries (the URL's path and
//! query, a secret value, the signing secret, a slot value) must reach nothing the agent, the log
//! or an approval can see beyond what C-910 allows.
//!
//! A child module of `apps` (it reuses that file's pipeline harness).
use super::*;
use aurora_gateway::ca::Ca;
use aurora_gateway::invalidation::Invalidatable;
use http_body_util::BodyExt;
use tokio_rustls::TlsAcceptor;

const PATH_CANARY: &str = "WHPATHCANARY0xA1B2C3";
const QUERY_CANARY: &str = "WHQUERYCANARY0xD4E5F6";
const SECRET: &str = "WHSECRETCANARY0x778899";
const SIGNING: &str = "whsec_WHSIGNINGCANARY0123456789abcdef";
const VALUE_CANARY: &str = "VALUECANARY-in-the-text";

/// `(method, path and query, headers, body)` of one request the fake received.
type Received = (String, String, Vec<(String, String)>, Vec<u8>);

#[derive(Default)]
struct Seen {
    requests: Vec<Received>,
}

fn hook_pq() -> String {
    format!("/hook/{PATH_CANARY}?key={QUERY_CANARY}")
}

async fn fake(
    seen: Arc<Mutex<Seen>>,
    port: u16,
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let (parts, body) = req.into_parts();
    let body = body
        .collect()
        .await
        .map(|b| b.to_bytes().to_vec())
        .unwrap_or_default();
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
        body.clone(),
    ));
    let to = |status: u16, loc: &str| {
        Response::builder()
            .status(status)
            .header("location", loc)
            .body(Full::new(Bytes::new()))
            .unwrap()
    };
    let path = parts.uri.path();
    Ok(match path {
        p if p.starts_with("/hook/") => Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .header("set-cookie", "sid=upstream-cookie")
            .header("retry-after", "7")
            .header("x-internal", "leak")
            .body(Full::new(Bytes::from("{\"ok\":true}")))
            .unwrap(),
        "/r307" => to(307, &format!("https://localhost:{port}/final")),
        "/r308" => to(308, &format!("https://localhost:{port}/final")),
        "/r303" => to(303, &format!("https://localhost:{port}/final")),
        "/loop" => to(302, &format!("https://localhost:{port}/loop")),
        "/to-http" => to(307, "http://localhost/plain"),
        "/to-private" => to(307, "https://169.254.169.254/latest/meta-data"),
        "/final" => Response::builder()
            .status(200)
            .body(Full::new(Bytes::from("final")))
            .unwrap(),
        "/gone" => Response::builder()
            .status(404)
            .body(Full::new(Bytes::new()))
            .unwrap(),
        // Echoes what it was sent: the secret header value and the body come back.
        "/echo" => Response::builder()
            .status(200)
            .body(Full::new(Bytes::from(format!(
                "you sent {}",
                String::from_utf8_lossy(&body)
            ))))
            .unwrap(),
        "/big" => Response::builder()
            .status(200)
            .body(Full::new(Bytes::from(vec![b'x'; 70 * 1024])))
            .unwrap(),
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(40)).await;
            Response::builder()
                .status(200)
                .body(Full::new(Bytes::new()))
                .unwrap()
        }
        _ => Response::builder()
            .status(418)
            .body(Full::new(Bytes::new()))
            .unwrap(),
    })
}

async fn server() -> (u16, String, Arc<Mutex<Seen>>) {
    let gen_ca = ca::generate("webhook-private-ca").unwrap();
    let ca = Ca::from_pem(&gen_ca.cert_pem, &gen_ca.key_pem).unwrap();
    let cfg = ca.server_config("localhost").unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s2 = seen.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = l.accept().await {
            let (cfg, seen) = (cfg.clone(), s2.clone());
            tokio::spawn(async move {
                let Ok(tls) = TlsAcceptor::from(cfg).accept(tcp).await else {
                    return;
                };
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(
                        TokioIo::new(tls),
                        service_fn(move |r| fake(seen.clone(), port, r)),
                    )
                    .await;
            });
        }
    });
    (port, gen_ca.cert_pem, seen)
}

fn snapshot_json() -> Value {
    let app = json!({
        "app_id": "webhook", "package_id": "pkg",
        "about": {"id": "webhook", "name": "Outgoing webhook"},
        "credentials": [{"name": "url", "secret": true, "optional": false}],
        "metadata": [{"name": "host"}, {"name": "name"}], "connect": [], "token": null,
        "tools": {"write": {"items": [
            {"id": "send", "name": "Send", "host": "*.link.aurora.internal", "path": "/", "methods": ["POST", "PUT"]}
        ]}},
        "approvals": {"send": {"title": "send a message to a webhook", "fields": []}},
        "agent": null, "database": null, "grants": {},
        "secret_link": {"credential": "url", "mode": "send"}
    });
    let mut snap = snapshot(vec![app], vec![]);
    snap["reserved_hosts"] = json!(["*.aurora.internal"]);
    snap
}

/// The stored configuration: a slot in the body, a secret in the body and one in a header.
fn config(extra: Value) -> Value {
    let mut c = json!({
        "version": 1, "method": "POST", "content_type": "application/json",
        "template": "{\"text\": {{text}}, \"topic\": \"{{secret:topic}}\", \"n\": {{n}}}",
        "slots": [
            {"name": "text", "type": "string", "required": true, "description": "The message", "max_length": 200},
            {"name": "n", "type": "integer", "required": false, "description": "A count"}
        ],
        "headers": [{"name": "Authorization", "value": "Bearer {{secret:topic}}"}],
        "description": "", "signing": true,
        "rate": {"per_minute": null, "per_day": null}, "preset": null
    });
    for (k, v) in extra.as_object().unwrap() {
        c[k] = v.clone();
    }
    c
}

struct Lane {
    env: Env,
    src: Arc<Src>,
    seen: Arc<Mutex<Seen>>,
    id: Uuid,
}

impl Lane {
    fn host(&self) -> String {
        format!("{}.link.aurora.internal", self.id)
    }

    fn upstream(&self) -> Vec<Received> {
        self.seen.lock().unwrap().requests.clone()
    }

    async fn post(&self, extra: &str, body: &str) -> Reply {
        self.call("POST", extra, body).await
    }

    async fn call(&self, method: &str, extra: &str, body: &str) -> Reply {
        request(&self.env, &self.host(), method, extra, body).await
    }

    async fn rows(&self) -> Vec<aurora_db::repo::gateway::RequestLogRow> {
        self.env.log.flush().await;
        self.env.rows.lock().unwrap().clone()
    }

    fn set_config(&self, cfg: Value, org: Option<Value>) {
        self.src.0.lock().unwrap().wh.insert(self.id, (cfg, org));
        self.env.resolve_cache.flush_all();
    }
}

async fn lane(
    url_path: &str,
    cfg: Value,
    org: Option<Value>,
    approval_pool: Option<aurora_db::PgPool>,
) -> Lane {
    let p = url_path.to_owned();
    lane_at(
        move |port| format!("https://localhost:{port}{p}"),
        cfg,
        org,
        approval_pool,
    )
    .await
}

/// [`lane`] with the stored URL built from the fake server's port.
async fn lane_at(
    url_of: impl FnOnce(u16) -> String,
    cfg: Value,
    org: Option<Value>,
    approval_pool: Option<aurora_db::PgPool>,
) -> Lane {
    let (port, ca_pem, seen) = server().await;
    let approval = approval_pool.is_some();
    let url = url_of(port);
    let creds = json!({"url": url, "secret_values": {"topic": SECRET}, "signing_secret": SIGNING})
        .to_string();
    let id: Arc<Mutex<Option<Uuid>>> = Arc::default();
    let id2 = id.clone();
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    let (env, src, _) = start_in(
        w,
        snapshot_json(),
        &[("c0", creds.as_str())],
        move |w| {
            let mut r = Rows::default();
            let mut c = conn(w, "webhook", "c0", None);
            c.metadata = json!({"host": "localhost", "name": "Ops alerts"});
            c.label = "Ops alerts".into();
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
            r.wh.insert(c.id, (cfg, org));
            *id2.lock().unwrap() = Some(c.id);
            r.conns.push(c);
            r
        },
        Some(ca_pem),
        approval_pool,
        None,
        approval,
    )
    .await;
    let id = id.lock().unwrap().unwrap();
    Lane { env, src, seen, id }
}

/// Like an agent: CONNECT to the synthetic host, TLS inside the tunnel, one request with a body.
async fn request(env: &Env, host: &str, method: &str, extra: &str, body: &str) -> Reply {
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
        "{method} /agent/path?x=1 HTTP/1.1\r\nHost: {host}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    t.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(60), t.read_to_end(&mut out)).await;
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

fn hdr(req: &Received, name: &str) -> Option<String> {
    req.2
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

const JSON: &str = "Content-Type: application/json\r\n";

/// No canary the agent must not see.
fn clean(what: &str, text: &str) {
    for c in [PATH_CANARY, QUERY_CANARY, SECRET, SIGNING] {
        assert!(!text.contains(c), "{what} leaked `{c}`: {text}");
    }
}

#[tokio::test]
async fn a_send_posts_exactly_the_rendered_body_with_only_the_allowed_headers() {
    let l = lane(&hook_pq(), config(json!({})), None, None).await;
    let r = l
        .post(
            &format!("{JSON}Authorization: Bearer agent-token\r\nCookie: agent=1\r\nX-Custom: yes\r\nX-Forwarded-For: 1.2.3.4\r\n"),
            &json!({"text": VALUE_CANARY, "n": 3}).to_string(),
        )
        .await;
    assert_eq!(r.status, 200, "{} {}", r.head, r.body);
    assert_eq!(r.body, "{\"ok\":true}");
    // Only Content-Type, Retry-After and the delivery id come back; never a cookie or an internal header.
    assert_eq!(
        header(&r, "content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(header(&r, "retry-after").as_deref(), Some("7"));
    assert!(
        header(&r, "set-cookie").is_none() && header(&r, "x-internal").is_none(),
        "{}",
        r.head
    );
    let delivery = header(&r, "x-aurora-delivery").expect("delivery id");
    clean("response", &format!("{} {}", r.head, r.body));
    // The receiver got the stored URL exactly (the agent's path and query are not forwarded).
    let got = l.upstream();
    assert_eq!(got.len(), 1);
    assert_eq!(
        (got[0].0.as_str(), got[0].1.as_str()),
        ("POST", hook_pq().as_str())
    );
    let sent: Value = serde_json::from_slice(&got[0].3).unwrap();
    assert_eq!(sent, json!({"text": VALUE_CANARY, "topic": SECRET, "n": 3}));
    assert_eq!(
        hdr(&got[0], "content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(
        hdr(&got[0], "authorization").unwrap(),
        format!("Bearer {SECRET}"),
        "the configured header, not the agent's"
    );
    assert_eq!(
        hdr(&got[0], "user-agent").as_deref(),
        Some("Aurora-Webhook/1")
    );
    assert_eq!(
        hdr(&got[0], "x-aurora-delivery").as_deref(),
        Some(delivery.as_str())
    );
    for h in ["cookie", "x-custom", "x-forwarded-for"] {
        assert!(hdr(&got[0], h).is_none(), "{h} must not be forwarded");
    }
    // The signature verifies over the raw body.
    let sig = hdr(&got[0], "x-aurora-signature").unwrap();
    let t: u64 = sig
        .split(',')
        .next()
        .unwrap()
        .trim_start_matches("t=")
        .parse()
        .unwrap();
    assert_eq!(sig, aurora_webhook::sign(SIGNING.as_bytes(), t, &got[0].3));
    // The log row holds the slot names, the size and the same delivery id: never a value or a secret.
    let rows = l.rows().await;
    let row = rows.last().unwrap();
    assert_eq!(row.status, 200);
    assert!(
        row.path.contains(&format!("delivery_id={delivery}")),
        "{}",
        row.path
    );
    assert!(
        row.path.contains("slots=text,n") && row.path.contains("dry_run=0"),
        "{}",
        row.path
    );
    let logged = format!("{row:?}");
    for c in [PATH_CANARY, QUERY_CANARY, SECRET, SIGNING, VALUE_CANARY] {
        assert!(
            !logged.contains(c),
            "the request log leaked `{c}`: {logged}"
        );
    }
}

#[tokio::test]
async fn refusals_come_before_any_upstream_call_and_say_they_are_the_gateways() {
    let l = lane(&hook_pq(), config(json!({})), None, None).await;
    let ok = json!({"text": "hi"}).to_string();
    let r = l.call("PUT", JSON, &ok).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (405, "method_not_allowed"),
        "{}",
        r.body
    );
    assert_eq!(
        header(&r, "x-aurora-error").as_deref(),
        Some("method_not_allowed")
    );
    let r = l.post("Content-Type: text/plain\r\n", &ok).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (415, "webhook_content_type"),
        "{}",
        r.body
    );
    let r = l
        .post(JSON, &json!({"text": "x".repeat(70_000)}).to_string())
        .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (413, "values_too_large"),
        "{}",
        r.body
    );
    for (body, slot, rule) in [
        (json!({}), "text", "missing"),
        (json!({"text": "hi", "extra": 1}), "extra", "unknown"),
        (json!({"text": 5}), "text", "type"),
        (json!({"text": "x".repeat(201)}), "text", "max_length"),
        (json!({"text": "hi", "n": "3"}), "n", "type"),
    ] {
        let r = l.post(JSON, &body.to_string()).await;
        assert_eq!(
            (r.status, r.code().as_str()),
            (422, "invalid_slot"),
            "{}",
            r.body
        );
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["error"]["details"], json!({"slot": slot, "rule": rule}));
        assert_eq!(
            header(&r, "x-aurora-error").as_deref(),
            Some("invalid_slot")
        );
    }
    let r = l.post(JSON, "not json").await;
    assert_eq!((r.status, r.code().as_str()), (422, "invalid_slot"));
    assert!(l.upstream().is_empty(), "nothing was sent");
    // The org switch off refuses every send.
    l.set_config(config(json!({})), Some(json!({"enabled": false})));
    let r = l.post(JSON, &ok).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (403, "outgoing_webhooks_disabled"),
        "{}",
        r.body
    );
    assert!(l.upstream().is_empty());
}

#[tokio::test]
async fn a_dry_run_shows_the_masked_body_sends_nothing_and_is_not_rate_counted() {
    let l = lane(
        &hook_pq(),
        config(json!({"rate": {"per_minute": 1, "per_day": null}})),
        None,
        None,
    )
    .await;
    for _ in 0..3 {
        let r = l
            .post(
                &format!("{JSON}X-Aurora-Dry-Run: 1\r\n"),
                &json!({"text": "hi"}).to_string(),
            )
            .await;
        assert_eq!(r.status, 200, "{} {}", r.head, r.body);
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["headers"], json!(["Authorization"]));
        let body: Value = serde_json::from_str(v["body"].as_str().unwrap()).unwrap();
        assert_eq!(body, json!({"text": "hi", "topic": "«secret:topic»"}));
        clean("dry run", &r.body);
    }
    assert!(l.upstream().is_empty());
    // The one real send still has its token.
    let r = l.post(JSON, &json!({"text": "hi"}).to_string()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(l.rows().await.iter().any(|x| x.path.contains("dry_run=1")));
}

#[tokio::test]
async fn the_lowest_of_the_instance_org_and_connection_limits_applies_at_once() {
    let l = lane(
        &hook_pq(),
        config(json!({"rate": {"per_minute": 5, "per_day": null}})),
        None,
        None,
    )
    .await;
    let ok = json!({"text": "hi"}).to_string();
    // The connection's own limit (5 a minute): the sixth send is refused, with the time to wait.
    for i in 0..5 {
        assert_eq!(l.post(JSON, &ok).await.status, 200, "send {i}");
    }
    let r = l.post(JSON, &ok).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (429, "webhook_rate_limited"),
        "{}",
        r.body
    );
    assert!(header(&r, "retry-after").is_some());
    assert_eq!(
        header(&r, "x-aurora-error").as_deref(),
        Some("webhook_rate_limited")
    );
    assert_eq!(l.upstream().len(), 5);
    // An org ceiling of 7 a day (set after): applies to the next send, the lowest wins.
    let l = lane(
        &hook_pq(),
        config(json!({})),
        Some(json!({"per_day_max": 2})),
        None,
    )
    .await;
    assert_eq!(l.post(JSON, &ok).await.status, 200);
    assert_eq!(l.post(JSON, &ok).await.status, 200);
    let r = l.post(JSON, &ok).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (429, "webhook_rate_limited"),
        "{}",
        r.body
    );
    // The org's per-minute ceiling lowers a connection that set more, from the next send.
    let l = lane(
        &hook_pq(),
        config(json!({"rate": {"per_minute": 20, "per_day": null}})),
        Some(json!({"per_minute_max": 1})),
        None,
    )
    .await;
    assert_eq!(l.post(JSON, &ok).await.status, 200);
    let r = l.post(JSON, &ok).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (429, "webhook_rate_limited"),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn redirects_307_keeps_the_body_303_becomes_get_and_unsafe_hops_are_refused() {
    let ok = json!({"text": "hi"}).to_string();
    let l = lane("/r307", config(json!({})), None, None).await;
    let r = l.post(JSON, &ok).await;
    assert_eq!(r.status, 200, "{} {}", r.head, r.body);
    assert_eq!(r.body, "final");
    let got = l.upstream();
    assert_eq!(got.len(), 2);
    assert_eq!((got[1].0.as_str(), got[1].1.as_str()), ("POST", "/final"));
    assert_eq!(got[1].3, got[0].3, "the body is resent");
    assert!(hdr(&got[1], "x-aurora-signature").is_some());
    // The redirect stays on the stored origin, so the configured header goes with it (a hop to
    // another origin would not carry it).
    assert_eq!(
        hdr(&got[1], "authorization").unwrap(),
        format!("Bearer {SECRET}")
    );
    // 303: a body-less GET.
    let l = lane("/r303", config(json!({})), None, None).await;
    let r = l.post(JSON, &ok).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let got = l.upstream();
    assert_eq!((got[1].0.as_str(), got[1].1.as_str()), ("GET", "/final"));
    assert!(got[1].3.is_empty() && hdr(&got[1], "content-type").is_none());
    // A loop: more than three hops.
    let l = lane("/loop", config(json!({})), None, None).await;
    let r = l.post(JSON, &ok).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (502, "link_too_many_redirects"),
        "{}",
        r.body
    );
    assert_eq!(l.upstream().len(), 4);
    // To http and to a private address: refused, nothing sent there.
    for p in ["/to-http", "/to-private"] {
        let l = lane(p, config(json!({})), None, None).await;
        let r = l.post(JSON, &ok).await;
        assert_eq!(
            (r.status, r.code().as_str()),
            (403, "link_destination_refused"),
            "{p}: {}",
            r.body
        );
        assert_eq!(l.upstream().len(), 1);
    }
}

#[tokio::test]
async fn the_receivers_answer_is_mapped_capped_and_checked_for_an_echo() {
    let ok = json!({"text": "hi"}).to_string();
    let l = lane("/gone", config(json!({})), None, None).await;
    let r = l.post(JSON, &ok).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (404, "link_not_found"),
        "{}",
        r.body
    );
    assert_eq!(
        header(&r, "x-aurora-error").as_deref(),
        Some("link_not_found")
    );
    // The receiver echoes the body (which holds the secret): withheld.
    let l = lane("/echo", config(json!({})), None, None).await;
    let r = l.post(JSON, &ok).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (502, "upstream_echoed_credential"),
        "{}",
        r.body
    );
    clean("echo", &format!("{} {}", r.head, r.body));
    // A long answer is cut at 64 KiB and says so.
    let l = lane("/big", config(json!({})), None, None).await;
    let r = l.post(JSON, &ok).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body.len(), 64 * 1024);
    assert_eq!(header(&r, "x-aurora-truncated").as_deref(), Some("1"));
    // A receiver's own 418 is its own answer (no gateway error header).
    let l = lane("/teapot", config(json!({})), None, None).await;
    let r = l.post(JSON, &ok).await;
    assert_eq!(r.status, 418);
    assert!(header(&r, "x-aurora-error").is_none());
}

#[tokio::test]
async fn a_timeout_is_504_may_have_been_delivered_and_the_receiver_saw_one_request() {
    let l = lane("/slow", config(json!({})), None, None).await;
    let r = l.post(JSON, &json!({"text": "hi"}).to_string()).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (504, "webhook_timeout"),
        "{}",
        r.body
    );
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["error"]["details"]["may_have_been_delivered"], true);
    assert_eq!(l.upstream().len(), 1, "no retry");
}

#[tokio::test]
async fn a_connect_failure_is_502_link_unreachable_and_nothing_was_delivered() {
    // Nothing listens on the stored port.
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let l = lane_at(
        move |_| format!("https://localhost:{dead}/hook/x"),
        config(json!({})),
        None,
        None,
    )
    .await;
    let r = l.post(JSON, &json!({"text": "hi"}).to_string()).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (502, "link_unreachable"),
        "{}",
        r.body
    );
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["error"]["details"]["may_have_been_delivered"], false);
    assert_eq!(
        header(&r, "x-aurora-error").as_deref(),
        Some("link_unreachable")
    );
}

#[tokio::test]
async fn an_approval_shows_the_slot_values_creates_none_for_an_invalid_send_and_sends_after_approve()
 {
    let db = aurora_testkit::TestDb::new().await;
    let pool = db.pool().clone();
    let l = Arc::new(lane(&hook_pq(), config(json!({})), None, Some(pool.clone())).await);
    // An invalid send never creates an approval.
    let r = l.post(JSON, &json!({"text": 1}).to_string()).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (422, "invalid_slot"),
        "{}",
        r.body
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // A valid one is held: nothing is sent while it is pending.
    let l2 = l.clone();
    let body = json!({"text": VALUE_CANARY, "n": 3}).to_string();
    let b2 = body.clone();
    let t = tokio::spawn(async move { l2.post(JSON, &b2).await });
    let (id, ws, summary) = super::link_lane::pending_for(&pool, l.id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!t.is_finished() && l.upstream().is_empty());
    let text = summary.to_string();
    assert!(
        text.contains("Send to Ops alerts")
            && text.contains(VALUE_CANARY)
            && text.contains("localhost"),
        "{text}"
    );
    for c in [
        PATH_CANARY,
        QUERY_CANARY,
        SECRET,
        SIGNING,
        "Authorization",
        "topic",
    ] {
        assert!(
            !text.contains(c),
            "the approval summary shows `{c}`: {text}"
        );
    }
    super::link_lane::decide(&pool, id, "approved", ws).await;
    let r = t.await.unwrap();
    assert_eq!(r.status, 200, "{} {}", r.head, r.body);
    assert_eq!(l.upstream().len(), 1, "one send after the approval");
    // Denied: nothing is sent.
    let l2 = l.clone();
    let t = tokio::spawn(async move { l2.post(JSON, &body).await });
    let (id2, ws2, _) = super::link_lane::pending_for(&pool, l.id).await;
    assert_ne!(id, id2);
    super::link_lane::decide(&pool, id2, "denied", ws2).await;
    let r = t.await.unwrap();
    assert_eq!(
        (r.status, r.code().as_str()),
        (403, "manual_approval_denied"),
        "{}",
        r.body
    );
    assert_eq!(l.upstream().len(), 1);
    // A dry run under an `ask` rule is not held.
    let r = l
        .post(
            &format!("{JSON}X-Aurora-Dry-Run: 1\r\n"),
            &json!({"text": "hi"}).to_string(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
}
