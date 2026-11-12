//! Listening-port discovery (`ports.state`, docs/11 §Port discovery, docs/16 §W5).
//!
//! The supervisor scans loopback TCP listeners and reports them as `ports.state { ports }`
//! on change: `listening_ports` is what the dashboard's "Add share" dialog offers and what
//! derives a share's `listening` status (`port ∈ listening_ports` of the current boot).
//! Scanning `/proc/net/tcp{,6}` (st == `0A`, LISTEN) needs no privileges inside the
//! container and costs no syscalls per probe; port 0 is skipped (`PortsState` schema).

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One `/proc/net/tcp{,6}` file, parsed: local LISTEN ports (st == `0A`).
fn parse(buf: &str) -> BTreeSet<u16> {
    let mut out = BTreeSet::new();
    // Rows: `sl local_address rem_address st tx:rx tr:tm<> retrnsmt uid timeout inode ...`
    for line in buf.lines().skip(1) {
        let mut f = line.split_whitespace();
        let Some(local) = f.nth(1) else {
            continue; // sl local_address
        };
        let Some(_rem) = f.next() else {
            continue; // rem_address
        };
        let Some(state) = f.next() else {
            continue; // st
        };
        // local_address is `ADDR:PORT` (hex).
        let Some((addr_hex, port_hex)) = local.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        if state != "0A" || port == 0 {
            continue;
        }
        // Only loopback-reachable addresses: loopback itself or a wildcard bind (0.0.0.0 /
        // ::), which accepts 127.0.0.1 connections — the addresses the stream engine dials.
        // A port bound to a specific non-loopback address is not dialable and is skipped.
        let Ok(addr) = parse_hex_address(addr_hex) else {
            continue;
        };
        let dialable = match addr {
            IpAddr::V4(a) => a.is_loopback() || a.is_unspecified(),
            IpAddr::V6(a) => {
                a.is_loopback()
                    || a.is_unspecified()
                    || a.to_ipv4_mapped().is_some_and(|v| v.is_loopback())
            }
        };
        if dialable {
            out.insert(port);
        }
    }
    out
}

/// Parses `/proc/net/tcp` local_address fields: `0100007F:0BB8` (v4) or
/// `00000000000000000000000001000000:0BB8` (v6). The kernel prints each 4 address bytes
/// as one native-endian u32 with `%08X` (127.0.0.1 → `0100007F`), so `to_le_bytes`
/// recovers the address bytes on little-endian hosts.
fn parse_hex_address(s: &str) -> Result<IpAddr, ()> {
    let words: [u32; 4] = match s.len() {
        8 => {
            let w = u32::from_str_radix(s, 16).map_err(|_| ())?;
            [w, 0, 0, 0]
        }
        32 => {
            let mut out = [0u32; 4];
            for (i, w) in out.iter_mut().enumerate() {
                *w = u32::from_str_radix(&s[i * 8..(i + 1) * 8], 16).map_err(|_| ())?;
            }
            out
        }
        _ => return Err(()),
    };
    let mut bytes = [0u8; 16];
    for (i, w) in words.iter().enumerate() {
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&w.to_le_bytes());
    }
    if words[1] == 0 && words[2] == 0 && words[3] == 0 {
        // A v4 row: exactly one word.
        Ok(IpAddr::V4(Ipv4Addr::new(
            bytes[0], bytes[1], bytes[2], bytes[3],
        )))
    } else {
        Ok(IpAddr::V6(Ipv6Addr::from(bytes)))
    }
}

/// Scans `/proc/net/tcp` and `/proc/net/tcp6` for LISTEN loopback ports.
pub fn scan() -> BTreeSet<u16> {
    let mut out = BTreeSet::new();
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(buf) = std::fs::read_to_string(path) else {
            continue;
        };
        out.extend(parse(&buf));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listeners_and_skips_others() {
        // Header + one LISTEN row on 127.0.0.1:3000, one TIME_WAIT row, one LISTEN on a
        // non-loopback address, a wildcard LISTEN and one LISTEN on port 0.
        let v4 = "  sl  local_address rem_address   st tx_queue rx_queue\n\
                    0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000\n\
                  1: 0100007F:0BB9 0200017F:C352 06 00000000:00000000\n\
                  2: 64011DAC:1F90 00000000:0000 0A 00000000:00000000\n\
                  3: 00000000:1F91 00000000:0000 0A 00000000:00000000\n\
                  4: 0100007F:0000 00000000:0000 0A 00000000:0000";
        let ports = parse(v4);
        assert_eq!(ports, BTreeSet::from([3000u16, 8081u16]));
    }

    #[test]
    fn parses_ipv6_loopback() {
        // ::1 (little-endian u32 words: all zero but the last word 0x0100_0000 → 1) port 8080.
        let v6 = "  sl  local_address rem_address   st\n\
                   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000 0A";
        let ports = parse(v6);
        assert_eq!(ports, BTreeSet::from([8080u16]));
    }

    #[test]
    fn garbage_is_skipped() {
        assert!(parse("").is_empty());
        assert!(parse("nonsense\nmore nonsense").is_empty());
        assert!(parse("  0: zz:zz 00:00 0A").is_empty());
    }

    #[test]
    fn real_proc_has_our_test_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let found = scan();
        assert!(found.contains(&port), "scan() misses {port}: {found:?}");
    }
}
