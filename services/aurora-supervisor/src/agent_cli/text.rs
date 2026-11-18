//! Output hygiene for `aurora-agent`: everything printed is one line per item, free of control
//! characters, and bounded (models read it; `AGENT_CLI_PREVIEW_MAX`).
use aurora_core::limits::AGENT_CLI_PREVIEW_MAX;

/// `s` on one line (control characters and runs of whitespace become one space), at most `max`
/// characters, ending with `…` when cut.
#[must_use]
pub fn oneline(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in s.chars() {
        if c.is_control() || c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    cut(&out, max)
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
    t.push('…');
    t
}

/// The whole text bounded by [`AGENT_CLI_PREVIEW_MAX`] characters, cut at a line end when it can.
#[must_use]
pub fn bound(s: &str) -> String {
    bound_to(s, AGENT_CLI_PREVIEW_MAX)
}

#[must_use]
pub fn bound_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let keep: String = s.chars().take(max.saturating_sub(24)).collect();
    let cut_at = keep.rfind('\n').unwrap_or(keep.len());
    format!("{}\n… (output cut)\n", keep[..cut_at].trim_end())
}

/// Percent-encoding of one URL component: everything except RFC 3986 unreserved characters.
#[must_use]
pub fn pct(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            o.push(char::from(b));
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// `YYYYMMDDTHHMMSSZ` of the current UTC time (no clock crate: civil-from-days).
#[must_use]
pub fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    stamp_of(secs)
}

#[must_use]
pub fn stamp_of(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oneline_strips_controls_and_clamps() {
        assert_eq!(oneline("a\n\tb\u{1b}[31m  c", 50), "a b [31m c");
        assert_eq!(oneline("abcdef", 4), "abc…");
    }

    #[test]
    fn pct_encodes_everything_but_unreserved() {
        assert_eq!(pct("a b/c?d#e%"), "a%20b%2Fc%3Fd%23e%25");
        assert_eq!(pct("é"), "%C3%A9");
        assert_eq!(pct("Az09-._~"), "Az09-._~");
    }

    #[test]
    fn stamp_matches_known_instants() {
        assert_eq!(stamp_of(0), "19700101T000000Z");
        assert_eq!(stamp_of(1_700_000_000), "20231114T221320Z");
        assert_eq!(stamp_of(1_709_164_800 + 86_399), "20240229T235959Z");
    }

    #[test]
    fn bound_marks_the_cut() {
        let s = "line\n".repeat(1000);
        let b = bound(&s);
        assert!(b.chars().count() <= AGENT_CLI_PREVIEW_MAX);
        assert!(b.contains("output cut"));
    }
}
