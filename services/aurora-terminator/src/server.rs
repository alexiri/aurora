//! The SSH server (`16 §W20`, SEC-92, SEC-95, D-356): certificate-only, hint-free authentication,
//! pre-auth limits, the channel and request allow-list, and the bridge from each accepted channel
//! to a leg-1 WebSocket. Everything about the sandbox is decided by the API and the runner; this
//! file never names a sandbox, a boot or a runner, holds no signing key and logs no bytes,
//! commands or certificates.
//!
//! **Authentication order (D-356).** `russh` calls the *query* callback with the certificate's
//! subject key and an algorithm that is `ssh-ed25519` for a plain key and a certificate alike, so
//! the query phase answers OK to any ssh-ed25519 offer (every holder gets the same answer) and
//! refuses other algorithms. After the client's signature a plain key is refused; for a
//! certificate the whole law ([`crate::cert::check`]) runs before any API call, and only then is
//! the API asked to open a session. Every refusal is the same `USERAUTH_FAILURE` listing
//! `publickey` only, after the same delay, with no banner.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use aurora_core::limits::{
    SSH_AUTH_ATTEMPTS, SSH_CHANNEL_POLL_MAX, SSH_ENV_VARS_MAX, SSH_EXEC_COMMAND_MAX,
};
use aurora_protocol::common::SecretString;
use aurora_protocol::terminator::{ChannelMode, SessionOpenRequest, SessionRefusal};
use bytes::Bytes;
use russh::keys::ssh_key::{Algorithm, Certificate, PrivateKey};
use russh::server::{Auth, Config, Handler, Msg, Session, VerifiedPublicKey};
use russh::{
    Channel, ChannelId, ChannelOpenFailure, Disconnect, MethodKind, MethodSet, Preferred, Pty, Sig,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

use crate::cert::{self, Refusal};
use crate::leg::{self, ChannelSink, LegEnd, LegOpen, ToLeg};
use crate::limits::{Admission, Refused};
use crate::state::{Sess, Shared};

/// Per-channel queue from the SSH side to the leg task (stdin before the leg is up waits here).
const CHAN_QUEUE: usize = 64;
/// The size a `pty-req` of 0×0 ("unspecified", RFC 4254 §6.2: a client whose stdin is not a terminal) gets.
const DEFAULT_COLS: u32 = 80;
const DEFAULT_ROWS: u32 = 24;
/// The terminal type of a `pty-req` with an empty `TERM` (the supervisor's fallback for an unusable one).
const DEFAULT_TERM: &str = "xterm-256color";
/// The SSH channel window: small on purpose (bulk data is paced by the runner's credit through
/// the leg's backpressure, not by a large window).
const WINDOW: u32 = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ConnError {
    #[error(transparent)]
    Russh(#[from] russh::Error),
    /// A policy ended the connection (too many attempts).
    #[error("connection ended by policy")]
    Policy,
}

const KEX: &[russh::kex::Name] = &[
    russh::kex::CURVE25519,
    russh::kex::CURVE25519_PRE_RFC_8731,
    russh::kex::EXTENSION_SUPPORT_AS_SERVER,
    russh::kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
];
const CIPHERS: &[russh::cipher::Name] = &[
    russh::cipher::CHACHA20_POLY1305,
    russh::cipher::AES_256_GCM,
    russh::cipher::AES_128_GCM,
];
const MACS: &[russh::mac::Name] = &[russh::mac::NONE];
const COMPRESSION: &[russh::compression::Name] = &[russh::compression::NONE];
const HOST_KEY_ALGS: &[Algorithm] = &[Algorithm::Ed25519];

/// The transport configuration: SSH-2 only, ed25519 host key, curve25519 key exchange, AEAD
/// ciphers, no compression, `publickey` as the only method, a fixed delay on every failure and
/// our own attempt counter (russh's backstop is one more, for the `none` probe it also counts).
pub fn ssh_config(host_key: PrivateKey, s: &crate::state::Settings) -> Config {
    Config {
        server_id: russh::SshId::Standard(Cow::Borrowed("SSH-2.0-aurora")),
        methods: MethodSet::from(&[MethodKind::PublicKey][..]),
        auth_rejection_time: s.auth_failure_delay,
        auth_rejection_time_initial: Some(s.auth_failure_delay),
        keys: vec![host_key],
        max_auth_attempts: SSH_AUTH_ATTEMPTS + 1,
        inactivity_timeout: None,
        // No SSH keepalive of our own (D-434): a client's reply to it would count as activity
        // for every live client; a dead peer is found by TCP keepalive (`set_tcp_keepalive`).
        keepalive_interval: None,
        keepalive_max: 0,
        window_size: WINDOW,
        maximum_packet_size: 32 * 1024,
        preferred: Preferred {
            kex: Cow::Borrowed(KEX),
            key: Cow::Borrowed(HOST_KEY_ALGS),
            cipher: Cow::Borrowed(CIPHERS),
            mac: Cow::Borrowed(MACS),
            compression: Cow::Borrowed(COMPRESSION),
            ..Preferred::default()
        },
        ..Config::default()
    }
}

/// What the accept loop and the handler share about one connection.
struct Ctl {
    authed: AtomicBool,
    session: OnceLock<Arc<Sess>>,
    /// Open SSH channels (the watcher writes goodbyes to them and waits for them to close).
    open: Mutex<HashSet<ChannelId>>,
}

/// (FIX-199, D-434) The connection's socket, watched for client traffic. russh 0.65.2 never
/// shows its handler a client's `keepalive@openssh.com` (global or channel request: it answers
/// them itself), so any byte read from the client restarts the session's idle clock instead:
/// keepalives, window adjusts and every other packet count, exactly like channel data. Only
/// reads count (never what the terminator writes), and the clock is touched at most once a
/// second ([`crate::state::Stats::touch_client`]). Before authentication there is no session
/// and nothing is counted.
struct ClientActivity<S> {
    inner: S,
    ctl: Arc<Ctl>,
}

impl<S: AsyncRead + Unpin> AsyncRead for ClientActivity<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(r, Poll::Ready(Ok(())))
            && buf.filled().len() > before
            && let Some(sess) = self.ctl.session.get()
        {
            sess.stats.touch_client();
        }
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ClientActivity<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// TCP keepalive of an accepted socket (D-434): the terminator sends no SSH keepalive, so a
/// dead peer is found here: first probe after 30 s of silence, then every 10 s, three
/// unanswered probes close it (about 60 s), instead of the kernel's two hours.
pub const TCP_KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
pub const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
pub const TCP_KEEPALIVE_PROBES: u32 = 3;

pub fn set_tcp_keepalive(stream: &tokio::net::TcpStream) -> std::io::Result<()> {
    socket2::SockRef::from(stream).set_tcp_keepalive(
        &socket2::TcpKeepalive::new()
            .with_time(TCP_KEEPALIVE_IDLE)
            .with_interval(TCP_KEEPALIVE_INTERVAL)
            .with_retries(TCP_KEEPALIVE_PROBES),
    )
}

struct Chan {
    pty: Option<(String, u16, u16)>,
    env: Vec<(String, String)>,
    started: bool,
    tx: Option<mpsc::Sender<ToLeg>>,
}

struct Conn {
    shared: Arc<Shared>,
    ctl: Arc<Ctl>,
    peer: Option<IpAddr>,
    admission: Option<Admission>,
    attempts: usize,
    sess: Option<Arc<Sess>>,
    permit_pty: bool,
    handle: Option<russh::server::Handle>,
    chans: HashMap<ChannelId, Chan>,
}

fn refused_metric(kind: &'static str) {
    metrics::counter!("aurora_terminator_channel_refused_total", "kind" => kind).increment(1);
}

impl Conn {
    /// One counted refusal: the metric (reason never shown to the client), then the one
    /// hint-free failure. The seventh real attempt ends the connection.
    fn refuse(&mut self, r: Refusal) -> Result<Auth, ConnError> {
        self.attempts += 1;
        metrics::counter!("aurora_terminator_auth_failures_total", "reason" => r.label())
            .increment(1);
        Ok(Auth::reject())
    }

    fn check_attempts(&self) -> Result<(), ConnError> {
        if self.attempts >= SSH_AUTH_ATTEMPTS {
            return Err(ConnError::Policy);
        }
        Ok(())
    }

    fn sess_cfg_channels(&self) -> usize {
        self.shared.cfg().channels_per_session as usize
    }
}

impl Handler for Conn {
    type Error = ConnError;

    /// The `none` probe is answered (listing `publickey`) without counting.
    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::reject())
    }

    async fn auth_password(&mut self, _user: &str, _password: &str) -> Result<Auth, Self::Error> {
        self.check_attempts()?;
        self.refuse(Refusal::NotCertificate)
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        _response: Option<russh::server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        self.check_attempts()?;
        self.refuse(Refusal::NotCertificate)
    }

    /// The query phase (D-356): ssh-ed25519 only; the same answer for every holder.
    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        offer: russh::server::PublicKeyOffer<'_>,
    ) -> Result<Auth, Self::Error> {
        if *offer.algorithm == Algorithm::Ed25519 {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    /// A verified plain key never authenticates.
    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        self.check_attempts()?;
        self.refuse(Refusal::NotCertificate)
    }

    /// The certificate law, then (only if it holds) the API's verdict.
    async fn auth_openssh_certificate(
        &mut self,
        user: &str,
        certificate: &Certificate,
    ) -> Result<Auth, Self::Error> {
        self.check_attempts()?;
        if !self.shared.is_healthy() {
            return self.refuse(Refusal::ApiError);
        }
        if let Err(r) = cert::check(
            certificate,
            user,
            &self.shared.cas(),
            std::time::SystemTime::now(),
        ) {
            return self.refuse(r);
        }
        let Ok(line) = certificate.to_openssh() else {
            return self.refuse(Refusal::KeyId);
        };
        let permit_pty = certificate.extensions().contains_key("permit-pty");
        let req = SessionOpenRequest {
            certificate: SecretString::new(line),
            username: user.to_owned(),
            client_address: self.peer.map(|p| p.to_string()),
        };
        match self.shared.plane.open_session(&req).await {
            Ok(crate::plane::Opened::Open(r)) => {
                let s = Sess::new(r.session_id);
                self.permit_pty = permit_pty;
                self.sess = Some(s.clone());
                let _ = self.ctl.session.set(s.clone());
                self.shared.add_session(s);
                Ok(Auth::Accept)
            }
            Ok(crate::plane::Opened::Refused(SessionRefusal::Limit)) => self.refuse(Refusal::Limit),
            Ok(crate::plane::Opened::Refused(_)) => self.refuse(Refusal::Denied),
            Err(_) => {
                metrics::counter!("aurora_terminator_connections_total", "outcome" => "api_error")
                    .increment(1);
                self.refuse(Refusal::ApiError)
            }
        }
    }

    async fn auth_succeeded(&mut self, session: &mut Session) -> Result<(), Self::Error> {
        // Authenticated: the connection leaves the pre-auth caps.
        self.admission = None;
        self.ctl.authed.store(true, Ordering::Release);
        metrics::counter!("aurora_terminator_connections_total", "outcome" => "authenticated")
            .increment(1);
        let handle = session.handle();
        self.handle = Some(handle.clone());
        if let Some(sess) = self.sess.clone() {
            tokio::spawn(watch_session(
                self.shared.clone(),
                self.ctl.clone(),
                sess,
                handle,
            ));
        }
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let id = channel.id();
        // The channel's own message queue is not used (data arrives through the handler); drop
        // it so a full queue can never stall the connection.
        drop(channel);
        if self.sess.is_none() || self.chans.len() >= self.sess_cfg_channels() {
            refused_metric("other");
            reply.reject(ChannelOpenFailure::ResourceShortage).await;
            return Ok(());
        }
        self.chans.insert(
            id,
            Chan {
                pty: None,
                env: Vec::new(),
                started: false,
                tx: None,
            },
        );
        self.ctl
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id);
        if let Some(s) = &self.sess {
            s.stats.channels_open.fetch_add(1, Ordering::Relaxed);
        }
        reply.accept().await;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        _channel: Channel<Msg>,
        _host: &str,
        _port: u32,
        _orig_host: &str,
        _orig_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        refused_metric("direct-tcpip");
        reply
            .reject(ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn channel_open_direct_streamlocal(
        &mut self,
        _channel: Channel<Msg>,
        _path: &str,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        refused_metric("streamlocal");
        reply
            .reject(ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn channel_open_x11(
        &mut self,
        _channel: Channel<Msg>,
        _host: &str,
        _port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        refused_metric("x11");
        reply
            .reject(ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn channel_open_forwarded_tcpip(
        &mut self,
        _channel: Channel<Msg>,
        _host: &str,
        _port: u32,
        _orig_host: &str,
        _orig_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        refused_metric("other");
        reply
            .reject(ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _pix_w: u32,
        _pix_h: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // RFC 4254 §6.2: a zero character dimension is "unspecified" — OpenSSH sends 0×0 when its stdin is not a
        // terminal (`ssh -tt host` fed by a pipe, a script). It gets the usual 80×24; anything above 1000 is refused.
        let (cols, rows) = (
            if cols == 0 { DEFAULT_COLS } else { cols },
            if rows == 0 { DEFAULT_ROWS } else { rows },
        );
        // An empty TERM is "unspecified" too (OpenSSH sends "" when TERM is unset: cron, CI, a script): the supervisor's
        // own default, the one it falls back to for a type it does not accept.
        let term = if term.is_empty() { DEFAULT_TERM } else { term };
        let ok = self.permit_pty
            && (1..=64).contains(&term.len())
            && term.bytes().all(|b| (0x20..=0x7e).contains(&b))
            && (1..=1000).contains(&cols)
            && (1..=1000).contains(&rows)
            && self
                .chans
                .get_mut(&channel)
                .filter(|c| !c.started && c.pty.is_none())
                .map(|c| c.pty = Some((term.to_owned(), cols as u16, rows as u16)))
                .is_some();
        if ok {
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let allowed = (name == "LANG" || name.starts_with("LC_"))
            && name.len() <= 64
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && value.len() <= 256
            && !value.contains('\0');
        let ok = allowed
            && self
                .chans
                .get_mut(&channel)
                .filter(|c| !c.started && c.env.len() < SSH_ENV_VARS_MAX)
                .map(|c| c.env.push((name.to_owned(), value.to_owned())))
                .is_some();
        if ok {
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.begin(channel, ChannelMode::Shell, None, session)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let cmd = std::str::from_utf8(data)
            .ok()
            .filter(|c| c.len() <= SSH_EXEC_COMMAND_MAX && !c.contains('\0'))
            .map(str::to_owned);
        if cmd.is_none() {
            session.channel_failure(channel)?;
            return Ok(());
        }
        self.begin(channel, ChannelMode::Exec, cmd, session)
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            refused_metric("subsystem");
            session.channel_failure(channel)?;
            return Ok(());
        }
        self.begin(channel, ChannelMode::Sftp, None, session)
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _pix_w: u32,
        _pix_h: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(c) = self.chans.get(&channel)
            && c.pty.is_some()
            && (1..=1000).contains(&cols)
            && (1..=1000).contains(&rows)
            && let Some(tx) = &c.tx
        {
            let _ = tx.try_send(ToLeg::Resize(cols as u16, rows as u16));
        }
        Ok(())
    }

    async fn x11_request(
        &mut self,
        channel: ChannelId,
        _single: bool,
        _proto: &str,
        _cookie: &str,
        _screen: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        refused_metric("x11");
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn agent_request(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        refused_metric("auth-agent");
        Ok(false)
    }

    async fn tcpip_forward(
        &mut self,
        _address: &str,
        _port: &mut u32,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        refused_metric("tcpip-forward");
        Ok(false)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        _address: &str,
        _port: u32,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        refused_metric("tcpip-forward");
        Ok(false)
    }

    async fn streamlocal_forward(
        &mut self,
        _path: &str,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        refused_metric("streamlocal");
        Ok(false)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.chans.get(&channel).and_then(|c| c.tx.as_ref()) {
            // Awaiting here is the backpressure: a leg that is not draining stalls this
            // connection's reads, and the client's window with it.
            let _ = tx.send(ToLeg::Data(Bytes::copy_from_slice(data))).await;
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.chans.get(&channel).and_then(|c| c.tx.as_ref()) {
            let _ = tx.send(ToLeg::Eof).await;
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(c) = self.chans.remove(&channel) {
            if let Some(tx) = c.tx {
                let _ = tx.try_send(ToLeg::Close);
            }
            if let Some(s) = &self.sess {
                s.stats.channels_open.fetch_sub(1, Ordering::Relaxed);
            }
        }
        self.ctl
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&channel);
        Ok(())
    }
}

impl Conn {
    /// A `shell`, `exec` or `subsystem sftp` request: the leg is opened by a task (the reply
    /// follows through the session handle), so a waking sandbox never blocks the connection.
    fn begin(
        &mut self,
        channel: ChannelId,
        mode: ChannelMode,
        command: Option<String>,
        session: &mut Session,
    ) -> Result<(), ConnError> {
        let (Some(sess), Some(handle)) = (self.sess.clone(), self.handle.clone()) else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        let Some(c) = self.chans.get_mut(&channel).filter(|c| !c.started) else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        if mode == ChannelMode::Sftp && c.pty.is_some() {
            session.channel_failure(channel)?;
            return Ok(());
        }
        c.started = true;
        let (tx, rx) = mpsc::channel(CHAN_QUEUE);
        c.tx = Some(tx);
        let open = LegOpen {
            mode,
            command,
            tty: c.pty.clone(),
            env: c.env.clone(),
        };
        tokio::spawn(run_channel(
            self.shared.clone(),
            sess,
            handle,
            channel,
            open,
            rx,
        ));
        Ok(())
    }
}

/// The signal name → the SSH `exit-signal` name.
fn sig_of(name: &str) -> Sig {
    match name {
        "ABRT" => Sig::ABRT,
        "ALRM" => Sig::ALRM,
        "FPE" => Sig::FPE,
        "HUP" => Sig::HUP,
        "ILL" => Sig::ILL,
        "INT" => Sig::INT,
        "KILL" => Sig::KILL,
        "PIPE" => Sig::PIPE,
        "QUIT" => Sig::QUIT,
        "SEGV" => Sig::SEGV,
        "TERM" => Sig::TERM,
        "USR1" => Sig::USR1,
        other => Sig::Custom(other.to_owned()),
    }
}

/// One accepted channel: the API's grant (waiting for a waking sandbox), the leg to the runner,
/// the bytes, the exit status. A failure before the leg is up is a one-line notice on stderr and
/// a failed request; a leg that ends without an exit status is `relay_error` (`exit-status` 255
/// and a notice).
async fn run_channel(
    shared: Arc<Shared>,
    sess: Arc<Sess>,
    handle: russh::server::Handle,
    ch: ChannelId,
    open: LegOpen,
    rx: mpsc::Receiver<ToLeg>,
) {
    use crate::plane::Granted;
    let fail = |msg: &'static str| {
        let handle = handle.clone();
        async move {
            let _ = handle
                .extended_data(ch, 1, Bytes::from(format!("aurora: {msg}\r\n")))
                .await;
            let _ = handle.channel_failure(ch).await;
            let _ = handle.close(ch).await;
        }
    };
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(u64::from(shared.cfg().wake_wait_secs));
    let grant = loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let wait = left.as_secs().min(SSH_CHANNEL_POLL_MAX.as_secs()).max(1) as u32;
        match shared.plane.channel(sess.id, open.mode, wait).await {
            Ok(Granted::Grant(g)) => break g,
            Ok(Granted::NotRunning { retry_after_secs }) => {
                if left.is_zero() {
                    return fail("the sandbox did not start in time").await;
                }
                tokio::time::sleep(Duration::from_secs(u64::from(retry_after_secs)).min(left))
                    .await;
            }
            Ok(Granted::SessionClosed) => {
                sess.request_close("session_closed");
                return fail("this session is no longer open").await;
            }
            Ok(Granted::ChannelLimit) => return fail("too many open channels").await,
            Ok(Granted::WakeLimited) => {
                return fail("the sandbox cannot be woken right now; try again later").await;
            }
            Err(_) => return fail("the service is unavailable; try again").await,
        }
    };
    let ws = match leg::connect(&grant.runner_endpoint, grant.grant.expose(), &open).await {
        Ok(ws) => ws,
        Err(_) => return fail("could not open a channel to the sandbox").await,
    };
    sess.stats.count_mode(open.mode);
    let _ = handle.channel_success(ch).await;
    // The channel's bytes count towards the session: the sink shares its stats.
    let sink = ChannelSink {
        handle: handle.clone(),
        channel: ch,
        stats: sess.stats.clone(),
    };
    match leg::bridge(ws, rx, sink).await {
        LegEnd::Exit { status, signal } => {
            if let Some(sig) = signal {
                let _ = handle
                    .exit_signal_request(ch, sig_of(&sig), false, String::new(), String::new())
                    .await;
            } else {
                let _ = handle
                    .exit_status_request(ch, status.unwrap_or(255).max(0) as u32)
                    .await;
            }
        }
        LegEnd::RelayError => {
            let _ = handle
                .extended_data(
                    ch,
                    1,
                    Bytes::from_static(b"aurora: the connection to the sandbox was lost\r\n"),
                )
                .await;
            let _ = handle.exit_status_request(ch, 255).await;
        }
        LegEnd::ClientClosed => return,
    }
    let _ = handle.eof(ch).await;
    let _ = handle.close(ch).await;
}

/// Idle, lifetime, revocation and shutdown for one authenticated connection: when one fires the
/// open channels get a one-line notice on stderr, `exit-status` 255, EOF and CLOSE; the client
/// gets up to `notice_wait` to close them (so OpenSSH prints the notice), then `DISCONNECT`.
async fn watch_session(
    shared: Arc<Shared>,
    ctl: Arc<Ctl>,
    sess: Arc<Sess>,
    handle: russh::server::Handle,
) {
    let mut close_rx = sess.close_tx.subscribe();
    let mut stop = shared.stop.clone();
    let (idle, life) = (shared.idle_limit(), shared.lifetime_limit());
    let reason: String = loop {
        let life_left = life.saturating_sub(sess.opened.elapsed());
        let idle_left = idle.saturating_sub(sess.idle_for());
        if life_left.is_zero() {
            break "lifetime".to_owned();
        }
        if idle_left.is_zero() {
            break "idle".to_owned();
        }
        let wait = life_left
            .min(idle_left)
            .min(Duration::from_millis(500))
            .max(Duration::from_millis(5));
        tokio::select! {
            r = close_rx.changed() => {
                if r.is_err() { return; }
                if let Some(r) = close_rx.borrow().clone() { break r; }
            }
            _ = stop.changed() => {
                if *stop.borrow() { break "terminator_shutdown".to_owned(); }
            }
            () = tokio::time::sleep(wait) => {}
        }
    };
    sess.request_close(&reason);
    let notice = match reason.split(':').next().unwrap_or("") {
        "idle" => "disconnected: no activity",
        "lifetime" => "disconnected: maximum session time reached",
        "terminator_shutdown" => "disconnected: the SSH service is restarting; reconnect",
        "lease_expired" => "disconnected: the SSH service lost contact with the platform",
        "revoked" => "disconnected: access to this agent was revoked or the session was ended",
        _ => "disconnected",
    };
    let channels: Vec<ChannelId> = ctl
        .open
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .copied()
        .collect();
    for ch in channels {
        let _ = handle
            .extended_data(ch, 1, Bytes::from(format!("aurora: {notice}\r\n")))
            .await;
        let _ = handle.exit_status_request(ch, 255).await;
        let _ = handle.eof(ch).await;
        let _ = handle.close(ch).await;
    }
    let ctl2 = ctl.clone();
    let _ = tokio::time::timeout(shared.settings.notice_wait, async move {
        while !ctl2
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    let _ = handle
        .disconnect(Disconnect::ByApplication, String::new(), String::new())
        .await;
}

/// Accept loop: pre-auth admission (a refused socket is closed without a message), one task per
/// connection, graceful shutdown (every session closed with `terminator_shutdown` and given a
/// moment to say goodbye).
pub async fn serve(
    listener: TcpListener,
    shared: Arc<Shared>,
    host_key: PrivateKey,
    mut stop: watch::Receiver<bool>,
) {
    let cfg = Arc::new(ssh_config(host_key, &shared.settings));
    loop {
        tokio::select! {
            _ = stop.wait_for(|v| *v) => break,
            acc = listener.accept() => {
                let Ok((stream, peer)) = acc else { continue };
                let adm = match shared.pre.admit(peer.ip()) {
                    Ok(a) => a,
                    Err(why) => {
                        let outcome = match why {
                            Refused::Preauth => "preauth_limited",
                            Refused::Rate => "rate_limited",
                        };
                        metrics::counter!("aurora_terminator_connections_total", "outcome" => outcome)
                            .increment(1);
                        drop(stream);
                        continue;
                    }
                };
                let _ = stream.set_nodelay(true);
                if let Err(e) = set_tcp_keepalive(&stream) {
                    tracing::warn!(error = %e, "could not set TCP keepalive on an SSH connection");
                }
                tokio::spawn(connection(shared.clone(), cfg.clone(), stream, peer, adm));
            }
        }
    }
    // Shutdown: every session ends with a notice; wait for them (bounded).
    for s in shared
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
    {
        s.request_close("terminator_shutdown");
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while !shared
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
}

async fn connection(
    shared: Arc<Shared>,
    cfg: Arc<Config>,
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    admission: Admission,
) {
    let ctl = Arc::new(Ctl {
        authed: AtomicBool::new(false),
        session: OnceLock::new(),
        open: Mutex::new(HashSet::new()),
    });
    let handler = Conn {
        shared: shared.clone(),
        ctl: ctl.clone(),
        peer: Some(peer.ip()),
        admission: Some(admission),
        attempts: 0,
        sess: None,
        permit_pty: false,
        handle: None,
        chans: HashMap::new(),
    };
    // `SSH_AUTH_TIMEOUT` runs from accept, so it also covers a client that never sends its
    // identification string (`run_stream` waits for it).
    let auth_timer = tokio::time::sleep(shared.settings.auth_timeout);
    tokio::pin!(auth_timer);
    let timeout = || {
        metrics::counter!("aurora_terminator_connections_total", "outcome" => "auth_timeout")
            .increment(1);
    };
    let started = tokio::select! {
        r = russh::server::run_stream(
            cfg,
            ClientActivity { inner: stream, ctl: ctl.clone() },
            handler,
        ) => r,
        () = &mut auth_timer => {
            timeout();
            return;
        }
    };
    let Ok(rs) = started else {
        return;
    };
    let handle = rs.handle();
    tokio::pin!(rs);
    // `ended`: the session finished by itself (a completed future is never polled again).
    let (timed_out, ended) = tokio::select! {
        _ = &mut rs => (false, true),
        () = &mut auth_timer => (!ctl.authed.load(Ordering::Acquire), false),
    };
    if timed_out {
        timeout();
        let _ = handle
            .disconnect(Disconnect::ByApplication, String::new(), String::new())
            .await;
    }
    if !ended {
        let _ = (&mut rs).await;
    }
    match ctl.session.get() {
        Some(sess) => {
            let reason = sess
                .close_tx
                .borrow()
                .clone()
                .unwrap_or_else(|| "client_disconnect".to_owned());
            shared.finish(sess, &reason).await;
        }
        None if !timed_out => {
            metrics::counter!("aurora_terminator_connections_total", "outcome" => "auth_failed")
                .increment(1);
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (D-434) A dead peer is found in about a minute, not the kernel's two hours.
    #[tokio::test]
    async fn tcp_keepalive_is_set_with_explicit_timings() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let c = tokio::net::TcpStream::connect(l.local_addr().unwrap())
            .await
            .unwrap();
        let (s, _) = l.accept().await.unwrap();
        set_tcp_keepalive(&s).unwrap();
        let sock = socket2::SockRef::from(&s);
        assert!(sock.keepalive().unwrap());
        assert_eq!(sock.tcp_keepalive_time().unwrap(), TCP_KEEPALIVE_IDLE);
        assert_eq!(
            sock.tcp_keepalive_interval().unwrap(),
            TCP_KEEPALIVE_INTERVAL
        );
        assert_eq!(sock.tcp_keepalive_retries().unwrap(), TCP_KEEPALIVE_PROBES);
        drop(c);
    }

    /// Only reads restart the clock, and at most once a second.
    #[test]
    fn client_traffic_touches_once_per_second() {
        let st = crate::state::Stats::default();
        assert_eq!(st.last_activity.load(Ordering::Relaxed), 0);
        st.touch_client();
        let t = st.last_activity.load(Ordering::Relaxed);
        assert!(t > 0);
        st.touch_client();
        assert!(st.last_activity.load(Ordering::Relaxed) >= t);
        assert_eq!(st.bytes_in.load(Ordering::Relaxed), 0, "no byte is counted");
    }
}
