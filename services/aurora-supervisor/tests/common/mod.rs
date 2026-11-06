#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const LO: &str = "127.0.0.1:0";

/// Stub gateway: records every `Proxy-Authorization` header per request.
pub async fn stub_gateway() -> (String, Arc<Mutex<Vec<Vec<String>>>>) {
    let l = TcpListener::bind(LO).await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let seen: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
    let s2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = l.accept().await.unwrap();
            let seen = s2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = c.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let auth: Vec<String> = head
                    .lines()
                    .filter_map(|l| l.strip_prefix("Proxy-Authorization: "))
                    .map(|v| v.trim().to_owned())
                    .collect();
                seen.lock().unwrap().push(auth);
                let resp: &[u8] = if head.starts_with("CONNECT") {
                    b"HTTP/1.1 200 Connection established\r\n\r\n"
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                };
                let _ = c.write_all(resp).await;
                let _ = c.shutdown().await;
            });
        }
    });
    (addr, seen)
}

pub fn decode_basic(v: &str) -> String {
    let b = base64::engine::general_purpose::STANDARD
        .decode(v.trim_start_matches("Basic "))
        .unwrap();
    String::from_utf8(b).unwrap()
}

/// Minimal temp dir (no `tempfile` in the workspace); removed on drop unless `keep`t.
pub struct TempDir(std::path::PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let p = std::env::temp_dir().join(format!("aurora-sup-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }

    /// Leaks the directory (tests whose supervisor outlives the test body).
    pub fn keep(self) -> std::path::PathBuf {
        let p = self.0.clone();
        std::mem::forget(self);
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Skills dirs are read-only (0555); make them writable before removal.
        let _ = aurora_supervisor::spawn::command("chmod", std::iter::empty::<(&str, &str)>())
            .args(["-R", "u+rwX"])
            .arg(&self.0)
            .status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
