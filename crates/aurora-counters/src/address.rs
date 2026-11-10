//! Validation of `ratelimit.counters.redis.address` (`15`): `rediss://host[:port]` with no
//! userinfo, path or query; `redis://` only for a loopback host or with `allow_plaintext`.

/// A validated Redis address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisAddr {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// `redis://` to a non-loopback host that `allow_plaintext` opted into (startup warning).
    pub plaintext_remote: bool,
}

fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Parse and validate; the error names what is wrong (the caller prefixes the config key).
pub fn parse_address(raw: &str, allow_plaintext: bool) -> Result<RedisAddr, String> {
    let (tls, rest) = if let Some(r) = raw.strip_prefix("rediss://") {
        (true, r)
    } else if let Some(r) = raw.strip_prefix("redis://") {
        (false, r)
    } else {
        return Err("must start with `rediss://` (or `redis://` for loopback)".into());
    };
    if rest.contains('@') {
        return Err(
            "must not carry credentials: set `username_env` / `password_env` instead (08 §4)"
                .into(),
        );
    }
    if rest.contains('?') || rest.contains('#') {
        return Err("must not carry a query".into());
    }
    let hostport = rest.strip_suffix('/').unwrap_or(rest);
    if hostport.contains('/') {
        return Err("must not carry a path (use `db`)".into());
    }
    if hostport.is_empty() {
        return Err("has no host".into());
    }
    let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
        let (h, tail) = v6.split_once(']').ok_or("has an unterminated IPv6 host")?;
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p),
            None if tail.is_empty() => None,
            None => return Err("has trailing text after the IPv6 host".into()),
        };
        (format!("[{h}]"), port)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), Some(p)),
            None => (hostport.to_owned(), None),
        }
    };
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err("has an invalid host".into());
    }
    let port = match port {
        Some(p) => p.parse::<u16>().map_err(|_| "has an invalid port")?,
        None => 6379,
    };
    let loopback = is_loopback(&host);
    if !tls && !loopback && !allow_plaintext {
        return Err(
            "is plaintext `redis://` to a non-loopback host: use `rediss://` or set \
             `allow_plaintext = true`"
                .into(),
        );
    }
    Ok(RedisAddr {
        tls,
        host,
        port,
        plaintext_remote: !tls && !loopback,
    })
}

/// `key_prefix`: at most 64 characters of `[A-Za-z0-9:_-]`.
pub fn check_key_prefix(p: &str) -> Result<(), String> {
    if p.len() > 64
        || !p
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
    {
        return Err("must be at most 64 characters of [A-Za-z0-9:_-]".into());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_refuses() {
        let a =
            parse_address("rediss://redis.example.org", false).unwrap_or_else(|e| panic!("{e}"));
        assert!(a.tls && a.port == 6379);
        assert!(parse_address("redis://127.0.0.1:6380", false).is_ok());
        assert!(parse_address("redis://[::1]:6380/", false).is_ok());
        assert!(parse_address("redis://redis.svc:6379", false).is_err());
        let p = parse_address("redis://redis.svc:6379", true).unwrap_or_else(|e| panic!("{e}"));
        assert!(p.plaintext_remote);
        assert!(
            parse_address("rediss://u:p@h", false)
                .unwrap_err()
                .contains("credentials")
        );
        assert!(parse_address("rediss://h/0", false).is_err());
        assert!(parse_address("rediss://h?x=1", false).is_err());
        assert!(parse_address("http://h", false).is_err());
        assert!(check_key_prefix("aurora:rl:").is_ok());
        assert!(check_key_prefix("a b").is_err());
    }
}
