//! Destination guard for Vault stores defined in the dashboard (SEC-89, SEC-90): an instance admin
//! chooses the address, so the address may not be a cloud metadata endpoint or a private,
//! loopback or link-local one unless the operator opened it with `gateway.allowed_private`
//! (the same table and grammar as the gateway's guard: `aurora_core::ranges`). Helm-defined
//! stores are the operator's own choice and are not guarded.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use aurora_core::ranges::PrivateAllow;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// `gateway.allowed_private` as a dial policy.
#[derive(Debug, Clone, Default)]
pub struct DialGuard {
    allow: PrivateAllow,
}

#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("destination refused")
    }
}

impl std::error::Error for Refused {}

fn host_of(address: &str) -> Option<(String, u16)> {
    let u = reqwest::Url::parse(address).ok()?;
    let host = u.host_str()?.trim_matches(['[', ']']).to_owned();
    Some((host, u.port_or_known_default().unwrap_or(443)))
}

impl DialGuard {
    #[must_use]
    pub fn new(allowed_private: &[String]) -> Self {
        Self {
            allow: PrivateAllow::new(allowed_private),
        }
    }

    /// May the address (as written) be dialled? IP literals are judged now, names by resolving
    /// them; every address a name resolves to must pass. A name that does not resolve now is not
    /// a violation (DNS may be down): the connect-time resolver judges it again.
    pub async fn permits_address(&self, address: &str) -> bool {
        let Some((host, port)) = host_of(address) else {
            return false;
        };
        if let Ok(ip) = host.parse::<IpAddr>() {
            return self.allow.permits(ip, &host);
        }
        let lookup = tokio::time::timeout(
            aurora_core::limits::SECRET_STORE_PROBE_TIMEOUT,
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await;
        let Ok(Ok(found)) = lookup else {
            return true;
        };
        found.into_iter().all(|a| self.allow.permits(a.ip(), &host))
    }

    /// An IP-literal address is judged here, because the HTTP client does not resolve literals
    /// and so never reaches the resolver: called whenever a client is built, so removing a range
    /// from `gateway.allowed_private` revokes the store at the next rebuild. Names pass (the
    /// resolver judges them at connect time).
    #[must_use]
    pub fn literal_permitted(&self, address: &str) -> bool {
        match host_of(address) {
            Some((host, _)) => host
                .parse::<IpAddr>()
                .map_or(true, |ip| self.allow.permits(ip, &host)),
            None => false,
        }
    }

    /// Resolver for the store's HTTP client: judges at connect time, so a name that changes its
    /// answer after the address was saved is still refused.
    pub(crate) fn resolver(&self) -> Arc<GuardedResolver> {
        Arc::new(GuardedResolver(self.allow.clone()))
    }
}

pub(crate) struct GuardedResolver(PrivateAllow);

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow = self.0.clone();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let all: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if all.is_empty() || all.iter().any(|a| !allow.permits(a.ip(), &host)) {
                return Err(Box::new(Refused) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(all.into_iter()) as Addrs)
        })
    }
}

/// Which stores of a rebuild are guarded (the dashboard-defined ones) and the policy.
#[derive(Debug, Clone, Default)]
pub struct StoreGuard {
    pub guard: DialGuard,
    pub store_ids: HashSet<String>,
}

impl StoreGuard {
    /// Guard every dashboard-defined store of the merged settings.
    #[must_use]
    pub fn for_dashboard_stores(
        eff: &aurora_config::Effective,
        allowed_private: &[String],
    ) -> Self {
        let store_ids = eff
            .keys
            .get("secrets.stores")
            .and_then(|k| k.entries.as_ref())
            .into_iter()
            .flatten()
            .filter(|e| e.source == aurora_config::Source::Dashboard)
            .map(|e| e.id.clone())
            .collect();
        Self {
            guard: DialGuard::new(allowed_private),
            store_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn literals_are_judged_by_the_shared_table_and_allowed_private() {
        let g = DialGuard::new(&["10.9.0.0/16".to_owned()]);
        assert!(g.permits_address("https://8.8.8.8:8200").await);
        for refused in [
            "http://169.254.169.254",
            "http://127.0.0.1:8200",
            "https://10.1.2.3",
            "https://[::1]:8200",
            "https://[fd00::1]",
            "not a url",
        ] {
            assert!(!g.permits_address(refused).await, "{refused}");
        }
        assert!(g.permits_address("https://10.9.4.4:8200").await);
        // Metadata only by an exact IP entry, never by a CIDR.
        let g = DialGuard::new(&["169.254.0.0/16".to_owned()]);
        assert!(!g.permits_address("http://169.254.169.254").await);
        let g = DialGuard::new(&["169.254.169.254".to_owned()]);
        assert!(g.permits_address("http://169.254.169.254").await);
    }

    #[test]
    fn a_literal_is_rejudged_when_a_client_is_built() {
        let open = DialGuard::new(&["10.9.0.0/16".to_owned()]);
        let closed = DialGuard::new(&[]);
        assert!(open.literal_permitted("https://10.9.4.4:8200"));
        assert!(
            !closed.literal_permitted("https://10.9.4.4:8200"),
            "range removed: revoked"
        );
        assert!(
            closed.literal_permitted("https://vault.example.org"),
            "names are judged at connect"
        );
        assert!(!closed.literal_permitted("http://169.254.169.254"));
    }
}
