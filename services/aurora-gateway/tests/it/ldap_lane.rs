//! (phase 5, P5-CERN-ADDONS, D-251) The gateway LDAP lane through the whole pipeline: a package's
//! tool request on `<app>.ldap.aurora.internal` is classified, granted and logged like HTTP, then
//! answered by one LDAP search against a fake directory (LDAPS, private CA) — and nothing else
//! can reach the directory: the fake records every operation it is sent.
//!
//! A child module of `apps` (it reuses that file's pipeline harness).
use super::*;
use aurora_gateway::ca::Ca;
use aurora_gateway::ldap::ber::{self, Tlv};
use tokio_rustls::TlsAcceptor;

const HOST: &str = "phonebook.ldap.aurora.internal";

#[derive(Clone)]
enum Script {
    /// Entries as `(dn, [(attribute, raw values)])`, then a done with `code`.
    Entries(Vec<(String, Vec<(String, Vec<Vec<u8>>)>)>, i64),
    /// The bind is refused with this code and a diagnostic of the server's own.
    BindRefused(i64),
    /// The search ends with this code and a diagnostic of the server's own.
    SearchRefused(i64),
}

#[derive(Default)]
struct Seen {
    connections: usize,
    /// The tag of every operation received, in order.
    ops: Vec<u8>,
    searches: Vec<Search>,
}

#[derive(Debug, Clone)]
struct Search {
    base: String,
    scope: i64,
    deref: i64,
    size_limit: i64,
    filter: Vec<u8>,
    attributes: Vec<String>,
}

const DIAG: &str = "SERVER-DIAGNOSTIC-TEXT";

fn msg(id: i64, op: Vec<u8>) -> Vec<u8> {
    ber::tlv(
        ber::SEQUENCE,
        &ber::concat(&[ber::int(ber::INTEGER, id), op]),
    )
}

fn result(tag: u8, code: i64) -> Vec<u8> {
    ber::tlv(
        tag,
        &ber::concat(&[
            ber::int(ber::ENUMERATED, code),
            ber::octets(b""),
            ber::octets(DIAG.as_bytes()),
        ]),
    )
}

async fn read_msg<R: AsyncReadExt + Unpin>(r: &mut R) -> Option<Vec<u8>> {
    let mut head = [0u8; 2];
    r.read_exact(&mut head).await.ok()?;
    let mut buf = head.to_vec();
    let len = if head[1] < 0x80 {
        usize::from(head[1])
    } else {
        let n = usize::from(head[1] & 0x7f);
        let mut lb = vec![0u8; n];
        r.read_exact(&mut lb).await.ok()?;
        buf.extend_from_slice(&lb);
        lb.iter().fold(0usize, |a, b| (a << 8) | usize::from(*b))
    };
    let at = buf.len();
    buf.resize(at + len, 0);
    r.read_exact(&mut buf[at..]).await.ok()?;
    Some(buf)
}

fn op_of(m: &[u8]) -> (i64, Tlv<'_>) {
    let (outer, _) = ber::read(m).unwrap();
    let (idt, rest) = ber::read(outer.content).unwrap();
    let (op, _) = ber::read(rest).unwrap();
    (ber::as_int(&idt).unwrap(), op)
}

fn parse_search(op: &Tlv<'_>) -> Search {
    let c = ber::children(op.content).unwrap();
    let text = |t: &Tlv<'_>| String::from_utf8(t.content.to_vec()).unwrap();
    // The filter is kept as its raw TLV: tag + length + content.
    let filter = {
        let mut rest = op.content;
        for _ in 0..6 {
            rest = ber::read(rest).unwrap().1;
        }
        let after = ber::read(rest).unwrap().1;
        rest[..rest.len() - after.len()].to_vec()
    };
    Search {
        base: text(&c[0]),
        scope: ber::as_int(&c[1]).unwrap(),
        deref: ber::as_int(&c[2]).unwrap(),
        size_limit: ber::as_int(&c[3]).unwrap(),
        filter,
        attributes: ber::children(c[7].content)
            .unwrap()
            .iter()
            .map(text)
            .collect(),
    }
}

/// A fake LDAPS directory; returns its port, its CA certificate and what it saw.
async fn directory(script: Script) -> (u16, String, Arc<Mutex<Seen>>) {
    let gen_ca = ca::generate("directory-private-ca").unwrap();
    let ca = Ca::from_pem(&gen_ca.cert_pem, &gen_ca.key_pem).unwrap();
    let cfg = ca.server_config("localhost").unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s2 = seen.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = l.accept().await {
            let (cfg, script, seen) = (cfg.clone(), script.clone(), s2.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = TlsAcceptor::from(cfg).accept(tcp).await else {
                    return;
                };
                seen.lock().unwrap().connections += 1;
                while let Some(m) = read_msg(&mut tls).await {
                    let (id, op) = op_of(&m);
                    seen.lock().unwrap().ops.push(op.tag);
                    match op.tag {
                        ber::OP_BIND_REQUEST => {
                            let code = if let Script::BindRefused(c) = &script {
                                *c
                            } else {
                                0
                            };
                            tls.write_all(&msg(id, result(ber::OP_BIND_RESPONSE, code)))
                                .await
                                .unwrap();
                        }
                        ber::OP_SEARCH_REQUEST => {
                            seen.lock().unwrap().searches.push(parse_search(&op));
                            match &script {
                                Script::Entries(entries, code) => {
                                    for (dn, attrs) in entries {
                                        let attrs: Vec<Vec<u8>> = attrs
                                            .iter()
                                            .map(|(n, vals)| {
                                                let vals: Vec<Vec<u8>> =
                                                    vals.iter().map(|v| ber::octets(v)).collect();
                                                ber::tlv(
                                                    ber::SEQUENCE,
                                                    &ber::concat(&[
                                                        ber::octets(n.as_bytes()),
                                                        ber::tlv(ber::SET, &ber::concat(&vals)),
                                                    ]),
                                                )
                                            })
                                            .collect();
                                        let e = ber::tlv(
                                            ber::OP_SEARCH_ENTRY,
                                            &ber::concat(&[
                                                ber::octets(dn.as_bytes()),
                                                ber::tlv(ber::SEQUENCE, &ber::concat(&attrs)),
                                            ]),
                                        );
                                        tls.write_all(&msg(id, e)).await.unwrap();
                                    }
                                    // A referral the lane must not follow.
                                    let r = ber::tlv(
                                        ber::OP_SEARCH_REFERENCE,
                                        &ber::octets(b"ldaps://elsewhere.invalid/"),
                                    );
                                    tls.write_all(&msg(id, r)).await.unwrap();
                                    tls.write_all(&msg(id, result(ber::OP_SEARCH_DONE, *code)))
                                        .await
                                        .unwrap();
                                }
                                Script::SearchRefused(c) => {
                                    tls.write_all(&msg(id, result(ber::OP_SEARCH_DONE, *c)))
                                        .await
                                        .unwrap();
                                }
                                Script::BindRefused(_) => {}
                            }
                        }
                        _ => return,
                    }
                }
            });
        }
    });
    (port, gen_ca.cert_pem, seen)
}

fn person(uid: &str) -> (String, Vec<(String, Vec<Vec<u8>>)>) {
    (
        format!("CN={uid},OU=Users,DC=cern,DC=ch"),
        vec![
            ("uid".into(), vec![uid.as_bytes().to_vec()]),
            (
                "mail".into(),
                vec![format!("{uid}@cern.ch").into_bytes(), vec![0xff, 0xfe]],
            ),
            (
                "displayName".into(),
                vec![format!("{uid} Doe").into_bytes()],
            ),
            // Not on the app's list: never leaves the lane.
            ("userPassword".into(), vec![b"hunter2".to_vec()]),
        ],
    )
}

fn ldap_snapshot(port: u16) -> Value {
    let mut app = json!({
        "app_id": "phonebook", "package_id": "pkg", "about": {"id": "phonebook", "name": "Phonebook"},
        "credentials": [], "metadata": [], "connect": [], "token": null,
        "tools": {"read": {"items": [
            {"id": "search", "name": "Search", "host": HOST, "path": "/search", "methods": ["POST"]}
        ]}},
        "approvals": {}, "agent": null, "database": null,
        "ldap": {"server": "localhost", "port": port, "base_dn": "DC=cern,DC=ch",
                 "attributes": ["uid", "mail", "displayName"], "max_entries": 2}
    });
    app["grants"] = json!({});
    snapshot(
        vec![app],
        vec![host_json("phonebook", None, json!({"host": HOST}))],
    )
}

async fn lane(script: Script, trust_directory: bool) -> (Env, Arc<Mutex<Seen>>) {
    let (port, ca_pem, seen) = directory(script).await;
    let (env, ..) = start_with_ca(
        ldap_snapshot(port),
        &[("c1", "{}")],
        |w| {
            let mut c = conn(w, "phonebook", "c1", None);
            c.workspace_id = Some(w.ws);
            granted(c)(w)
        },
        trust_directory.then_some(ca_pem),
    )
    .await;
    (env, seen)
}

async fn post(env: &Env, method: &str, path: &str, body: &str) -> Reply {
    // Like an agent: CONNECT to the synthetic host, TLS with the gateway's leaf inside the tunnel
    // (a credential-free app is still only served over TLS to the claimed port).
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
    let target = format!("{HOST}:443");
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
    assert!(
        String::from_utf8_lossy(&head).contains(" 200"),
        "{}",
        String::from_utf8_lossy(&head)
    );
    let cfg = crate::common::client_config(&crate::common::trust_pem(env.proxy), &[b"http/1.1"]);
    let mut t = tokio_rustls::TlsConnector::from(cfg)
        .connect(
            rustls::pki_types::ServerName::try_from(HOST.to_owned()).unwrap(),
            s,
        )
        .await
        .unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {HOST}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    t.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), t.read_to_end(&mut out)).await;
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

#[tokio::test]
async fn a_search_is_translated_and_only_a_search_reaches_the_directory() {
    let (env, seen) = lane(Script::Entries(vec![person("jdoe")], 0), true).await;
    let r = post(
        &env,
        "POST",
        "/search",
        r#"{"filter":"(uid=jdoe)","attributes":["MAIL","uid"]}"#,
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["truncated"], false);
    let e = &v["entries"][0];
    assert_eq!(e["dn"], "CN=jdoe,OU=Users,DC=cern,DC=ch");
    // The listed attributes only, spelled as the manifest spells them; the value that is not
    // UTF-8 is dropped; the attribute the app does not list never appears.
    assert_eq!(e["attributes"]["mail"], json!(["jdoe@cern.ch"]));
    assert_eq!(e["attributes"]["uid"], json!(["jdoe"]));
    // Only what was asked for: the directory sent displayName too, the request did not name it.
    assert!(e["attributes"].get("displayName").is_none(), "{e}");
    assert!(
        !r.body.contains("hunter2") && !r.body.contains("userPassword"),
        "{}",
        r.body
    );

    // The unbind follows the answer: wait for it with a deadline.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while seen.lock().unwrap().ops.len() < 3 && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    let s = seen.lock().unwrap();
    assert_eq!(s.connections, 1);
    // Bind, search, unbind — no other operation exists on the lane.
    assert_eq!(
        s.ops,
        vec![
            ber::OP_BIND_REQUEST,
            ber::OP_SEARCH_REQUEST,
            ber::OP_UNBIND_REQUEST
        ]
    );
    let q = &s.searches[0];
    assert_eq!(q.base, "DC=cern,DC=ch");
    assert_eq!(
        (q.scope, q.deref, q.size_limit),
        (2, 0, 3),
        "subtree, never dereference, cap + 1"
    );
    assert_eq!(q.attributes, vec!["mail", "uid"]);
    assert_eq!(q.filter[0], 0xa3, "an equality match");
}

#[tokio::test]
async fn more_entries_than_the_cap_are_cut_and_flagged() {
    let people = vec![person("a"), person("b"), person("c")];
    let (env, _) = lane(Script::Entries(people, 4), true).await;
    let r = post(
        &env,
        "POST",
        "/search",
        r#"{"filter":"(uid=*)","limit":10}"#,
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(
        v["entries"].as_array().unwrap().len(),
        2,
        "max_entries is 2"
    );
    assert_eq!(v["truncated"], true);
}

#[tokio::test]
async fn bad_requests_are_refused_before_the_directory_is_touched() {
    let (env, seen) = lane(Script::Entries(vec![person("jdoe")], 0), true).await;
    for (body, why) in [
        (
            r#"{"filter":"(userPassword=*)"}"#,
            "an attribute the app does not list in a filter",
        ),
        (
            r#"{"filter":"(uid=a)","attributes":["userPassword"]}"#,
            "an unlisted attribute asked for",
        ),
        (
            r#"{"filter":"(uid=a)","base":"DC=ch"}"#,
            "a base above the app's",
        ),
        (
            r#"{"filter":"(uid=a)","base":"OU=x\\,DC=cern,DC=ch"}"#,
            "an escaped comma",
        ),
        (r#"{"filter":"(:dn:=x)"}"#, "extensible match"),
        (
            r#"{"filter":"(uid=a)","add":{"uid":"x"}}"#,
            "a field that would be a write",
        ),
        (
            r#"{"filter":"(uid=a)","scope":"x"}"#,
            "a scope that does not exist",
        ),
        ("not json", "not JSON"),
    ] {
        let r = post(&env, "POST", "/search", body).await;
        assert_eq!(
            (r.status, r.code().as_str()),
            (400, "ldap_request_invalid"),
            "{why}: {}",
            r.body
        );
    }
    let big = format!(r#"{{"filter":"(uid=a)","base":"{}"}}"#, "x".repeat(20_000));
    assert_eq!(post(&env, "POST", "/search", &big).await.status, 413);
    // Other operations on the synthetic host are not tools: nothing is classified, nothing is sent.
    for (m, p) in [
        ("GET", "/search"),
        ("POST", "/modify"),
        ("DELETE", "/search"),
    ] {
        let r = post(&env, m, p, "{}").await;
        assert_ne!(r.status, 200, "{m} {p}: {}", r.body);
    }
    assert_eq!(
        seen.lock().unwrap().connections,
        0,
        "the directory saw no connection"
    );
}

#[tokio::test]
async fn the_directory_certificate_is_verified() {
    // The gateway does not trust the directory's private CA: the handshake fails, no bind is sent.
    let (env, seen) = lane(Script::Entries(vec![person("jdoe")], 0), false).await;
    let r = post(&env, "POST", "/search", r#"{"filter":"(uid=jdoe)"}"#).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (502, "ldap_unavailable"),
        "{}",
        r.body
    );
    assert!(seen.lock().unwrap().ops.is_empty());
}

#[tokio::test]
async fn directory_refusals_are_answered_without_the_servers_own_text() {
    for script in [Script::BindRefused(49), Script::SearchRefused(50)] {
        let (env, _) = lane(script, true).await;
        let r = post(&env, "POST", "/search", r#"{"filter":"(uid=jdoe)"}"#).await;
        assert_eq!(
            (r.status, r.code().as_str()),
            (502, "ldap_refused"),
            "{}",
            r.body
        );
        assert!(!r.body.contains(DIAG), "{}", r.body);
    }
}

#[tokio::test]
async fn an_ungranted_agent_cannot_search() {
    let (port, ca_pem, seen) = directory(Script::Entries(vec![person("jdoe")], 0)).await;
    let (env, ..) = start_with_ca(
        ldap_snapshot(port),
        &[("c1", "{}")],
        |w| {
            let mut r = Rows::default();
            r.ws.default_action = "block".into();
            let mut c = conn(w, "phonebook", "c1", None);
            c.workspace_id = Some(w.ws);
            r.conns.push(c);
            r
        },
        Some(ca_pem),
    )
    .await;
    let r = post(&env, "POST", "/search", r#"{"filter":"(uid=jdoe)"}"#).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (403, "ldap_no_connection"),
        "{}",
        r.body
    );
    assert_eq!(seen.lock().unwrap().connections, 0);
}
