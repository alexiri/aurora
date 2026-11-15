//! Wire-level request guard (SEC-85, FIX-088): the smuggling corpus checked on the bytes the
//! browser sent, not on what hyper made of them.
//!
//! hyper normalises before the service sees a request: it drops a `Content-Length` that comes
//! with `Transfer-Encoding`, accepts a bare LF as a line end, merges header folding. The
//! relay is the only HTTP parser in front of the app, so it must not forward a request whose
//! framing two parsers could read differently. [`Guard`] wraps the connection, follows each
//! request's framing (head, then `Content-Length` or chunked body, then the next head) and
//! raises a per-connection flag the moment a head is ambiguous: bare LF or CR, obsolete line
//! folding, a duplicate `Content-Length`, `Content-Length` together with `Transfer-Encoding`,
//! a `Transfer-Encoding` other than exactly `chunked`, a malformed length, a chunk framing
//! violation. The service answers every request of a flagged connection with 400 and closes it.
//!
//! After an upgrade request (`Upgrade`, `CONNECT`) the bytes are no longer HTTP, so the guard
//! stops inspecting; the service closes the connection after any answer that is not `101`.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A head larger than this without its blank line is refused.
const HEAD_MAX: usize = 256 * 1024;
/// Longest chunk-size or trailer line.
const LINE_MAX: usize = 8 * 1024;

/// Where the guard is in the request stream.
enum Scan {
    /// Reading a head; the bytes of it so far.
    Head(Vec<u8>),
    /// This many body bytes (`Content-Length`) are still to come.
    Length(u64),
    /// Inside a chunked body.
    Chunked(ChunkSkip),
    /// Not HTTP any more (upgraded) or already flagged: nothing is inspected.
    Through,
}

/// The connection wrapper: reads are scanned, writes pass through untouched.
pub struct Guard<T> {
    inner: T,
    scan: Scan,
    bad: Arc<AtomicBool>,
    /// The head just completed asked for an upgrade (read by the service through [`Wire`]).
    upgrade: Arc<AtomicBool>,
}

/// The service's view of the guard: whether the connection carried an ambiguous request, and
/// whether it was asked to upgrade.
#[derive(Clone, Default)]
pub struct Wire {
    bad: Arc<AtomicBool>,
    upgrade: Arc<AtomicBool>,
}

impl Wire {
    /// An ambiguous request was seen on this connection.
    pub fn is_bad(&self) -> bool {
        self.bad.load(Ordering::SeqCst)
    }

    /// An upgrade (or `CONNECT`) request was seen: only a `101` keeps the connection.
    pub fn upgrade_seen(&self) -> bool {
        self.upgrade.load(Ordering::SeqCst)
    }

    /// Wraps `io`.
    pub fn guard<T>(&self, io: T) -> Guard<T> {
        Guard {
            inner: io,
            scan: Scan::Head(Vec::new()),
            bad: self.bad.clone(),
            upgrade: self.upgrade.clone(),
        }
    }
}

impl<T> Guard<T> {
    fn violation(&mut self) {
        self.bad.store(true, Ordering::SeqCst);
        self.scan = Scan::Through;
    }

    fn feed(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            match &mut self.scan {
                Scan::Through => return,
                Scan::Length(n) => {
                    let k = usize::try_from(*n).unwrap_or(usize::MAX).min(data.len());
                    *n -= k as u64;
                    data = &data[k..];
                    if *n == 0 {
                        self.scan = Scan::Head(Vec::new());
                    }
                }
                Scan::Chunked(skip) => match skip.feed(data) {
                    Err(()) => return self.violation(),
                    Ok((used, done)) => {
                        data = &data[used..];
                        if done {
                            self.scan = Scan::Head(Vec::new());
                        }
                    }
                },
                Scan::Head(buf) => {
                    // Byte-wise only to find the end of the head; checks run on the whole.
                    let mut end = None;
                    for (i, &b) in data.iter().enumerate() {
                        let prev = buf.last().copied();
                        // A bare LF or a bare CR is refused at once, even in a half head.
                        if (b == b'\n' && prev != Some(b'\r'))
                            || (prev == Some(b'\r') && b != b'\n')
                        {
                            return self.violation();
                        }
                        buf.push(b);
                        if b == b'\n' && buf.ends_with(b"\r\n\r\n") {
                            end = Some(i + 1);
                            break;
                        }
                        if buf.len() > HEAD_MAX {
                            return self.violation();
                        }
                    }
                    let Some(used) = end else { return };
                    data = &data[used..];
                    let head = std::mem::take(buf);
                    match judge_head(&head) {
                        Err(()) => return self.violation(),
                        Ok(Next::Head) => self.scan = Scan::Head(Vec::new()),
                        Ok(Next::Length(n)) => self.scan = Scan::Length(n),
                        Ok(Next::Chunked) => self.scan = Scan::Chunked(ChunkSkip::default()),
                        Ok(Next::Upgrade) => {
                            self.upgrade.store(true, Ordering::SeqCst);
                            self.scan = Scan::Through;
                        }
                    }
                }
            }
        }
    }
}

/// What follows a clean head.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    Head,
    Length(u64),
    Chunked,
    Upgrade,
}

/// Judges one complete head (CRLF-only line ends are guaranteed by the byte scan).
fn judge_head(head: &[u8]) -> Result<Next, ()> {
    let text = std::str::from_utf8(head).map_err(|_| ())?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(())?;
    let method = request_line.split(' ').next().unwrap_or_default();
    let (mut cl, mut te, mut upgrade) = (Vec::new(), Vec::new(), false);
    for line in lines {
        if line.is_empty() {
            continue;
        }
        // Obsolete line folding.
        if line.starts_with([' ', '\t']) {
            return Err(());
        }
        let (name, value) = line.split_once(':').ok_or(())?;
        // No whitespace between the name and the colon (RFC 9112 §5.1).
        if name.is_empty() || name.contains([' ', '\t']) {
            return Err(());
        }
        let value = value.trim_matches([' ', '\t']);
        match name.to_ascii_lowercase().as_str() {
            "content-length" => cl.push(value.to_owned()),
            "transfer-encoding" => te.push(value.to_owned()),
            "upgrade" => upgrade = true,
            _ => {}
        }
    }
    if cl.len() > 1 || te.len() > 1 || (!cl.is_empty() && !te.is_empty()) {
        return Err(());
    }
    if let Some(t) = te.first() {
        if !t.eq_ignore_ascii_case("chunked") {
            return Err(());
        }
        return Ok(Next::Chunked);
    }
    if upgrade || method.eq_ignore_ascii_case("CONNECT") {
        return Ok(Next::Upgrade);
    }
    match cl.first() {
        None => Ok(Next::Head),
        Some(v) => {
            if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                return Err(());
            }
            match v.parse::<u64>().map_err(|_| ())? {
                0 => Ok(Next::Head),
                n => Ok(Next::Length(n)),
            }
        }
    }
}

/// Counts through a chunked body without copying it; strict about every line ending.
#[derive(Default)]
pub struct ChunkSkip {
    state: SkipState,
    /// The current size or trailer line (without its CRLF).
    line: Vec<u8>,
    /// The previous byte of a line was CR.
    cr: bool,
}

#[derive(Default)]
enum SkipState {
    #[default]
    Size,
    Data(u64),
    /// The CRLF after a chunk's data; how many of its bytes were seen.
    DataEnd(u8),
    Trailer,
}

impl ChunkSkip {
    /// Consumes bytes of the body; `Ok((used, true))` once the last chunk and the trailer
    /// section ended (`used` stops there: what follows is the next request).
    fn feed(&mut self, data: &[u8]) -> Result<(usize, bool), ()> {
        let mut i = 0;
        while i < data.len() {
            match self.state {
                SkipState::Data(left) => {
                    let k = usize::try_from(left)
                        .unwrap_or(usize::MAX)
                        .min(data.len() - i);
                    i += k;
                    let left = left - k as u64;
                    self.state = if left == 0 {
                        SkipState::DataEnd(0)
                    } else {
                        SkipState::Data(left)
                    };
                }
                SkipState::DataEnd(seen) => {
                    let want = if seen == 0 { b'\r' } else { b'\n' };
                    if data[i] != want {
                        return Err(());
                    }
                    i += 1;
                    self.state = if seen == 0 {
                        SkipState::DataEnd(1)
                    } else {
                        SkipState::Size
                    };
                }
                SkipState::Size | SkipState::Trailer => {
                    let b = data[i];
                    i += 1;
                    if self.cr {
                        if b != b'\n' {
                            return Err(());
                        }
                        self.cr = false;
                        let line = std::mem::take(&mut self.line);
                        if matches!(self.state, SkipState::Trailer) {
                            if line.is_empty() {
                                return Ok((i, true));
                            }
                            continue;
                        }
                        let size = line.split(|c| *c == b';').next().unwrap_or_default();
                        if size.is_empty() || !size.iter().all(u8::is_ascii_hexdigit) {
                            return Err(());
                        }
                        let n = u64::from_str_radix(std::str::from_utf8(size).map_err(|_| ())?, 16)
                            .map_err(|_| ())?;
                        self.state = if n == 0 {
                            SkipState::Trailer
                        } else {
                            SkipState::Data(n)
                        };
                    } else if b == b'\r' {
                        self.cr = true;
                    } else if b == b'\n' {
                        return Err(());
                    } else {
                        self.line.push(b);
                        if self.line.len() > LINE_MAX {
                            return Err(());
                        }
                    }
                }
            }
        }
        Ok((i, false))
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Guard<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &r {
            let fresh = buf.filled()[before..].to_vec();
            self.feed(&fresh);
        }
        r
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Guard<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `raw` in pieces of `step` bytes and reports (flagged, upgrade seen).
    fn run(raw: &[u8], step: usize) -> (bool, bool) {
        let w = Wire::default();
        let mut g = w.guard(tokio::io::empty());
        for c in raw.chunks(step) {
            g.feed(c);
        }
        (w.is_bad(), w.upgrade_seen())
    }

    #[test]
    fn clean_requests_pass_at_every_split() {
        let raw = b"POST /a HTTP/1.1\r\nhost: x\r\ncontent-length: 3\r\n\r\nabc\
GET /b HTTP/1.1\r\nhost: x\r\n\r\n\
POST /c HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n3;x=1\r\nabc\r\n0\r\nt: 1\r\n\r\n\
GET /d HTTP/1.1\r\nhost: x\r\n\r\n";
        for step in [1, 2, 3, 7, 64, raw.len()] {
            assert_eq!(run(raw, step), (false, false), "step {step}");
        }
        // A violation after the framing was followed correctly is still found.
        let mut bad = raw.to_vec();
        bad.extend_from_slice(b"GET /e HTTP/1.1\nhost: x\r\n\r\n");
        for step in [1, 5, bad.len()] {
            assert_eq!(run(&bad, step), (true, false), "step {step}");
        }
    }

    #[test]
    fn the_smuggling_corpus_is_flagged() {
        let cases: &[(&str, &[u8])] = &[
            (
                "CL+TE",
                b"POST / HTTP/1.1\r\ncontent-length: 4\r\ntransfer-encoding: chunked\r\n\r\n",
            ),
            (
                "TE+CL",
                b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\ncontent-length: 4\r\n\r\n",
            ),
            (
                "dup CL",
                b"POST / HTTP/1.1\r\ncontent-length: 2\r\ncontent-length: 2\r\n\r\n",
            ),
            (
                "TE gzip",
                b"POST / HTTP/1.1\r\ntransfer-encoding: gzip, chunked\r\n\r\n",
            ),
            ("fold", b"GET / HTTP/1.1\r\nx-a: 1\r\n 2\r\n\r\n"),
            ("tab fold", b"GET / HTTP/1.1\r\nx-a: 1\r\n\t2\r\n\r\n"),
            ("bare LF line", b"GET / HTTP/1.1\nhost: x\r\n\r\n"),
            (
                "bare LF header",
                b"GET / HTTP/1.1\r\nx-a: 1\nx-b: 2\r\n\r\n",
            ),
            ("bare LF end", b"GET / HTTP/1.1\r\nhost: x\n\n"),
            ("bare CR", b"GET / HTTP/1.1\r\nx-a: 1\rx-b: 2\r\n\r\n"),
            (
                "space before colon",
                b"POST / HTTP/1.1\r\ncontent-length : 3\r\n\r\n",
            ),
            ("CL sign", b"POST / HTTP/1.1\r\ncontent-length: +3\r\n\r\n"),
            ("CL empty", b"POST / HTTP/1.1\r\ncontent-length:\r\n\r\n"),
            (
                "bad chunk size",
                b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\nzz\r\n",
            ),
            (
                "bad chunk end",
                b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n3\r\nabcXX",
            ),
            (
                "bare LF in chunk size",
                b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n3\nabc\r\n",
            ),
        ];
        for (name, raw) in cases {
            for step in [1, raw.len()] {
                assert!(run(raw, step).0, "{name} (step {step}) was not flagged");
            }
        }
    }

    #[test]
    fn an_upgrade_stops_the_inspection() {
        let raw = b"GET /ws HTTP/1.1\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\r\n\
\x81\x05hello\nanything\r at all";
        assert_eq!(run(raw, 3), (false, true));
        assert_eq!(run(b"CONNECT a:1 HTTP/1.1\r\n\r\n\xff\n", 4), (false, true));
    }
}
