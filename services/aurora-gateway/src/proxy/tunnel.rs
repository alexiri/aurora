//! Byte pipes: open-lane tunnels and upgraded (WebSocket) connections.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;
use uuid::Uuid;

struct Metered<T> {
    inner: T,
    count: Arc<AtomicU64>,
    last: Arc<AtomicU64>,
    start: Instant,
}

impl<T> Metered<T> {
    fn touch(&self) {
        let ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last.store(ms, Ordering::Relaxed);
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Metered<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &r {
            self.count
                .fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
            self.touch();
        }
        r
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Metered<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let r = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(_)) = &r {
            self.touch();
        }
        r
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Copies both ways until EOF, error, idle timeout or shutdown. Returns
/// `(client->upstream, upstream->client)` byte counts.
pub async fn bridge<A, B>(
    client: A,
    upstream: B,
    idle: Option<Duration>,
    mut stop: watch::Receiver<bool>,
) -> (u64, u64)
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let up = Arc::new(AtomicU64::new(0));
    let down = Arc::new(AtomicU64::new(0));
    let last = Arc::new(AtomicU64::new(0));
    let start = Instant::now();
    let mut a = Metered {
        inner: client,
        count: up.clone(),
        last: last.clone(),
        start,
    };
    let mut b = Metered {
        inner: upstream,
        count: down.clone(),
        last: last.clone(),
        start,
    };
    let idle_watch = async {
        match idle {
            None => std::future::pending::<()>().await,
            Some(d) => loop {
                let now = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                let since = Duration::from_millis(now.saturating_sub(last.load(Ordering::Relaxed)));
                if since >= d {
                    break;
                }
                tokio::time::sleep(d - since).await;
            },
        }
    };
    tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut a, &mut b) => {}
        () = idle_watch => {}
        () = stopped(&mut stop) => {}
    }
    (up.load(Ordering::Relaxed), down.load(Ordering::Relaxed))
}

/// (FIX-106, `06` §Tunnel limits) Inbound bounds of a `CONNECT` tunnel.
#[derive(Debug, Clone, Copy)]
pub struct TunnelLimits {
    /// The inner TLS handshake of a MITM tunnel, and the dial of an open-lane tunnel.
    pub handshake: Duration,
    /// A request head inside a MITM tunnel.
    pub header: Duration,
    /// Every tunnel kind ends after this long without traffic.
    pub idle: Duration,
    /// Live tunnels of one credential.
    pub per_credential: usize,
    /// Live tunnels of the gateway.
    pub total: usize,
}

impl Default for TunnelLimits {
    fn default() -> Self {
        use aurora_core::limits as l;
        Self {
            handshake: l::TUNNEL_HANDSHAKE_TIMEOUT,
            header: l::TUNNEL_HEADER_TIMEOUT,
            idle: l::TUNNEL_IDLE_TIMEOUT,
            per_credential: l::TUNNEL_PER_CREDENTIAL_MAX,
            total: l::TUNNEL_TOTAL_MAX,
        }
    }
}

/// Live-tunnel counters (total and per credential).
#[derive(Default)]
pub(super) struct TunnelCaps {
    total: AtomicUsize,
    per: Mutex<HashMap<Uuid, usize>>,
}

/// Held for a tunnel's whole life; releases its slot on drop.
pub(super) struct TunnelSlot {
    caps: Arc<TunnelCaps>,
    credential: Uuid,
}

impl TunnelCaps {
    /// Takes a slot for `credential`, or says which cap is full.
    pub(super) fn acquire(
        self: &Arc<Self>,
        credential: Uuid,
        limits: &TunnelLimits,
    ) -> Result<TunnelSlot, &'static str> {
        let mut per = self.per.lock().unwrap_or_else(PoisonError::into_inner);
        if self.total.load(Ordering::SeqCst) >= limits.total {
            return Err("total");
        }
        let n = per.entry(credential).or_insert(0);
        if *n >= limits.per_credential {
            return Err("credential");
        }
        *n += 1;
        self.total.fetch_add(1, Ordering::SeqCst);
        Ok(TunnelSlot {
            caps: self.clone(),
            credential,
        })
    }
}

impl Drop for TunnelSlot {
    fn drop(&mut self) {
        let mut per = self.caps.per.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = per.get_mut(&self.credential) {
            *n -= 1;
            if *n == 0 {
                per.remove(&self.credential);
            }
        }
        self.caps.total.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Last traffic of a MITM tunnel and the requests it has in flight (a request held for approval
/// or waiting on its upstream is not idle).
pub(super) struct Activity {
    start: Instant,
    last_ms: AtomicU64,
    in_flight: AtomicUsize,
}

impl Activity {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            last_ms: AtomicU64::new(0),
            in_flight: AtomicUsize::new(0),
        })
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn touch(&self) {
        self.last_ms.store(self.now_ms(), Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        if self.in_flight.load(Ordering::SeqCst) > 0 {
            return Duration::ZERO;
        }
        Duration::from_millis(
            self.now_ms()
                .saturating_sub(self.last_ms.load(Ordering::Relaxed)),
        )
    }

    /// Resolves once the tunnel has been idle for `limit`.
    pub(super) async fn expired(&self, limit: Duration) {
        loop {
            let since = self.idle_for();
            if since >= limit {
                return;
            }
            tokio::time::sleep(limit - since).await;
        }
    }
}

/// Marks one request in flight for its lifetime.
pub(super) struct InFlight(Arc<Activity>);

impl InFlight {
    pub(super) fn new(a: &Arc<Activity>) -> Self {
        a.in_flight.fetch_add(1, Ordering::SeqCst);
        Self(a.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.touch();
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An IO wrapper recording traffic in an [`Activity`].
pub(super) struct Active<T> {
    pub(super) inner: T,
    pub(super) act: Arc<Activity>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Active<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &r {
            self.act.touch();
        }
        r
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Active<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let r = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(_)) = &r {
            self.act.touch();
        }
        r
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Resolves when shutdown has been signalled.
pub(super) async fn stopped(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|s| *s).await;
}
