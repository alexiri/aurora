//! `preview-echo`: a minimal HTTP + WebSocket echo server for preview round-trip tests
//! (P4-PRV-2, `scripts/e2e/kind.sh` k6). Binds `127.0.0.1:<port>` only — exactly the address
//! the supervisor's stream engine dials (`11 §Port previews`: the app listens on the loopback
//! inside the sandbox). The e2e share's `start_command` runs `preview-echo 3000`.
//!
//! Not a general web server: HTTP/1.0-style request handling (`Connection: close`), a fixed
//! echo response, and RFC 6455 WebSocket echo (server frames unmasked). Kept in the supervisor
//! crate so the sandbox image ships it with no extra build step.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn main() -> std::process::ExitCode {
    let Some(port) = std::env::args().nth(1).and_then(|a| a.parse::<u16>().ok()) else {
        eprintln!("usage: preview-echo <port>");
        return std::process::ExitCode::FAILURE;
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("preview-echo: bind 127.0.0.1:{port}: {e}");
                return std::process::ExitCode::FAILURE;
            }
        };
        eprintln!("preview-echo: listening on 127.0.0.1:{port}");
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            tokio::spawn(serve(stream));
        }
    })
}

async fn serve(mut stream: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _ = stream.set_nodelay(true);
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    // Read to the end of the header block (one byte at a time: the echo server serves tests).
    loop {
        match stream.read(&mut byte).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
                if head.len() > 64 * 1024 {
                    return;
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut method = String::new();
    let mut path = String::new();
    if let Some((m, rest)) = request_line.split_once(' ') {
        method = m.to_owned();
        path = rest.split(' ').next().unwrap_or_default().to_owned();
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    // WebSocket upgrade: 101, then echo every frame (unmasked; RFC 6455 server frames).
    if header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
        && header("sec-websocket-key").is_some()
    {
        let key = header("sec-websocket-key").unwrap();
        let mut accept = key;
        accept.push_str("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        let digest = sha1(accept.as_bytes());
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            base64::engine::general_purpose::STANDARD.encode(digest)
        );
        if stream.write_all(response.as_bytes()).await.is_err() {
            return;
        }
        echo_ws(stream).await;
        return;
    }
    // Read the body (Content-Length only; the echo tests send fixed-length bodies).
    let content_length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    if content_length > 0 && stream.read_exact(&mut body).await.is_err() {
        return;
    }
    // The echo response: what the app saw, as JSON (the tests assert substrings).
    let body_text = String::from_utf8_lossy(&body);
    let payload = format!(
        "{{\"app\":\"preview-echo\",\"method\":\"{}\",\"path\":\"{}\",\"body\":\"{}\"}}",
        method,
        path,
        body_text.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    if stream.write_all(response.as_bytes()).await.is_err() {
        return;
    }
    let _ = stream.flush().await;
}

/// WebSocket echo: read frames, strip the client's mask, reply unmasked with the same opcode.
/// A close frame or an EOF ends the connection; pings are answered with matching pongs.
async fn echo_ws(mut stream: tokio::net::TcpStream) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        // Frame header: FIN/opcode byte, then the length byte(s).
        let Ok(n) = stream.read(&mut buf).await else {
            return;
        };
        if n < 2 {
            return;
        }
        let fin = buf[0] & 0x80 != 0;
        let opcode = buf[0] & 0x0f;
        let masked = buf[1] & 0x80 != 0;
        let mut len = (buf[1] & 0x7f) as usize;
        let mut offset = 2;
        if len == 126 {
            if n < 4 {
                return;
            }
            len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            offset = 4;
        } else if len == 127 {
            if n < 10 {
                return;
            }
            let mut b = [0u8; 8];
            b.copy_from_slice(&buf[2..10]);
            len = u64::from_be_bytes(b) as usize;
            offset = 10;
        }
        let mask: [u8; 4] = if masked {
            if n < offset + 4 {
                return;
            }
            let m = [
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
            ];
            offset += 4;
            m
        } else {
            [0; 4]
        };
        // (Interleaved reads could split a frame; the tests send one small frame at a time.)
        if n < offset + len {
            return;
        }
        let payload: Vec<u8> = buf[offset..offset + len]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ mask[i % 4])
            .collect();
        if opcode == 8 {
            // Close: answer close and stop.
            let _ = stream.write_all(&[0x88, 0x00]).await;
            return;
        }
        let reply_op = if opcode == 9 { 10 } else { opcode }; // ping -> pong
        let b0 = if fin { 0x80 } else { 0 } | reply_op;
        let header: Vec<u8> = if payload.len() < 126 {
            vec![b0, payload.len() as u8]
        } else if payload.len() <= u16::MAX as usize {
            let l = payload.len() as u16;
            vec![b0, 126, (l >> 8) as u8, (l & 0xff) as u8]
        } else {
            let l = payload.len() as u64;
            let mut h = vec![b0, 127];
            h.extend_from_slice(&l.to_be_bytes());
            h
        };
        if stream.write_all(&header).await.is_err() || stream.write_all(&payload).await.is_err() {
            return;
        }
        if opcode == 1 && payload == b"close-after-echo" {
            return;
        }
    }
}

/// SHA-1 (the WebSocket accept hash). Compacted standard implementation; the known-answer
/// test below pins it.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sha1;

    #[test]
    fn sha1_known_answers() {
        assert_eq!(
            hex::encode(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex::encode(sha1(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        // RFC 6455 §1.3 example: the accept for key "dGhlIHNhbXBsZSBub25jZQ==".
        use base64::Engine as _;
        let mut key = "dGhlIHNhbXBsZSBub25jZQ==".to_owned();
        key.push_str("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        assert_eq!(
            base64::engine::general_purpose::STANDARD.encode(sha1(key.as_bytes())),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    /// The full loop the e2e relies on: HTTP echo (with a body) and a masked client frame
    /// answered with an unmasked server echo (tokio-tungstenite, as the stream engine's
    /// opposite end would be).
    #[tokio::test]
    async fn http_and_websocket_echo() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(super::serve(stream));
            }
        });

        // HTTP: POST with a body echoes method, path and body.
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        c.write_all(b"POST /x?a=1 HTTP/1.1\r\nhost: h\r\ncontent-length: 5\r\n\r\nhello")
            .await
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let n = c.read(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
        assert!(text.contains(r#""path":"/x?a=1""#), "{text}");
        assert!(text.contains(r#""body":"hello""#), "{text}");

        // WebSocket: handshake, then a masked text frame echoed unmasked.
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        c.write_all(
            b"GET /ws HTTP/1.1\r\nhost: h\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\r\n",
        )
        .await
        .unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            c.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let head = String::from_utf8_lossy(&head).to_string();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        assert!(
            head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "{head}"
        );
        // Client frame: FIN+text, masked "ping" with mask 0x01020304.
        let mask = [1u8, 2, 3, 4];
        let payload: Vec<u8> = b"ping"
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ mask[i % 4])
            .collect();
        let mut frame = vec![0x81, 0x80 | 4];
        frame.extend_from_slice(&mask);
        frame.extend_from_slice(&payload);
        c.write_all(&frame).await.unwrap();
        let n = c.read(&mut buf).await.unwrap();
        assert!(n >= 6, "short echo: {n}");
        assert_eq!(buf[0], 0x81, "unmasked text echo");
        let len = (buf[1] & 0x7f) as usize;
        assert_eq!(&buf[2..2 + len], b"ping");
    }
}
