//! In-memory packfile parser (gitformat-pack, version 2/3, SHA-1 only; P5-SKLIB-1, C-1125,
//! SEC-148). No file is written. Caps: inflated bytes of the whole pack, object count, delta
//! chain depth; every object's id is recomputed from its bytes (`<type> <size>\0<bytes>`), so
//! what the caller looks up by id is exactly what has that id. Malformed input is an error,
//! never a panic.

use std::collections::HashMap;

use flate2::{Decompress, FlushDecompress, Status};

pub type Oid = [u8; 20];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Tree => "tree",
            Self::Blob => "blob",
            Self::Tag => "tag",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub kind: Kind,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackError {
    /// The bytes are not a valid pack (fixed reason, safe to show).
    Malformed(&'static str),
    /// A cap of C-1125 was reached.
    TooLarge(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub inflate_max: usize,
    pub objects_max: usize,
    pub depth_max: usize,
}

impl Limits {
    #[must_use]
    pub fn standard() -> Self {
        Self {
            inflate_max: aurora_core::limits::LIBRARY_INFLATE_MAX,
            objects_max: aurora_core::limits::LIBRARY_TREE_MAX * 2,
            depth_max: 50,
        }
    }
}

/// SHA-1 of `<kind> <len>\0<data>`.
#[must_use]
pub fn object_id(kind: Kind, data: &[u8]) -> Oid {
    let mut c = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY);
    c.update(kind.as_str().as_bytes());
    c.update(format!(" {}\0", data.len()).as_bytes());
    c.update(data);
    let mut out = [0u8; 20];
    out.copy_from_slice(c.finish().as_ref());
    out
}

#[must_use]
pub fn hex(id: &Oid) -> String {
    ::hex::encode(id)
}

/// A 40-hex lowercase id.
#[must_use]
pub fn parse_hex(s: &str) -> Option<Oid> {
    if s.len() != 40 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0u8; 20];
    ::hex::decode_to_slice(s, &mut out).ok()?;
    Some(out)
}

#[derive(Clone, Copy)]
enum Base {
    Plain(Kind),
    Ofs(usize),
    Ref(Oid),
}

struct Entry {
    base: Base,
    data: Vec<u8>,
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn byte(&mut self) -> Result<u8, PackError> {
        let c = *self
            .b
            .get(self.pos)
            .ok_or(PackError::Malformed("the pack is truncated"))?;
        self.pos += 1;
        Ok(c)
    }
}

/// Inflates one zlib stream that must produce exactly `size` bytes; returns the bytes and how
/// many input bytes it used.
fn inflate(input: &[u8], size: usize) -> Result<(Vec<u8>, usize), PackError> {
    let bad = PackError::Malformed("a pack object does not inflate");
    let mut d = Decompress::new(true);
    // One byte of room past `size` so an over-long stream is noticed rather than cut.
    let mut out = Vec::with_capacity(size + 1);
    loop {
        let (before_in, before_out) = (d.total_in(), d.total_out());
        let consumed = usize::try_from(d.total_in()).map_err(|_| bad)?;
        let status = d
            .decompress_vec(
                &input[consumed.min(input.len())..],
                &mut out,
                FlushDecompress::None,
            )
            .map_err(|_| bad)?;
        if out.len() > size {
            return Err(bad);
        }
        match status {
            Status::StreamEnd => break,
            Status::Ok | Status::BufError => {
                if d.total_in() == before_in && d.total_out() == before_out {
                    return Err(bad);
                }
            }
        }
    }
    if out.len() != size {
        return Err(bad);
    }
    let used = usize::try_from(d.total_in()).map_err(|_| bad)?;
    Ok((out, used))
}

fn varint(c: &mut Cursor<'_>) -> Result<usize, PackError> {
    let mut v: u64 = 0;
    let mut shift = 0u32;
    loop {
        let b = c.byte()?;
        if shift > 56 {
            return Err(PackError::Malformed("a size in the pack is too large"));
        }
        v |= u64::from(b & 0x7f) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            break;
        }
    }
    usize::try_from(v).map_err(|_| PackError::Malformed("a size in the pack is too large"))
}

/// Applies a git delta to `base`; the result must be at most `budget` bytes.
pub fn apply_delta(base: &[u8], delta: &[u8], budget: usize) -> Result<Vec<u8>, PackError> {
    let bad = PackError::Malformed("a delta in the pack is invalid");
    let mut c = Cursor { b: delta, pos: 0 };
    let src = varint(&mut c).map_err(|_| bad)?;
    let dst = varint(&mut c).map_err(|_| bad)?;
    if src != base.len() {
        return Err(bad);
    }
    if dst > budget {
        return Err(PackError::TooLarge(
            "the repository's objects are too large to read",
        ));
    }
    let mut out = Vec::with_capacity(dst);
    while c.pos < delta.len() {
        let op = c.byte()?;
        if op & 0x80 != 0 {
            let mut off: usize = 0;
            for i in 0..4 {
                if op & (1 << i) != 0 {
                    off |= usize::from(c.byte().map_err(|_| bad)?) << (8 * i);
                }
            }
            let mut len: usize = 0;
            for i in 0..3 {
                if op & (0x10 << i) != 0 {
                    len |= usize::from(c.byte().map_err(|_| bad)?) << (8 * i);
                }
            }
            if len == 0 {
                len = 0x10000;
            }
            let end = off.checked_add(len).ok_or(bad)?;
            let part = base.get(off..end).ok_or(bad)?;
            if out.len() + part.len() > dst {
                return Err(bad);
            }
            out.extend_from_slice(part);
        } else if op != 0 {
            let n = usize::from(op);
            let part = delta.get(c.pos..c.pos + n).ok_or(bad)?;
            c.pos += n;
            if out.len() + part.len() > dst {
                return Err(bad);
            }
            out.extend_from_slice(part);
        } else {
            return Err(bad);
        }
    }
    if out.len() != dst {
        return Err(bad);
    }
    Ok(out)
}

/// Parses a whole pack: every object by its recomputed id.
pub fn parse(pack: &[u8], limits: Limits) -> Result<HashMap<Oid, Object>, PackError> {
    if pack.len() < 32 || &pack[..4] != b"PACK" {
        return Err(PackError::Malformed("the answer is not a git pack"));
    }
    let version = u32::from_be_bytes([pack[4], pack[5], pack[6], pack[7]]);
    if version != 2 && version != 3 {
        return Err(PackError::Malformed("unsupported pack version"));
    }
    let count = u32::from_be_bytes([pack[8], pack[9], pack[10], pack[11]]) as usize;
    if count > limits.objects_max {
        return Err(PackError::TooLarge("the repository sent too many objects"));
    }
    let body_end = pack.len() - 20;
    let mut sum = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY);
    sum.update(&pack[..body_end]);
    if sum.finish().as_ref() != &pack[body_end..] {
        return Err(PackError::Malformed("the pack checksum does not match"));
    }
    let body = &pack[..body_end];
    let mut c = Cursor { b: body, pos: 12 };
    let mut budget = limits.inflate_max;
    let mut entries: Vec<Entry> = Vec::with_capacity(count.min(4096));
    let mut by_offset: HashMap<usize, usize> = HashMap::new();
    for _ in 0..count {
        let offset = c.pos;
        let mut b = c.byte()?;
        let t = (b >> 4) & 7;
        let mut size: u64 = u64::from(b & 0x0f);
        let mut shift = 4u32;
        while b & 0x80 != 0 {
            b = c.byte()?;
            if shift > 56 {
                return Err(PackError::Malformed("a size in the pack is too large"));
            }
            size |= u64::from(b & 0x7f) << shift;
            shift += 7;
        }
        let size = usize::try_from(size)
            .map_err(|_| PackError::TooLarge("the repository's objects are too large to read"))?;
        let base = match t {
            1 => Base::Plain(Kind::Commit),
            2 => Base::Plain(Kind::Tree),
            3 => Base::Plain(Kind::Blob),
            4 => Base::Plain(Kind::Tag),
            6 => {
                let mut b = c.byte()?;
                let mut ofs = usize::from(b & 0x7f);
                while b & 0x80 != 0 {
                    b = c.byte()?;
                    ofs = ofs
                        .checked_add(1)
                        .and_then(|o| o.checked_mul(128))
                        .and_then(|o| o.checked_add(usize::from(b & 0x7f)))
                        .ok_or(PackError::Malformed("a delta offset is out of range"))?;
                }
                if ofs == 0 || ofs > offset {
                    return Err(PackError::Malformed("a delta offset is out of range"));
                }
                Base::Ofs(offset - ofs)
            }
            7 => {
                let id = body
                    .get(c.pos..c.pos + 20)
                    .ok_or(PackError::Malformed("the pack is truncated"))?;
                c.pos += 20;
                let mut oid = [0u8; 20];
                oid.copy_from_slice(id);
                Base::Ref(oid)
            }
            _ => return Err(PackError::Malformed("unknown pack object type")),
        };
        if size > budget {
            return Err(PackError::TooLarge(
                "the repository's objects are too large to read",
            ));
        }
        let (data, used) = inflate(&body[c.pos..], size)?;
        budget -= size;
        c.pos += used;
        by_offset.insert(offset, entries.len());
        entries.push(Entry { base, data });
    }
    if c.pos != body.len() {
        return Err(PackError::Malformed("the pack has trailing bytes"));
    }
    // Resolve: plain objects at once; deltas when their base is known, at most `depth_max`
    // deltas deep. Ofs bases precede their delta, so one pass in order resolves those; a ref
    // base may come later, hence the bounded repeat.
    let n = entries.len();
    // Per entry: (kind, delta depth, id) once resolved, and its content.
    let mut resolved: Vec<Option<(Kind, usize, Oid)>> = vec![None; n];
    let mut content: Vec<Vec<u8>> = vec![Vec::new(); n];
    let mut by_id: HashMap<Oid, usize> = HashMap::new();
    let mut left = n;
    for _ in 0..=limits.depth_max + 1 {
        let before = left;
        for i in 0..n {
            if resolved[i].is_some() {
                continue;
            }
            let base = entries[i].base;
            let base_idx = match base {
                Base::Plain(k) => {
                    let data = std::mem::take(&mut entries[i].data);
                    let id = object_id(k, &data);
                    by_id.insert(id, i);
                    content[i] = data;
                    resolved[i] = Some((k, 0, id));
                    left -= 1;
                    continue;
                }
                Base::Ofs(o) => *by_offset
                    .get(&o)
                    .ok_or(PackError::Malformed("a delta offset is out of range"))?,
                Base::Ref(id) => match by_id.get(&id) {
                    Some(&j) => j,
                    None => continue,
                },
            };
            let Some((kind, depth, _)) = resolved[base_idx] else {
                continue;
            };
            if depth + 1 > limits.depth_max {
                return Err(PackError::Malformed(
                    "a delta chain in the pack is too deep",
                ));
            }
            let out = apply_delta(&content[base_idx], &entries[i].data, budget)?;
            budget -= out.len();
            entries[i].data = Vec::new();
            let id = object_id(kind, &out);
            by_id.insert(id, i);
            content[i] = out;
            resolved[i] = Some((kind, depth + 1, id));
            left -= 1;
        }
        if left == 0 || left == before {
            break;
        }
    }
    if left != 0 {
        return Err(PackError::Malformed(
            "a delta base is missing from the pack",
        ));
    }
    let mut out = HashMap::with_capacity(n);
    for (i, data) in content.into_iter().enumerate() {
        if let Some((kind, _, id)) = resolved[i] {
            out.insert(id, Object { kind, data });
        }
    }
    Ok(out)
}

// ---- object contents ---------------------------------------------------------------------------

/// One entry of a tree object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTreeEntry {
    /// The octal mode as git writes it (`100644`, `100755`, `120000`, `40000`, `160000`).
    pub mode: String,
    pub name: String,
    pub id: Oid,
}

/// Parses a tree object. A name that is not UTF-8, empty, `.`/`..` or contains `/` is an error.
pub fn tree_entries(data: &[u8]) -> Result<Vec<RawTreeEntry>, PackError> {
    let bad = PackError::Malformed("a tree in the pack is invalid");
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let sp = data[pos..].iter().position(|&b| b == b' ').ok_or(bad)? + pos;
        let mode = std::str::from_utf8(&data[pos..sp]).map_err(|_| bad)?;
        if mode.is_empty() || mode.len() > 6 || !mode.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            return Err(bad);
        }
        let nul = data[sp + 1..].iter().position(|&b| b == 0).ok_or(bad)? + sp + 1;
        let name = std::str::from_utf8(&data[sp + 1..nul]).map_err(|_| bad)?;
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(bad);
        }
        let id = data.get(nul + 1..nul + 21).ok_or(bad)?;
        let mut oid = [0u8; 20];
        oid.copy_from_slice(id);
        out.push(RawTreeEntry {
            mode: mode.to_owned(),
            name: name.to_owned(),
            id: oid,
        });
        pos = nul + 21;
    }
    Ok(out)
}

/// The root tree of a commit object (its first header line).
pub fn commit_tree(data: &[u8]) -> Result<Oid, PackError> {
    let bad = PackError::Malformed("a commit in the pack is invalid");
    let first = data.split(|&b| b == b'\n').next().ok_or(bad)?;
    let hex = first.strip_prefix(b"tree ").ok_or(bad)?;
    parse_hex(std::str::from_utf8(hex).map_err(|_| bad)?).ok_or(bad)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write as _;

    fn z(data: &[u8]) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn header(t: u8, mut size: usize) -> Vec<u8> {
        let mut b = (t << 4) | (size & 0x0f) as u8;
        size >>= 4;
        let mut out = Vec::new();
        while size > 0 {
            out.push(b | 0x80);
            b = (size & 0x7f) as u8;
            size >>= 7;
        }
        out.push(b);
        out
    }

    fn ofs(mut o: usize) -> Vec<u8> {
        let mut bytes = vec![(o & 0x7f) as u8];
        o >>= 7;
        while o > 0 {
            o -= 1;
            bytes.push(0x80 | (o & 0x7f) as u8);
            o >>= 7;
        }
        bytes.reverse();
        bytes
    }

    pub(crate) enum E {
        Plain(Kind, Vec<u8>),
        /// Delta against the entry `n` entries back (ofs-delta).
        Ofs(usize, Vec<u8>, usize),
        Ref(Oid, Vec<u8>, usize),
        /// Raw entry bytes (malformed input).
        Raw(Vec<u8>),
    }

    /// Builds a pack (with its trailing checksum).
    pub(crate) fn build(entries: &[E]) -> Vec<u8> {
        let mut p = b"PACK".to_vec();
        p.extend_from_slice(&2u32.to_be_bytes());
        p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        let mut offsets = Vec::new();
        for e in entries {
            offsets.push(p.len());
            match e {
                E::Plain(k, d) => {
                    let t = match k {
                        Kind::Commit => 1,
                        Kind::Tree => 2,
                        Kind::Blob => 3,
                        Kind::Tag => 4,
                    };
                    p.extend(header(t, d.len()));
                    p.extend(z(d));
                }
                E::Ofs(back, delta, _result_len) => {
                    // `back` 0 is the entry before this one (`offsets` already holds this one's).
                    let here = p.len();
                    let base = offsets[offsets.len() - 2 - back];
                    // The size in a delta's header is that of the delta data, as in git's own packs.
                    p.extend(header(6, delta.len()));
                    p.extend(ofs(here - base));
                    p.extend(z(delta));
                }
                E::Ref(id, delta, _result_len) => {
                    p.extend(header(7, delta.len()));
                    p.extend_from_slice(id);
                    p.extend(z(delta));
                }
                E::Raw(b) => p.extend_from_slice(b),
            }
        }
        let sum = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY, &p);
        p.extend_from_slice(sum.as_ref());
        p
    }

    fn vint(mut n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
        out
    }

    /// A delta that copies `base[..keep]` and appends `tail`.
    pub(crate) fn delta(base_len: usize, keep: usize, tail: &[u8]) -> (Vec<u8>, usize) {
        let mut d = vint(base_len);
        let out_len = keep + tail.len();
        d.extend(vint(out_len));
        if keep > 0 {
            d.push(0x80 | 0x01 | 0x02 | 0x10 | 0x20);
            d.push(0);
            d.push(0);
            d.push((keep & 0xff) as u8);
            d.push(((keep >> 8) & 0xff) as u8);
        }
        for chunk in tail.chunks(127) {
            d.push(chunk.len() as u8);
            d.extend_from_slice(chunk);
        }
        (d, out_len)
    }

    #[test]
    fn plain_and_delta_objects_get_their_recomputed_ids() {
        let base = b"hello world, this is the base".to_vec();
        let (d1, n1) = delta(base.len(), 11, b"!");
        let (d2, n2) = delta(n1, 5, b" again");
        let base_id = object_id(Kind::Blob, &base);
        let (d3, n3) = delta(base.len(), 5, b"-ref");
        let p = build(&[
            E::Plain(Kind::Blob, base.clone()),
            E::Ofs(0, d1, n1),
            E::Ofs(0, d2, n2),
            E::Ref(base_id, d3, n3),
        ]);
        let m = parse(&p, Limits::standard()).unwrap();
        assert_eq!(m.len(), 4);
        assert_eq!(m[&base_id].data, base);
        assert_eq!(
            m[&object_id(Kind::Blob, b"hello world!")].data,
            b"hello world!"
        );
        assert_eq!(m[&object_id(Kind::Blob, b"hello again")].kind, Kind::Blob);
        assert!(m.contains_key(&object_id(Kind::Blob, b"hello-ref")));
        // git's own id of the empty blob
        assert_eq!(
            hex(&object_id(Kind::Blob, b"")),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
    }

    #[test]
    fn a_ref_delta_before_its_base_resolves() {
        let base = b"base content".to_vec();
        let (d, n) = delta(base.len(), 4, b"!");
        let p = build(&[
            E::Ref(object_id(Kind::Blob, &base), d, n),
            E::Plain(Kind::Blob, base),
        ]);
        let m = parse(&p, Limits::standard()).unwrap();
        assert!(m.contains_key(&object_id(Kind::Blob, b"base!")));
    }

    #[test]
    fn malformed_packs_are_errors() {
        let ok = build(&[E::Plain(Kind::Blob, b"x".to_vec())]);
        // truncated at every length
        for n in 0..ok.len() {
            assert!(parse(&ok[..n], Limits::standard()).is_err(), "{n}");
        }
        // wrong checksum
        let mut bad = ok.clone();
        let l = bad.len();
        bad[l - 1] ^= 1;
        assert!(matches!(
            parse(&bad, Limits::standard()),
            Err(PackError::Malformed(_))
        ));
        // bad zlib
        let mut raw = header(3, 5);
        raw.extend_from_slice(b"notzlib!");
        assert!(parse(&build(&[E::Raw(raw)]), Limits::standard()).is_err());
        // declared size differs from the inflated size
        let mut raw = header(3, 4);
        raw.extend(z(b"12345"));
        assert!(parse(&build(&[E::Raw(raw)]), Limits::standard()).is_err());
        // unknown type 5
        let mut raw = header(5, 1);
        raw.extend(z(b"x"));
        assert!(parse(&build(&[E::Raw(raw)]), Limits::standard()).is_err());
        // ofs-delta pointing before the pack / at itself
        let mut raw = header(6, 1);
        raw.extend(ofs(100));
        raw.extend(z(b"\x01\x01\x01x"));
        assert!(parse(&build(&[E::Raw(raw)]), Limits::standard()).is_err());
        // ref-delta whose base is not in the pack (no thin packs)
        let (d, n) = delta(1, 1, b"");
        assert!(matches!(
            parse(&build(&[E::Ref([7; 20], d, n)]), Limits::standard()),
            Err(PackError::Malformed(_))
        ));
        // a delta copying outside its base
        let mut d = vint(1);
        d.extend(vint(10));
        d.extend([0x80 | 0x01 | 0x10, 5, 10]);
        let p = build(&[E::Plain(Kind::Blob, b"x".to_vec()), E::Ofs(0, d, 10)]);
        assert!(parse(&p, Limits::standard()).is_err());
        // a delta with the reserved opcode 0
        let mut d = vint(1);
        d.extend(vint(1));
        d.push(0);
        let p = build(&[E::Plain(Kind::Blob, b"x".to_vec()), E::Ofs(0, d, 1)]);
        assert!(parse(&p, Limits::standard()).is_err());
        // a delta whose source size is not its base's
        let (d, n) = delta(2, 1, b"");
        let p = build(&[E::Plain(Kind::Blob, b"x".to_vec()), E::Ofs(0, d, n)]);
        assert!(parse(&p, Limits::standard()).is_err());
        // not a pack / wrong version
        assert!(
            parse(
                b"NOPE0000000000000000000000000000000000",
                Limits::standard()
            )
            .is_err()
        );
        let mut v9 = ok.clone();
        v9[7] = 9;
        assert!(parse(&v9, Limits::standard()).is_err());
    }

    #[test]
    fn caps_on_objects_inflation_and_chain_depth() {
        let few = Limits {
            inflate_max: 1 << 20,
            objects_max: 2,
            depth_max: 50,
        };
        let three = build(&[
            E::Plain(Kind::Blob, b"a".to_vec()),
            E::Plain(Kind::Blob, b"b".to_vec()),
            E::Plain(Kind::Blob, b"c".to_vec()),
        ]);
        assert_eq!(
            parse(&three, few),
            Err(PackError::TooLarge("the repository sent too many objects"))
        );
        let small = Limits {
            inflate_max: 1000,
            objects_max: 10,
            depth_max: 50,
        };
        let big = build(&[E::Plain(Kind::Blob, vec![b'z'; 1001])]);
        assert!(matches!(parse(&big, small), Err(PackError::TooLarge(_))));
        // a declared size far beyond the budget is refused before allocating
        let mut raw = header(3, 1 << 40);
        raw.extend(z(b"x"));
        assert!(matches!(
            parse(&build(&[E::Raw(raw)]), Limits::standard()),
            Err(PackError::TooLarge(_))
        ));
        // a delta result beyond the budget
        let (d, n) = delta(1, 1, &[b'q'; 2000]);
        let p = build(&[E::Plain(Kind::Blob, b"x".to_vec()), E::Ofs(0, d, n)]);
        assert!(matches!(parse(&p, small), Err(PackError::TooLarge(_))));
        // chain depth: 50 deltas resolve, 51 do not
        let chain = |len: usize| {
            let mut es = vec![E::Plain(Kind::Blob, b"0".to_vec())];
            let mut cur = 1;
            for _ in 0..len {
                let (d, n) = delta(cur, cur, b"1");
                es.push(E::Ofs(0, d, n));
                cur = n;
            }
            build(&es)
        };
        assert!(parse(&chain(50), Limits::standard()).is_ok());
        assert_eq!(
            parse(&chain(51), Limits::standard()),
            Err(PackError::Malformed(
                "a delta chain in the pack is too deep"
            ))
        );
    }

    #[test]
    fn trees_and_commits() {
        let id = object_id(Kind::Blob, b"x");
        let mut t = b"100644 SKILL.md\0".to_vec();
        t.extend_from_slice(&id);
        t.extend_from_slice(b"40000 sub\0");
        t.extend_from_slice(&id);
        let e = tree_entries(&t).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(
            (e[0].mode.as_str(), e[0].name.as_str()),
            ("100644", "SKILL.md")
        );
        assert_eq!(e[1].mode, "40000");
        for bad in [
            &b"100644 a"[..],
            b"100644 a\0short",
            b"10x644 a\0aaaaaaaaaaaaaaaaaaaa",
            b"100644 ..\0aaaaaaaaaaaaaaaaaaaa",
            b"100644 a/b\0aaaaaaaaaaaaaaaaaaaa",
            b"100644 \0aaaaaaaaaaaaaaaaaaaa",
        ] {
            assert!(tree_entries(bad).is_err(), "{bad:?}");
        }
        let c = format!("tree {}\nparent x\n\nmsg", hex(&id));
        assert_eq!(commit_tree(c.as_bytes()).unwrap(), id);
        assert!(commit_tree(b"parent x\n").is_err());
        assert!(commit_tree(b"tree XYZ\n").is_err());
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_packs_never_panic(b in proptest::collection::vec(proptest::num::u8::ANY, 0..400)) {
            let mut p = b"PACK\0\0\0\x02\0\0\0\x03".to_vec();
            p.extend_from_slice(&b);
            let sum = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY, &p);
            p.extend_from_slice(sum.as_ref());
            let _ = parse(&p, Limits::standard());
        }

        #[test]
        fn arbitrary_deltas_never_panic(base in proptest::collection::vec(proptest::num::u8::ANY, 0..64),
                                        d in proptest::collection::vec(proptest::num::u8::ANY, 0..128)) {
            let _ = apply_delta(&base, &d, 1 << 16);
        }

        #[test]
        fn arbitrary_trees_never_panic(b in proptest::collection::vec(proptest::num::u8::ANY, 0..200)) {
            let _ = tree_entries(&b);
        }
    }
}
