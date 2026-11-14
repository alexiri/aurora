//! The small slice of BER (X.690) the LDAP lane needs: definite lengths, single-byte tags.
//! Encoding is limited to the three requests the lane can send (anonymous bind, search, unbind),
//! which is what makes the lane read-only by construction — no write operation can be expressed.

/// Largest single LDAP message the lane reads (a search page of entries).
pub const MAX_MESSAGE: usize = 4 << 20;

pub const SEQUENCE: u8 = 0x30;
pub const SET: u8 = 0x31;
pub const INTEGER: u8 = 0x02;
pub const OCTET_STRING: u8 = 0x04;
pub const ENUMERATED: u8 = 0x0a;
pub const BOOLEAN: u8 = 0x01;

pub const OP_BIND_REQUEST: u8 = 0x60;
pub const OP_BIND_RESPONSE: u8 = 0x61;
pub const OP_UNBIND_REQUEST: u8 = 0x42;
pub const OP_SEARCH_REQUEST: u8 = 0x63;
pub const OP_SEARCH_ENTRY: u8 = 0x64;
pub const OP_SEARCH_DONE: u8 = 0x65;
pub const OP_SEARCH_REFERENCE: u8 = 0x73;

#[must_use]
pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 6);
    out.push(tag);
    let n = content.len();
    if n < 0x80 {
        out.push(u8::try_from(n).unwrap_or(0));
    } else {
        let bytes = n.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        out.push(0x80 | u8::try_from(bytes.len() - skip).unwrap_or(0));
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(content);
    out
}

#[must_use]
pub fn int(tag: u8, v: i64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let mut s = 0;
    while s < 7
        && ((bytes[s] == 0 && bytes[s + 1] & 0x80 == 0)
            || (bytes[s] == 0xff && bytes[s + 1] & 0x80 != 0))
    {
        s += 1;
    }
    tlv(tag, &bytes[s..])
}

#[must_use]
pub fn octets(v: &[u8]) -> Vec<u8> {
    tlv(OCTET_STRING, v)
}

#[must_use]
pub fn concat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

/// One decoded element: the tag, its content and what follows it.
#[derive(Debug, PartialEq, Eq)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub content: &'a [u8],
}

/// Reads the element at the start of `buf`; returns it and the rest. `None` for anything that is
/// not a well-formed definite-length element with a single-byte tag.
#[must_use]
pub fn read(buf: &[u8]) -> Option<(Tlv<'_>, &[u8])> {
    let (&tag, rest) = buf.split_first()?;
    if tag & 0x1f == 0x1f {
        return None;
    }
    let (&l0, rest) = rest.split_first()?;
    let (len, rest) = if l0 < 0x80 {
        (usize::from(l0), rest)
    } else {
        let n = usize::from(l0 & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n]
            .iter()
            .fold(0usize, |a, b| (a << 8) | usize::from(*b));
        (len, &rest[n..])
    };
    if len > MAX_MESSAGE || rest.len() < len {
        return None;
    }
    Some((
        Tlv {
            tag,
            content: &rest[..len],
        },
        &rest[len..],
    ))
}

/// The elements of a constructed value, or `None` if it does not tile exactly.
#[must_use]
pub fn children(mut content: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let mut out = Vec::new();
    while !content.is_empty() {
        let (t, rest) = read(content)?;
        out.push(t);
        content = rest;
    }
    Some(out)
}

#[must_use]
pub fn as_int(t: &Tlv<'_>) -> Option<i64> {
    if t.content.is_empty() || t.content.len() > 8 {
        return None;
    }
    let mut v: i64 = if t.content[0] & 0x80 != 0 { -1 } else { 0 };
    for b in t.content {
        v = (v << 8) | i64::from(*b);
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_round_trip() {
        for n in [0usize, 1, 127, 128, 255, 256, 65_535, 70_000] {
            let c = vec![7u8; n];
            let e = tlv(OCTET_STRING, &c);
            let (t, rest) = read(&e).unwrap();
            assert_eq!((t.tag, t.content.len(), rest.len()), (OCTET_STRING, n, 0));
        }
    }

    #[test]
    fn integers_round_trip() {
        for v in [0i64, 1, 127, 128, 255, 256, -1, -128, -129, 65_535, 1 << 31] {
            let e = int(INTEGER, v);
            let (t, _) = read(&e).unwrap();
            assert_eq!(as_int(&t), Some(v), "{v}");
        }
    }

    #[test]
    fn malformed_input_is_refused() {
        assert!(read(&[]).is_none());
        assert!(read(&[0x04]).is_none());
        assert!(read(&[0x04, 0x05, 1, 2]).is_none(), "short content");
        assert!(read(&[0x04, 0x80]).is_none(), "indefinite length");
        assert!(
            read(&[0x04, 0x85, 0, 0, 0, 0, 1]).is_none(),
            "length of length 5"
        );
        assert!(read(&[0x1f, 0x01, 0x00]).is_none(), "high tag number");
        assert!(
            read(&[0x04, 0x84, 0xff, 0xff, 0xff, 0xff]).is_none(),
            "huge length"
        );
        assert!(
            children(&[0x04, 0x01, 0x61, 0x04]).is_none(),
            "trailing garbage"
        );
    }
}
