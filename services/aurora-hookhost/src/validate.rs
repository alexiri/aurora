//! Output validation against the manifest (docs/09 §8.2).

use aurora_policy::HostPattern;
use aurora_registry::manifest::Hook;
use aurora_registry::snapshot::{AppEntry, PackageEntry, Snapshot};
use aurora_summary::{ApprovalSummary, SummaryField, SummarySource};
use reqwest::Url;
use serde_json::Value;

use crate::runtime::{CallError, Outcome, types};

fn bad(m: impl Into<String>) -> CallError {
    CallError::new("internal", format!("invalid hook output: {}", m.into()))
}

fn https_host(url: &str) -> Result<(String, u16), CallError> {
    let u = Url::parse(url).map_err(|_| bad("url is not valid"))?;
    if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
        return Err(bad("url must be https without credentials"));
    }
    let host = u.host_str().ok_or_else(|| bad("url has no host"))?;
    Ok((
        host.trim_end_matches('.').to_ascii_lowercase(),
        u.port_or_known_default().unwrap_or(443),
    ))
}

fn declared_keys(
    kv: &[types::Kv],
    allowed: impl Fn(&str) -> bool,
    what: &str,
) -> Result<(), CallError> {
    for e in kv {
        if !allowed(&e.key) {
            return Err(bad(format!("{what} key `{}` is not declared", e.key)));
        }
    }
    Ok(())
}

pub struct Ctx<'a> {
    pub snapshot: &'a Snapshot,
    pub package: &'a PackageEntry,
    pub app: &'a AppEntry,
}

impl Ctx<'_> {
    fn creds_ok(&self, kv: &[types::Kv]) -> Result<(), CallError> {
        declared_keys(
            kv,
            |k| self.app.credentials.iter().any(|c| c.name == k),
            "credential",
        )
    }

    fn tenant_ok(&self, host: &str) -> Result<(), CallError> {
        if host.is_empty() || host.contains([':', '/', '@', '?', '#']) {
            return Err(bad("tenant-host must be a bare host name"));
        }
        let zones = self
            .snapshot
            .hosts
            .iter()
            .filter(|h| h.app_id == self.app.app_id)
            .filter_map(|h| h.tenant.as_ref());
        if zones
            .map(|t| HostPattern::lenient(&t.zone))
            .any(|p| p.matches(&host.to_ascii_lowercase(), 443))
        {
            Ok(())
        } else {
            Err(bad("tenant-host is outside the declared zone"))
        }
    }

    fn continue_host_ok(&self, url: &str) -> Result<(), CallError> {
        let (host, port) = https_host(url)?;
        let declared = self
            .package
            .egress
            .get(&Hook::ConnectExchange)
            .is_some_and(|l| {
                l.iter()
                    .any(|p| HostPattern::lenient(p).matches(&host, port))
            });
        let oauth = self
            .snapshot
            .oauth_clients
            .iter()
            .filter(|c| c.package_id == self.package.package_id)
            .any(|c| match &c.authorize_url {
                aurora_registry::manifest::UrlOrTemplate::Url(a) => Url::parse(a)
                    .ok()
                    .is_some_and(|u| u.host_str().is_some_and(|h| h.eq_ignore_ascii_case(&host))),
                aurora_registry::manifest::UrlOrTemplate::Templated(z) => {
                    HostPattern::lenient(&z.zone).matches(&host, port)
                }
            });
        if declared || oauth {
            Ok(())
        } else {
            Err(bad("continue url host is not declared"))
        }
    }
}

/// Validate (and for summaries, clamp) a hook outcome.
pub fn validate(ctx: &Ctx<'_>, o: Outcome) -> Result<Outcome, CallError> {
    match &o {
        Outcome::Authorize(a) => {
            https_host(&a.url)?;
        }
        Outcome::Exchange(types::ExchangeOutput::Connected(c)) => {
            ctx.creds_ok(&c.credentials)?;
            declared_keys(
                &c.metadata,
                |k| ctx.app.metadata.iter().any(|m| m.name == k),
                "metadata",
            )?;
            if let Some(h) = &c.tenant_host {
                ctx.tenant_ok(h)?;
            }
        }
        Outcome::Exchange(types::ExchangeOutput::Continue(c)) => {
            ctx.continue_host_ok(&c.url)?;
            if c.continuation.is_empty() {
                return Err(bad("continuation must not be empty"));
            }
        }
        Outcome::Mint(m) => {
            ctx.creds_ok(&m.credentials)?;
            if m.expires_at == 0 {
                return Err(bad("expires-at must be set"));
            }
        }
        Outcome::Summarize(s) => {
            let clamped = ApprovalSummary {
                title: s.title.clone(),
                fields: s
                    .fields
                    .iter()
                    .map(|f| SummaryField {
                        label: f.key.clone(),
                        value: f.value.clone(),
                    })
                    .collect(),
                text: s.text.clone(),
                link: s.link.clone(),
                group: s.group.clone(),
                source: SummarySource::Hook,
            }
            .clamp();
            return Ok(Outcome::Summarize(types::Summary {
                title: clamped.title,
                fields: clamped
                    .fields
                    .into_iter()
                    .map(|f| types::Kv {
                        key: f.label,
                        value: f.value,
                    })
                    .collect(),
                text: clamped.text,
                link: clamped.link,
                group: clamped.group,
            }));
        }
    }
    Ok(o)
}

/// Wire form of a validated outcome (summaries use `{label, value}` fields).
pub fn to_json(o: &Outcome) -> Value {
    match o {
        Outcome::Summarize(s) => serde_json::json!({
            "title": s.title,
            "fields": s.fields.iter().map(|f| serde_json::json!({"key": f.key, "value": f.value})).collect::<Vec<_>>(),
            "text": s.text,
            "link": s.link,
            "group": s.group,
        }),
        other => crate::wire::output_json(other),
    }
}
