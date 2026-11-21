//! git pkt-line framing (gitprotocol-common): a 4-hex-digit length that counts itself, then the
//! payload; `0000` flush, `0001` delimiter, `0002` response end. Pure parsing of an in-memory
//! body; every malformed length is an error, never a panic.

/// One frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pkt<'a> {
    Flush,
    Delim,
    ResponseEnd,
    Data(&'a [u8]),
}

/// The longest frame git sends (65520 bytes including the 4 length bytes).
pub const MAX_FRAME: usize = 65520;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

/// Iterates the frames of `buf`; stops at its end. A truncated or invalid frame yields `Err`
/// once and then ends.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    failed: bool,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            pos: 0,
            failed: false,
        }
    }

    /// Bytes not consumed yet.
    #[must_use]
    pub fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos.min(self.buf.len())..]
    }

    fn hex4(b: &[u8]) -> Option<usize> {
        let s = std::str::from_utf8(b).ok()?;
        if s.len() != 4 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        usize::from_str_radix(s, 16).ok()
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = Result<Pkt<'a>, Malformed>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.pos >= self.buf.len() {
            return None;
        }
        let Some(head) = self.buf.get(self.pos..self.pos + 4) else {
            self.failed = true;
            return Some(Err(Malformed));
        };
        let Some(len) = Self::hex4(head) else {
            self.failed = true;
            return Some(Err(Malformed));
        };
        let pkt = match len {
            0 => Pkt::Flush,
            1 => Pkt::Delim,
            2 => Pkt::ResponseEnd,
            3 => {
                self.failed = true;
                return Some(Err(Malformed));
            }
            n if n > MAX_FRAME => {
                self.failed = true;
                return Some(Err(Malformed));
            }
            n => {
                let Some(data) = self.buf.get(self.pos + 4..self.pos + n) else {
                    self.failed = true;
                    return Some(Err(Malformed));
                };
                self.pos += n;
                return Some(Ok(Pkt::Data(data)));
            }
        };
        self.pos += 4;
        Some(Ok(pkt))
    }
}

/// Appends one data frame (`payload` at most `MAX_FRAME - 4` bytes; longer input is the caller's
/// bug: the requests we build are short text lines).
pub fn data(out: &mut Vec<u8>, payload: &[u8]) {
    let n = payload.len() + 4;
    debug_assert!(n <= MAX_FRAME);
    out.extend_from_slice(format!("{n:04x}").as_bytes());
    out.extend_from_slice(payload);
}

/// Appends a text line with its trailing `\n`.
pub fn line(out: &mut Vec<u8>, text: &str) {
    let mut p = Vec::with_capacity(text.len() + 1);
    p.extend_from_slice(text.as_bytes());
    p.push(b'\n');
    data(out, &p);
}

pub fn flush(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0000");
}

pub fn delim(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0001");
}

/// A frame's text without its trailing `\n`, when it is UTF-8.
#[must_use]
pub fn text(data: &[u8]) -> Option<&str> {
    let d = data.strip_suffix(b"\n").unwrap_or(data);
    std::str::from_utf8(d).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn all(b: &[u8]) -> Vec<Result<Pkt<'_>, Malformed>> {
        Reader::new(b).collect()
    }

    #[test]
    fn frames_flush_delim_and_end() {
        let mut b = Vec::new();
        line(&mut b, "hello");
        delim(&mut b);
        data(&mut b, b"\x01pack");
        flush(&mut b);
        b.extend_from_slice(b"0002");
        assert_eq!(
            all(&b),
            vec![
                Ok(Pkt::Data(b"hello\n")),
                Ok(Pkt::Delim),
                Ok(Pkt::Data(b"\x01pack")),
                Ok(Pkt::Flush),
                Ok(Pkt::ResponseEnd)
            ]
        );
        assert_eq!(text(b"abc\n"), Some("abc"));
    }

    #[test]
    fn bad_lengths_and_truncation_are_errors_not_panics() {
        for bad in [
            &b"00"[..],
            b"zzzz",
            b"0003",
            b"0009abc",
            b"ffffabc",
            b"fff0",
            b"+004",
            b"00 5x",
        ] {
            let v = all(bad);
            assert_eq!(v.last(), Some(&Err(Malformed)), "{bad:?}");
        }
        // after an error the reader ends
        let mut r = Reader::new(b"0003xxxx0000");
        assert_eq!(r.next(), Some(Err(Malformed)));
        assert_eq!(r.next(), None);
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_input_never_panics(b in proptest::collection::vec(proptest::num::u8::ANY, 0..300)) {
            let _ = all(&b);
        }
    }
}
