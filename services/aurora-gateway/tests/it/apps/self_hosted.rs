//! (FIX-290, D-511) An instance administrator's self-hosted app host is reachable on a private
//! address by that app's own granted requests, and by nothing else.
//!
//! The upstream is a TLS server on one of this machine's own private interface addresses. The
//! guard's test-only static names answer `wiki.corp.test` with it: requests are for port 443, as
//! in a running gateway, and only the TCP connection goes to the test server's port.

use super::*;
use aurora_core::ranges::{AppHost, RefusedRanges};
use aurora_gateway::guard::Guard;
use aurora_gateway::invalidation::Invalidatable;
use std::net::IpAddr;

const HOST: &str = "wiki.corp.test";

/// A private (RFC 1918) address of this machine: a developer's LAN or a CI runner's `eth0`.
fn private_ip() -> IpAddr {
    aurora_gateway::guard::local_addresses()
        .into_iter()
        .find(|ip| matches!(ip, IpAddr::V4(v4) if v4.is_private()))
        .expect("this test needs a network interface with a private IPv4 address")
}

/// A TLS server for [`HOST`] on `ip`: where it listens and the CA that signed its certificate.
async fn tls_upstream(ip: IpAddr) -> (SocketAddr, String) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
    let key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec![HOST.to_owned()])
        .unwrap()
        .signed_by(&key, &Issuer::from_params(&ca, &ca_key))
        .unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let l = TcpListener::bind((ip, 0)).await.unwrap();
    let at = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(s).await else {
                    return;
                };
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(tls), service_fn(upstream))
                    .await;
            });
        }
    });
    (at, ca_pem)
}

struct Setup {
    env: Env,
    src: Arc<Src>,
    /// The test server.
    at: SocketAddr,
    /// The instance administrator's registration of `inst` at `/wiki`.
    admin: Uuid,
}

impl Setup {
    fn guard(&self) -> &Arc<Guard> {
        &self.env.guard
    }

    fn app(&self) -> AppHost {
        AppHost::new(HOST, 443).unwrap()
    }

    fn flush(&self) {
        self.env
            .resolve_cache
            .invalidate(&aurora_gateway::invalidation::Invalidate {
                scope: aurora_gateway::invalidation::Scope::All,
            });
    }

    /// Point [`HOST`] at `ip`, both as asked and fully qualified.
    fn resolve_to(&self, ip: IpAddr) {
        let at = vec![SocketAddr::new(ip, self.at.port())];
        self.guard().set_static_names(vec![
            (HOST.to_owned(), at.clone()),
            (format!("{HOST}."), at),
        ]);
    }

    /// `GET path` on the registered host through the MITM lane.
    async fn get(&self, path: &str) -> Reply {
        tunnel_get(&self.env, 443, path).await
    }
}

/// CONNECT to [`HOST`]`:port` as `user`; the stream and the proxy's answer head.
async fn open_tunnel(
    env: &Env,
    user: &str,
    token: &str,
    port: u16,
) -> (
    tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    String,
) {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{token}"));
    let target = format!("{HOST}:{port}");
    s.write_all(
        format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
        )
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
    (s, String::from_utf8_lossy(&head).into_owned())
}

async fn tunnel_get(env: &Env, port: u16, path: &str) -> Reply {
    let (s, head) = open_tunnel(env, "agent", TOKEN, port).await;
    assert!(head.contains(" 200"), "{head}");
    let cfg = crate::common::client_config(&crate::common::trust_pem(env.proxy), &[b"http/1.1"]);
    let mut t = tokio_rustls::TlsConnector::from(cfg)
        .connect(
            rustls::pki_types::ServerName::try_from(HOST.to_owned()).unwrap(),
            s,
        )
        .await
        .unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {HOST}:{port}\r\nConnection: close\r\n\r\n");
    t.write_all(req.as_bytes()).await.unwrap();
    t.flush().await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), t.read_to_end(&mut out)).await;
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

/// Two apps on one host: `inst` (the administrator's registration, `/wiki`) and `other` (an
/// organization's, `/other`), each with a granted connection. A network rule also allows the
/// whole host, so that a request without any connection is refused by the guard alone.
async fn setup() -> Setup {
    let (at, ca_pem) = tls_upstream(private_ip()).await;
    let mut other_app = app_json("other", json!(null), false);
    other_app["instances"] = json!({"label": "Other"});
    let mut snap = inst_snapshot(vec![other_app], vec![]);
    let mut other_entry = host_json(
        "other",
        None,
        json!({"inject": [{"bearer": "{credentials.tok}"}]}),
    );
    other_entry["host"] = json!("{instance}");
    snap["instance_hosts"]
        .as_array_mut()
        .unwrap()
        .push(other_entry);
    let (admin, org) = (Uuid::now_v7(), Uuid::now_v7());
    let (env, src, _) = start_with_ca(
        snap,
        &[
            ("c1", r#"{"tok":"ADMINAPP"}"#),
            ("c2", r#"{"tok":"OTHERAPP"}"#),
        ],
        move |w| {
            let mine = bound(w, "c1", Some(admin));
            let mut theirs = conn(w, "other", "c2", None);
            theirs.instance_id = Some(org);
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&mine, None));
            r.ws.rules.push(allow_conn(&theirs, None));
            r.ws.rules.push(rule(
                "net",
                "allow",
                json!([{"type": "network", "host": HOST, "path": "/**"}]),
                false,
            ));
            r.conns.push(mine);
            r.conns.push(theirs);
            let mut a = instance_row_at(admin, "inst", HOST, Some("/wiki"));
            a.admin_registered = true;
            r.instances.push(a);
            r.instances
                .push(instance_row_at(org, "other", HOST, Some("/other")));
            r
        },
        Some(ca_pem),
    )
    .await;
    // The upstream listens on one of this host's own interfaces, which in a cluster is the
    // gateway's pod address and never an app's.
    env.guard.set_local_addresses(Vec::new());
    // The shared test guard opens loopback; a running gateway's does not.
    env.guard.set_allowed_private(&[]).unwrap();
    let s = Setup {
        env,
        src,
        at,
        admin,
    };
    s.resolve_to(at.ip());
    s
}

const MINE: &str = "/wiki/api/v4/user";
const THEIRS: &str = "/other/api/v4/user";

#[track_caller]
fn assert_reached(r: &Reply) {
    assert_eq!(r.status, 200, "{}\n{}", r.head, r.body);
    assert!(has_token(r, "ADMINAPP"), "{}", r.body);
}

#[track_caller]
fn assert_refused(r: &Reply) {
    assert_eq!(
        (r.status, r.code().as_str()),
        (403, "destination_refused"),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn the_apps_own_request_reaches_its_private_host_and_no_other_request_does() {
    let s = setup().await;
    assert_reached(&s.get(MINE).await);
    // After a successful request (its upstream connection is pooled): another app's granted
    // request to the same host and port is judged afresh and refused.
    assert_refused(&s.get(THEIRS).await);
    // A request that no registration serves: no app, no allowance.
    assert_refused(&s.get("/elsewhere").await);
    // And the app still works.
    assert_reached(&s.get(MINE).await);
}

#[tokio::test]
async fn a_request_without_a_granted_connection_gets_no_allowance() {
    let s = setup().await;
    assert_reached(&s.get(MINE).await);
    // The same app and registration, but no connection is granted: policy still allows the
    // host, so what refuses it is the guard.
    s.src.0.lock().unwrap().ws.rules.remove(0);
    s.flush();
    assert_refused(&s.get(MINE).await);
}

#[tokio::test]
async fn the_open_lane_gets_no_allowance() {
    let s = setup().await;
    assert_reached(&s.get(MINE).await);
    let (_, head) = open_tunnel(&s.env, "open", BROWSER, 443).await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
}

/// An organization's registration and a package preset (the resolver marks neither as an
/// instance administrator's), and a registration that is removed or disabled (no longer among
/// the active ones).
#[tokio::test]
async fn only_an_active_admin_registration_opens_the_host_and_changes_apply_at_once() {
    let s = setup().await;
    assert_reached(&s.get(MINE).await);
    let set_admin = |on: bool| {
        let mut rows = s.src.0.lock().unwrap();
        for i in &mut rows.instances {
            if i.id == s.admin {
                i.admin_registered = on;
            }
        }
    };
    set_admin(false);
    s.flush();
    assert_refused(&s.get(MINE).await);
    set_admin(true);
    s.flush();
    assert_reached(&s.get(MINE).await);
    s.src
        .0
        .lock()
        .unwrap()
        .instances
        .retain(|i| i.id != s.admin);
    s.flush();
    let r = s.get(MINE).await;
    assert_refused(&r);
    assert!(!r.body.contains("ADMINAPP"), "{}", r.body);
}

#[tokio::test]
async fn a_name_resolving_to_loopback_link_local_or_metadata_is_refused() {
    let s = setup().await;
    assert_reached(&s.get(MINE).await);
    for bad in [
        "127.0.0.1",
        "::1",
        "0.0.0.0",
        "169.254.169.254",
        "169.254.1.1",
        "fe80::1",
        "fd00:ec2::254",
        "fd20:ce::254",
        "100.100.100.200",
        "64:ff9b::a00:1",
    ] {
        s.resolve_to(bad.parse().unwrap());
        assert!(s.guard().resolve_app(&s.app()).await.is_err(), "{bad}");
        assert!(
            s.guard().judge_app_name(&s.app()).await.is_err(),
            "{bad} (egress proxy)"
        );
    }
    // Through the proxy too (a fresh gateway, so that no pooled connection answers).
    let s = setup().await;
    s.resolve_to("127.0.0.1".parse().unwrap());
    assert_refused(&s.get(MINE).await);
}

#[tokio::test]
async fn auroras_own_services_are_never_opened() {
    let s = setup().await;
    let ip = s.at.ip();
    // Another of Aurora's services, named by address, lives where the app's name resolves to.
    s.guard().set_own_services(vec![ip.to_string()]);
    assert_refused(&s.get(MINE).await);
    // The same, named by host name.
    s.guard().set_own_services(vec!["db.corp.test".to_owned()]);
    s.guard().set_static_names(vec![
        (HOST.to_owned(), vec![s.at]),
        (format!("{HOST}."), vec![s.at]),
        ("db.corp.test".to_owned(), vec![s.at]),
    ]);
    assert_refused(&s.get(MINE).await);
    // The gateway's own interface address, on any port.
    s.guard().set_own_services(Vec::new());
    s.guard().set_local_addresses(vec![ip]);
    assert_refused(&s.get(MINE).await);
    s.guard().set_local_addresses(Vec::new());
    assert_reached(&s.get(MINE).await);
}

/// The operator's refused ranges (a cluster's pod and Service CIDRs) close an address that the
/// allowance would otherwise open. (The ranges are read at start-up, so each gateway here gets
/// them before its first request: no pooled connection predates them.)
#[tokio::test]
async fn an_allowed_host_that_resolves_into_a_refused_range_is_refused() {
    let s = setup().await;
    let own = format!("{}/32", s.at.ip());
    s.guard()
        .set_refused_ranges(RefusedRanges::parse(&[own]).unwrap());
    assert!(s.guard().resolve_app(&s.app()).await.is_err());
    assert!(s.guard().judge_app_name(&s.app()).await.is_err());
    assert_refused(&s.get(MINE).await);
    // A range elsewhere closes nothing here.
    let s = setup().await;
    s.guard()
        .set_refused_ranges(RefusedRanges::parse(&["203.0.113.0/24".to_owned()]).unwrap());
    assert_reached(&s.get(MINE).await);
}

/// A short name that only the cluster's DNS search domains complete (`service.namespace`) is not
/// the registered name: the fully qualified name must give the same address.
#[tokio::test]
async fn a_name_that_resolves_only_through_a_search_domain_is_refused() {
    let s = setup().await;
    s.guard().set_static_names(vec![
        (HOST.to_owned(), vec![s.at]),
        (format!("{HOST}."), Vec::new()),
    ]);
    assert!(s.guard().resolve_app(&s.app()).await.is_err());
    assert_refused(&s.get(MINE).await);
    s.resolve_to(s.at.ip());
    assert_reached(&s.get(MINE).await);
}

#[tokio::test]
async fn only_port_443_of_the_registered_host_is_opened() {
    let s = setup().await;
    assert!(s.guard().resolve_app(&s.app()).await.is_ok());
    // The same address without the allowance stays refused.
    assert!(s.guard().resolve(HOST, 443).await.is_err());
    assert_reached(&s.get(MINE).await);
    // The same app and connection on another port: nothing is opened there.
    assert_refused(&tunnel_get(&s.env, 8443, MINE).await);
}

/// Which stored registrations are an instance administrator's: instance-wide and not a
/// package's preset; only active ones are served at all.
#[tokio::test]
async fn the_stored_registrations_that_count_are_the_instance_wide_ones_that_are_not_presets() {
    let db = aurora_testkit::TestDb::new().await;
    let pool = db.pool();
    let org = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext')",
    )
    .bind(org)
    .execute(pool)
    .await
    .unwrap();
    for (host, org, builtin, status) in [
        ("admin.corp.test", None, false, "active"),
        ("preset.corp.test", None, true, "active"),
        ("team.corp.test", Some(org), false, "active"),
        ("off.corp.test", None, false, "disabled"),
    ] {
        sqlx::query(
            "INSERT INTO app_instances (id, org_id, package_id, app_id, host, label, builtin, status) \
             VALUES ($1,$2,'p','wiki',$3,'l',$4,$5)",
        )
        .bind(Uuid::now_v7())
        .bind(org)
        .bind(host)
        .bind(builtin)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    }
    let rows = aurora_db::repo::gateway::visible_instances(pool, org)
        .await
        .unwrap();
    let got: Vec<(&str, bool)> = rows
        .iter()
        .map(|r| (r.host.as_str(), r.admin_registered))
        .collect();
    assert_eq!(
        got,
        [
            ("admin.corp.test", true),
            ("preset.corp.test", false),
            ("team.corp.test", false),
        ]
    );
}
