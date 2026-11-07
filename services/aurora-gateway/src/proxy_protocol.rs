//! PROXY protocol v1/v2 header parsing (phase 2, RFE-025; docs/06 §BYO admission).
//!
//! Only used for peers inside `gateway.trusted_proxies`: the header is the *only* source of the
//! client address for those connections, and a missing or malformed header closes the connection
//! before the TLS handshake. Exactly the header bytes are consumed; nothing of the TLS stream is
//! read.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncRead, AsyncReadExt};

const V2_SIG: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";
/// Longest v1 line including CRLF (spec: 107 bytes).
const V1_MAX: usize = 107;
/// Upper bound for a v2 payload we are willing to read (addresses + a few TLVs).
const V2_MAX_PAYLOAD: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Header {
    /// A proxied connection with the original client address.
    Proxied(SocketAddr),
    /// `PROXY UNKNOWN` / v2 `LOCAL`: the proxy's own connection (health check); no client.
    Local,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("no PROXY protocol header")]
    Missing,
    #[error("malformed PROXY protocol header")]
    Malformed,
    #[error("i/o error reading the PROXY protocol header")]
    Io,
}

/// Whether the first bytes can only be the start of a PROXY header (used to tell an untrusted
/// peer's PROXY header apart from garbage in metrics; both are closed).
pub fn looks_like_proxy(first: &[u8]) -> bool {
    let v1 = b"PROXY ";
    let n = first.len().min(v1.len());
    let m = first.len().min(V2_SIG.len());
    (n > 0 && first[..n] == v1[..n]) || (m > 0 && first[..m] == V2_SIG[..m])
}

/// Reads exactly one PROXY header (v1 or v2) from `r`.
pub async fn read_header<R: AsyncRead + Unpin>(r: &mut R) -> Result<Header, Error> {
    let mut first = [0u8; 1];
    r.read_exact(&mut first).await.map_err(|_| Error::Io)?;
    match first[0] {
        b'P' => read_v1(r).await,
        0x0D => read_v2(r).await,
        _ => Err(Error::Missing),
    }
}

async fn read_v1<R: AsyncRead + Unpin>(r: &mut R) -> Result<Header, Error> {
    let mut line = vec![b'P'];
    loop {
        let mut b = [0u8; 1];
        r.read_exact(&mut b).await.map_err(|_| Error::Io)?;
        line.push(b[0]);
        if line.ends_with(b"\r\n") {
            break;
        }
        if line.len() >= V1_MAX {
            return Err(Error::Malformed);
        }
    }
    parse_v1(&line[..line.len() - 2])
}

fn parse_v1(line: &[u8]) -> Result<Header, Error> {
    let s = std::str::from_utf8(line).map_err(|_| Error::Malformed)?;
    let mut it = s.split(' ');
    if it.next() != Some("PROXY") {
        return Err(Error::Malformed);
    }
    match it.next() {
        Some("UNKNOWN") => Ok(Header::Local),
        Some(proto @ ("TCP4" | "TCP6")) => {
            let (src, dst, sp, dp) = (it.next(), it.next(), it.next(), it.next());
            if it.next().is_some() {
                return Err(Error::Malformed);
            }
            let (Some(src), Some(dst), Some(sp), Some(dp)) = (src, dst, sp, dp) else {
                return Err(Error::Malformed);
            };
            let src: IpAddr = src.parse().map_err(|_| Error::Malformed)?;
            let _dst: IpAddr = dst.parse().map_err(|_| Error::Malformed)?;
            if src.is_ipv4() != (proto == "TCP4") {
                return Err(Error::Malformed);
            }
            let sp: u16 = sp.parse().map_err(|_| Error::Malformed)?;
            let _dp: u16 = dp.parse().map_err(|_| Error::Malformed)?;
            Ok(Header::Proxied(SocketAddr::new(src, sp)))
        }
        _ => Err(Error::Malformed),
    }
}

async fn read_v2<R: AsyncRead + Unpin>(r: &mut R) -> Result<Header, Error> {
    // 12-byte signature (first byte already read), ver/cmd, fam/proto, len(2).
    let mut rest = [0u8; 15];
    r.read_exact(&mut rest).await.map_err(|_| Error::Io)?;
    if rest[..11] != V2_SIG[1..] {
        return Err(Error::Malformed);
    }
    let (ver_cmd, fam, len) = (
        rest[11],
        rest[12],
        usize::from(u16::from_be_bytes([rest[13], rest[14]])),
    );
    if ver_cmd >> 4 != 2 || len > V2_MAX_PAYLOAD {
        return Err(Error::Malformed);
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).await.map_err(|_| Error::Io)?;
    match ver_cmd & 0x0F {
        0x0 => Ok(Header::Local),
        0x1 => match fam >> 4 {
            0x1 => {
                // AF_INET: src(4) dst(4) sport(2) dport(2)
                if payload.len() < 12 {
                    return Err(Error::Malformed);
                }
                let ip = Ipv4Addr::new(payload[0], payload[1], payload[2], payload[3]);
                let port = u16::from_be_bytes([payload[8], payload[9]]);
                Ok(Header::Proxied(SocketAddr::new(IpAddr::V4(ip), port)))
            }
            0x2 => {
                // AF_INET6: src(16) dst(16) sport(2) dport(2)
                if payload.len() < 36 {
                    return Err(Error::Malformed);
                }
                let mut o = [0u8; 16];
                o.copy_from_slice(&payload[..16]);
                let port = u16::from_be_bytes([payload[32], payload[33]]);
                Ok(Header::Proxied(SocketAddr::new(
                    IpAddr::V6(Ipv6Addr::from(o)),
                    port,
                )))
            }
            // AF_UNSPEC / AF_UNIX: no usable client address.
            _ => Err(Error::Malformed),
        },
        _ => Err(Error::Malformed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parse(bytes: &[u8]) -> Result<Header, Error> {
        let mut r = bytes;
        read_header(&mut r).await
    }

    fn v2(cmd: u8, fam: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = V2_SIG.to_vec();
        v.push(0x20 | cmd);
        v.push(fam);
        v.extend((payload.len() as u16).to_be_bytes());
        v.extend(payload);
        v
    }

    #[tokio::test]
    async fn v1_tcp4_tcp6_unknown() {
        assert_eq!(
            parse(b"PROXY TCP4 203.0.113.7 10.0.0.1 51000 4102\r\n").await,
            Ok(Header::Proxied("203.0.113.7:51000".parse().unwrap()))
        );
        assert_eq!(
            parse(b"PROXY TCP6 2001:db8::7 2001:db8::1 51000 4102\r\n").await,
            Ok(Header::Proxied("[2001:db8::7]:51000".parse().unwrap()))
        );
        assert_eq!(parse(b"PROXY UNKNOWN\r\n").await, Ok(Header::Local));
        assert_eq!(
            parse(b"PROXY TCP4 2001:db8::7 10.0.0.1 1 2\r\n").await,
            Err(Error::Malformed),
            "family must match"
        );
        assert_eq!(
            parse(b"PROXY TCP4 1.2.3.4 5.6.7.8 1\r\n").await,
            Err(Error::Malformed)
        );
        assert_eq!(
            parse(b"PROXY TCP4 1.2.3.4 5.6.7.8 1 2 3\r\n").await,
            Err(Error::Malformed)
        );
        assert_eq!(parse(b"PROXYX TCP4\r\n").await, Err(Error::Malformed));
    }

    #[tokio::test]
    async fn v1_without_terminator_or_too_long_is_malformed() {
        let mut long = b"PROXY TCP4 ".to_vec();
        long.extend(std::iter::repeat_n(b'1', 200));
        assert_eq!(parse(&long).await, Err(Error::Malformed));
        assert_eq!(
            parse(b"PROXY TCP4 1.2.3.4").await,
            Err(Error::Io),
            "eof before CRLF"
        );
    }

    #[tokio::test]
    async fn v2_inet_inet6_local() {
        let mut p4 = vec![198, 51, 100, 9, 10, 0, 0, 1];
        p4.extend(40000u16.to_be_bytes());
        p4.extend(4102u16.to_be_bytes());
        assert_eq!(
            parse(&v2(1, 0x11, &p4)).await,
            Ok(Header::Proxied("198.51.100.9:40000".parse().unwrap()))
        );
        let mut p6 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 9)
            .octets()
            .to_vec();
        p6.extend(Ipv6Addr::LOCALHOST.octets());
        p6.extend(40000u16.to_be_bytes());
        p6.extend(4102u16.to_be_bytes());
        assert_eq!(
            parse(&v2(1, 0x21, &p6)).await,
            Ok(Header::Proxied("[2001:db8::9]:40000".parse().unwrap()))
        );
        assert_eq!(parse(&v2(0, 0x00, &[])).await, Ok(Header::Local));
        assert_eq!(
            parse(&v2(1, 0x11, &p4[..8])).await,
            Err(Error::Malformed),
            "short payload"
        );
        assert_eq!(
            parse(&v2(1, 0x00, &[])).await,
            Err(Error::Malformed),
            "AF_UNSPEC proxied"
        );
        assert_eq!(
            parse(&v2(1, 0x31, &[0; 216])).await,
            Err(Error::Malformed),
            "AF_UNIX"
        );
        let mut bad = v2(1, 0x11, &p4);
        bad[12] = 0x30; // version 3
        assert_eq!(parse(&bad).await, Err(Error::Malformed));
    }

    #[tokio::test]
    async fn only_the_header_is_consumed() {
        let mut data = b"PROXY TCP4 1.2.3.4 5.6.7.8 1 2\r\n".to_vec();
        data.extend([0x16, 0x03, 0x01]);
        let mut r = data.as_slice();
        read_header(&mut r).await.unwrap();
        assert_eq!(r, &[0x16, 0x03, 0x01]);
    }

    #[tokio::test]
    async fn tls_or_garbage_is_missing() {
        assert_eq!(parse(&[0x16, 0x03, 0x01, 0x02]).await, Err(Error::Missing));
        assert_eq!(parse(b"GET / HTTP/1.1\r\n").await, Err(Error::Missing));
        assert_eq!(parse(b"").await, Err(Error::Io));
    }

    #[test]
    fn signature_detection() {
        assert!(looks_like_proxy(b"PROXY TCP4"));
        assert!(looks_like_proxy(&V2_SIG));
        assert!(looks_like_proxy(b"PRO"));
        assert!(!looks_like_proxy(&[0x16, 0x03, 0x01]));
        assert!(!looks_like_proxy(b""));
    }
}
