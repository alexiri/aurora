//! SEC-92 / D-356: the authentication matrix and the pre-auth limits.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aurora_terminator::limits::PreAuth;
use russh::keys::PrivateKeyWithHashAlg;
use russh::keys::ssh_key::certificate::CertType;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::common::*;

const DELAY: Duration = Duration::from_millis(150);

async fn timed_cert(
    r: &Rig,
    user: &str,
    cert: russh::keys::ssh_key::Certificate,
) -> (Option<Failure>, Duration) {
    let mut c = connect(r.ssh).await;
    let t = Instant::now();
    let res = c
        .authenticate_openssh_cert(user, r.user_key.clone(), cert)
        .await
        .unwrap();
    (failure(&res), t.elapsed())
}

/// Every refusal: the same `USERAUTH_FAILURE` (methods `publickey` only, no partial success),
/// after at least the fixed delay, and **no API call** for anything that fails before the API.
#[tokio::test(flavor = "multi_thread")]
async fn every_refusal_is_the_same_failure_after_the_same_delay_with_no_api_call() {
    let r = rig(RigOpts::default()).await;
    let agent = r.agent.to_string();
    let now = chrono::Utc::now().timestamp();
    let want = Failure {
        methods: vec!["publickey".to_owned()],
        partial: false,
    };
    let mut observed: Vec<(&str, Option<Failure>, Duration)> = Vec::new();

    // A plain key (the same key the certificate certifies) and the password / keyboard-interactive
    // methods never authenticate.
    {
        let mut c = connect(r.ssh).await;
        let t = Instant::now();
        let res = c
            .authenticate_publickey(&agent, PrivateKeyWithHashAlg::new(r.user_key.clone(), None))
            .await
            .unwrap();
        observed.push(("plain key", failure(&res), t.elapsed()));
    }
    {
        let mut c = connect(r.ssh).await;
        let t = Instant::now();
        let res = c.authenticate_password(&agent, "hunter2").await.unwrap();
        observed.push(("password", failure(&res), t.elapsed()));
    }
    {
        let mut c = connect(r.ssh).await;
        let t = Instant::now();
        let res = c
            .authenticate_keyboard_interactive_start(&agent, None)
            .await
            .unwrap();
        let f = match res {
            russh::client::KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => Some(Failure {
                methods: remaining_methods
                    .iter()
                    .map(|m| <&str>::from(m).to_owned())
                    .collect(),
                partial: partial_success,
            }),
            _ => None,
        };
        observed.push(("keyboard-interactive", f, t.elapsed()));
    }

    // Certificates that fail the law offline.
    let s = r.spec();
    let other = Uuid::new_v4().to_string();
    let variants: Vec<(&str, String, russh::keys::ssh_key::Certificate)> = vec![
        ("expired", agent.clone(), {
            let mut s = s.clone();
            s.valid_after = now - 900;
            s.valid_before = now - 300;
            r.cert(&s)
        }),
        ("not yet valid", agent.clone(), {
            let mut s = s.clone();
            s.valid_after = now + 300;
            s.valid_before = now + 900;
            r.cert(&s)
        }),
        ("wrong principal", agent.clone(), {
            let mut s = s.clone();
            s.principals = Some(vec![other.clone()]);
            r.cert(&s)
        }),
        ("username is not the principal", other.clone(), r.cert(&s)),
        // SEC-147: a certificate for this agent presented with another agent's handle.
        ("another agent's handle", "giskard".to_owned(), {
            let mut s = s.clone();
            s.principals = Some(vec![agent.clone(), "daneel".into()]);
            r.cert(&s)
        }),
        ("the handle alone", "daneel".to_owned(), {
            let mut s = s.clone();
            s.principals = Some(vec!["daneel".into()]);
            r.cert(&s)
        }),
        ("the handle first", "daneel".to_owned(), {
            let mut s = s.clone();
            s.principals = Some(vec!["daneel".into(), agent.clone()]);
            r.cert(&s)
        }),
        ("three principals", "daneel".to_owned(), {
            let mut s = s.clone();
            s.principals = Some(vec![agent.clone(), "daneel".into(), "giskard".into()]);
            r.cert(&s)
        }),
        (
            "first principal is not the key id's agent",
            "daneel".to_owned(),
            {
                let mut s = s.clone();
                s.principals = Some(vec![other.clone(), "daneel".into()]);
                r.cert(&s)
            },
        ),
        ("foreign CA", agent.clone(), {
            let mut s = s.clone();
            s.ca = key(0x77);
            r.cert(&s)
        }),
        ("host certificate", agent.clone(), {
            let mut s = s.clone();
            s.cert_type = CertType::Host;
            r.cert(&s)
        }),
        ("critical option", agent.clone(), {
            let mut s = s.clone();
            s.critical = true;
            r.cert(&s)
        }),
        ("malformed key id", agent.clone(), {
            let mut s = s.clone();
            s.key_id = Some("garbage".into());
            r.cert(&s)
        }),
        ("key id names another agent", agent.clone(), {
            let mut s = s.clone();
            s.key_id = Some(format!(
                "aurora:u={}:k={}:a={}:w={}",
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4()
            ));
            r.cert(&s)
        }),
    ];
    for (name, user, cert) in variants {
        let (f, d) = timed_cert(&r, &user, cert).await;
        observed.push((name, f, d));
    }
    assert_eq!(
        r.stub.count("sessions"),
        0,
        "the API saw a call for a certificate that fails the law, or for a plain key"
    );

    // A valid certificate the API refuses (denied, over a cap) or cannot answer: one call each,
    // the very same failure.
    for (name, mode) in [
        ("api denied", Open::Denied),
        ("api limit", Open::Limit),
        ("api error", Open::Error),
    ] {
        *r.stub.open.lock().unwrap() = mode;
        let (f, d) = timed_cert(&r, &agent, r.good()).await;
        observed.push((name, f, d));
    }
    assert_eq!(r.stub.count("sessions"), 3);

    for (name, f, d) in &observed {
        assert_eq!(f.as_ref(), Some(&want), "{name}: {f:?}");
        assert!(
            *d >= DELAY - Duration::from_millis(30),
            "{name} answered in {d:?}"
        );
        assert!(*d < Duration::from_secs(3), "{name} took {d:?}");
    }
    // The sign-in that is allowed still works afterwards (no state was poisoned).
    *r.stub.open.lock().unwrap() = Open::Ok;
    let (f, _) = timed_cert(&r, &agent, r.good()).await;
    assert_eq!(f, None);
    assert_eq!(r.stub.count("sessions"), 4);
}

/// SEC-147, D-445: a `[uuid, handle]` certificate logs in as the handle and as the id, and the API
/// is asked to open a session for the key id's agent each time, never for the typed name. A handle
/// that spells another agent's UUID still opens the certificate's agent.
#[tokio::test(flavor = "multi_thread")]
async fn the_handle_and_the_id_both_log_in_and_the_api_is_asked_for_the_certificates_agent() {
    let r = rig(RigOpts::default()).await;
    let agent = r.agent.to_string();
    let mut s = r.spec();
    s.principals = Some(vec![agent.clone(), "daneel".into()]);
    for user in ["daneel", agent.as_str()] {
        let (f, _) = timed_cert(&r, user, r.cert(&s)).await;
        assert_eq!(f, None, "{user} did not log in");
    }
    // Another agent's id as a handle (a handle may spell a UUID): typed, it opens this agent.
    let other = Uuid::new_v4();
    let spelled: String = format!("a{}", &other.to_string()[1..]);
    s.principals = Some(vec![agent.clone(), spelled.clone()]);
    let (f, _) = timed_cert(&r, &spelled, r.cert(&s)).await;
    assert_eq!(f, None);
    // A certificate minted before D-445 (`[uuid]`) still logs in with the id.
    let (f, _) = timed_cert(&r, &agent, r.good()).await;
    assert_eq!(f, None);

    let asked: Vec<String> = r
        .stub
        .sessions
        .lock()
        .unwrap()
        .iter()
        .map(|b| b["username"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        asked,
        vec![agent.clone(); 4],
        "the API was asked for another name"
    );
}

/// The `none` probe is answered (listing `publickey`) without counting; six real failures are
/// answered and the seventh attempt ends the connection (`SSH_AUTH_ATTEMPTS`).
#[tokio::test(flavor = "multi_thread")]
async fn the_none_probe_is_uncounted_and_the_seventh_attempt_ends_the_connection() {
    let r = rig(RigOpts::default()).await;
    let agent = r.agent.to_string();
    let mut c = connect(r.ssh).await;
    let probe = c.authenticate_none(&agent).await.unwrap();
    assert_eq!(
        failure(&probe),
        Some(Failure {
            methods: vec!["publickey".to_owned()],
            partial: false
        })
    );
    for i in 1..=6 {
        let res = c.authenticate_password(&agent, "x").await.unwrap();
        assert!(failure(&res).is_some(), "attempt {i}");
    }
    // The seventh attempt ends the connection (the client may see an error or a last failure).
    let _ = c.authenticate_password(&agent, "x").await;
    for _ in 0..100 {
        if c.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(c.is_closed(), "the seventh attempt must end the connection");
    assert_eq!(r.stub.count("sessions"), 0);
}

async fn read_until_closed(mut s: tokio::net::TcpStream, within: Duration) -> (Vec<u8>, bool) {
    let mut got = Vec::new();
    let end = Instant::now() + within;
    let mut buf = [0u8; 256];
    loop {
        let left = end.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, s.read(&mut buf)).await {
            Err(_) => return (got, false),
            Ok(Ok(0) | Err(_)) => return (got, true),
            Ok(Ok(n)) => got.extend_from_slice(&buf[..n]),
        }
    }
}

/// Over the per-address pre-auth cap and over the per-address bucket the socket is closed with
/// no message (no SSH identification is even sent).
#[tokio::test(flavor = "multi_thread")]
async fn preauth_caps_and_the_bucket_close_the_socket_without_a_message() {
    // Per-address cap 1.
    let r = rig(RigOpts {
        pre: PreAuth::new(100, 1, 6000, 100),
        ..RigOpts::default()
    })
    .await;
    let mut first = raw_tcp(r.ssh).await;
    let mut id = [0u8; 14];
    tokio::time::timeout(
        Duration::from_secs(3),
        tokio::io::AsyncReadExt::read_exact(&mut first, &mut id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        &id, b"SSH-2.0-aurora",
        "the identification string has no version number"
    );
    let (bytes, closed) = read_until_closed(raw_tcp(r.ssh).await, Duration::from_secs(3)).await;
    assert!(
        closed && bytes.is_empty(),
        "the second connection got {bytes:?}"
    );
    drop(first);

    // Bucket: burst 2, no refill.
    let r = rig(RigOpts {
        pre: PreAuth::new(100, 100, 1, 2),
        ..RigOpts::default()
    })
    .await;
    for _ in 0..2 {
        let mut s = raw_tcp(r.ssh).await;
        let mut id = [0u8; 14];
        tokio::io::AsyncReadExt::read_exact(&mut s, &mut id)
            .await
            .unwrap();
    }
    let (bytes, closed) = read_until_closed(raw_tcp(r.ssh).await, Duration::from_secs(3)).await;
    assert!(
        closed && bytes.is_empty(),
        "the third connection got {bytes:?}"
    );
}

struct HostKeyProbe(Arc<std::sync::Mutex<Option<String>>>);

impl russh::client::Handler for HostKeyProbe {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        k: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        if let russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } = k {
            *self.0.lock().unwrap() = key.to_openssh().ok();
        }
        Ok(true)
    }
}

/// SEC-96: the served host key is the configured one.
#[tokio::test(flavor = "multi_thread")]
async fn the_served_host_key_is_the_configured_one() {
    let r = rig(RigOpts::default()).await;
    let seen = Arc::new(std::sync::Mutex::new(None));
    let cfg = Arc::new(russh::client::Config::default());
    let _c = russh::client::connect(cfg, r.ssh, HostKeyProbe(seen.clone()))
        .await
        .unwrap();
    assert_eq!(
        seen.lock().unwrap().clone().unwrap(),
        key(0x33).public_key().to_openssh().unwrap()
    );
}

/// `ssh -Q`-style probe: a client that offers only algorithms outside the allowed list (CBC/CTR
/// ciphers, group-exchange or DH SHA-1 key exchange) cannot connect at all.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_allowed_algorithms_negotiate() {
    let r = rig(RigOpts::default()).await;
    for (what, preferred) in [
        (
            "aes-ctr only",
            russh::Preferred {
                cipher: std::borrow::Cow::Borrowed(&[russh::cipher::AES_256_CTR]),
                ..russh::Preferred::default()
            },
        ),
        (
            "aes-cbc only",
            russh::Preferred {
                cipher: std::borrow::Cow::Borrowed(&[russh::cipher::AES_256_CBC]),
                ..russh::Preferred::default()
            },
        ),
        (
            "dh sha1 only",
            russh::Preferred {
                kex: std::borrow::Cow::Borrowed(&[russh::kex::DH_G14_SHA1]),
                ..russh::Preferred::default()
            },
        ),
        (
            "dh sha256 only",
            russh::Preferred {
                kex: std::borrow::Cow::Borrowed(&[russh::kex::DH_G14_SHA256]),
                ..russh::Preferred::default()
            },
        ),
    ] {
        let cfg = Arc::new(russh::client::Config {
            preferred,
            ..russh::client::Config::default()
        });
        let res = russh::client::connect(cfg, r.ssh, Client).await;
        assert!(res.is_err(), "{what} must not negotiate");
    }
    // And the allowed ones do (the default client offers them).
    let _ok = connect(r.ssh).await;
}

/// A connection that does not authenticate within `SSH_AUTH_TIMEOUT` is closed.
#[tokio::test(flavor = "multi_thread")]
async fn the_auth_timeout_closes_an_idle_connection() {
    let mut o = RigOpts::default();
    o.settings.auth_timeout = Duration::from_millis(400);
    let r = rig(o).await;
    let started = Instant::now();
    let (_, closed) = read_until_closed(raw_tcp(r.ssh).await, Duration::from_secs(5)).await;
    assert!(closed, "still open after the timeout");
    assert!(started.elapsed() >= Duration::from_millis(350));
    let _ = Arc::strong_count(&r.shared);
}
