#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-CALDAV (D-273, D-275, SEC-107, SEC-109): the two link lanes end to end, black box. An agent in
//! a real sandbox reads `~/.aurora/links.json` and fetches a recorded ICS through the gateway's
//! secret-link lane (the real URL — its path and query canaries — is never in the sandbox, the
//! request log or the agent's output), and talks CalDAV to a person-pinned server over TLS
//! (`~/.aurora/servers.json`; the password is never in the sandbox; the gateway adds `Basic` only on
//! the pinned origin; the same host on another port, and a pinned origin over plain `http`, get no
//! credential). The stack runs with the transport rule strict (no loopback exemption), so the
//! TLS-only rule is exercised over the full proxy path. Gated by AURORA_TEST_PODMAN=1; run via
//! scripts/e2e/hosted.sh. The fake servers listen on `127.0.0.2` (in `gateway.allowed_private`):
//! the sandbox's `NO_PROXY` exempts `127.0.0.1`, so only another address goes through the gateway.
mod common;

use common::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ICS_PATH_CANARY: &str = "E2EICSPATHCANARY0x91";
const ICS_KEY_CANARY: &str = "E2EICSKEYCANARY0x92";
const PW_CANARY: &str = "E2EDAVPWCANARY0x93";
const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:e2e-1\r\nSUMMARY:Recorded standup\r\nDTSTART:20261201T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// Every request (head and body) a fake server saw.
type Seen = Arc<Mutex<Vec<String>>>;

struct Tls {
    ca_pem: String,
    acceptor: tokio_rustls::TlsAcceptor,
}

/// A CA and a leaf for `127.0.0.2` signed by it (the gateway and the API trust the CA through
/// `tls.extra_ca_bundle`).
fn tls() -> Tls {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
    let ca_params = || {
        let mut p = CertificateParams::new(vec![]).unwrap();
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.distinguished_name
            .push(DnType::CommonName, "e2e links ca");
        p
    };
    let ca_key = KeyPair::generate().unwrap();
    let ca = ca_params().self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca_params(), &ca_key);
    let key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["127.0.0.2".to_owned()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().clone()],
        rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
    )
    .unwrap();
    Tls {
        ca_pem: ca.pem(),
        acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(cfg)),
    }
}

/// `(status, extra header lines, body)` for a request `(method, path+query, head text, body)`.
type Handler = Arc<dyn Fn(&str, &str, &str, &str) -> (u16, String, String) + Send + Sync>;

/// A one-shot-per-connection HTTP server on `127.0.0.2` (TLS when `acceptor` is given).
async fn serve(acceptor: Option<tokio_rustls::TlsAcceptor>, h: Handler) -> (u16, Seen) {
    let l = tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else {
                return;
            };
            let (acceptor, seen, h) = (acceptor.clone(), seen2.clone(), h.clone());
            tokio::spawn(async move {
                match acceptor {
                    Some(a) => {
                        if let Ok(tls) = a.accept(tcp).await {
                            handle(tls, seen, h).await;
                        }
                    }
                    None => handle(tcp, seen, h).await,
                }
            });
        }
    });
    (port, seen)
}

async fn handle<S: AsyncReadExt + AsyncWriteExt + Unpin>(mut s: S, seen: Seen, h: Handler) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        match s.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let want: usize = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < head_end + want {
        match s.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let body = String::from_utf8_lossy(&buf[head_end..]).into_owned();
    let mut first = head.lines().next().unwrap_or("").split(' ');
    let (m, p) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    seen.lock().unwrap().push(format!("{head}{body}"));
    let (status, extra, out) = h(m, p, &head.to_ascii_lowercase(), &body);
    let resp = format!(
        "HTTP/1.1 {status} X\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{out}",
        out.len()
    );
    let _ = s.write_all(resp.as_bytes()).await;
    let _ = s.shutdown().await;
}

fn basic() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(format!("alice:{PW_CANARY}"))
}

/// A tiny CalDAV server: `alice` with the canary password; a calendar with one stored event.
fn dav() -> Handler {
    let store: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let want = format!("authorization: basic {}", basic().to_ascii_lowercase());
    Arc::new(move |m, p, head, body| {
        if !head.contains(&want) {
            return (
                401,
                "www-authenticate: Basic realm=\"e2e\"\r\n".into(),
                String::new(),
            );
        }
        match m {
            "PROPFIND" => (
                207,
                "content-type: application/xml\r\n".into(),
                "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/dav/calendars/alice/work/</d:href></d:response></d:multistatus>".into(),
            ),
            "PUT" => {
                let mut s = store.lock().unwrap();
                if head.contains("if-none-match: *") && s.contains_key(p) {
                    return (412, String::new(), String::new());
                }
                s.insert(p.to_owned(), body.to_owned());
                (201, "etag: \"v1\"\r\n".into(), String::new())
            }
            "GET" => match store.lock().unwrap().get(p) {
                Some(b) => (200, "content-type: text/calendar\r\n".into(), b.clone()),
                None => (404, String::new(), String::new()),
            },
            _ => (200, String::new(), String::new()),
        }
    })
}

fn ok_server() -> Handler {
    Arc::new(|_, _, _, _| (200, String::new(), "plain-ok".into()))
}

/// The registry snapshot of the two real packages, activated.
async fn install_packages(st: &Stack) {
    use aurora_registry::manifest::Manifest;
    use aurora_registry::snapshot::{ActivePackage, compile};
    let load = |yaml: &str| ActivePackage {
        manifest: Manifest::parse(yaml, "0.1.0").unwrap(),
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    let pk = [
        load(include_str!(
            "../../../plugins/builtin/ics-calendar/plugin.yaml"
        )),
        load(include_str!("../../../plugins/builtin/caldav/plugin.yaml")),
    ];
    let c = compile(&pk, &[], &[], 1).unwrap();
    let snapshot: Value = serde_json::from_slice(&c.json).unwrap();
    sqlx::query(
        "INSERT INTO registry_snapshots (id, contract, content_hash, snapshot, compiled_by, activated_at) \
         VALUES ($1, 1, $2, $3, 'e2e-links', now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(&c.hash)
    .bind(&snapshot)
    .execute(st.pool())
    .await
    .unwrap();
    sqlx::query("SELECT pg_notify('aurora_registry', '{}')")
        .execute(st.pool())
        .await
        .unwrap();
}

/// A sealed connection row, as the connect flow stores it.
async fn connection(
    st: &Stack,
    api: &Api,
    app: &str,
    label: &str,
    credentials: Value,
    metadata: Value,
) -> uuid::Uuid {
    use aurora_secrets::{Purpose, SealContext, SecretVault, StoreRegistry};
    let (org, ws): (uuid::Uuid, uuid::Uuid) = (api.org.parse().unwrap(), api.ws.parse().unwrap());
    let conn = uuid::Uuid::now_v7();
    let keys = {
        use base64::Engine;
        format!(
            "k1:{}",
            base64::engine::general_purpose::STANDARD.encode([b'a'; 32])
        )
    };
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&keys, "k1").unwrap());
    let vault = SecretVault::new(
        StoreRegistry::from_config(&aurora_config::SecretsSection::default(), keyring).unwrap(),
    );
    let sealed = vault
        .seal(
            &SealContext {
                purpose: Purpose::ConnectionCredentials,
                owner_id: conn,
                org_id: org,
                workspace_id: Some(ws),
            },
            aurora_core::Secret::from(serde_json::to_vec(&credentials).unwrap()),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, metadata) VALUES ($1,$2,$3,$4,$4,'1.0.0',$5,'active',$6,$7)")
        .bind(conn).bind(org).bind(ws).bind(app).bind(label).bind(sealed.to_column()).bind(metadata)
        .execute(st.pool()).await.unwrap();
    let (s, v) = api
        .put(
            &format!("/v1/agents/{}/grants/connections/{conn}", api.daneel),
            json!({"mode": "full"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    conn
}

fn tools_sock(container: &str, req: &Value) -> String {
    let script = "import socket,sys\ns=socket.socket(socket.AF_UNIX);s.settimeout(20)\ns.connect('/run/aurora/tools.sock');s.sendall((sys.argv[1]+'\\n').encode());print(s.makefile().readline())";
    podman_rc(&[
        "exec",
        "--user",
        "1000:1000",
        container,
        "python3",
        "-c",
        script,
        &req.to_string(),
    ])
    .1
}

/// Runs `command` as the agent would (the tools socket's `process_start`) and returns what it wrote.
async fn run(c: &str, conv: &str, name: &str, command: &str) -> String {
    let cmd = format!(
        "{{ {command}; }} > /workspace/{name}.out 2>&1; echo done-marker >> /workspace/{name}.out"
    );
    let r = tools_sock(
        c,
        &json!({"id": 1, "session": conv, "method": "call", "tool": "process_start",
                "arguments": {"name": name, "command": cmd}}),
    );
    assert!(r.contains("\"result\""), "process_start: {r}");
    let end = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let out = exec_in(c, &format!("cat /workspace/{name}.out 2>/dev/null")).1;
        if out.contains("done-marker") {
            return out;
        }
        assert!(
            std::time::Instant::now() < end,
            "{name} did not finish: {out}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn assert_sandbox_holds_none(c: &str, needles: &[&str]) {
    let (_, env) = podman_rc(&["inspect", "--format", "{{json .Config.Env}}", c]);
    for n in needles {
        assert!(!env.contains(n), "{n} is in the container environment");
        let (_, found) = exec_in(
            c,
            &format!(
                "grep -rIl --exclude-dir=proc --exclude-dir=sys -e '{n}' /workspace /run /tmp /etc /home 2>/dev/null; \
                 for p in /proc/[0-9]*; do cat $p/environ 2>/dev/null | tr '\\0' '\\n'; done | grep -c -e '{n}'"
            ),
        );
        assert!(
            found
                .lines()
                .all(|l| l.trim() == "0" || l.trim().is_empty()),
            "{n} is present in the sandbox: {found}"
        );
    }
}

async fn file_in(c: &str, path: &str, needle: &str) -> String {
    for _ in 0..120 {
        let t = exec_in(c, &format!("cat {path} 2>/dev/null")).1;
        if t.contains(needle) {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("{path} never contained {needle}");
}

#[tokio::test]
async fn s15_link_lanes_ics_and_caldav_hold_no_secret_in_the_sandbox() {
    if !enabled() {
        return;
    }
    let t = tls();
    let ca_file =
        std::env::temp_dir().join(format!("aurora-e2e-links-ca-{}.pem", uuid::Uuid::now_v7()));
    std::fs::write(&ca_file, &t.ca_pem).unwrap();
    Stack::wipe();
    let st = Stack::start_with(
        Opts {
            strict_transport: true,
            ..Opts::default()
        },
        &format!("\n[tls]\nextra_ca_bundle = \"{}\"\n", ca_file.display()),
    )
    .await;
    let api = st.signup("owner@e2e.test").await;
    install_packages(&st).await;

    // The recorded ICS (the real link: two canaries, one in the path and one in the query).
    let secret_path = format!("/ics/{ICS_PATH_CANARY}/basic.ics");
    let (ics_port, ics_seen) = {
        let sp = secret_path.clone();
        serve(
            Some(t.acceptor.clone()),
            Arc::new(move |_, p, _, _| {
                if p.starts_with(&sp) && p.contains(ICS_KEY_CANARY) {
                    (200, "content-type: text/calendar\r\n".into(), ICS.into())
                } else {
                    (404, String::new(), String::new())
                }
            }),
        )
        .await
    };
    let (dav_port, dav_seen) = serve(Some(t.acceptor.clone()), dav()).await;
    let (other_port, other_seen) = serve(Some(t.acceptor.clone()), ok_server()).await;
    let (plain_port, plain_seen) = serve(None, ok_server()).await;

    let ics_url = format!("https://127.0.0.2:{ics_port}{secret_path}?key={ICS_KEY_CANARY}");
    connection(
        &st,
        &api,
        "ics-calendar",
        "Team calendar",
        json!({"url": ics_url}),
        json!({"host": "127.0.0.2", "name": "Team calendar"}),
    )
    .await;
    let home = format!("https://127.0.0.2:{dav_port}/dav/calendars/alice/");
    connection(
        &st,
        &api,
        "caldav",
        "alice at 127.0.0.2",
        json!({"username": "alice", "password": PW_CANARY}),
        json!({"url": home, "hosts": [format!("127.0.0.2:{dav_port}")]}),
    )
    .await;
    // A second account whose stored origin is plain `http` (never produced by connect): even so,
    // the credential leaves only over TLS to the pinned port.
    connection(
        &st, &api, "caldav", "alice over http",
        json!({"username": "alice", "password": PW_CANARY}),
        json!({"url": format!("http://127.0.0.2:{plain_port}/dav/"), "hosts": [format!("127.0.0.2:{plain_port}")]}),
    )
    .await;
    api.set_policy("allow", json!([])).await;

    let conv = api.direct().await;
    let v = api.say(&conv, "[[say: warm]]").await;
    let (s, e) = st
        .wait_turn(
            v["turn"]["id"].as_str().unwrap(),
            &["done", "failed", "aborted"],
            120,
        )
        .await;
    assert_eq!((s.as_str(), e), ("done", None), "{}", st.log_tails());
    let cs = containers();
    assert_eq!(cs.len(), 1);
    let cn = cs[0].as_str();

    // ICS: the agent reads links.json (a synthetic address) and downloads through the lane.
    let links = file_in(cn, "/workspace/.aurora/links.json", "link.aurora.internal").await;
    let links: Value = serde_json::from_str(&links).unwrap();
    assert_eq!(links[0]["app"], "ics-calendar");
    let url = links[0]["url"].as_str().unwrap().to_owned();
    assert!(
        !url.contains("127.0.0.2") && !url.contains(ICS_PATH_CANARY),
        "{url}"
    );
    let out = run(
        cn, &conv, "ics",
        &format!("curl -sS -o /workspace/cal.ics -w 'status=%{{http_code}}\\n' '{url}'; head -c 200 /workspace/cal.ics"),
    )
    .await;
    assert!(
        out.contains("status=200") && out.contains("Recorded standup"),
        "{out}\n{}",
        st.log_tails()
    );
    {
        let seen = ics_seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].contains(&secret_path) && seen[0].contains(ICS_KEY_CANARY));
        assert!(
            !seen[0].to_ascii_lowercase().contains("authorization"),
            "{}",
            seen[0]
        );
    }
    assert!(!out.contains(ICS_PATH_CANARY) && !out.contains(ICS_KEY_CANARY));

    // CalDAV: servers.json names the server; the agent talks to it with no password.
    let servers = file_in(cn, "/workspace/.aurora/servers.json", &home).await;
    let servers: Value = serde_json::from_str(&servers).unwrap();
    assert_eq!(
        servers.as_array().unwrap().len(),
        1,
        "http origins are not listed: {servers}"
    );
    assert_eq!(servers[0]["app"], "caldav");
    assert_eq!(servers[0]["url"], home.as_str());
    let out = run(
        cn, &conv, "dav1",
        &format!(
            "curl -sS -X PROPFIND -H 'Depth: 1' -w '\\nstatus=%{{http_code}}\\n' '{home}'; \
             printf 'BEGIN:VCALENDAR\\r\\nBEGIN:VEVENT\\r\\nUID:n1\\r\\nSUMMARY:Created by the agent\\r\\nEND:VEVENT\\r\\nEND:VCALENDAR\\r\\n' > /workspace/n1.ics; \
             for i in 1 2; do curl -sS -X PUT -H 'If-None-Match: *' -H 'Content-Type: text/calendar' --data-binary @/workspace/n1.ics -o /dev/null -w \"put$i=%{{http_code}}\\n\" '{home}work/n1.ics'; done; \
             curl -sS '{home}work/n1.ics'"
        ),
    )
    .await;
    assert!(
        out.contains("status=207") && out.contains("/dav/calendars/alice/work/"),
        "{out}\n{}",
        st.log_tails()
    );
    assert!(
        out.contains("put1=201") && out.contains("put2=412"),
        "create, then a conflicting create: {out}"
    );
    assert!(out.contains("Created by the agent"), "{out}");
    let expect = format!("authorization: basic {}", basic().to_ascii_lowercase());
    {
        let seen = dav_seen.lock().unwrap();
        assert!(!seen.is_empty());
        assert!(
            seen.iter()
                .all(|r| r.to_ascii_lowercase().contains(&expect)),
            "every request to the pinned origin carries the gateway's Basic: {seen:?}"
        );
    }

    // Credential scope: the same host on another port, and a pinned origin over plain http.
    let out = run(
        cn, &conv, "scope",
        &format!(
            "curl -sS -X PROPFIND -w '\\nother=%{{http_code}}\\n' 'https://127.0.0.2:{other_port}/dav/'; \
             curl -sS -X PROPFIND -w '\\nplain=%{{http_code}}\\n' 'http://127.0.0.2:{plain_port}/dav/'"
        ),
    )
    .await;
    assert!(
        out.contains("other=200") && out.contains("plain=200"),
        "{out}\n{}",
        st.log_tails()
    );
    for (name, seen) in [("other port", &other_seen), ("plain http", &plain_seen)] {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{name}: {seen:?}");
        let r = seen[0].to_ascii_lowercase();
        assert!(
            !r.contains("authorization") && !r.contains(&basic().to_ascii_lowercase()),
            "{name} got a credential: {r}"
        );
    }

    // Nothing secret anywhere an agent (or the operator's log) can see.
    assert_sandbox_holds_none(cn, &[ICS_PATH_CANARY, ICS_KEY_CANARY, PW_CANARY, &basic()]);
    let rows: Vec<String> = sqlx::query_scalar("SELECT row_to_json(r)::text FROM request_log r")
        .fetch_all(st.pool())
        .await
        .unwrap();
    let all = rows.join("\n");
    assert!(!rows.is_empty());
    for n in [ICS_PATH_CANARY, ICS_KEY_CANARY, PW_CANARY, basic().as_str()] {
        assert!(!all.contains(n), "{n} is in the request log");
    }
    let events = api.events(&conv).await;
    let text = transcript_text(&events);
    for n in [ICS_PATH_CANARY, ICS_KEY_CANARY, PW_CANARY] {
        assert!(!text.contains(n), "{n} is in the transcript");
    }
    let logs = st.log_tails();
    for n in [ICS_PATH_CANARY, ICS_KEY_CANARY, PW_CANARY] {
        assert!(!logs.contains(n), "{n} is in a service log");
    }
    let _ = std::fs::remove_file(ca_file);
    st.finish().await;
}
