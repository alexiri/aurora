//! Loopback PostgreSQL relay (`docs/12-supervisor-harness.md` §Loopback proxies, D-89).
//!
//! `psql service=<name>` connects here, on 127.0.0.1 only. The relay opens TLS to the gateway's
//! proxy address, verified against the boot CA with the connection's own name
//! `<connection_id>.db.aurora.internal` (the gateway presents a CA leaf for it), forwards the
//! client's `StartupMessage` (user = connection id) and answers the gateway's cleartext password
//! request **itself** with the credential the HTTP forwarders use (boot token, or the turn context
//! credential while exactly one turn is active). The client never sees an authentication request,
//! and no file or environment variable of the sandbox holds a gateway credential. From
//! `AuthenticationOk` on the bytes are relayed untouched. The loopback leg is plaintext, the same
//! documented exception as the HTTP forwarders; the client's `SSLRequest`/`GSSENCRequest` are
//! answered `N`. A `CancelRequest` is forwarded over TLS too. If TLS verification fails, no
//! credential is sent.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

use crate::proxy::{Credential, GatewayUpstream, ProxyHandle};

/// Fixed loopback port of the relay (the HTTP forwarders use 3128 and 3129).
pub const RELAY_PORT: u16 = 3130;

const SSL_REQUEST: u32 = 80_877_103;
const GSSENC_REQUEST: u32 = 80_877_104;
const CANCEL_REQUEST: u32 = 80_877_102;
/// Largest pre-authentication packet either way.
const MAX_PACKET: usize = 16 * 1024;
const STEP: Duration = Duration::from_secs(30);
const HOST_SUFFIX: &str = ".db.aurora.internal";

/// The relay's credential: the boot token, overridden by the turn context credential while
/// exactly one turn is active (with several concurrent turns the process cannot be attributed to
/// one of them, so the boot token is used).
pub struct DbTurns {
    cred: Arc<Credential>,
    active: Mutex<Vec<(Uuid, String)>>,
}

// (FIX-114, SEC-01) The per-turn context credentials are secrets: never printed.
impl std::fmt::Debug for DbTurns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbTurns")
            .field("cred", &self.cred)
            .field("active", &"[redacted]")
            .finish()
    }
}

impl DbTurns {
    pub fn new(boot_token: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            cred: Credential::new("x", boot_token),
            active: Mutex::new(Vec::new()),
        })
    }

    fn refresh(&self, g: &[(Uuid, String)]) {
        self.cred.set_context(match g {
            [(_, c)] => Some(c.clone()),
            _ => None,
        });
    }

    pub fn begin(&self, turn: Uuid, credential: String) {
        if let Ok(mut g) = self.active.lock() {
            g.retain(|(t, _)| *t != turn);
            g.push((turn, credential));
            self.refresh(&g);
        }
    }

    pub fn end(&self, turn: Uuid) {
        if let Ok(mut g) = self.active.lock() {
            g.retain(|(t, _)| *t != turn);
            self.refresh(&g);
        }
    }
}

/// Starts the relay on `bind`; dropping the handle stops accepting (open sessions continue).
pub async fn spawn_db_relay(
    bind: SocketAddr,
    upstream: GatewayUpstream,
    turns: Arc<DbTurns>,
) -> std::io::Result<ProxyHandle> {
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let upstream = Arc::new(upstream);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let (upstream, turns) = (upstream.clone(), turns.clone());
            tokio::spawn(async move {
                if let Err(e) = handle(stream, &upstream, &turns).await {
                    tracing::debug!(error = %e, "database relay connection ended");
                }
            });
        }
    });
    Ok(ProxyHandle::new(addr, task))
}

/// One initial packet: `(code or version, whole body after the length)`.
async fn read_initial<R: AsyncRead + Unpin>(s: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await?;
    let n = u32::from_be_bytes(len) as usize;
    if !(8..=MAX_PACKET).contains(&n) {
        return Err(std::io::Error::other("bad startup packet length"));
    }
    let mut body = vec![0u8; n - 4];
    s.read_exact(&mut body).await?;
    Ok(body)
}

fn code_of(body: &[u8]) -> u32 {
    body.get(..4)
        .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// The `user` parameter of a protocol-3 `StartupMessage` body.
fn startup_user(body: &[u8]) -> Option<String> {
    let mut parts = body.get(4..)?.split(|b| *b == 0);
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if k.is_empty() {
            break;
        }
        if k == b"user" {
            return String::from_utf8(v.to_vec()).ok();
        }
    }
    None
}

fn with_len(body: &[u8]) -> Vec<u8> {
    let mut m = u32::try_from(body.len() + 4)
        .unwrap_or(u32::MAX)
        .to_be_bytes()
        .to_vec();
    m.extend_from_slice(body);
    m
}

/// A FATAL `ErrorResponse` for the client.
async fn fatal<W: AsyncWrite + Unpin>(w: &mut W, code: &str, msg: &str) {
    let mut f = Vec::new();
    for (k, v) in [(b'S', "FATAL"), (b'V', "FATAL"), (b'C', code), (b'M', msg)] {
        f.push(k);
        f.extend_from_slice(v.as_bytes());
        f.push(0);
    }
    f.push(0);
    let mut m = vec![b'E'];
    m.extend_from_slice(&u32::try_from(f.len() + 4).unwrap_or(0).to_be_bytes());
    m.extend_from_slice(&f);
    let _ = w.write_all(&m).await;
}

/// One backend message: `(type, payload)`.
async fn read_frame<R: AsyncRead + Unpin>(s: &mut R) -> std::io::Result<(u8, Vec<u8>)> {
    let mut h = [0u8; 5];
    s.read_exact(&mut h).await?;
    let n = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
    if !(4..=MAX_PACKET).contains(&n) {
        return Err(std::io::Error::other("bad backend message length"));
    }
    let mut p = vec![0u8; n - 4];
    s.read_exact(&mut p).await?;
    Ok((h[0], p))
}

async fn handle(
    mut client: TcpStream,
    upstream: &GatewayUpstream,
    turns: &DbTurns,
) -> std::io::Result<()> {
    for _ in 0..3 {
        let body = tokio::time::timeout(STEP, read_initial(&mut client))
            .await
            .map_err(|_| std::io::Error::other("startup timeout"))??;
        match code_of(&body) {
            // The loopback leg is plaintext by design; decline both.
            SSL_REQUEST | GSSENC_REQUEST => client.write_all(b"N").await?,
            CANCEL_REQUEST => return cancel(&body, upstream).await,
            v if v >> 16 == 3 => return login(client, &body, upstream, turns).await,
            _ => {
                fatal(&mut client, "0A000", "unsupported frontend protocol").await;
                return Ok(());
            }
        }
    }
    Ok(())
}

/// `CancelRequest` forwarded over verified TLS (the gateway maps its own keys).
async fn cancel(body: &[u8], upstream: &GatewayUpstream) -> std::io::Result<()> {
    let mut up = tokio::time::timeout(STEP, upstream.connect_db(None))
        .await
        .map_err(|_| std::io::Error::other("gateway timeout"))??;
    up.write_all(&with_len(body)).await?;
    let mut b = [0u8; 1];
    let _ = tokio::time::timeout(Duration::from_secs(2), up.read(&mut b)).await;
    Ok(())
}

async fn login(
    mut client: TcpStream,
    startup: &[u8],
    upstream: &GatewayUpstream,
    turns: &DbTurns,
) -> std::io::Result<()> {
    let Some(user) = startup_user(startup).filter(|u| Uuid::parse_str(u).is_ok()) else {
        fatal(
            &mut client,
            "28000",
            "the user must be the database connection id (psql service=<name>)",
        )
        .await;
        return Ok(());
    };
    let name = format!("{}{HOST_SUFFIX}", user.to_ascii_lowercase());
    let mut up = match tokio::time::timeout(STEP, upstream.connect_db(Some(&name))).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "gateway TLS or connect failed for the database relay; no credential sent");
            fatal(
                &mut client,
                "08006",
                "The sandbox could not verify or reach the Aurora gateway; no credential was sent.",
            )
            .await;
            return Ok(());
        }
        Err(_) => {
            fatal(
                &mut client,
                "08006",
                "The Aurora gateway did not answer in time.",
            )
            .await;
            return Ok(());
        }
    };
    up.write_all(&with_len(startup)).await?;
    let mut sent = false;
    loop {
        let (t, p) = tokio::time::timeout(STEP, read_frame(&mut up))
            .await
            .map_err(|_| std::io::Error::other("authentication timeout"))??;
        match (t, code_of(&p)) {
            // AuthenticationCleartextPassword: answered here, never passed to the client.
            (b'R', 3) if !sent => {
                sent = true;
                let secret = turns.cred.secret();
                let mut m = vec![b'p'];
                m.extend_from_slice(&u32::try_from(secret.len() + 5).unwrap_or(0).to_be_bytes());
                m.extend_from_slice(secret.as_bytes());
                m.push(0);
                up.write_all(&m).await?;
            }
            (b'R', 0) => {
                let mut m = vec![b'R'];
                m.extend_from_slice(&u32::try_from(p.len() + 4).unwrap_or(0).to_be_bytes());
                m.extend_from_slice(&p);
                client.write_all(&m).await?;
                break;
            }
            (b'E', _) => {
                let mut m = vec![b'E'];
                m.extend_from_slice(&u32::try_from(p.len() + 4).unwrap_or(0).to_be_bytes());
                m.extend_from_slice(&p);
                client.write_all(&m).await?;
                return Ok(());
            }
            _ => {
                fatal(
                    &mut client,
                    "08P01",
                    "unexpected authentication exchange with the gateway",
                )
                .await;
                return Ok(());
            }
        }
    }
    tokio::io::copy_bidirectional(&mut client, &mut up).await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn startup_user_is_parsed() {
        let mut b = 196_608u32.to_be_bytes().to_vec();
        b.extend_from_slice(b"database\0x\0user\0abc\0\0");
        assert_eq!(startup_user(&b).as_deref(), Some("abc"));
        assert_eq!(startup_user(&196_608u32.to_be_bytes()), None);
    }

    #[test]
    fn exactly_one_active_turn_sets_the_context() {
        let t = DbTurns::new("aub_boot");
        assert_eq!(t.cred.secret(), "aub_boot");
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        t.begin(a, "auc_a".into());
        assert_eq!(t.cred.secret(), "auc_a");
        t.begin(b, "auc_b".into());
        assert_eq!(t.cred.secret(), "aub_boot");
        t.end(a);
        assert_eq!(t.cred.secret(), "auc_b");
        t.end(b);
        assert_eq!(t.cred.secret(), "aub_boot");
    }
}
