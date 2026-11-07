use super::dto::*;
use crate::app::AppState;
use crate::domains::secrets::gov;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::{credentials, governance as repo};
use aurora_llm::governance::{
    self as g, BELOW_MIN, EffectiveGovernance as Eff, OriginStatus, Source, StoredOrigin,
};
use aurora_llm::{KeyOriginRule, ModelRef};
use serde_json::{Value, json};
use uuid::Uuid;

fn stale() -> ApiError {
    ApiError::conflict(
        "stale_revision",
        "The governance document changed since you loaded it. Reload and try again.",
    )
}

fn not_found() -> ApiError {
    ApiError::not_found("Organization not found.")
}

fn malformed(org: Uuid, e: &serde_json::Error) -> ApiError {
    tracing::error!(%org, error = %e, "malformed organization governance document");
    ApiError::internal("Internal error.")
}

fn src(s: Source) -> String {
    match s {
        Source::Instance => "instance",
        Source::Org => "org",
    }
    .to_owned()
}

fn sourced<T>(value: T, s: Source) -> Sourced<T> {
    Sourced {
        value,
        source: src(s),
    }
}

fn llm_posture(p: g::LlmPosture) -> String {
    match p {
        g::LlmPosture::Open => "open",
        g::LlmPosture::SanctionedOnly => "sanctioned_only",
    }
    .to_owned()
}

fn egress_posture(p: g::EgressPosture) -> String {
    match p {
        g::EgressPosture::Open => "open",
        g::EgressPosture::CredentialedOnly => "credentialed_only",
    }
    .to_owned()
}

fn open_lane_posture(p: g::OpenLanePosture) -> String {
    p.as_str().to_owned()
}

fn rule(r: &KeyOriginRule) -> OriginRule {
    OriginRule {
        method: r.method.as_str().to_owned(),
        allowed_organization_ids: r.allowed_organization_ids.clone(),
        allowed_workspace_ids: r.allowed_workspace_ids.clone(),
        allowed_team_ids: r.allowed_team_ids.clone(),
    }
}

fn allow(a: &g::AllowEntry) -> AllowlistEntry {
    AllowlistEntry {
        host: a.host.clone(),
        path: a.path.clone(),
    }
}

pub async fn get(state: &AppState, org: Uuid) -> Result<GovernanceDocument, ApiError> {
    let (raw, revision) = repo::get(&state.pool, org).await?.ok_or_else(not_found)?;
    let doc = g::from_stored(&raw, &state.gov_instance).map_err(|e| malformed(org, &e))?;
    Ok(GovernanceDocument {
        document: serde_json::to_value(doc).map_err(|_| ApiError::internal("Internal error."))?,
        revision,
    })
}

/// Why a secret does not comply with the effective governance (`None` = compliant).
fn classify(eff: &Eff, s: &repo::LlmSecretRef) -> Option<&'static str> {
    let ep = s.llm_endpoint_id.as_deref()?;
    let e = eff.endpoints.get(ep).filter(|e| e.permitted);
    let Some(e) = e else {
        return Some("not_permitted");
    };
    let scope = if s.workspace_id.is_some() {
        "workspace"
    } else {
        "organization"
    };
    if !e.key_scopes.iter().any(|k| k == scope) {
        return Some("not_permitted");
    }
    e.origin_rule.as_ref()?;
    let Some(origin) = s.origin.as_ref() else {
        return Some("pending");
    };
    let Ok(stored) = serde_json::from_value::<StoredOrigin>(origin.clone()) else {
        return Some("pending");
    };
    match stored.status {
        OriginStatus::Pending => Some("pending"),
        OriginStatus::Unverifiable => Some("unverifiable"),
        OriginStatus::Mismatch => Some("mismatch"),
        OriginStatus::Verified => {
            (!eff.origin_complies(ep, origin, i64::from(s.value_version))).then_some("mismatch")
        }
    }
}

pub async fn effective(state: &AppState, org: Uuid) -> Result<EffectiveGovernance, ApiError> {
    let eff = gov::effective_for(state, org).await?;
    let secrets = repo::llm_secrets(&state.pool, org).await?;
    let endpoints = state
        .llm
        .endpoints()
        .iter()
        .filter_map(|ep| {
            let e = eff.endpoints.get(&ep.id)?;
            Some(EffectiveEndpoint {
                id: ep.id.clone(),
                provider: ep.provider.clone(),
                display_name: ep.display_name.clone(),
                permitted: sourced(e.permitted, e.permitted_source),
                key_scopes: sourced(e.key_scopes.clone(), e.key_scopes_source),
                allowed_models: sourced(e.allowed_models.clone(), e.allowed_models_source),
                origin_rule: e.origin_rule.as_ref().map(rule),
            })
        })
        .collect();
    let (mut org_list, mut baseline) = (vec![], vec![]);
    for a in &eff.egress_allowlist {
        let entry = AllowlistEntry {
            host: a.host.as_str().to_owned(),
            path: a.path.as_ref().map(|p| p.as_str().to_owned()),
        };
        match a.source {
            Source::Org => org_list.push(entry),
            Source::Instance => baseline.push(entry),
        }
    }
    let non_compliant_secrets = secrets
        .iter()
        .filter_map(|s| {
            classify(&eff, s).map(|r| NonCompliantSecret {
                id: s.id,
                name: s.name.clone(),
                workspace_id: s.workspace_id,
                reason: r.to_owned(),
            })
        })
        .collect();
    let org_doc = aurora_db::repo::gateway::org_governance(&state.pool, org)
        .await?
        .ok_or_else(not_found)?;
    let org_byo = g::OrgByo::from_stored(org_doc.get("byo"));
    let byo = effective_byo_view(&state.gov_instance, &org_byo);
    Ok(EffectiveGovernance {
        byo,
        llm: EffectiveLlm {
            posture: sourced(llm_posture(eff.llm_posture), eff.llm_posture_source),
            endpoints,
        },
        egress: EffectiveEgress {
            posture: sourced(
                egress_posture(eff.egress_posture),
                eff.egress_posture_source,
            ),
            open_lane: sourced(open_lane_posture(eff.open_lane), eff.open_lane_source),
            org_allowlist: org_list,
            baseline_allowlist: baseline,
        },
        non_compliant_secrets,
    })
}

/// Org-level effective BYO controls with the level that decided each value (RFE-025).
fn effective_byo_view(inst: &g::Instance, org: &g::OrgByo) -> EffectiveByo {
    let e = g::effective_byo(
        &inst.byo,
        &g::ByoInputs {
            org: org.clone(),
            ..g::ByoInputs::default()
        },
    );
    let lvl = |l: Option<g::ByoLevel>| match l {
        Some(g::ByoLevel::Org) => "org".to_owned(),
        _ => "instance".to_owned(),
    };
    EffectiveByo {
        posture: Sourced {
            value: e.posture.as_str().to_owned(),
            source: lvl(Some(e.posture_level)),
        },
        source_cidrs: Sourced {
            value: e.source_strings(),
            source: lvl(e.source_level),
        },
        require_context_credentials: Sourced {
            value: e.require_context,
            source: lvl(e.require_context_level),
        },
    }
}

/// Result of a governance change, for the audit record.
pub struct Applied {
    pub out: GovernanceDocument,
    pub secrets_marked: Vec<Uuid>,
    pub agents_cleared: Vec<Uuid>,
}

pub async fn put(state: &AppState, org: Uuid, req: PutGovernance) -> Result<Applied, ApiError> {
    // Cheap pre-check so stale clients get 409 before validation noise; re-checked under lock.
    let (_, current) = repo::get(&state.pool, org).await?.ok_or_else(not_found)?;
    if current != req.revision {
        return Err(stale());
    }
    let doc = g::validate(&req.document, &state.gov_instance).map_err(|errs| {
        let code = if errs.iter().any(|e| e.code == BELOW_MIN) {
            "governance_below_instance_minimum"
        } else if errs.iter().any(|e| e.code == g::BYO_CIDR_OUTSIDE_DEFAULT) {
            // (RFE-025) an org source list entry outside the instance default list.
            "byo_cidr_outside_default"
        } else {
            "governance_invalid"
        };
        let fields: Vec<Value> = errs
            .iter()
            .map(|e| json!({"path": e.path, "message": e.message, "code": e.code}))
            .collect();
        ApiError::unprocessable(code, "The governance document is not valid.")
            .with_details(json!({ "fields": fields }))
    })?;
    let stored = serde_json::to_value(&doc).map_err(|_| ApiError::internal("Internal error."))?;

    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let (_, rev) = repo::lock(&mut tx, org).await?.ok_or_else(not_found)?;
    if rev != req.revision {
        return Err(stale());
    }
    let revision = repo::store(&mut tx, org, &stored).await?;

    // Re-evaluate stored origins and agent model overrides (no provider calls).
    let eff = g::effective(&state.gov_instance, &doc);
    let secrets = repo::llm_secrets(&mut *tx, org).await?;
    let secrets_marked: Vec<Uuid> = secrets
        .iter()
        .filter(|s| {
            classify(&eff, s) == Some("mismatch")
                && s.origin
                    .as_ref()
                    .and_then(|o| o.get("status"))
                    .and_then(Value::as_str)
                    == Some("verified")
        })
        .map(|s| s.id)
        .collect();
    if !secrets_marked.is_empty() {
        repo::mark_mismatch(&mut tx, &secrets_marked).await?;
    }
    let agents_cleared: Vec<Uuid> = repo::agent_models(&mut tx, org)
        .await?
        .into_iter()
        .filter(|a| {
            let m = a
                .model_id
                .clone()
                .map_or(ModelRef::NoModel, ModelRef::Model);
            eff.model_allowed(&a.model_endpoint_id, &m).is_err()
        })
        .map(|a| a.id)
        .collect();
    if !agents_cleared.is_empty() {
        repo::clear_agent_models(&mut tx, &agents_cleared).await?;
    }
    credentials::notify_gateway_invalidate(&mut *tx, "org", org).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Applied {
        out: GovernanceDocument {
            document: stored,
            revision,
        },
        secrets_marked,
        agents_cleared,
    })
}

pub fn instance(state: &AppState) -> InstanceGovernance {
    let i = &state.gov_instance;
    InstanceGovernance {
        llm: InstanceLlm {
            allowed_key_scopes: i.allowed_key_scopes.clone(),
            minimum_posture: llm_posture(i.llm_minimum_posture),
            require_key_origin: i.require_key_origin,
            known_llm_hosts: i.known_llm_hosts.clone(),
            endpoints: state
                .llm
                .endpoints()
                .iter()
                .map(|ep| InstanceEndpoint {
                    id: ep.id.clone(),
                    provider: ep.provider.clone(),
                    display_name: ep.display_name.clone(),
                    allowed_models: ep.allowed_models.clone(),
                    key_origin: ep.key_origin.as_ref().map(rule),
                })
                .collect(),
        },
        egress: InstanceEgress {
            minimum_posture: egress_posture(i.egress_minimum_posture),
            open_lane_minimum: open_lane_posture(i.open_lane_minimum),
            baseline_allowlist: i.baseline_allowlist.iter().map(allow).collect(),
        },
        byo: InstanceByo {
            enabled: i.byo.enabled,
            minimum_posture: i.byo.minimum_posture.as_str().to_owned(),
            default_source_cidrs: i
                .byo
                .default_source_cidrs
                .iter()
                .map(ToString::to_string)
                .collect(),
            require_context_credentials: i.byo.require_context_credentials,
        },
    }
}
