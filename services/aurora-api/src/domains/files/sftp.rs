//! The API's SFTP client of one Files session (docs/16 §W21, C-901): the leg-1 WebSocket to the
//! runner's :4108 listener, adapted into a byte duplex the `russh-sftp` raw session speaks SFTP
//! version 3 over, plus a thin typed facade of exactly the requests `16 §W21` allows. File
//! contents are never logged; only byte totals are counted.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error as ClientError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use aurora_core::limits::{FILES_SESSION_IDLE, FILES_SESSION_LIFETIME, FILES_TRANSFER_IDLE};

/// One WebSocket data frame carries at most this many SFTP bytes (`16 §W20`: 16 KiB chunks).
const CHUNK: usize = 16 * 1024;
const TAG_DATA: u8 = 0x02;
const TAG_STDERR: u8 = 0x03;
const POSIX_RENAME: &str = "posix-rename@openssh.com";
/// SFTP header bytes of a WRITE request on top of its data (length, type, id, handle string,
/// offset, data length: ~30 B with OpenSSH's 4-byte handles) plus headroom for longer handles.
const REQUEST_HEADROOM: usize = 512;
/// Data bytes per SFTP READ/WRITE request: 32 KiB less [`REQUEST_HEADROOM`], so a full pipeline
/// of whole requests fits the leg's credit window (OpenSSH's `sftp-server` caps a request at
/// 256 KiB). With exactly 32 KiB of data, 4 requests are 131 180 bytes: over the 131 072 the
/// runner allows unreplied, and it closes the leg when credit lags the burst.
pub const BLOCK: usize = 32 * 1024 - REQUEST_HEADROOM;
/// Requests outstanding per transfer (`16 §W21`: "4 × 32 KiB").
pub const PIPELINE: usize = 4;

const _: () = assert!(
    PIPELINE * (BLOCK + REQUEST_HEADROOM) <= aurora_core::limits::TERMINAL_STREAM_WINDOW,
    "a full write pipeline must fit the leg's credit window"
);

/// Why a session could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// The runner refused the leg before or at the handshake (old runner, 4502 before 101).
    RunnerRefused,
    /// The supervisor or image cannot run `sftp-server` (4502 after 101) or lacks the
    /// `posix-rename@openssh.com` extension.
    ImageRefused,
    /// `FILE_SESSIONS_PER_SANDBOX` live legs (4409).
    Limit,
    /// Anything else: 4503, timeout, transport failure.
    Unavailable,
}

/// A failed SFTP request, already mapped to what C-899 distinguishes.
#[derive(Debug, Clone, PartialEq)]
pub enum Fail {
    NotFound,
    Denied,
    /// `FAILURE` and everything else the server answered.
    Failed,
    /// The leg is gone or the request timed out: 503 `files_unavailable`.
    Down,
    Eof,
}

fn map_err(e: ClientError) -> Fail {
    match e {
        ClientError::Status(s) => match s.status_code {
            StatusCode::NoSuchFile => Fail::NotFound,
            StatusCode::PermissionDenied => Fail::Denied,
            StatusCode::Eof => Fail::Eof,
            StatusCode::NoConnection | StatusCode::ConnectionLost => Fail::Down,
            _ => Fail::Failed,
        },
        ClientError::Timeout | ClientError::IO(_) | ClientError::UnexpectedPacket => Fail::Down,
        // The leg task ended (the runner closed it): the session is gone, not the operation.
        ClientError::UnexpectedBehavior(m) if m == "session closed" => Fail::Down,
        _ => Fail::Failed,
    }
}

type Res<T> = Result<T, Fail>;

/// What `stat`/`lstat`/a listing entry says about a path.
#[derive(Debug, Clone)]
pub struct Attr {
    /// `file` | `dir` | `symlink` | `other`.
    pub kind: &'static str,
    pub size: u64,
    /// Permission bits only (`0o7777`).
    pub mode: u32,
    pub mtime: Option<i64>,
}

impl Attr {
    fn from(a: &FileAttributes) -> Self {
        let perm = a.permissions.unwrap_or(0);
        let kind = match perm & 0o170_000 {
            0o040_000 => "dir",
            0o100_000 => "file",
            0o120_000 => "symlink",
            _ => "other",
        };
        Self {
            kind,
            size: a.size.unwrap_or(0),
            mode: perm & 0o7777,
            mtime: a.mtime.map(i64::from),
        }
    }
    pub fn owner_write(&self) -> bool {
        self.mode & 0o200 != 0
    }
}

/// Shared state of the leg task: liveness, the close code and byte totals.
#[derive(Default)]
pub struct LegState {
    closed: AtomicBool,
    close_code: AtomicU16,
    stderr_bytes: AtomicU64,
    /// Milliseconds since `born` of the last request or transfer progress.
    last_touch_ms: AtomicU64,
}

pub struct Sftp {
    raw: RawSftpSession,
    state: Arc<LegState>,
    born: Instant,
}

impl Sftp {
    /// Dial the runner with the `auf_` grant, run the first-frame handshake, start the SFTP
    /// session and require `posix-rename@openssh.com` in the server's `VERSION`.
    pub async fn open(endpoint: &str, grant: &str) -> Result<Arc<Self>, OpenError> {
        let mut req = endpoint
            .into_client_request()
            .map_err(|_| OpenError::Unavailable)?;
        req.headers_mut().insert(
            "authorization",
            format!("Bearer {grant}")
                .parse()
                .map_err(|_| OpenError::Unavailable)?,
        );
        let dial = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(req),
        )
        .await;
        let mut ws = match dial {
            Ok(Ok((ws, _))) => ws,
            // An HTTP answer instead of 101: an older runner refusing the unknown kind.
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(_))) => {
                return Err(OpenError::RunnerRefused);
            }
            _ => return Err(OpenError::Unavailable),
        };
        ws.send(Message::Text(
            r#"{"kind":"exec.open","mode":"sftp"}"#.to_string().into(),
        ))
        .await
        .map_err(|_| OpenError::Unavailable)?;
        // `exec.opened` (text) within 10 s, or a close code.
        let opened = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Text(t))) if t.contains("exec.opened") => {
                        return Ok(());
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(f))) => {
                        return Err(match f.map(|f| u16::from(f.code)) {
                            Some(4409) => OpenError::Limit,
                            Some(4502) => OpenError::ImageRefused,
                            _ => OpenError::Unavailable,
                        });
                    }
                    _ => return Err(OpenError::Unavailable),
                }
            }
        })
        .await;
        match opened {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(OpenError::Unavailable),
        }
        let (client_io, server_io) = tokio::io::duplex(4 * CHUNK);
        let state = Arc::new(LegState::default());
        let born = Instant::now();
        tokio::spawn(pump(ws, server_io, state.clone(), born));
        let cfg = russh_sftp::client::Config {
            request_timeout_secs: FILES_TRANSFER_IDLE.as_secs(),
            ..Default::default()
        };
        let raw = RawSftpSession::new_with_config(client_io, cfg);
        let version = raw.init().await.map_err(|_| OpenError::Unavailable)?;
        if !version.extensions.contains_key(POSIX_RENAME) {
            return Err(OpenError::ImageRefused);
        }
        Ok(Arc::new(Self { raw, state, born }))
    }

    /// The leg is gone (closed, idle- or lifetime-expired, or the runner dropped it).
    pub fn is_closed(&self) -> bool {
        self.state.closed.load(Ordering::Acquire)
    }

    /// The leg's close code, once closed (4503 after a W6 reconnect or a park).
    pub fn close_code(&self) -> Option<u16> {
        match self.state.close_code.load(Ordering::Acquire) {
            0 => None,
            c => Some(c),
        }
    }

    pub fn stderr_bytes(&self) -> u64 {
        self.state.stderr_bytes.load(Ordering::Relaxed)
    }

    /// Mark activity (a request or a transfer chunk): resets the idle clock.
    pub fn touch(&self) {
        let ms = u64::try_from(self.born.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.state.last_touch_ms.store(ms, Ordering::Release);
    }

    fn check(&self) -> Res<()> {
        if self.is_closed() {
            Err(Fail::Down)
        } else {
            self.touch();
            Ok(())
        }
    }

    pub async fn realpath(&self, path: &str) -> Res<String> {
        self.check()?;
        let n = self.raw.realpath(path).await.map_err(map_err)?;
        n.files
            .into_iter()
            .next()
            .map(|f| f.filename)
            .ok_or(Fail::Failed)
    }

    pub async fn stat(&self, path: &str) -> Res<Attr> {
        self.check()?;
        let a = self.raw.stat(path).await.map_err(map_err)?;
        Ok(Attr::from(&a.attrs))
    }

    pub async fn lstat(&self, path: &str) -> Res<Attr> {
        self.check()?;
        let a = self.raw.lstat(path).await.map_err(map_err)?;
        Ok(Attr::from(&a.attrs))
    }

    pub async fn readlink(&self, path: &str) -> Res<String> {
        self.check()?;
        let n = self.raw.readlink(path).await.map_err(map_err)?;
        n.files
            .into_iter()
            .next()
            .map(|f| f.filename)
            .ok_or(Fail::Failed)
    }

    /// Up to `max` entries (without `.` and `..`); the bool is "more existed".
    pub async fn list(&self, path: &str, max: usize) -> Res<(Vec<(String, Attr)>, bool)> {
        self.check()?;
        let h = self.raw.opendir(path).await.map_err(map_err)?.handle;
        let mut out = Vec::new();
        let mut more = false;
        let r = async {
            loop {
                self.touch();
                match self.raw.readdir(h.clone()).await {
                    Ok(n) => {
                        for f in n.files {
                            if f.filename == "." || f.filename == ".." {
                                continue;
                            }
                            if out.len() >= max {
                                more = true;
                                return Ok(());
                            }
                            out.push((f.filename, Attr::from(&f.attrs)));
                        }
                    }
                    Err(e) => {
                        return match map_err(e) {
                            Fail::Eof => Ok(()),
                            other => Err(other),
                        };
                    }
                }
            }
        }
        .await;
        let _ = self.raw.close(h).await;
        r.map(|()| (out, more))
    }

    pub async fn open_read(&self, path: &str) -> Res<String> {
        self.check()?;
        Ok(self
            .raw
            .open(path, OpenFlags::READ, FileAttributes::empty())
            .await
            .map_err(map_err)?
            .handle)
    }

    /// Create a new file exclusively (`0600`); the caller renames it into place.
    pub async fn create_excl(&self, path: &str, mode: u32) -> Res<String> {
        self.check()?;
        let attrs = FileAttributes {
            permissions: Some(mode),
            ..FileAttributes::empty()
        };
        Ok(self
            .raw
            .open(
                path,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                attrs,
            )
            .await
            .map_err(map_err)?
            .handle)
    }

    /// One read; an empty vector is end of file.
    pub async fn read_at(&self, handle: &str, offset: u64, len: u32) -> Res<Vec<u8>> {
        self.check()?;
        match self.raw.read(handle, offset, len).await {
            Ok(d) => Ok(d.data),
            Err(e) => match map_err(e) {
                Fail::Eof => Ok(Vec::new()),
                other => Err(other),
            },
        }
    }

    pub async fn write_at(&self, handle: &str, offset: u64, data: Vec<u8>) -> Res<()> {
        self.check()?;
        self.raw
            .write(handle, offset, data)
            .await
            .map(|_| ())
            .map_err(map_err)
    }

    pub async fn fstat(&self, handle: &str) -> Res<Attr> {
        self.check()?;
        let a = self.raw.fstat(handle).await.map_err(map_err)?;
        Ok(Attr::from(&a.attrs))
    }

    pub async fn fsetstat_mode(&self, handle: &str, mode: u32) -> Res<()> {
        self.check()?;
        let attrs = FileAttributes {
            permissions: Some(mode),
            ..FileAttributes::empty()
        };
        self.raw
            .fsetstat(handle, attrs)
            .await
            .map(|_| ())
            .map_err(map_err)
    }

    pub async fn close_handle(&self, handle: &str) {
        let _ = self.raw.close(handle).await;
    }

    pub async fn mkdir(&self, path: &str, mode: u32) -> Res<()> {
        self.check()?;
        let attrs = FileAttributes {
            permissions: Some(mode),
            ..FileAttributes::empty()
        };
        self.raw
            .mkdir(path, attrs)
            .await
            .map(|_| ())
            .map_err(map_err)
    }

    pub async fn rmdir(&self, path: &str) -> Res<()> {
        self.check()?;
        self.raw.rmdir(path).await.map(|_| ()).map_err(map_err)
    }

    pub async fn remove(&self, path: &str) -> Res<()> {
        self.check()?;
        self.raw.remove(path).await.map(|_| ()).map_err(map_err)
    }

    /// `posix-rename@openssh.com`: atomic, replaces an existing target. The extension's data is
    /// two SSH strings (`uint32` length + bytes each).
    pub async fn posix_rename(&self, from: &str, to: &str) -> Res<()> {
        self.check()?;
        let mut data = Vec::with_capacity(8 + from.len() + to.len());
        for s in [from, to] {
            data.extend_from_slice(&(s.len() as u32).to_be_bytes());
            data.extend_from_slice(s.as_bytes());
        }
        match self
            .raw
            .extended(POSIX_RENAME, data)
            .await
            .map_err(map_err)?
        {
            Packet::Status(s) if s.status_code == StatusCode::Ok => Ok(()),
            Packet::Status(s) => Err(match s.status_code {
                StatusCode::NoSuchFile => Fail::NotFound,
                StatusCode::PermissionDenied => Fail::Denied,
                _ => Fail::Failed,
            }),
            _ => Err(Fail::Failed),
        }
    }
}

impl Drop for Sftp {
    fn drop(&mut self) {
        // Dropping the raw session ends the duplex; the leg task then closes the WebSocket.
        self.state.closed.store(true, Ordering::Release);
    }
}

/// The leg task: WebSocket frames ⇄ the duplex the SFTP client speaks over. Binary `[0x02]`
/// frames carry the SFTP stream, `[0x03]` the server's stderr (counted, discarded); the task
/// ends on a close frame, a transport error, the client side hanging up, the idle limit
/// (`FILES_SESSION_IDLE` without a request) or the lifetime (`FILES_SESSION_LIFETIME`).
async fn pump(
    mut ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    mut io: tokio::io::DuplexStream,
    state: Arc<LegState>,
    born: Instant,
) {
    let mut buf = vec![0u8; CHUNK + 1];
    buf[0] = TAG_DATA;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let write_timeout = Duration::from_secs(30);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let idle_ms = u64::try_from(born.elapsed().as_millis()).unwrap_or(u64::MAX)
                    .saturating_sub(state.last_touch_ms.load(Ordering::Acquire));
                if Duration::from_millis(idle_ms) >= FILES_SESSION_IDLE
                    || born.elapsed() >= FILES_SESSION_LIFETIME
                {
                    let _ = ws.send(Message::Close(None)).await;
                    break;
                }
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(b))) => match b.first().copied() {
                    Some(TAG_DATA) => {
                        if io.write_all(&b[1..]).await.is_err() {
                            break;
                        }
                    }
                    Some(TAG_STDERR) => {
                        state.stderr_bytes.fetch_add((b.len() - 1) as u64, Ordering::Relaxed);
                    }
                    _ => break,
                },
                Some(Ok(Message::Text(_) | Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Close(f))) => {
                    if let Some(f) = f {
                        state.close_code.store(u16::from(f.code), Ordering::Release);
                    }
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            },
            n = io.read(&mut buf[1..]) => match n {
                Ok(0) | Err(_) => {
                    let _ = ws.send(Message::Text(r#"{"kind":"exec.eof"}"#.to_string().into())).await;
                    let _ = ws.send(Message::Close(None)).await;
                    break;
                }
                Ok(n) => {
                    let frame = Message::Binary(axum::body::Bytes::copy_from_slice(&buf[..=n]));
                    match tokio::time::timeout(write_timeout, ws.send(frame)).await {
                        Ok(Ok(())) => {}
                        _ => break,
                    }
                }
            },
        }
    }
    state.closed.store(true, Ordering::Release);
    // Dropping `io` fails every in-flight request of the client side.
}
