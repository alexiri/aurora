//! Package-level rules needing the archive, keys, PSL or other packages.
use std::collections::HashMap;

use aurora_policy::HostPattern;

use crate::Violation;
use crate::manifest::{Manifest, UrlOrTemplate};
use crate::package::{HOOKS_WASM, Package};
use crate::signature::{TrustStore, verify};

/// Inputs from the running instance.
#[derive(Debug, Clone, Default)]
pub struct ValidationContext {
    pub running_version: String,
    /// LLM endpoint hosts, instance external host, internal service hosts,
    /// `*.aurora.internal`.
    pub reserved_hosts: Vec<HostPattern>,
    /// app id -> id of the active package that owns it.
    pub active_app_ids: HashMap<String, String>,
}

/// Full validation: signature, manifest rules (REG-001), wildcard superset,
/// zones, reserved hosts, egress, hooks.wasm presence, cross-package ids.
pub fn package(
    pkg: &Package,
    ctx: &ValidationContext,
    trust: &TrustStore,
    allow_unsigned: bool,
) -> Vec<Violation> {
    let mut out = Vec::new();
    if let Err(v) = verify(pkg, trust, allow_unsigned) {
        out.push(v);
    }
    let m = match pkg.manifest(&ctx.running_version) {
        Ok(m) => m,
        Err(mut v) => {
            out.append(&mut v);
            return out;
        }
    };
    out.extend(super::manifest(&m));
    out.extend(super::wildcard::wildcards(&m));
    zones(&m, &mut out);
    reserved_claims(&m, ctx, &mut out);
    egress(&m, ctx, &mut out);
    hooks_wasm(&m, pkg, &mut out);
    for (i, a) in m.apps.iter().enumerate() {
        if let Some(owner) = ctx.active_app_ids.get(&a.about.id) {
            if *owner != m.package.id {
                out.push(Violation::new(
                    "duplicate_app_id",
                    format!("/apps/{i}/about/id"),
                    format!(
                        "app id `{}` is already used by package `{owner}`",
                        a.about.id
                    ),
                ));
            }
        }
    }
    out
}

fn hooks_wasm(m: &Manifest, pkg: &Package, out: &mut Vec<Violation>) {
    let has = pkg.files.contains_key(HOOKS_WASM);
    let wants = !m.permissions.hooks.is_empty();
    if wants && !has {
        out.push(Violation::new(
            "hooks_wasm_missing",
            HOOKS_WASM,
            "permissions.hooks is non-empty but hooks.wasm is absent",
        ));
    } else if has && !wants {
        out.push(Violation::new(
            "hooks_wasm_unexpected",
            HOOKS_WASM,
            "hooks.wasm present but permissions.hooks is empty",
        ));
    } else if pkg
        .file(HOOKS_WASM)
        .is_some_and(|b| !b.starts_with(b"\0asm"))
    {
        out.push(Violation::new(
            "hooks_wasm_invalid",
            HOOKS_WASM,
            "not a WebAssembly binary",
        ));
    }
}

/// A zone `*.<suffix>` is acceptable iff `<suffix>` is a registrable domain or
/// below one per the public-suffix list (PSL, incl. private section).
pub fn zone_too_broad(zone: &str) -> bool {
    let Some(suffix) = zone.strip_prefix("*.") else {
        return zone == "*" || zone.contains('*');
    };
    !suffix.is_empty() && psl::domain(suffix.as_bytes()).is_none() || suffix.is_empty()
}

fn zones(m: &Manifest, out: &mut Vec<Violation>) {
    let mut chk = |zone: &str, path: String| {
        if zone_too_broad(zone) {
            out.push(Violation::new(
                "zone_too_broad",
                path,
                format!("zone `{zone}` is broader than `*.<registrable domain>`"),
            ));
        }
    };
    for (i, c) in m.oauth_clients.iter().enumerate() {
        if let UrlOrTemplate::Templated(z) = &c.token_url {
            chk(&z.zone, format!("/oauth_clients/{i}/token_url/zone"));
        }
    }
    for (i, a) in m.apps.iter().enumerate() {
        for (j, h) in a.hosts.iter().enumerate() {
            if let Some(t) = &h.tenant {
                chk(&t.zone, format!("/apps/{i}/hosts/{j}/tenant/zone"));
            }
            if let Some(r) = &h.rewrite_host {
                chk(&r.zone, format!("/apps/{i}/hosts/{j}/rewrite_host/zone"));
            }
        }
    }
}

/// Split `host[:port]` pattern into `(prefix, suffix)` around the single `*`,
/// or `(whole, None)` for an exact host. Ports are ignored (reserved
/// regardless of port).
fn shape(p: &str) -> (String, Option<String>) {
    let host = p
        .rsplit_once(':')
        .map_or(p, |(h, _)| h)
        .to_ascii_lowercase();
    match host.split_once('*') {
        Some((a, b)) => (a.to_owned(), Some(b.to_owned())),
        None => (host, None),
    }
}

/// Conservative overlap test: could some host match both patterns?
fn overlaps(a: &HostPattern, b: &HostPattern) -> bool {
    let (ap, asx) = shape(a.as_str());
    let (bp, bsx) = shape(b.as_str());
    match (asx, bsx) {
        (None, None) => ap == bp,
        (Some(_), None) => a.matches(&bp, 443) || wild_matches_ignoring_port(&ap, a, &bp),
        (None, Some(_)) => b.matches(&ap, 443) || wild_matches_ignoring_port(&bp, b, &ap),
        (Some(asx), Some(bsx)) => {
            (ap.starts_with(&bp) || bp.starts_with(&ap))
                && (asx.ends_with(&bsx) || bsx.ends_with(&asx))
        }
    }
}

fn wild_matches_ignoring_port(_prefix: &str, w: &HostPattern, host: &str) -> bool {
    let bare = w.as_str().rsplit_once(':').map_or(w.as_str(), |(h, _)| h);
    HostPattern::parse(bare).is_ok_and(|p| p.matches(host, 443))
}

fn is_reserved(h: &HostPattern, ctx: &ValidationContext) -> bool {
    ctx.reserved_hosts.iter().any(|r| overlaps(h, r))
}

fn reserved_claims(m: &Manifest, ctx: &ValidationContext, out: &mut Vec<Violation>) {
    for (i, a) in m.apps.iter().enumerate() {
        for (j, h) in a.hosts.iter().enumerate() {
            let mut claims = vec![(
                h.matcher.host.as_str(),
                format!("/apps/{i}/hosts/{j}/match/host"),
            )];
            if let Some(t) = &h.tenant {
                claims.push((t.zone.as_str(), format!("/apps/{i}/hosts/{j}/tenant/zone")));
            }
            if let Some(r) = &h.rewrite_host {
                claims.push((
                    r.zone.as_str(),
                    format!("/apps/{i}/hosts/{j}/rewrite_host/zone"),
                ));
            }
            for (host, path) in claims {
                if HostPattern::parse(host).is_ok_and(|p| is_reserved(&p, ctx)) {
                    out.push(Violation::new(
                        "reserved_host",
                        path,
                        format!("`{host}` overlaps a reserved host"),
                    ));
                }
            }
        }
    }
}

fn private_ip(host: &str) -> bool {
    use std::net::IpAddr;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => {
            a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || (a.octets()[0] == 100 && (a.octets()[1] & 0xc0) == 64)
        }
        Ok(IpAddr::V6(a)) => {
            let s = a.segments();
            a.is_loopback()
                || a.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || a.to_ipv4_mapped().is_some()
        }
        Err(_) => false,
    }
}

fn egress(m: &Manifest, ctx: &ValidationContext, out: &mut Vec<Violation>) {
    for (hook, hosts) in &m.permissions.egress {
        for (i, h) in hosts.iter().enumerate() {
            let path = format!("/permissions/egress/{}/{i}", hook_name(*hook));
            let Ok(p) = HostPattern::parse(h) else {
                out.push(Violation::new(
                    "host_pattern_invalid",
                    path,
                    "invalid host pattern",
                ));
                continue;
            };
            let bare = h.rsplit_once(':').map_or(h.as_str(), |(a, _)| a);
            let bare = bare.to_ascii_lowercase();
            if private_ip(&bare) || bare == "localhost" || bare.ends_with(".localhost") {
                out.push(Violation::new(
                    "egress_private",
                    path,
                    format!("egress host `{h}` is private or local"),
                ));
            } else if is_reserved(&p, ctx) {
                out.push(Violation::new(
                    "egress_reserved",
                    path,
                    format!("egress host `{h}` overlaps a reserved host"),
                ));
            }
        }
    }
}

fn hook_name(h: crate::manifest::Hook) -> &'static str {
    use crate::manifest::Hook::*;
    match h {
        ConnectAuthorize => "connect.authorize",
        ConnectExchange => "connect.exchange",
        TokenMint => "token.mint",
        ApprovalSummarize => "approval.summarize",
    }
}
