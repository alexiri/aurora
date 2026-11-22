//! (P5-GUIDE-6, `docs/24 §10a`, SEC-118, SEC-120, `18 §App guides on demand`) `aurora-agent` app
//! targets against a local mock upstream: what is sent, what is refused before sending, what is
//! saved and printed, and the exit codes.
//!
//! SEC-118 is tested at the client: whatever the model types, the only host contacted is the
//! account's, the only Aurora header is `x-aurora-connection-id`, no credential header is ever
//! built, and redirects and foreign `next` links are not followed. The *authority* (the gateway
//! deciding every request) is not the CLI's and is covered by the gateway suites and the hosted
//! end-to-end run (`scripts/e2e/hosted.sh`), where `aurora-agent` runs behind the real proxy.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aurora_supervisor::agent_cli::call::{Env, Outcome};
use aurora_supervisor::agent_cli::{self, Verb};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct Req {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.as_str())
    }
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn json(status: u16, v: &Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: serde_json::to_vec(v).unwrap(),
        }
    }
}

struct Mock {
    port: u16,
    seen: Arc<Mutex<Vec<Req>>>,
}

type Handler = dyn Fn(&Req, u16) -> Resp + Send + Sync;

async fn mock(handler: impl Fn(&Req, u16) -> Resp + Send + Sync + 'static) -> Mock {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<Req>>> = Arc::default();
    let h: Arc<Handler> = Arc::new(handler);
    let s = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = l.accept().await else {
                return;
            };
            let (h, s) = (h.clone(), s.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.lines();
                let first = lines.next().unwrap().to_owned();
                let mut parts = first.split(' ');
                let (method, target) = (
                    parts.next().unwrap().to_owned(),
                    parts.next().unwrap().to_owned(),
                );
                let headers: Vec<(String, String)> = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                    .collect();
                let len: usize = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let req = Req {
                    method,
                    target,
                    headers,
                    body: buf[head_end..].to_vec(),
                };
                let port = sock.local_addr().unwrap().port();
                s.lock().unwrap().push(req.clone());
                let r = h(&req, port);
                let mut out = format!(
                    "HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                    r.status,
                    r.body.len()
                );
                for (k, v) in &r.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.write_all(&r.body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Mock { port, seen }
}

impl Mock {
    fn seen(&self) -> Vec<Req> {
        self.seen.lock().unwrap().clone()
    }
}

struct Sandbox {
    root: PathBuf,
    apps: PathBuf,
    env: Env,
}

fn sandbox(tag: &str) -> Sandbox {
    let root = std::env::temp_dir().join(format!("aurora-agent-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let apps = root.join("home/.aurora/apps");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::create_dir_all(root.join("home/memory")).unwrap();
    let env = Env {
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        out_dir: root.join("tmp-out"),
        protected: vec![root.join("home/.aurora"), root.join("home/memory")],
        cwd: root.clone(),
        wait_notice: Duration::from_secs(60),
        tools_socket: root.join("tools.sock"),
        session: None,
    };
    Sandbox { root, apps, env }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write_api(apps: &Path, app: &str, doc: &Value) {
    let d = apps.join(app);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("api.json"), serde_json::to_vec(doc).unwrap()).unwrap();
}

fn api_doc(port: u16, accounts: usize, ops: Value) -> Value {
    let accts: Vec<Value> = (0..accounts)
        .map(|i| {
            let letter = ["A", "B"][i];
            json!({"letter": letter, "connection_id": format!("c_{i}"), "label": format!("Acct {i}"),
                   "base_url": format!("http://127.0.0.1:{port}"), "grant": "full"})
        })
        .collect();
    json!({"format": "aurora.app-api/1", "app": {"id": "demo", "name": "Demo", "version": "1"},
           "accounts": accts, "operations": ops, "recipes": [], "untooled": {}})
}

fn op(id: &str, method: &str, path: &str, extra: Value) -> Value {
    let mut o = json!({"id": id, "method": method, "path": path, "host": null, "summary": id,
                       "tags": [], "params": [], "body": null, "pagination": null, "items": null,
                       "tool": null, "access": {}, "source": "package"});
    for (k, v) in extra.as_object().unwrap() {
        o[k] = v.clone();
    }
    o
}

fn call(target: &str, rest: &[&str]) -> Verb {
    Verb::Call {
        target: target.into(),
        rest: rest.iter().map(|s| (*s).to_owned()).collect(),
        account: None,
        all: false,
        max_pages: None,
        path: None,
        method: None,
        body: None,
        out: None,
        raw: false,
    }
}

fn with(v: Verb, f: impl FnOnce(&mut Verb)) -> Verb {
    let mut v = v;
    f(&mut v);
    v
}

macro_rules! set {
    ($verb:expr, $($field:ident = $val:expr),+) => {
        with($verb, |v| {
            if let Verb::Call { $($field),+, .. } = v {
                $(*$field = $val;)+
            }
        })
    };
}

fn text(o: &Outcome) -> String {
    String::from_utf8_lossy(&o.out).to_string()
}

fn saved_path(o: &Outcome) -> PathBuf {
    let t = text(o);
    let line = t.lines().find(|l| l.starts_with("saved: ")).unwrap();
    PathBuf::from(line.trim_start_matches("saved: "))
}

#[tokio::test]
async fn all_joins_pages_sends_only_the_connection_header_and_saves_the_file() {
    let m = mock(|r, _| {
        let page: u32 = r
            .target
            .split("page=")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let items: Vec<Value> = if page <= 2 {
            (0..2)
                .map(|i| json!({"id": page * 10 + i, "name": "x"}))
                .collect()
        } else {
            vec![]
        };
        Resp::json(200, &json!({"data": items}))
    })
    .await;
    let s = sandbox("all");
    let list = op(
        "list_things",
        "GET",
        "/v1/things",
        json!({"params": [{"name": "q", "in": "query", "type": "string"}],
               "pagination": {"style": "page_param", "param": "page", "start": 1},
               "items": "$.data"}),
    );
    write_api(&s.apps, "demo", &api_doc(m.port, 1, json!([list])));
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        set!(call("demo", &["list_things", "q=a b"]), all = true),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.err);
    let t = text(&o);
    assert!(t.contains("3 pages"), "{t}");
    assert!(t.contains("items: 4"), "{t}");
    assert!(t.chars().count() <= aurora_core::limits::AGENT_CLI_PREVIEW_MAX);
    let saved: Value = serde_json::from_slice(&std::fs::read(saved_path(&o)).unwrap()).unwrap();
    assert_eq!(saved.as_array().unwrap().len(), 4);
    let seen = m.seen();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].target, "/v1/things?q=a%20b&page=1");
    assert_eq!(seen[1].target, "/v1/things?q=a%20b&page=2");
    for r in &seen {
        assert_eq!(r.header("x-aurora-connection-id"), Some("c_0"));
        // SEC-118: nothing else from Aurora, no credential header of any kind.
        for (k, _) in &r.headers {
            let k = k.to_ascii_lowercase();
            assert!(
                !k.starts_with("x-aurora-") || k == "x-aurora-connection-id",
                "{k}"
            );
            assert!(
                !matches!(
                    k.as_str(),
                    "authorization" | "cookie" | "proxy-authorization"
                ),
                "{k}"
            );
        }
    }
}

#[tokio::test]
async fn max_pages_stops_and_says_so() {
    let m = mock(|_, _| Resp::json(200, &json!({"data": [{"id": 1}]}))).await;
    let s = sandbox("maxpages");
    let list = op(
        "l",
        "GET",
        "/l",
        json!({"pagination": {"style": "page_param", "param": "p"}, "items": "$.data"}),
    );
    write_api(&s.apps, "demo", &api_doc(m.port, 1, json!([list])));
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        set!(call("demo", &["l"]), all = true, max_pages = Some(3)),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(
        text(&o).contains("stopped at --max-pages 3"),
        "{}",
        text(&o)
    );
    assert_eq!(m.seen().len(), 3);
}

#[tokio::test]
async fn one_page_without_all_says_whether_there_is_more() {
    let m = mock(|_, _| Resp::json(200, &json!([{"id": 1}]))).await;
    let s = sandbox("more");
    let list = op(
        "l",
        "GET",
        "/l",
        json!({"pagination": {"style": "page_param", "param": "p"}, "items": "$"}),
    );
    write_api(&s.apps, "demo", &api_doc(m.port, 1, json!([list])));
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["l"])).await;
    assert!(
        text(&o).contains("more pages: yes (add --all"),
        "{}",
        text(&o)
    );
    assert_eq!(m.seen().len(), 1);
}

#[tokio::test]
async fn link_header_follows_the_same_origin_only() {
    let m = mock(|r, port| {
        if r.target.contains("page=2") {
            Resp::json(200, &json!([{"id": 3}]))
        } else {
            let mut x = Resp::json(200, &json!([{"id": 1}, {"id": 2}]));
            x.headers.push((
                "link".into(),
                format!("<http://127.0.0.1:{port}/l?page=2>; rel=\"next\""),
            ));
            x
        }
    })
    .await;
    let s = sandbox("link");
    write_api(
        &s.apps,
        "demo",
        &api_doc(
            m.port,
            1,
            json!([op(
                "l",
                "GET",
                "/l",
                json!({"pagination": {"style": "link_header"}, "items": "$"})
            )]),
        ),
    );
    let o = agent_cli::execute(&s.env, &s.apps, set!(call("demo", &["l"]), all = true)).await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(text(&o).contains("items: 3"), "{}", text(&o));

    // A `next` link to another host is not followed (SEC-118).
    let evil = mock(|_, _| Resp::json(200, &json!([]))).await;
    let eport = evil.port;
    let m2 = mock(move |_, _| {
        let mut x = Resp::json(200, &json!([{"id": 1}]));
        x.headers.push((
            "link".into(),
            format!("<http://127.0.0.1:{eport}/steal>; rel=\"next\""),
        ));
        x
    })
    .await;
    write_api(
        &s.apps,
        "demo",
        &api_doc(
            m2.port,
            1,
            json!([op(
                "l",
                "GET",
                "/l",
                json!({"pagination": {"style": "link_header"}, "items": "$"})
            )]),
        ),
    );
    let o = agent_cli::execute(&s.env, &s.apps, set!(call("demo", &["l"]), all = true)).await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(
        text(&o).contains("another origin was not followed"),
        "{}",
        text(&o)
    );
    assert!(evil.seen().is_empty(), "a foreign next link was followed");
}

#[tokio::test]
async fn cursor_pagination_reads_next_until_absent() {
    let m = mock(|r, _| {
        if r.target.contains("cursor=c2") {
            Resp::json(200, &json!({"items": [{"id": 3}], "next": null}))
        } else if r.target.contains("cursor=c1") {
            Resp::json(200, &json!({"items": [{"id": 2}], "next": "c2"}))
        } else {
            Resp::json(200, &json!({"items": [{"id": 1}], "next": "c1"}))
        }
    })
    .await;
    let s = sandbox("cursor");
    write_api(
        &s.apps,
        "demo",
        &api_doc(
            m.port,
            1,
            json!([op(
                "l",
                "GET",
                "/l",
                json!({"pagination": {"style": "cursor", "param": "cursor", "next": "$.next"}, "items": "$.items"})
            )]),
        ),
    );
    let o = agent_cli::execute(&s.env, &s.apps, set!(call("demo", &["l"]), all = true)).await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(text(&o).contains("items: 3"), "{}", text(&o));
}

#[tokio::test]
async fn a_path_value_is_one_segment_on_the_wire() {
    let m = mock(|_, _| Resp::json(200, &json!({"ok": true}))).await;
    let s = sandbox("segment");
    write_api(
        &s.apps,
        "demo",
        &api_doc(
            m.port,
            1,
            json!([op(
                "get_user",
                "GET",
                "/users/{id}/repos",
                json!({"params": [{"name": "id", "in": "path", "required": true, "type": "string"}]})
            )]),
        ),
    );
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        call("demo", &["get_user", "id=../admin?x=1#y"]),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(m.seen()[0].target, "/users/..%2Fadmin%3Fx%3D1%23y/repos");
    // `..` alone is refused before anything is sent.
    let before = m.seen().len();
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["get_user", "id=.."])).await;
    assert_eq!(o.code, 2);
    assert_eq!(m.seen().len(), before);
}

#[tokio::test]
async fn path_option_cannot_name_another_host_or_scheme() {
    let m = mock(|_, _| Resp::json(200, &json!({}))).await;
    let s = sandbox("pathopt");
    write_api(&s.apps, "demo", &api_doc(m.port, 1, json!([])));
    for bad in [
        "//evil.test/x",
        "http://evil.test/x",
        "@evil.test/x",
        "/x?y=1",
        "/x#frag",
        "/../x",
    ] {
        let o = agent_cli::execute(
            &s.env,
            &s.apps,
            set!(call("demo", &[]), path = Some(bad.into())),
        )
        .await;
        assert_eq!(o.code, 2, "{bad}: {}", o.err);
    }
    assert!(m.seen().is_empty(), "something was sent for a refused path");
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        set!(call("demo", &["q=1"]), path = Some("/ok/path".into())),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(m.seen()[0].method, "GET");
    assert_eq!(m.seen()[0].target, "/ok/path?q=1");
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let evil = mock(|_, _| Resp::json(200, &json!({}))).await;
    let eport = evil.port;
    let m = mock(move |_, _| Resp {
        status: 302,
        headers: vec![("location".into(), format!("http://127.0.0.1:{eport}/x"))],
        body: vec![],
    })
    .await;
    let s = sandbox("redirect");
    write_api(
        &s.apps,
        "demo",
        &api_doc(m.port, 1, json!([op("o", "GET", "/o", json!({}))])),
    );
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["o"])).await;
    assert_eq!(o.code, 1);
    assert!(o.err.contains("not followed"), "{}", o.err);
    assert!(evil.seen().is_empty());
}

#[tokio::test]
async fn a_gateway_refusal_exits_3_and_an_upstream_error_exits_1() {
    let m = mock(|r, _| {
        if r.target.starts_with("/blocked") {
            Resp::json(403, &json!({"error": {"code": "blocked_by_policy", "message": "Workspace policy 'no-writes' blocks this request.", "details": {"scope": "workspace"}}}))
        } else {
            Resp::json(404, &json!({"message": "Not Found"}))
        }
    })
    .await;
    let s = sandbox("refusal");
    write_api(
        &s.apps,
        "demo",
        &api_doc(
            m.port,
            1,
            json!([
                op("blocked", "POST", "/blocked", json!({})),
                op("missing", "GET", "/missing", json!({}))
            ]),
        ),
    );
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["blocked"])).await;
    assert_eq!(o.code, 3);
    assert!(
        o.err.contains("blocked_by_policy") && o.err.contains("no-writes"),
        "{}",
        o.err
    );
    assert!(
        o.err.contains("workspace"),
        "details are printed: {}",
        o.err
    );
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["missing"])).await;
    assert_eq!(o.code, 1);
    assert!(o.err.contains("HTTP 404"), "{}", o.err);
    assert!(!o.err.contains("gateway"));
}

#[tokio::test]
async fn several_accounts_need_a_choice_and_the_choice_sets_the_header() {
    let m = mock(|_, _| Resp::json(200, &json!({"ok": 1}))).await;
    let s = sandbox("accounts");
    write_api(
        &s.apps,
        "demo",
        &api_doc(m.port, 2, json!([op("o", "GET", "/o", json!({}))])),
    );
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["o"])).await;
    assert_eq!(o.code, 2);
    assert!(
        o.err.contains("--account") && o.err.contains("Acct 1"),
        "{}",
        o.err
    );
    assert!(m.seen().is_empty(), "no guessing");
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        set!(call("demo", &["o"]), account = Some("b".into())),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(m.seen()[0].header("x-aurora-connection-id"), Some("c_1"));
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        set!(call("demo", &["o"]), account = Some("c_0".into())),
    )
    .await;
    assert_eq!(o.code, 0);
    assert_eq!(m.seen()[1].header("x-aurora-connection-id"), Some("c_0"));
}

#[tokio::test]
async fn bodies_are_sent_as_declared_and_unknown_arguments_are_refused_unsent() {
    let m = mock(|_, _| Resp::json(201, &json!({"id": 9}))).await;
    let s = sandbox("body");
    let mk = op(
        "mk",
        "POST",
        "/things",
        json!({"body": {"content_type": "application/json", "fields": [
            {"name": "name", "type": "string", "required": true}, {"name": "n", "type": "integer"}]}}),
    );
    write_api(&s.apps, "demo", &api_doc(m.port, 1, json!([mk])));
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["mk", "name=x", "n=3"])).await;
    assert_eq!(o.code, 0, "{}", o.err);
    let r = &m.seen()[0];
    assert_eq!(r.method, "POST");
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(
        serde_json::from_slice::<Value>(&r.body).unwrap(),
        json!({"name": "x", "n": 3})
    );
    for bad in [
        vec!["mk", "n=3"],
        vec!["mk", "name=x", "n=three"],
        vec!["mk", "name=x", "extra=1"],
    ] {
        let o = agent_cli::execute(&s.env, &s.apps, call("demo", &bad)).await;
        assert_eq!(o.code, 2, "{bad:?}");
    }
    assert_eq!(m.seen().len(), 1, "refused calls send nothing");
}

#[tokio::test]
async fn out_is_refused_under_the_managed_roots_and_nothing_is_sent() {
    let m = mock(|_, _| Resp::json(200, &json!({"a": 1}))).await;
    let s = sandbox("out");
    write_api(
        &s.apps,
        "demo",
        &api_doc(m.port, 1, json!([op("o", "GET", "/o", json!({}))])),
    );
    for bad in [
        s.root.join("home/.aurora/apps/demo/api.json"),
        s.root.join("home/memory/x.md"),
        s.root.join("home/.aurora/new.json"),
    ] {
        let o = agent_cli::execute(
            &s.env,
            &s.apps,
            set!(call("demo", &["o"]), out = Some(bad.clone())),
        )
        .await;
        assert_eq!(o.code, 2, "{}", bad.display());
    }
    assert!(m.seen().is_empty());
    let api_before = std::fs::read(s.apps.join("demo/api.json")).unwrap();
    let ok = s.root.join("elsewhere/r.json");
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        set!(call("demo", &["o"]), out = Some(ok.clone())),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(std::fs::read(&ok).unwrap(), br#"{"a":1}"#);
    assert_eq!(
        std::fs::read(s.apps.join("demo/api.json")).unwrap(),
        api_before
    );
}

#[tokio::test]
async fn the_default_output_lands_in_the_private_directory_and_raw_prints_the_body() {
    let m = mock(|_, _| Resp::json(200, &json!({"a": 1}))).await;
    let s = sandbox("raw");
    write_api(
        &s.apps,
        "demo",
        &api_doc(m.port, 1, json!([op("o", "GET", "/o", json!({}))])),
    );
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["o"])).await;
    assert!(saved_path(&o).starts_with(&s.env.out_dir));
    let o = agent_cli::execute(&s.env, &s.apps, set!(call("demo", &["o"]), raw = true)).await;
    assert_eq!(text(&o), r#"{"a":1}"#);
}

#[tokio::test]
async fn a_huge_response_is_cut_at_the_save_limit_and_the_preview_stays_bounded() {
    // 1 MiB of JSON is under the limit; assert the preview bound with a wide array instead.
    let m = mock(|_, _| {
        let items: Vec<Value> = (0..2000)
            .map(|i| json!({"id": i, "text": "y".repeat(300)}))
            .collect();
        Resp::json(200, &Value::Array(items))
    })
    .await;
    let s = sandbox("big");
    write_api(
        &s.apps,
        "demo",
        &api_doc(m.port, 1, json!([op("o", "GET", "/o", json!({}))])),
    );
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["o"])).await;
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(text(&o).chars().count() <= aurora_core::limits::AGENT_CLI_PREVIEW_MAX);
    assert!(std::fs::metadata(saved_path(&o)).unwrap().len() > 600_000);
}

#[tokio::test]
async fn a_catalogue_entry_needs_an_explicit_path_and_an_unlisted_app_is_a_usage_error() {
    let s = sandbox("catalogue");
    let mut cat = op(
        "list_events",
        "GET",
        "/events/*",
        json!({"source": "catalogue"}),
    );
    cat["params"] = json!([]);
    write_api(&s.apps, "demo", &api_doc(1, 1, json!([cat])));
    let o = agent_cli::execute(&s.env, &s.apps, call("demo", &["list_events"])).await;
    assert_eq!(o.code, 2);
    assert!(o.err.contains("explicit --path"), "{}", o.err);
    let o = agent_cli::execute(&s.env, &s.apps, call("nope", &["x"])).await;
    assert_eq!(o.code, 2);
    // A database app (a folder without api.json) says so and points to app.md.
    std::fs::create_dir_all(s.apps.join("dbapp")).unwrap();
    let o = agent_cli::execute(&s.env, &s.apps, call("dbapp", &["x"])).await;
    assert_eq!(o.code, 2);
    assert!(o.err.contains("app.md"), "{}", o.err);
}

#[tokio::test]
async fn target_aurora_needs_a_conversation_and_refuses_app_options_first() {
    let s = sandbox("aurora");
    for v in [
        call("aurora", &["memory_save"]),
        Verb::Describe {
            target: "aurora".into(),
            name: "x".into(),
        },
        Verb::List {
            target: Some("aurora".into()),
        },
    ] {
        let o = agent_cli::execute(&s.env, &s.apps, v).await;
        assert_eq!(o.code, 2);
        // No `AURORA_SESSION`: nothing is sent (the platform tools are for a conversation's turn).
        assert!(o.err.contains("inside a conversation turn"), "{}", o.err);
    }
    let o = agent_cli::execute(&s.env, &s.apps, set!(call("aurora", &["x"]), all = true)).await;
    assert_eq!(o.code, 2);
    assert!(o.err.contains("--all does not apply"), "{}", o.err);
    // It stays out of `list` and `search`.
    write_api(
        &s.apps,
        "demo",
        &api_doc(1, 1, json!([op("search_things", "GET", "/s", json!({}))])),
    );
    let l = agent_cli::execute(&s.env, &s.apps, Verb::List { target: None }).await;
    assert!(!text(&l).contains("aurora"), "{}", text(&l));
    let q = agent_cli::execute(
        &s.env,
        &s.apps,
        Verb::Search {
            words: vec!["things".into()],
            target: None,
            limit: 15,
        },
    )
    .await;
    assert!(
        text(&q).contains("search_things") && !text(&q).contains("aurora "),
        "{}",
        text(&q)
    );
}

// ---- P5-WEBHOOK-3 (C-911): a webhook is an account-bound operation ----

fn webhook_op() -> Value {
    op(
        "send_ops_alerts",
        "POST",
        "/",
        json!({
            "summary": "Ops alerts — Deploy results.",
            "tags": ["webhook"],
            "notes": "Deploy results go here.",
            "account": "B",
            "source": "connection",
            "tool": "send",
            "params": [{"name": "X-Aurora-Dry-Run", "in": "header", "type": "string", "enum": ["1"],
                        "description": "Check and show the body without sending"}],
            "body": {"content_type": "application/json", "fields": [
                {"name": "text", "required": true, "type": "string", "description": "The message"},
                {"name": "card", "type": "json"}
            ]}
        }),
    )
}

async fn webhook_sandbox(m: &Mock, tag: &str) -> Sandbox {
    let s = sandbox(tag);
    write_api(
        &s.apps,
        "webhook",
        &api_doc(m.port, 2, json!([webhook_op()])),
    );
    s
}

#[tokio::test]
async fn a_webhook_call_uses_its_own_account_and_refuses_another() {
    let m = mock(|_, _| Resp::json(200, &json!({"ok": 1}))).await;
    let s = webhook_sandbox(&m, "wh-account").await;
    // Two accounts on the app are no ambiguity: the operation belongs to B.
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        call(
            "webhook",
            &[
                "send_ops_alerts",
                "text=Deploy 42 failed",
                "card:={\"a\":[1]}",
            ],
        ),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.err);
    let r = &m.seen()[0];
    assert_eq!(r.header("x-aurora-connection-id"), Some("c_1"));
    assert_eq!(
        serde_json::from_slice::<Value>(&r.body).unwrap(),
        json!({"text": "Deploy 42 failed", "card": {"a": [1]}})
    );
    // Naming the other account is refused before anything is sent.
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        set!(
            call("webhook", &["send_ops_alerts", "text=x"]),
            account = Some("a".into())
        ),
    )
    .await;
    assert_eq!(o.code, 2);
    assert!(o.err.contains("belongs to account B"), "{}", o.err);
    assert_eq!(m.seen().len(), 1, "nothing sent");
    // Naming its own account is fine.
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        set!(
            call("webhook", &["send_ops_alerts", "text=x"]),
            account = Some("B".into())
        ),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.err);
}

#[tokio::test]
async fn a_json_field_takes_only_colon_equals() {
    let m = mock(|_, _| Resp::json(200, &json!({"ok": 1}))).await;
    let s = webhook_sandbox(&m, "wh-json").await;
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        call("webhook", &["send_ops_alerts", "text=x", "card={\"a\":1}"]),
    )
    .await;
    assert_eq!(o.code, 2);
    assert!(o.err.contains("card:=<json>"), "the hint: {}", o.err);
    assert!(m.seen().is_empty());
}

#[tokio::test]
async fn the_dry_run_header_can_be_set_and_nothing_else_of_aurora_can() {
    let m = mock(|_, _| Resp::json(200, &json!({"dry_run": true, "body": "{}"}))).await;
    let s = webhook_sandbox(&m, "wh-dry").await;
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        call(
            "webhook",
            &["send_ops_alerts", "text=x", "X-Aurora-Dry-Run=1"],
        ),
    )
    .await;
    assert_eq!(o.code, 0, "{}", o.err);
    let r = &m.seen()[0];
    assert_eq!(r.header("x-aurora-dry-run"), Some("1"));
    assert_eq!(r.header("x-aurora-connection-id"), Some("c_1"));
    // Any other value is refused by the declared enum.
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        call(
            "webhook",
            &["send_ops_alerts", "text=x", "X-Aurora-Dry-Run=2"],
        ),
    )
    .await;
    assert_eq!(o.code, 2);
}

#[tokio::test]
async fn a_webhook_gateway_refusal_exits_3_and_a_receiver_400_exits_1() {
    let m = mock(|r, _| {
        if r.body.windows(4).any(|w| w == b"bad!") {
            // The receiver's own 400, in its own words.
            Resp::json(400, &json!({"error": "invalid_payload"}))
        } else {
            let mut x = Resp::json(504, &json!({"error": {"code": "webhook_timeout", "message": "The receiver did not answer in time.", "details": {"may_have_been_delivered": true}}}));
            x.headers.push(("x-aurora-error".into(), "webhook_timeout".into()));
            x
        }
    })
    .await;
    let s = webhook_sandbox(&m, "wh-exit").await;
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        call("webhook", &["send_ops_alerts", "text=ok"]),
    )
    .await;
    assert_eq!(o.code, 3);
    assert!(
        o.err.contains("webhook_timeout") && o.err.contains("may_have_been_delivered: true"),
        "{}",
        o.err
    );
    let o = agent_cli::execute(
        &s.env,
        &s.apps,
        call("webhook", &["send_ops_alerts", "text=bad!"]),
    )
    .await;
    assert_eq!(o.code, 1);
    assert!(
        o.err.contains("HTTP 400") && !o.err.contains("gateway"),
        "{}",
        o.err
    );
}

#[tokio::test]
async fn describe_and_search_cover_the_webhook_notes_and_slots() {
    let m = mock(|_, _| Resp::json(200, &json!({}))).await;
    let s = webhook_sandbox(&m, "wh-describe").await;
    let d = agent_cli::execute(
        &s.env,
        &s.apps,
        Verb::Describe {
            target: "webhook".into(),
            name: "send_ops_alerts".into(),
        },
    )
    .await;
    let t = text(&d);
    assert!(t.contains("notes: Deploy results go here."), "{t}");
    assert!(t.contains("account: B"), "{t}");
    assert!(
        t.contains("text (string, required)") && t.contains("card (json)"),
        "{t}"
    );
    assert!(
        t.contains("example: aurora-agent call demo send_ops_alerts text=<string>"),
        "{t}"
    );
    assert!(
        t.contains("dry run (sends nothing):") && t.contains("X-Aurora-Dry-Run=1"),
        "{t}"
    );
    for words in ["deploy", "card"] {
        let q = agent_cli::execute(
            &s.env,
            &s.apps,
            Verb::Search {
                words: vec![words.into()],
                target: None,
                limit: 10,
            },
        )
        .await;
        assert!(
            text(&q).contains("send_ops_alerts"),
            "{words}: {}",
            text(&q)
        );
    }
}

#[test]
fn agent_cli_is_the_wrapper_being_executable() {
    use std::os::unix::fs::PermissionsExt;
    let d = std::env::temp_dir().join(format!("aurora-agent-wrapper-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let w = d.join("aurora-agent");
    assert!(!agent_cli::agent_cli_available(&w));
    std::fs::write(&w, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&w, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!agent_cli::agent_cli_available(&w));
    std::fs::set_permissions(&w, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(agent_cli::agent_cli_available(&w));
    let _ = std::fs::remove_dir_all(&d);
}
