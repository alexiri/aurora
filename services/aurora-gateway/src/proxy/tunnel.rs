//! Byte pipes: open-lane tunnels and upgraded (WebSocket) connections.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;

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

/// Resolves when shutdown has been signalled.
pub(super) async fn stopped(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|s| *s).await;
}
