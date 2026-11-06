//! JSON-lines logging to stderr: rate-limited (100 lines/s), credential-looking values redacted
//! (`docs/12-supervisor-harness.md` §Logging).

use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAX_LINES_PER_SEC: u32 = 100;
const PREFIXES: [&str; 5] = ["aub_", "aup_", "auc_", "aur_", "Basic "];

/// Replace `aub_…`/`aup_…`/`auc_…`/`Basic …` token-looking values with `[redacted]`.
pub fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some((idx, p)) = PREFIXES
        .iter()
        .filter_map(|p| rest.find(p).map(|i| (i, *p)))
        .min_by_key(|(i, _)| *i)
    {
        out.push_str(&rest[..idx]);
        let tail = &rest[idx + p.len()..];
        let n = tail
            .find(|c: char| {
                !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '=' | '+' | '/'))
            })
            .unwrap_or(tail.len());
        if n >= 8 {
            out.push_str("[redacted]");
            rest = &tail[n..];
        } else {
            out.push_str(p);
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

struct Window {
    start: Instant,
    count: u32,
}

static WINDOW: Mutex<Option<Window>> = Mutex::new(None);

fn admit(lines: u32) -> bool {
    let Ok(mut g) = WINDOW.lock() else {
        return true;
    };
    let now = Instant::now();
    let w = g.get_or_insert(Window {
        start: now,
        count: 0,
    });
    if now.duration_since(w.start) >= Duration::from_secs(1) {
        w.start = now;
        w.count = 0;
    }
    if w.count + lines > MAX_LINES_PER_SEC {
        return false;
    }
    w.count += lines;
    true
}

/// `io::Write` sink for tracing: redacts and rate-limits whole writes.
pub struct LogWriter;

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let lines = buf.iter().filter(|b| **b == b'\n').count().max(1) as u32;
        if admit(lines) {
            let text = String::from_utf8_lossy(buf);
            let mut err = std::io::stderr().lock();
            err.write_all(redact(&text).as_bytes())?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn init() {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_writer(|| LogWriter)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redacts_tokens() {
        assert_eq!(
            redact("x auc_abcdef123456 y aub_short"),
            "x [redacted] y aub_short"
        );
        assert_eq!(redact("Basic eDphdWNfMTIzNDU2Nzg="), "[redacted]");
    }
}
