//! Pre-authentication limits (`16 §W20`, SEC-92): unauthenticated connections in total and per
//! client address, a per-address token bucket on new connections, and the 20 s auth timeout
//! (enforced by the server). A refused connection is closed without a message. Everything is
//! per replica and in memory; the address is the socket peer.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Why a connection was not admitted (metrics `connections_total{outcome}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The global or per-address unauthenticated cap.
    Preauth,
    /// The per-address token bucket.
    Rate,
}

struct PerIp {
    open: u32,
    tokens: f64,
    last: Instant,
}

pub struct PreAuth {
    max_total: u32,
    max_per_ip: u32,
    burst: f64,
    per_sec: f64,
    inner: Mutex<(u32, HashMap<IpAddr, PerIp>)>,
}

/// An admitted, still unauthenticated connection; dropping it (on authentication or on any
/// close) frees the slot.
pub struct Admission {
    owner: Arc<PreAuth>,
    ip: IpAddr,
    live: bool,
}

impl PreAuth {
    pub fn new(max_total: u32, max_per_ip: u32, rate_per_min: u32, burst: u32) -> Arc<Self> {
        Arc::new(Self {
            max_total,
            max_per_ip,
            burst: f64::from(burst.max(1)),
            per_sec: f64::from(rate_per_min) / 60.0,
            inner: Mutex::new((0, HashMap::new())),
        })
    }

    pub fn admit(self: &Arc<Self>, ip: IpAddr) -> Result<Admission, Refused> {
        let now = Instant::now();
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (total, map) = &mut *g;
        let e = map.entry(ip).or_insert(PerIp {
            open: 0,
            tokens: self.burst,
            last: now,
        });
        e.tokens =
            (e.tokens + now.duration_since(e.last).as_secs_f64() * self.per_sec).min(self.burst);
        e.last = now;
        if e.tokens < 1.0 {
            return Err(Refused::Rate);
        }
        if *total >= self.max_total || e.open >= self.max_per_ip {
            return Err(Refused::Preauth);
        }
        e.tokens -= 1.0;
        e.open += 1;
        *total += 1;
        Ok(Admission {
            owner: self.clone(),
            ip,
            live: true,
        })
    }

    fn release(&self, ip: IpAddr) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (total, map) = &mut *g;
        *total = total.saturating_sub(1);
        if let Some(e) = map.get_mut(&ip) {
            e.open = e.open.saturating_sub(1);
        }
        // Bound the table: forget addresses with nothing open and a full bucket.
        if map.len() > 4096 {
            let burst = self.burst;
            map.retain(|_, e| e.open > 0 || e.tokens < burst);
        }
    }

    /// Unauthenticated connections now (gauge, tests).
    pub fn open(&self) -> u32 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).0
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if self.live {
            self.live = false;
            self.owner.release(self.ip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_and_bucket() {
        let p = PreAuth::new(3, 2, 60, 5);
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        let a1 = p.admit(a).unwrap();
        let _a2 = p.admit(a).unwrap();
        assert_eq!(p.admit(a).err(), Some(Refused::Preauth), "per address");
        let _b1 = p.admit(b).unwrap();
        assert_eq!(p.admit(b).err(), Some(Refused::Preauth), "global");
        drop(a1);
        let _b2 = p.admit(b).expect("a freed slot is reusable");
        assert_eq!(p.open(), 3);
    }

    #[test]
    fn the_bucket_empties_and_refills() {
        let p = PreAuth::new(100, 100, 6000, 3);
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..3 {
            drop(p.admit(a).unwrap());
        }
        assert_eq!(p.admit(a).err(), Some(Refused::Rate));
        std::thread::sleep(std::time::Duration::from_millis(40));
        assert!(p.admit(a).is_ok());
    }
}
