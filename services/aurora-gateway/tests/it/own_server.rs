//! (phase 5, P5-CALDAV, D-275, SEC-109, SEC-59) A person-pinned server through the whole pipeline:
//! the connection's own credential is injected only on an origin (`host:port`) its stored `hosts`
//! name, never where a static claim, a registered instance or another connection owns the origin,
//! and a read-only connection passes only the read tools.
//!
//! A child module of `apps` (it reuses that file's pipeline harness). The harness's loopback
//! exemption of the TLS rule is on, so plain `http` to `localhost` is injected here; the transport
//! rule itself is a unit test in `aurora-registry` and the TLS-only behaviour is covered by the
//! hosted end-to-end.
use super::*;
use aurora_gateway::invalidation::Invalidatable;

const PASSWORD_CANARY: &str = "CALDAVPWCANARY0x7F";
const CREDS: &str = r#"{"username":"alice","password":"CALDAVPWCANARY0x7F"}"#;

fn own_app(id: &str) -> Value {
    json!({
        "app_id": id, "package_id": "pkg", "about": {"id": id, "name": id},
        "credentials": [{"name": "username", "secret": false, "optional": false},
                        {"name": "password", "secret": true, "optional": false}],
        "metadata": [{"name": "url"}, {"name": "hosts"}], "connect": [], "token": null,
        "own_server": {
            "url_field": "url", "hosts_field": "hosts",
            "inject": [{"basic": {"user": "{credentials.username}",
                                  "password": "{credentials.password}"}}]
        },
        "tools": {"read": {"items": [
            {"id": "discover", "name": "d", "host": "{connection}", "path": "/**", "methods": ["PROPFIND"]},
            {"id": "query", "name": "q", "host": "{connection}", "path": "/**", "methods": ["REPORT", "GET"]}
        ]}, "write": {"items": [
            {"id": "put_event", "name": "p", "host": "{connection}", "path": "/**", "methods": ["PUT"]}
        ]}},
        "approvals": {}, "agent": null, "database": null
    })
}

fn own_entry(id: &str) -> Value {
    json!({
        "host": "{connection}", "path_prefix": null, "app_id": id, "claim_status": "active",
        "tenant": null, "rewrite_host": null, "finalize": null, "transform": null,
        "intercept": null, "passthrough": [],
        "inject": [{"basic": {"user": "{credentials.username}",
                              "password": "{credentials.password}"}}]
    })
}

fn own_snapshot(extra_apps: Vec<Value>, extra_hosts: Vec<Value>) -> Value {
    let mut apps = vec![own_app("cal")];
    apps.extend(extra_apps);
    let mut s = snapshot(apps, extra_hosts);
    s["own_server_hosts"] = json!([own_entry("cal")]);
    s
}

fn pinned(w: &World, store_path: &str, hosts: &[String]) -> AppConnectionRow {
    let mut c = conn(w, "cal", store_path, None);
    c.metadata = json!({"url": "https://cal.example/dav/", "hosts": hosts});
    c
}

fn basic(user: &str) -> String {
    format!(
        "authorization: Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{PASSWORD_CANARY}"))
    )
}

/// The upstream echoes the whole request; the gateway withholds an answer holding the password or
/// the Basic value, so the test asks for it hex-encoded (`x-echo-hex`) and decodes it here.
async fn call(env: &Env, method: &str, host: &str, path: &str, extra: &str) -> Reply {
    let mut r = call_plain(
        env,
        method,
        host,
        path,
        &format!("x-echo-hex: 1\r\n{extra}"),
    )
    .await;
    if r.body.len() % 2 == 0 && !r.body.is_empty() && r.body.bytes().all(|b| b.is_ascii_hexdigit())
    {
        let bytes: Vec<u8> = (0..r.body.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&r.body[i..i + 2], 16).unwrap())
            .collect();
        r.body = String::from_utf8_lossy(&bytes).into_owned();
    }
    r
}

/// As [`call`], but the upstream echoes in the clear (what a server reflecting the request does).
async fn call_plain(env: &Env, method: &str, host: &str, path: &str, extra: &str) -> Reply {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
    let req = format!(
        "{method} http://{host}:{port}{path} HTTP/1.1\r\nHost: {host}:{port}\r\nProxy-Authorization: Basic {auth}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n",
        port = env.up.port()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
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

/// Nothing of the credential, in any form, reached the upstream or came back.
fn uncredentialed(r: &Reply) {
    let all = format!("{}\n{}", r.head, r.body);
    assert!(!all.contains(PASSWORD_CANARY), "{all}");
    assert!(
        !all.to_ascii_lowercase().contains("authorization: basic"),
        "{all}"
    );
}

/// Starts the gateway; `rows` is built once the upstream's port is known (a pinned origin names
/// it) from the world and the pinned origin `localhost:<port>`.
async fn start_pinned(snap: Value, rows: impl FnOnce(&World, &str) -> Rows) -> (Env, Arc<Src>) {
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    let (env, src, _) = start_in(
        w,
        snap,
        &[("c1", CREDS), ("c2", CREDS)],
        |_| Rows::default(),
        None,
        None,
        None,
        false,
    )
    .await;
    let origin = format!("localhost:{}", env.up.port());
    *src.0.lock().unwrap() = rows(&w, &origin);
    env.resolve_cache.flush_all();
    (env, src)
}

fn grant(c: &AppConnectionRow) -> Rows {
    let mut r = Rows::default();
    r.ws.rules.push(allow_conn(c, None));
    r.conns.push(c.clone());
    r
}

#[tokio::test]
async fn the_credential_is_injected_on_the_pinned_origin_only() {
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, origin| {
        grant(&pinned(w, "c1", &[origin.to_owned()]))
    })
    .await;
    // The pinned origin: Basic with the person's own username and password.
    let r = call(&env, "PROPFIND", "localhost", "/dav/", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains(&basic("alice")), "{}", r.body);
    // The same server by another name (another origin): nothing is injected.
    let r = call(&env, "PROPFIND", "127.0.0.1", "/dav/", "").await;
    uncredentialed(&r);
}

/// CalDAV answers name the account (`/dav/calendars/alice/…` hrefs) and the agent's own URL holds
/// it too: the username is not an "injected value echoed from the URL" (it was found by the hosted
/// end-to-end: every PROPFIND of a real server came back 502 `upstream_echoed_credential`).
#[tokio::test]
async fn an_answer_naming_the_account_is_not_an_echo() {
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, origin| {
        grant(&pinned(w, "c1", &[origin.to_owned()]))
    })
    .await;
    // The upstream echoes the path (the username is in it) but no header, in the clear.
    let r = call_plain(
        &env,
        "PROPFIND",
        "localhost",
        "/dav/calendars/alice/work/",
        "x-echo-no-headers: 1\r\n",
    )
    .await;
    assert_eq!(r.status, 200, "{} {}", r.head, r.body);
    uncredentialed(&r);
    assert!(r.body.contains("/dav/calendars/alice/work/"), "{}", r.body);
    assert!(!r.body.contains("upstream_echoed_credential"), "{}", r.body);
}

/// (SEC-109, D-275) A server reflecting the `Authorization` header (the base64 of `user:password`)
/// is withheld: 502 `upstream_echoed_credential`, and neither the password nor the Basic value is
/// in anything the agent gets.
#[tokio::test]
async fn a_server_reflecting_the_authorization_header_is_withheld() {
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, origin| {
        grant(&pinned(w, "c1", &[origin.to_owned()]))
    })
    .await;
    let r = call_plain(&env, "PROPFIND", "localhost", "/dav/calendars/alice/", "").await;
    assert_eq!(r.status, 502, "{} {}", r.head, r.body);
    assert!(r.body.contains("upstream_echoed_credential"), "{}", r.body);
    uncredentialed(&r);
    let b64 = base64::engine::general_purpose::STANDARD.encode(format!("alice:{PASSWORD_CANARY}"));
    assert!(!format!("{}\n{}", r.head, r.body).contains(&b64));
}

#[tokio::test]
async fn another_port_on_the_pinned_host_gets_nothing() {
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, _| {
        // Pinned `localhost:1`; the request goes to `localhost:<upstream port>`.
        grant(&pinned(w, "c1", &["localhost:1".to_owned()]))
    })
    .await;
    let r = call(&env, "PROPFIND", "localhost", "/dav/", "").await;
    uncredentialed(&r);
}

#[tokio::test]
async fn the_host_match_is_case_insensitive_and_the_list_may_hold_several_origins() {
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, origin| {
        grant(&pinned(
            w,
            "c1",
            &["other.example:443".to_owned(), origin.to_ascii_uppercase()],
        ))
    })
    .await;
    let r = call(&env, "PROPFIND", "localhost", "/dav/", "").await;
    assert!(r.body.contains(&basic("alice")), "{}", r.body);
}

#[tokio::test]
async fn a_static_claim_on_the_pinned_host_wins_and_nothing_is_injected() {
    // A package claims `localhost` statically after the connection was made (a snapshot change):
    // the stored host equals the claimed one, the connection still gets nothing (SEC-59).
    let mut claim = host_json("other", None, json!({}));
    claim["host"] = json!("localhost");
    let (env, _) = start_pinned(
        own_snapshot(vec![app_json("other", json!(null), false)], vec![claim]),
        |w, origin| grant(&pinned(w, "c1", &[origin.to_owned()])),
    )
    .await;
    let r = call(&env, "PROPFIND", "localhost", "/dav/", "").await;
    uncredentialed(&r);
}

#[tokio::test]
async fn a_registered_instance_host_never_receives_a_pinned_credential() {
    let inst = Uuid::now_v7();
    let mut s = inst_snapshot(vec![own_app("cal")], vec![]);
    s["own_server_hosts"] = json!([own_entry("cal")]);
    let (env, _) = start_pinned(s, move |w, origin| {
        let mut r = grant(&pinned(w, "c1", &[origin.to_owned()]));
        r.instances.push(instance_row(inst, "localhost"));
        r
    })
    .await;
    let r = call(&env, "PROPFIND", "localhost", "/dav/", "").await;
    uncredentialed(&r);
}

#[tokio::test]
async fn a_reserved_host_never_receives_a_pinned_credential() {
    let mut s = own_snapshot(vec![], vec![]);
    s["reserved_hosts"] = json!(["localhost"]);
    let (env, _) = start_pinned(s, |w, origin| grant(&pinned(w, "c1", &[origin.to_owned()]))).await;
    let r = call(&env, "PROPFIND", "localhost", "/dav/", "").await;
    uncredentialed(&r);
}

#[tokio::test]
async fn without_a_grant_or_with_a_disabled_connection_nothing_is_injected() {
    // No grant rule: the connection is no candidate.
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, origin| {
        let c = pinned(w, "c1", &[origin.to_owned()]);
        let mut r = Rows::default();
        r.conns.push(c);
        r
    })
    .await;
    uncredentialed(&call(&env, "PROPFIND", "localhost", "/dav/", "").await);
    // Disabled: no candidate either.
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, origin| {
        let mut c = pinned(w, "c1", &[origin.to_owned()]);
        c.status = "disabled".into();
        grant(&c)
    })
    .await;
    uncredentialed(&call(&env, "PROPFIND", "localhost", "/dav/", "").await);
}

#[tokio::test]
async fn a_connection_without_hosts_metadata_is_never_injected() {
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, _| {
        let mut c = pinned(w, "c1", &[]);
        c.metadata = json!({"url": "https://cal.example/dav/"});
        grant(&c)
    })
    .await;
    uncredentialed(&call(&env, "PROPFIND", "localhost", "/dav/", "").await);
}

#[tokio::test]
async fn read_only_connection_passes_the_read_tools_and_refuses_the_writes() {
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, origin| {
        let mut c = pinned(w, "c1", &[origin.to_owned()]);
        c.access = "read".into();
        grant(&c)
    })
    .await;
    for m in ["PROPFIND", "REPORT"] {
        let r = call(&env, m, "localhost", "/dav/", "").await;
        assert_eq!(r.status, 200, "{m}: {}", r.body);
        assert!(r.body.contains(&basic("alice")), "{m}: {}", r.body);
    }
    let r = call(&env, "PUT", "localhost", "/dav/cal/e.ics", "").await;
    assert_ne!(r.status, 200, "{}", r.body);
    uncredentialed(&r);
    // MKCALENDAR matches no tool at all: refused (untooled) on a read-only connection.
    let r = call(&env, "MKCALENDAR", "localhost", "/dav/new/", "").await;
    assert_ne!(r.status, 200, "{}", r.body);
    uncredentialed(&r);
}

#[tokio::test]
async fn a_write_connection_passes_put() {
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, origin| {
        grant(&pinned(w, "c1", &[origin.to_owned()]))
    })
    .await;
    let r = call(&env, "PUT", "localhost", "/dav/cal/e.ics", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains(&basic("alice")), "{}", r.body);
}

#[tokio::test]
async fn two_connections_on_one_origin_follow_the_usual_choice_rule() {
    let (env, _) = start_pinned(own_snapshot(vec![], vec![]), |w, origin| {
        let a = pinned(w, "c1", &[origin.to_owned()]);
        let b = pinned(w, "c2", &[origin.to_owned()]);
        let mut r = grant(&a);
        r.ws.rules.push(allow_conn(&b, None));
        r.conns.push(b);
        r
    })
    .await;
    let r = call(&env, "PROPFIND", "localhost", "/dav/", "").await;
    assert_eq!(r.status, 409, "{}", r.body);
    assert_eq!(r.code(), "multiple_connections");
    uncredentialed(&r);
}

#[tokio::test]
async fn a_credential_that_is_not_the_agents_grant_is_not_injected_on_another_apps_origin() {
    // Another own-server app pins the same origin: ambiguous between apps, fails closed.
    let mut other = own_app("cal2");
    other["about"] = json!({"id": "cal2", "name": "cal2"});
    let mut s = own_snapshot(vec![other], vec![]);
    s["own_server_hosts"] = json!([own_entry("cal"), own_entry("cal2")]);
    let (env, _) = start_pinned(s, |w, origin| {
        let a = pinned(w, "c1", &[origin.to_owned()]);
        let mut b = pinned(w, "c2", &[origin.to_owned()]);
        b.app_id = "cal2".into();
        let mut r = grant(&a);
        r.ws.rules.push(allow_conn(&b, None));
        r.conns.push(b);
        r
    })
    .await;
    uncredentialed(&call(&env, "PROPFIND", "localhost", "/dav/", "").await);
}
