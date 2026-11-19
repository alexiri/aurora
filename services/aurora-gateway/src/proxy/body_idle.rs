//! (FIX-180, D-395) Idle limit of a proxied request body: no byte for the limit ends the request.

use crate::BoxError;
use bytes::Bytes;
use hyper::body::{Body, Frame, SizeHint};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::{Instant, Sleep};

/// Set by [`IdleBody`] when its limit expired; the request's task waits on it.
#[derive(Default)]
pub(super) struct Stall {
    set: AtomicBool,
    notify: Notify,
}

impl Stall {
    pub(super) fn is_set(&self) -> bool {
        self.set.load(Ordering::SeqCst)
    }

    /// Resolves once the body stalled (never, if it does not).
    pub(super) async fn tripped(&self) {
        loop {
            let n = self.notify.notified();
            if self.is_set() {
                return;
            }
            n.await;
        }
    }

    fn trip(&self) {
        self.set.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

/// The error a stalled body yields to whoever reads it.
#[derive(Debug)]
pub(super) struct BodyIdle;

impl std::fmt::Display for BodyIdle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("request body idle limit reached")
    }
}

impl std::error::Error for BodyIdle {}

/// Passes frames through; while a reader is waiting for the next frame, the clock runs. The clock
/// restarts at every frame and does not run while nobody reads (a body the decision path has not
/// asked for yet is not idle).
pub(super) struct IdleBody<B> {
    inner: B,
    limit: Duration,
    sleep: Pin<Box<Sleep>>,
    waiting: bool,
    stall: Arc<Stall>,
}

impl<B> IdleBody<B> {
    pub(super) fn new(inner: B, limit: Duration, stall: Arc<Stall>) -> Self {
        Self {
            inner,
            limit,
            sleep: Box::pin(tokio::time::sleep(limit)),
            waiting: false,
            stall,
        }
    }
}

impl<B> Body for IdleBody<B>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        if this.stall.is_set() {
            return Poll::Ready(Some(Err(Box::new(BodyIdle))));
        }
        if !this.waiting {
            this.waiting = true;
            this.sleep.as_mut().reset(Instant::now() + this.limit);
        }
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(r) => {
                this.waiting = false;
                Poll::Ready(r.map(|r| r.map_err(Into::into)))
            }
            Poll::Pending => {
                if this.sleep.as_mut().poll(cx).is_ready() {
                    this.waiting = false;
                    this.stall.trip();
                    Poll::Ready(Some(Err(Box::new(BodyIdle))))
                } else {
                    Poll::Pending
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
