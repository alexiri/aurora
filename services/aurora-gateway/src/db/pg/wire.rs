//! PostgreSQL frontend/backend protocol v3 framing (the subset a proxy needs).

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const SSL_REQUEST: u32 = 80_877_103;
pub const GSSENC_REQUEST: u32 = 80_877_104;
pub const CANCEL_REQUEST: u32 = 80_877_102;
pub const PROTOCOL_3_0: u32 = 196_608;

/// Largest startup packet accepted from a client.
pub const MAX_STARTUP: usize = 10_000;

/// First message of a connection (no tag byte).
#[derive(Debug, PartialEq, Eq)]
pub enum Initial {
    Ssl,
    GssEnc,
    Cancel {
        pid: u32,
        key: u32,
    },
    Startup {
        minor: u16,
        params: Vec<(String, String)>,
    },
}

/// Why the first message is not usable.
#[derive(Debug, PartialEq, Eq)]
pub enum InitialError {
    Malformed,
    UnsupportedVersion,
}

/// Parses a startup-phase packet body (everything after the 4-byte length).
pub fn parse_initial(body: &[u8]) -> Result<Initial, InitialError> {
    if body.len() < 4 {
        return Err(InitialError::Malformed);
    }
    let code = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    match code {
        SSL_REQUEST if body.len() == 4 => Ok(Initial::Ssl),
        GSSENC_REQUEST if body.len() == 4 => Ok(Initial::GssEnc),
        CANCEL_REQUEST if body.len() == 12 => Ok(Initial::Cancel {
            pid: u32::from_be_bytes([body[4], body[5], body[6], body[7]]),
            key: u32::from_be_bytes([body[8], body[9], body[10], body[11]]),
        }),
        c if c >> 16 == 3 => {
            let mut params = Vec::new();
            let mut rest = &body[4..];
            loop {
                let Some(k) = take_cstr(&mut rest) else {
                    return Err(InitialError::Malformed);
                };
                if k.is_empty() {
                    break;
                }
                let Some(v) = take_cstr(&mut rest) else {
                    return Err(InitialError::Malformed);
                };
                params.push((k, v));
            }
            Ok(Initial::Startup {
                minor: u16::try_from(c & 0xffff).unwrap_or(0),
                params,
            })
        }
        c if c >> 16 != 3 && c != SSL_REQUEST && c != GSSENC_REQUEST && c != CANCEL_REQUEST => {
            Err(InitialError::UnsupportedVersion)
        }
        _ => Err(InitialError::Malformed),
    }
}

/// Splits a NUL-terminated UTF-8 string off the front of `b`.
pub fn take_cstr(b: &mut &[u8]) -> Option<String> {
    let n = b.iter().position(|c| *c == 0)?;
    let s = std::str::from_utf8(&b[..n]).ok()?.to_owned();
    *b = &b[n + 1..];
    Some(s)
}

/// Reads one startup-phase packet: `Ok(None)` on a clean EOF before any byte.
pub async fn read_initial<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_be_bytes(len) as usize;
    if !(8..=MAX_STARTUP).contains(&n) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad startup length",
        ));
    }
    let mut body = vec![0u8; n - 4];
    r.read_exact(&mut body).await?;
    Ok(Some(body))
}

pub fn encode(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 5);
    v.push(tag);
    v.extend_from_slice(
        &u32::try_from(body.len() + 4)
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    v.extend_from_slice(body);
    v
}

fn cstr(out: &mut Vec<u8>, s: &str) {
    out.extend(s.bytes().filter(|b| *b != 0));
    out.push(0);
}

/// `ErrorResponse`. `severity` is `ERROR` or `FATAL`.
pub fn error_response(
    severity: &str,
    sqlstate: &str,
    message: &str,
    hint: Option<&str>,
) -> Vec<u8> {
    let mut b = Vec::new();
    for (code, val) in [
        (b'S', severity),
        (b'V', severity),
        (b'C', sqlstate),
        (b'M', message),
    ] {
        b.push(code);
        cstr(&mut b, val);
    }
    if let Some(h) = hint {
        b.push(b'H');
        cstr(&mut b, h);
    }
    b.push(0);
    encode(b'E', &b)
}

pub fn ready_for_query(status: u8) -> Vec<u8> {
    encode(b'Z', &[status])
}

pub fn auth_request(code: u32, extra: &[u8]) -> Vec<u8> {
    let mut b = code.to_be_bytes().to_vec();
    b.extend_from_slice(extra);
    encode(b'R', &b)
}

pub fn parameter_status(name: &str, value: &str) -> Vec<u8> {
    let mut b = Vec::new();
    cstr(&mut b, name);
    cstr(&mut b, value);
    encode(b'S', &b)
}

pub fn backend_key_data(pid: u32, key: u32) -> Vec<u8> {
    let mut b = pid.to_be_bytes().to_vec();
    b.extend_from_slice(&key.to_be_bytes());
    encode(b'K', &b)
}

/// `NegotiateProtocolVersion` (we speak 3.0 only).
pub fn negotiate_protocol(unrecognized: &[String]) -> Vec<u8> {
    let mut b = 0u32.to_be_bytes().to_vec(); // newest minor version supported
    b.extend_from_slice(&u32::try_from(unrecognized.len()).unwrap_or(0).to_be_bytes());
    for n in unrecognized {
        cstr(&mut b, n);
    }
    encode(b'v', &b)
}

/// Frontend startup message with the given parameters.
pub fn startup_message(params: &[(&str, &str)]) -> Vec<u8> {
    let mut b = PROTOCOL_3_0.to_be_bytes().to_vec();
    for (k, v) in params {
        cstr(&mut b, k);
        cstr(&mut b, v);
    }
    b.push(0);
    let mut out = u32::try_from(b.len() + 4)
        .unwrap_or(u32::MAX)
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(&b);
    out
}

/// One tagged message with a bounded body.
#[derive(Debug)]
pub struct Frame {
    pub tag: u8,
    pub body: Vec<u8>,
    /// The body was larger than the cap and was discarded unread (`body` is empty).
    pub oversize: bool,
}

/// Reads a tagged message. Bodies over `cap` are skipped and reported as `oversize`; bodies over
/// `hard` close the connection (error). `Ok(None)` is a clean EOF.
pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
    cap: usize,
    hard: usize,
) -> std::io::Result<Option<Frame>> {
    let mut head = [0u8; 5];
    match r.read_exact(&mut head).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if len < 4 || len - 4 > hard {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad message length",
        ));
    }
    let n = len - 4;
    if n > cap {
        let mut limited = r.take(n as u64);
        let copied = tokio::io::copy(&mut limited, &mut tokio::io::sink()).await?;
        if copied != n as u64 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        return Ok(Some(Frame {
            tag: head[0],
            body: Vec::new(),
            oversize: true,
        }));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).await?;
    Ok(Some(Frame {
        tag: head[0],
        body,
        oversize: false,
    }))
}

/// Header of a backend message whose body is streamed.
pub async fn read_header<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<(u8, usize)>> {
    let mut head = [0u8; 5];
    match r.read_exact(&mut head).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if len < 4 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad message length",
        ));
    }
    Ok(Some((head[0], len - 4)))
}

pub async fn write_all<W: AsyncWrite + Unpin>(w: &mut W, b: &[u8]) -> std::io::Result<()> {
    w.write_all(b).await?;
    w.flush().await
}

/// Fields of an `ErrorResponse` / `NoticeResponse` body: `(code byte, value)`.
pub fn error_fields(body: &[u8]) -> Vec<(u8, String)> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some((&c, tail)) = rest.split_first() {
        if c == 0 {
            break;
        }
        rest = tail;
        let Some(v) = take_cstr(&mut rest) else { break };
        out.push((c, v));
    }
    out
}

pub fn error_message(body: &[u8]) -> String {
    let f = error_fields(body);
    let get = |c: u8| f.iter().find(|(k, _)| *k == c).map(|(_, v)| v.clone());
    format!(
        "{} ({})",
        get(b'M').unwrap_or_else(|| "server error".into()),
        get(b'C').unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_roundtrip() {
        let m = startup_message(&[("user", "u"), ("database", "d")]);
        match parse_initial(&m[4..]).unwrap() {
            Initial::Startup { minor, params } => {
                assert_eq!(minor, 0);
                assert_eq!(
                    params,
                    vec![("user".into(), "u".into()), ("database".into(), "d".into())]
                );
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn initial_kinds() {
        assert_eq!(parse_initial(&SSL_REQUEST.to_be_bytes()), Ok(Initial::Ssl));
        let mut c = CANCEL_REQUEST.to_be_bytes().to_vec();
        c.extend_from_slice(&7u32.to_be_bytes());
        c.extend_from_slice(&9u32.to_be_bytes());
        assert_eq!(parse_initial(&c), Ok(Initial::Cancel { pid: 7, key: 9 }));
        assert_eq!(
            parse_initial(&0x0002_0000u32.to_be_bytes()),
            Err(InitialError::UnsupportedVersion)
        );
        assert_eq!(parse_initial(&[0, 3]), Err(InitialError::Malformed));
        // unterminated parameter list
        let mut s = PROTOCOL_3_0.to_be_bytes().to_vec();
        s.extend_from_slice(b"user\0u");
        assert_eq!(parse_initial(&s), Err(InitialError::Malformed));
    }

    #[test]
    fn error_response_fields() {
        let e = error_response("ERROR", "42501", "no", Some("h"));
        assert_eq!(e[0], b'E');
        let f = error_fields(&e[5..]);
        assert!(f.contains(&(b'C', "42501".into())));
        assert!(f.contains(&(b'M', "no".into())));
        assert!(f.contains(&(b'H', "h".into())));
    }

    #[tokio::test]
    async fn oversize_frames_are_skipped() {
        let mut data = encode(b'Q', &[b'x'; 100]);
        data.extend(encode(b'S', &[]));
        let mut r = &data[..];
        let f = read_frame(&mut r, 10, 1000).await.unwrap().unwrap();
        assert!(f.oversize);
        let f = read_frame(&mut r, 10, 1000).await.unwrap().unwrap();
        assert_eq!(f.tag, b'S');
        assert!(read_frame(&mut r, 10, 1000).await.unwrap().is_none());
        let big = encode(b'Q', &[b'x'; 100]);
        assert!(read_frame(&mut &big[..], 10, 50).await.is_err());
    }
}
