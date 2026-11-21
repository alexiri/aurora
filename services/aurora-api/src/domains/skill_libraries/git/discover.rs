//! The text side of git smart HTTP (gitprotocol-http, gitprotocol-v2, gitprotocol-pack): the
//! discovery answer (a v2 capability advertisement or a v0/v1 ref advertisement), the v2
//! `ls-refs` answer, the requests we send, and the side-band demultiplexing of a fetch answer.
//! Pure functions over in-memory bodies (P5-SKLIB-1, C-1125).

use std::collections::HashMap;

use super::pack::{Oid, parse_hex};
use super::pktline::{self, Pkt};

/// Why an answer could not be used. `Refused` carries no server text (an `ERR` line, e.g.
/// "not our ref"): callers decide what it means (blob wants refused → whole pack).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoError {
    Malformed,
    Refused,
    Sha256,
}

/// What the server can do for a fetch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchCaps {
    pub filter: bool,
    pub shallow: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    V2 { caps: FetchCaps, ls_refs: bool },
    V0(Advertisement),
}

/// A v0/v1 ref advertisement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Advertisement {
    /// Ref name → id (for an annotated tag, the tag object's id).
    pub refs: HashMap<String, Oid>,
    /// Ref name → peeled id (`<name>^{}` lines).
    pub peeled: HashMap<String, Oid>,
    /// `symref=HEAD:<target>`.
    pub head_target: Option<String>,
    pub caps: FetchCaps,
}

/// One ref of an `ls-refs` answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLine {
    pub name: String,
    pub id: Oid,
    pub symref_target: Option<String>,
    pub peeled: Option<Oid>,
}

fn id(s: &str) -> Result<Oid, ProtoError> {
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ProtoError::Sha256);
    }
    parse_hex(s).ok_or(ProtoError::Malformed)
}

fn frames(body: &[u8]) -> Result<Vec<Pkt<'_>>, ProtoError> {
    pktline::Reader::new(body)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ProtoError::Malformed)
}

fn is_err_line(d: &[u8]) -> bool {
    d.starts_with(b"ERR ")
}

/// Parses `GET …/info/refs?service=git-upload-pack`.
pub fn parse_discovery(body: &[u8]) -> Result<Discovery, ProtoError> {
    let pkts = frames(body)?;
    let mut i = 0;
    // `# service=git-upload-pack` then a flush (smart HTTP); tolerate its absence.
    if let Some(Pkt::Data(d)) = pkts.first()
        && d.starts_with(b"# service=")
    {
        i = 1;
        if matches!(pkts.get(1), Some(Pkt::Flush)) {
            i = 2;
        }
    }
    let rest = &pkts[i..];
    match rest.first() {
        Some(Pkt::Data(d)) if pktline::text(d) == Some("version 2") => {
            let mut caps = FetchCaps::default();
            let mut ls_refs = false;
            let mut fetch = false;
            for p in &rest[1..] {
                match p {
                    Pkt::Data(d) => {
                        let t = pktline::text(d).ok_or(ProtoError::Malformed)?;
                        let (k, v) = t.split_once('=').unwrap_or((t, ""));
                        match k {
                            "ls-refs" => ls_refs = true,
                            "fetch" => {
                                fetch = true;
                                for f in v.split(' ') {
                                    match f {
                                        "filter" => caps.filter = true,
                                        "shallow" => caps.shallow = true,
                                        _ => {}
                                    }
                                }
                            }
                            "object-format" if v != "sha1" => return Err(ProtoError::Sha256),
                            _ => {}
                        }
                    }
                    Pkt::Flush => break,
                    _ => return Err(ProtoError::Malformed),
                }
            }
            if !fetch {
                return Err(ProtoError::Malformed);
            }
            Ok(Discovery::V2 { caps, ls_refs })
        }
        Some(Pkt::Data(d)) if pktline::text(d) == Some("version 1") => {
            parse_v0(&rest[1..]).map(Discovery::V0)
        }
        Some(Pkt::Data(d)) if is_err_line(d) => Err(ProtoError::Refused),
        _ => parse_v0(rest).map(Discovery::V0),
    }
}

fn parse_v0(pkts: &[Pkt<'_>]) -> Result<Advertisement, ProtoError> {
    let mut adv = Advertisement::default();
    let mut first = true;
    for p in pkts {
        let d = match p {
            Pkt::Data(d) => *d,
            Pkt::Flush => break,
            _ => return Err(ProtoError::Malformed),
        };
        if is_err_line(d) {
            return Err(ProtoError::Refused);
        }
        let d = d.strip_suffix(b"\n").unwrap_or(d);
        let (line, caps) = match d.iter().position(|&b| b == 0) {
            Some(n) if first => (&d[..n], Some(&d[n + 1..])),
            Some(_) => return Err(ProtoError::Malformed),
            None => (d, None),
        };
        if let Some(caps) = caps {
            let caps = std::str::from_utf8(caps).map_err(|_| ProtoError::Malformed)?;
            for c in caps.split(' ') {
                match c.split_once('=') {
                    Some(("symref", v)) => {
                        if let Some(t) = v.strip_prefix("HEAD:") {
                            adv.head_target = Some(t.to_owned());
                        }
                    }
                    Some(("object-format", v)) if v != "sha1" => return Err(ProtoError::Sha256),
                    None if c == "filter" => adv.caps.filter = true,
                    None if c == "shallow" => adv.caps.shallow = true,
                    _ => {}
                }
            }
        }
        first = false;
        let line = std::str::from_utf8(line).map_err(|_| ProtoError::Malformed)?;
        let (hex, name) = line.split_once(' ').ok_or(ProtoError::Malformed)?;
        let oid = id(hex)?;
        if name == "capabilities^{}" {
            continue;
        }
        if let Some(base) = name.strip_suffix("^{}") {
            adv.peeled.insert(base.to_owned(), oid);
        } else {
            adv.refs.insert(name.to_owned(), oid);
        }
    }
    Ok(adv)
}

/// The v2 `ls-refs` request for these prefixes (`symrefs`, `peel`).
#[must_use]
pub fn ls_refs_request(agent: &str, prefixes: &[String]) -> Vec<u8> {
    let mut b = Vec::new();
    pktline::line(&mut b, "command=ls-refs");
    pktline::line(&mut b, &format!("agent={agent}"));
    pktline::delim(&mut b);
    pktline::line(&mut b, "peel");
    pktline::line(&mut b, "symrefs");
    for p in prefixes {
        pktline::line(&mut b, &format!("ref-prefix {p}"));
    }
    pktline::flush(&mut b);
    b
}

/// Parses an `ls-refs` answer.
pub fn parse_ls_refs(body: &[u8]) -> Result<Vec<RefLine>, ProtoError> {
    let mut out = Vec::new();
    for p in frames(body)? {
        let d = match p {
            Pkt::Data(d) => d,
            Pkt::Flush => break,
            _ => return Err(ProtoError::Malformed),
        };
        if is_err_line(d) {
            return Err(ProtoError::Refused);
        }
        let t = pktline::text(d).ok_or(ProtoError::Malformed)?;
        let mut parts = t.split(' ');
        let hex = parts.next().ok_or(ProtoError::Malformed)?;
        let name = parts.next().ok_or(ProtoError::Malformed)?;
        // An unborn HEAD is `unborn HEAD symref-target:…`: nothing to read.
        if hex == "unborn" {
            continue;
        }
        let mut r = RefLine {
            name: name.to_owned(),
            id: id(hex)?,
            symref_target: None,
            peeled: None,
        };
        for a in parts {
            if let Some(t) = a.strip_prefix("symref-target:") {
                r.symref_target = Some(t.to_owned());
            } else if let Some(p) = a.strip_prefix("peeled:") {
                r.peeled = Some(id(p)?);
            }
        }
        out.push(r);
    }
    Ok(out)
}

/// What one fetch asks for.
pub struct FetchAsk<'a> {
    pub wants: &'a [String],
    /// `deepen 1` (only when the server can do shallow fetches).
    pub deepen: bool,
    /// `filter blob:none` (only when advertised).
    pub filter: bool,
}

/// A v2 `fetch` request.
#[must_use]
pub fn fetch_v2(agent: &str, a: &FetchAsk<'_>) -> Vec<u8> {
    let mut b = Vec::new();
    pktline::line(&mut b, "command=fetch");
    pktline::line(&mut b, &format!("agent={agent}"));
    pktline::delim(&mut b);
    pktline::line(&mut b, "no-progress");
    pktline::line(&mut b, "ofs-delta");
    if a.deepen {
        pktline::line(&mut b, "deepen 1");
    }
    if a.filter {
        pktline::line(&mut b, "filter blob:none");
    }
    for w in a.wants {
        pktline::line(&mut b, &format!("want {w}"));
    }
    pktline::line(&mut b, "done");
    pktline::flush(&mut b);
    b
}

/// A v0 upload-pack request (stateless: wants, shallow options, flush, `done`).
#[must_use]
pub fn fetch_v0(agent: &str, a: &FetchAsk<'_>) -> Vec<u8> {
    let mut b = Vec::new();
    let mut caps = format!("side-band-64k ofs-delta no-progress agent={agent}");
    if a.deepen {
        caps.push_str(" shallow");
    }
    if a.filter {
        caps.push_str(" filter");
    }
    for (i, w) in a.wants.iter().enumerate() {
        if i == 0 {
            pktline::line(&mut b, &format!("want {w} {caps}"));
        } else {
            pktline::line(&mut b, &format!("want {w}"));
        }
    }
    if a.deepen {
        pktline::line(&mut b, "deepen 1");
    }
    if a.filter {
        pktline::line(&mut b, "filter blob:none");
    }
    pktline::flush(&mut b);
    pktline::line(&mut b, "done");
    b
}

/// The pack bytes of a fetch answer (v2 sections or v0 lines, then side-band band 1). Band 2
/// (progress) is dropped, band 3 and `ERR` are a refusal.
pub fn demux_pack(body: &[u8]) -> Result<Vec<u8>, ProtoError> {
    let mut pack = Vec::new();
    for p in pktline::Reader::new(body) {
        let Pkt::Data(d) = p.map_err(|_| ProtoError::Malformed)? else {
            continue;
        };
        if is_err_line(d) {
            return Err(ProtoError::Refused);
        }
        match d.first() {
            Some(1) => pack.extend_from_slice(&d[1..]),
            Some(2) => {}
            Some(3) => return Err(ProtoError::Refused),
            // Section headers and `shallow …`, `NAK`, `ACK …` lines.
            _ => {}
        }
    }
    if pack.is_empty() {
        return Err(ProtoError::Malformed);
    }
    Ok(pack)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef01234567";
    const B: &str = "89abcdef0123456789abcdef0123456789abcdef";

    fn pkts(lines: &[&[u8]]) -> Vec<u8> {
        let mut b = Vec::new();
        for l in lines {
            match *l {
                b"FLUSH" => pktline::flush(&mut b),
                b"DELIM" => pktline::delim(&mut b),
                d => pktline::data(&mut b, d),
            }
        }
        b
    }

    #[test]
    fn v2_capabilities() {
        let b = pkts(&[
            b"# service=git-upload-pack\n",
            b"FLUSH",
            b"version 2\n",
            b"agent=git/2.45\n",
            b"ls-refs=unborn\n",
            b"fetch=shallow wait-for-done filter\n",
            b"server-option\n",
            b"object-format=sha1\n",
            b"FLUSH",
        ]);
        assert_eq!(
            parse_discovery(&b).unwrap(),
            Discovery::V2 {
                caps: FetchCaps {
                    filter: true,
                    shallow: true
                },
                ls_refs: true
            }
        );
        let b = pkts(&[b"version 2\n", b"fetch=shallow\n", b"FLUSH"]);
        assert!(matches!(
            parse_discovery(&b).unwrap(),
            Discovery::V2 {
                caps: FetchCaps { filter: false, .. },
                ..
            }
        ));
        let b = pkts(&[
            b"version 2\n",
            b"fetch\n",
            b"object-format=sha256\n",
            b"FLUSH",
        ]);
        assert_eq!(parse_discovery(&b), Err(ProtoError::Sha256));
        let b = pkts(&[b"version 2\n", b"ls-refs\n", b"FLUSH"]);
        assert_eq!(parse_discovery(&b), Err(ProtoError::Malformed));
    }

    #[test]
    fn v0_advertisement_with_peeled_tags_and_symref() {
        let first = format!(
            "{A} HEAD\0multi_ack side-band-64k shallow filter symref=HEAD:refs/heads/main agent=git/2\n"
        );
        let main = format!("{A} refs/heads/main\n");
        let tag = format!("{B} refs/tags/v1\n");
        let peeled = format!("{A} refs/tags/v1^{{}}\n");
        let b = pkts(&[
            b"# service=git-upload-pack\n",
            b"FLUSH",
            first.as_bytes(),
            main.as_bytes(),
            tag.as_bytes(),
            peeled.as_bytes(),
            b"FLUSH",
        ]);
        let Discovery::V0(a) = parse_discovery(&b).unwrap() else {
            panic!()
        };
        assert_eq!(a.head_target.as_deref(), Some("refs/heads/main"));
        assert_eq!(a.refs["refs/heads/main"], parse_hex(A).unwrap());
        assert_eq!(a.refs["refs/tags/v1"], parse_hex(B).unwrap());
        assert_eq!(a.peeled["refs/tags/v1"], parse_hex(A).unwrap());
        assert!(a.caps.filter && a.caps.shallow);
        // sha256 capability or a 64-hex id
        let s = format!("{A} HEAD\0object-format=sha256\n");
        assert_eq!(
            parse_discovery(&pkts(&[s.as_bytes(), b"FLUSH"])),
            Err(ProtoError::Sha256)
        );
        let s = format!("{A}{} HEAD\0x\n", &B[..24]);
        assert_eq!(
            parse_discovery(&pkts(&[s.as_bytes(), b"FLUSH"])),
            Err(ProtoError::Sha256)
        );
        // an empty repository
        let e = "0000000000000000000000000000000000000000 capabilities^{}\0shallow\n";
        let Discovery::V0(a) = parse_discovery(&pkts(&[e.as_bytes(), b"FLUSH"])).unwrap() else {
            panic!()
        };
        assert!(a.refs.is_empty());
        assert_eq!(
            parse_discovery(&pkts(&[b"ERR access denied\n"])),
            Err(ProtoError::Refused)
        );
        assert_eq!(
            parse_discovery(b"<html>not git</html>"),
            Err(ProtoError::Malformed)
        );
    }

    #[test]
    fn ls_refs_answer() {
        let l1 = format!("{A} HEAD symref-target:refs/heads/main\n");
        let l2 = format!("{B} refs/tags/v1 peeled:{A}\n");
        let b = pkts(&[l1.as_bytes(), l2.as_bytes(), b"unborn x\n", b"FLUSH"]);
        let r = parse_ls_refs(&b).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].symref_target.as_deref(), Some("refs/heads/main"));
        assert_eq!(r[1].peeled, Some(parse_hex(A).unwrap()));
        assert_eq!(
            parse_ls_refs(&pkts(&[b"ERR no\n"])),
            Err(ProtoError::Refused)
        );
        assert_eq!(
            parse_ls_refs(&pkts(&[b"zz HEAD\n"])),
            Err(ProtoError::Malformed)
        );
    }

    #[test]
    fn requests_are_exact() {
        let w = vec![A.to_owned()];
        let a = FetchAsk {
            wants: &w,
            deepen: true,
            filter: true,
        };
        let v2 = String::from_utf8(fetch_v2("x/1", &a)).unwrap();
        assert_eq!(
            v2,
            format!(
                "0012command=fetch\n000eagent=x/1\n00010010no-progress\n000eofs-delta\n000ddeepen 1\n0015filter blob:none\n0032want {A}\n0009done\n0000"
            )
        );
        let v0 = String::from_utf8(fetch_v0("x/1", &a)).unwrap();
        assert!(v0.starts_with(&format!(
            "006fwant {A} side-band-64k ofs-delta no-progress agent=x/1 shallow filter\n"
        )));
        assert!(v0.ends_with("00000009done\n"));
        let ls = String::from_utf8(ls_refs_request("x/1", &["HEAD".into()])).unwrap();
        assert!(ls.contains("ref-prefix HEAD\n"));
    }

    #[test]
    fn demux_keeps_band_one_and_refuses_errors() {
        let b = pkts(&[
            b"shallow-info\n",
            b"DELIM",
            b"packfile\n",
            b"\x02progress\n",
            b"\x01PACK",
            b"\x01more",
            b"FLUSH",
        ]);
        assert_eq!(demux_pack(&b).unwrap(), b"PACKmore");
        let b = pkts(&[b"NAK\n", b"\x01PACK", b"FLUSH"]);
        assert_eq!(demux_pack(&b).unwrap(), b"PACK");
        assert_eq!(
            demux_pack(&pkts(&[b"ERR upload-pack: not our ref\n"])),
            Err(ProtoError::Refused)
        );
        assert_eq!(
            demux_pack(&pkts(&[b"packfile\n", b"\x03fatal\n"])),
            Err(ProtoError::Refused)
        );
        assert_eq!(
            demux_pack(&pkts(&[b"NAK\n", b"FLUSH"])),
            Err(ProtoError::Malformed)
        );
        assert_eq!(demux_pack(b"00"), Err(ProtoError::Malformed));
    }
}
