//! Log redaction: known secret values and credential-looking tokens are masked.

const MASK: &str = "[redacted]";

fn tokenish(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+' | '=' | '~')
}

/// Mask every literal occurrence of `secrets` (len >= 4) and any run of 20 or more
/// token characters, then cut to `max` bytes on a char boundary.
pub fn redact(msg: &str, secrets: &[String], max: usize) -> String {
    let mut s = msg.to_owned();
    for sec in secrets.iter().filter(|x| x.len() >= 4) {
        if s.contains(sec.as_str()) {
            s = s.replace(sec.as_str(), MASK);
        }
    }
    let mut out = String::with_capacity(s.len().min(max + 16));
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 20 {
            out.push_str(MASK);
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in s.chars() {
        if tokenish(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(if c.is_control() { ' ' } else { c });
        }
    }
    flush(&mut run, &mut out);
    truncate(&mut out, max);
    out
}

pub fn truncate(s: &mut String, max: usize) {
    if s.len() > max {
        let mut n = max;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn masks_secrets_and_tokens() {
        let r = redact(
            "token hunter22 and abcdefghijklmnopqrstuvwxyz0123 ok",
            &["hunter22".into()],
            1024,
        );
        assert_eq!(r, "token [redacted] and [redacted] ok");
    }
    #[test]
    fn truncates() {
        assert_eq!(redact("héllo", &[], 2).len(), 1);
    }
}
