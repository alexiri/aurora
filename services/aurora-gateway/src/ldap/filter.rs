//! RFC 4515 string filters, parsed strictly and encoded to BER. Every attribute a filter names
//! must be on the app's allow-list (a filter on an unlisted attribute is an oracle for it), the
//! size, depth and node counts are bounded, and extensible match is refused.
use super::ber::{SEQUENCE, concat, octets, tlv};

pub const MAX_FILTER_LEN: usize = 1024;
const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 64;
const MAX_VALUE: usize = 256;

#[derive(Debug, PartialEq, Eq)]
pub struct FilterError(pub &'static str);

struct P<'a> {
    s: &'a [u8],
    i: usize,
    nodes: usize,
    allowed: &'a [String],
}

/// Encodes `text` as the BER `Filter` of a search request.
///
/// # Errors
/// A malformed filter, one over the limits, one naming an attribute that is not allowed, or one
/// using extensible match.
pub fn encode(text: &str, allowed: &[String]) -> Result<Vec<u8>, FilterError> {
    if text.is_empty() || text.len() > MAX_FILTER_LEN {
        return Err(FilterError("the filter is empty or longer than 1024 bytes"));
    }
    if text.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(FilterError("the filter holds a control character"));
    }
    let mut p = P {
        s: text.as_bytes(),
        i: 0,
        nodes: 0,
        allowed,
    };
    let f = p.filter(0)?;
    if p.i != p.s.len() {
        return Err(FilterError("trailing text after the filter"));
    }
    Ok(f)
}

impl P<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> Result<(), FilterError> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(FilterError("the filter is not well formed"))
        }
    }

    fn filter(&mut self, depth: usize) -> Result<Vec<u8>, FilterError> {
        if depth > MAX_DEPTH {
            return Err(FilterError("the filter is nested too deeply"));
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(FilterError("the filter has too many terms"));
        }
        self.eat(b'(')?;
        let out = match self.peek() {
            Some(c @ (b'&' | b'|')) => {
                self.i += 1;
                let mut parts = Vec::new();
                while self.peek() == Some(b'(') {
                    parts.push(self.filter(depth + 1)?);
                }
                if parts.is_empty() {
                    return Err(FilterError("an and/or needs at least one term"));
                }
                tlv(if c == b'&' { 0xa0 } else { 0xa1 }, &concat(&parts))
            }
            Some(b'!') => {
                self.i += 1;
                tlv(0xa2, &self.filter(depth + 1)?)
            }
            _ => self.item()?,
        };
        self.eat(b')')?;
        Ok(out)
    }

    fn item(&mut self) -> Result<Vec<u8>, FilterError> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            self.i += 1;
        }
        let attr = std::str::from_utf8(&self.s[start..self.i]).unwrap_or_default();
        if attr.is_empty() || !attr.as_bytes()[0].is_ascii_alphabetic() {
            return Err(FilterError(
                "the filter names no attribute (extensible match is not supported)",
            ));
        }
        if !self.allowed.iter().any(|a| a.eq_ignore_ascii_case(attr)) {
            return Err(FilterError(
                "the filter names an attribute the app does not list",
            ));
        }
        let (tag, op_len) = match (self.peek(), self.s.get(self.i + 1).copied()) {
            (Some(b'>'), Some(b'=')) => (0xa5u8, 2),
            (Some(b'<'), Some(b'=')) => (0xa6, 2),
            (Some(b'~'), Some(b'=')) => (0xa8, 2),
            (Some(b'='), _) => (0xa3, 1),
            _ => return Err(FilterError("the filter is not well formed")),
        };
        self.i += op_len;
        // The value runs to the closing parenthesis; an unescaped `(` or `)` ends or breaks it.
        let vstart = self.i;
        while self.peek().is_some_and(|b| b != b')' && b != b'(') {
            self.i += 1;
        }
        let raw = &self.s[vstart..self.i];
        if raw.len() > MAX_VALUE * 3 {
            return Err(FilterError("a filter value is too long"));
        }
        let attr_oct = octets(attr.as_bytes());
        if tag == 0xa3 && raw == b"*" {
            return Ok(tlv(0x87, attr.as_bytes()));
        }
        if tag == 0xa3 && raw.contains(&b'*') {
            return Ok(tlv(0xa4, &concat(&[attr_oct, self.substrings(raw)?])));
        }
        let value = unescape(raw)?;
        Ok(tlv(tag, &concat(&[attr_oct, octets(&value)])))
    }

    fn substrings(&mut self, raw: &[u8]) -> Result<Vec<u8>, FilterError> {
        let pieces: Vec<&[u8]> = raw.split(|b| *b == b'*').collect();
        let last = pieces.len() - 1;
        let mut out = Vec::new();
        for (n, piece) in pieces.iter().enumerate() {
            if piece.is_empty() {
                continue;
            }
            let tag = if n == 0 {
                0x80
            } else if n == last {
                0x82
            } else {
                0x81
            };
            out.push(tlv(tag, &unescape(piece)?));
        }
        if out.is_empty() {
            return Err(FilterError("a substring filter needs some text"));
        }
        Ok(tlv(SEQUENCE, &concat(&out)))
    }
}

fn unescape(raw: &[u8]) -> Result<Vec<u8>, FilterError> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\\' {
            let hex = raw
                .get(i + 1..i + 3)
                .ok_or(FilterError("a bad escape in a filter value"))?;
            let s = std::str::from_utf8(hex)
                .map_err(|_| FilterError("a bad escape in a filter value"))?;
            out.push(
                u8::from_str_radix(s, 16)
                    .map_err(|_| FilterError("a bad escape in a filter value"))?,
            );
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    if out.len() > MAX_VALUE {
        return Err(FilterError("a filter value is too long"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::ber::{children, read};

    fn allowed() -> Vec<String> {
        ["uid", "mail", "displayName", "cn"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn the_forms_encode() {
        let a = allowed();
        let (t, _) = read(&encode("(uid=jdoe)", &a).unwrap())
            .map(|(t, r)| (t.tag, r))
            .unwrap();
        assert_eq!(t, 0xa3);
        assert_eq!(read(&encode("(mail=*)", &a).unwrap()).unwrap().0.tag, 0x87);
        assert_eq!(
            read(&encode("(cn=ab*cd*ef)", &a).unwrap()).unwrap().0.tag,
            0xa4
        );
        let or = encode("(|(displayName=jo*)(mail=jo*)(cn=jo*))", &a).unwrap();
        let (t, _) = read(&or).unwrap();
        assert_eq!((t.tag, children(t.content).unwrap().len()), (0xa1, 3));
        assert_eq!(
            read(&encode("(!(uid=x))", &a).unwrap()).unwrap().0.tag,
            0xa2
        );
        assert_eq!(read(&encode("(uid>=a)", &a).unwrap()).unwrap().0.tag, 0xa5);
        // Escapes decode to bytes: `\2a` is a literal star, not a wildcard.
        let e = encode(r"(cn=a\2ab)", &a).unwrap();
        let (t, _) = read(&e).unwrap();
        assert_eq!(t.tag, 0xa3);
        assert!(t.content.windows(3).any(|w| w == b"a*b"));
    }

    #[test]
    fn the_refusals() {
        let a = allowed();
        for (f, why) in [
            ("", "empty"),
            ("uid=jdoe", "no parentheses"),
            ("(uid=jdoe", "unclosed"),
            ("(uid=jdoe))", "trailing"),
            ("(&)", "empty and"),
            ("(userPassword=*)", "unlisted attribute"),
            ("(|(uid=a)(userPassword=b))", "unlisted attribute inside"),
            ("(:dn:=x)", "extensible"),
            ("(uid:caseExactMatch:=x)", "extensible with a rule"),
            (r"(uid=\zz)", "bad escape"),
            ("(uid=a(b)", "bare parenthesis"),
            ("(uid=**)", "stars only"),
            ("(uid=x)\n", "control character"),
        ] {
            assert!(encode(f, &a).is_err(), "{why}: {f}");
        }
        assert!(
            encode(&format!("(uid={})", "a".repeat(2000)), &a).is_err(),
            "long"
        );
        let deep = format!("{}(uid=a){}", "(!".repeat(20), ")".repeat(20));
        assert!(encode(&deep, &a).is_err(), "deep");
        let many = format!("(|{})", "(uid=a)".repeat(70));
        assert!(encode(&many, &a).is_err(), "many terms");
    }
}
