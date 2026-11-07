//! Minimal ZIP writer (method 0, "stored") for conversation exports. No ZIP64: an export is
//! bounded far below 4 GiB and 65 535 entries (`export_too_large` otherwise).

pub const MAX_ENTRIES: usize = 60_000;

struct Entry {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

#[derive(Default)]
pub struct ZipWriter {
    out: Vec<u8>,
    entries: Vec<Entry>,
}

impl ZipWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one file. Returns `false` (and adds nothing) when the archive would need ZIP64.
    pub fn add(&mut self, name: &str, data: &[u8]) -> bool {
        let (Ok(size), Ok(offset), Ok(nlen)) = (
            u32::try_from(data.len()),
            u32::try_from(self.out.len()),
            u16::try_from(name.len()),
        ) else {
            return false;
        };
        if self.entries.len() >= MAX_ENTRIES {
            return false;
        }
        let crc = crc32fast::hash(data);
        let o = &mut self.out;
        o.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        o.extend_from_slice(&20_u16.to_le_bytes()); // version needed
        o.extend_from_slice(&0x0800_u16.to_le_bytes()); // UTF-8 names
        o.extend_from_slice(&0_u16.to_le_bytes()); // stored
        o.extend_from_slice(&0_u16.to_le_bytes()); // time
        o.extend_from_slice(&0x0021_u16.to_le_bytes()); // date 1980-01-01
        o.extend_from_slice(&crc.to_le_bytes());
        o.extend_from_slice(&size.to_le_bytes());
        o.extend_from_slice(&size.to_le_bytes());
        o.extend_from_slice(&nlen.to_le_bytes());
        o.extend_from_slice(&0_u16.to_le_bytes()); // extra
        o.extend_from_slice(name.as_bytes());
        o.extend_from_slice(data);
        self.entries.push(Entry {
            name: name.to_owned(),
            crc,
            size,
            offset,
        });
        true
    }

    pub fn finish(mut self) -> Vec<u8> {
        let cd_start = u32::try_from(self.out.len()).unwrap_or(u32::MAX);
        let mut cd = Vec::new();
        for e in &self.entries {
            cd.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
            cd.extend_from_slice(&20_u16.to_le_bytes()); // made by
            cd.extend_from_slice(&20_u16.to_le_bytes()); // needed
            cd.extend_from_slice(&0x0800_u16.to_le_bytes());
            cd.extend_from_slice(&0_u16.to_le_bytes());
            cd.extend_from_slice(&0_u16.to_le_bytes());
            cd.extend_from_slice(&0x0021_u16.to_le_bytes());
            cd.extend_from_slice(&e.crc.to_le_bytes());
            cd.extend_from_slice(&e.size.to_le_bytes());
            cd.extend_from_slice(&e.size.to_le_bytes());
            cd.extend_from_slice(&u16::try_from(e.name.len()).unwrap_or(0).to_le_bytes());
            cd.extend_from_slice(&[0; 2 + 2 + 2 + 2 + 4]); // extra, comment, disk, int attr, ext attr
            cd.extend_from_slice(&e.offset.to_le_bytes());
            cd.extend_from_slice(e.name.as_bytes());
        }
        let n = u16::try_from(self.entries.len()).unwrap_or(u16::MAX);
        let cd_len = u32::try_from(cd.len()).unwrap_or(u32::MAX);
        self.out.extend_from_slice(&cd);
        self.out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
        self.out.extend_from_slice(&[0; 4]); // disk numbers
        self.out.extend_from_slice(&n.to_le_bytes());
        self.out.extend_from_slice(&n.to_le_bytes());
        self.out.extend_from_slice(&cd_len.to_le_bytes());
        self.out.extend_from_slice(&cd_start.to_le_bytes());
        self.out.extend_from_slice(&0_u16.to_le_bytes()); // comment
        self.out
    }
}

/// A file name safe to put inside the archive (no separators, bounded).
pub fn safe_name(s: &str) -> String {
    let n: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    let n = n.trim_start_matches('.');
    if n.is_empty() {
        "file".to_owned()
    } else {
        n.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_has_expected_structure() {
        let mut z = ZipWriter::new();
        assert!(z.add("a.txt", b"hello"));
        assert!(z.add("dir/b.bin", &[1, 2, 3]));
        let b = z.finish();
        assert_eq!(&b[..4], b"PK\x03\x04");
        let eocd = b.len() - 22;
        assert_eq!(&b[eocd..eocd + 4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([b[eocd + 10], b[eocd + 11]]), 2);
        assert!(b.windows(5).any(|w| w == b"hello"));
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name(""), "file");
        assert_eq!(safe_name("..."), "file");
    }
}
