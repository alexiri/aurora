//! PostgreSQL wire adapter of the database lane.

pub mod relay;
pub mod server;
pub mod wire;

use crate::db::DbEndpoint;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Whether the first bytes of a connection are a PostgreSQL startup-phase packet (`SSLRequest`,
/// `GSSENCRequest`, `CancelRequest` or a protocol-3 `StartupMessage`) and not TLS or HTTP.
/// `buf` is the (possibly partial) peek; `None` = need more bytes.
pub fn sniff(buf: &[u8]) -> Option<bool> {
    if buf.len() < 8 {
        // Early exit for bytes that cannot be the 4-byte length of a startup packet.
        if buf.first().is_some_and(|b| *b != 0) {
            return Some(false);
        }
        return None;
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let code = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let ok = match code {
        wire::SSL_REQUEST | wire::GSSENC_REQUEST => len == 8,
        wire::CANCEL_REQUEST => len == 16,
        c => c >> 16 == 3 && (8..=wire::MAX_STARTUP).contains(&len),
    };
    Some(ok)
}

/// What a cancel key leads to.
struct Target {
    endpoint: Arc<DbEndpoint>,
    pid: u32,
    key: u32,
}

/// Gateway-generated cancel keys mapped to the server's `(pid, key)`. The server's key never
/// leaves the gateway.
#[derive(Default)]
pub struct CancelRegistry(Mutex<HashMap<(u32, u32), Target>>);

impl CancelRegistry {
    /// Registers a session and returns the `(pid, key)` pair the client is told.
    pub fn register(&self, endpoint: Arc<DbEndpoint>, pid: u32, key: u32) -> (u32, u32) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let b = crate::db::random16();
            let gw_pid = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) & 0x7fff_ffff;
            let gw_key = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
            if gw_pid != 0 && !map.contains_key(&(gw_pid, gw_key)) {
                map.insert((gw_pid, gw_key), Target { endpoint, pid, key });
                return (gw_pid, gw_key);
            }
        }
    }

    pub fn unregister(&self, id: (u32, u32)) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }

    /// The server-side target of a client's cancel request, if the session is live.
    pub fn lookup(&self, id: (u32, u32)) -> Option<(Arc<DbEndpoint>, u32, u32)> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .map(|t| (t.endpoint.clone(), t.pid, t.key))
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffing() {
        let mut ssl = 8u32.to_be_bytes().to_vec();
        ssl.extend_from_slice(&wire::SSL_REQUEST.to_be_bytes());
        assert_eq!(sniff(&ssl), Some(true));
        assert_eq!(sniff(&ssl[..5]), None);
        let startup = wire::startup_message(&[("user", "u")]);
        assert_eq!(sniff(&startup), Some(true));
        // TLS ClientHello, HTTP CONNECT and absolute-form requests are not database traffic
        assert_eq!(sniff(&[0x16, 0x03, 0x01, 0x02, 0, 1, 0, 1]), Some(false));
        assert_eq!(sniff(b"CONNECT x:443 HTTP/1.1"), Some(false));
        assert_eq!(sniff(b"GET http://x/ HTTP/1.1"), Some(false));
        // a wrong length for the request code
        let mut bad = 9u32.to_be_bytes().to_vec();
        bad.extend_from_slice(&wire::SSL_REQUEST.to_be_bytes());
        assert_eq!(sniff(&bad), Some(false));
    }
}
