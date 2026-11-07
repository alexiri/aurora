//! Governance helpers: instance ceiling from config, effective view, governed-host overlap.
use crate::app::AppState;
use aurora_core::ApiError;
use aurora_llm::ProviderRegistry;
use aurora_llm::governance::{
    AllowEntry, EffectiveGovernance, EgressPosture, Instance, InstanceEndpoint, LlmPosture,
    OrgGovernance, effective,
};
use uuid::Uuid;

pub fn instance_from_config(
    g: &aurora_config::Governance,
    registry: &ProviderRegistry,
) -> Instance {
    Instance {
        allowed_key_scopes: g.allowed_key_scopes.clone(),
        llm_minimum_posture: match g.llm_minimum_posture {
            aurora_config::LlmPosture::Open => LlmPosture::Open,
            aurora_config::LlmPosture::SanctionedOnly => LlmPosture::SanctionedOnly,
        },
        egress_minimum_posture: match g.egress_minimum_posture {
            aurora_config::EgressPosture::Open => EgressPosture::Open,
            aurora_config::EgressPosture::CredentialedOnly => EgressPosture::CredentialedOnly,
        },
        require_key_origin: g.require_key_origin,
        endpoints: registry
            .endpoints()
            .iter()
            .map(|ep| {
                let p = registry.provider_for(ep);
                InstanceEndpoint {
                    id: ep.id.clone(),
                    allowed_models: ep.allowed_models.clone(),
                    key_origin: ep.key_origin.clone(),
                    hosts: p.map(|p| p.hosts(ep)).unwrap_or_default(),
                    origin_methods: p.map(|p| p.origin_methods().to_vec()).unwrap_or_default(),
                }
            })
            .collect(),
        known_llm_hosts: g.known_llm_hosts.clone(),
        baseline_allowlist: g
            .baseline_allowlist
            .iter()
            .map(|e| AllowEntry {
                host: e.host.clone(),
                path: e.path.clone(),
            })
            .collect(),
    }
}

/// Effective governance of an org; a malformed stored document fails closed (500).
pub async fn effective_for(state: &AppState, org: Uuid) -> Result<EffectiveGovernance, ApiError> {
    let raw = aurora_db::repo::gateway::org_governance(&state.pool, org)
        .await?
        .ok_or_else(|| ApiError::not_found("Organization not found."))?;
    let doc: OrgGovernance = aurora_llm::governance::from_stored(&raw, &state.gov_instance)
        .map_err(|e| {
            tracing::error!(%org, error = %e, "malformed organization governance document");
            ApiError::internal("Internal error.")
        })?;
    Ok(effective(&state.gov_instance, &doc))
}

enum Shape<'a> {
    Exact(&'a str),
    Wild(&'a str, &'a str),
}

fn split(p: &str) -> (Shape<'_>, Option<&str>) {
    let (host, port) = match p.rsplit_once(':') {
        Some((h, po)) if !po.is_empty() && po.bytes().all(|b| b.is_ascii_digit()) => (h, Some(po)),
        _ => (p, None),
    };
    match host.split_once('*') {
        Some((a, b)) => (Shape::Wild(a, b), port),
        None => (Shape::Exact(host), port),
    }
}

/// Conservative: can some host:port match both patterns? (A missing port matches any port.)
pub fn patterns_overlap(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    let ((sa, pa), (sb, pb)) = (split(&a), split(&b));
    if let (Some(x), Some(y)) = (pa, pb)
        && x != y
    {
        return false;
    }
    match (sa, sb) {
        (Shape::Exact(x), Shape::Exact(y)) => x == y,
        (Shape::Exact(x), Shape::Wild(p, s)) | (Shape::Wild(p, s), Shape::Exact(x)) => {
            x.len() > p.len() + s.len() && x.starts_with(p) && x.ends_with(s)
        }
        (Shape::Wild(p1, s1), Shape::Wild(p2, s2)) => {
            (p1.starts_with(p2) || p2.starts_with(p1)) && (s1.ends_with(s2) || s2.ends_with(s1))
        }
    }
}

/// Governed LLM hosts: endpoint hosts plus known LLM hosts.
pub fn governed_hosts(inst: &Instance) -> Vec<&str> {
    inst.endpoints
        .iter()
        .flat_map(|e| e.hosts.iter())
        .chain(inst.known_llm_hosts.iter())
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::patterns_overlap as o;
    #[test]
    fn overlap() {
        assert!(o("api.openai.com", "api.openai.com"));
        assert!(o("*.openai.com", "api.openai.com"));
        assert!(o("*", "api.openai.com"));
        assert!(o("api.*", "*.com"));
        assert!(!o("api.example.com", "api.openai.com"));
        assert!(!o("api.openai.com:8443", "api.openai.com:9000"));
        assert!(o("api.openai.com", "api.openai.com:8443"));
        assert!(!o("*.example.com", "api.openai.com"));
    }
}
