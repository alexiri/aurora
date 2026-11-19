//! Where organizations stand against the instance ceiling (SET-2, C-778): the effective postures
//! of each organization, the settings where it is stricter than the instance floor or clamped by
//! it, and the per-key before/after of a pending instance change (the settings preview's
//! `affected_orgs`, C-774). One computation serves both. Metadata only: never stored content.

use crate::domains::settings::dto::{AffectedChange, AffectedOrg};
use crate::http::pagination::{Cursor, Page, Pagination};
use aurora_core::ApiError;
use aurora_db::repo::governance as repo;
use aurora_llm::governance::{self as g, Instance};
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::ToSchema;

/// Effective values of one organization under one instance ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    pub llm_posture: String,
    pub egress_posture: String,
    pub open_lane: String,
    pub byo_posture: String,
    pub token_max_age_days: Option<u32>,
    pub baseline_allowlist: Vec<String>,
    pub key_scopes: Vec<String>,
    /// Settings where the organization's own value is tighter than the instance floor.
    pub stricter: Vec<String>,
    /// Settings where the organization's own value is looser than the floor and is overridden.
    pub clamped: Vec<String>,
}

fn llm_str(p: g::LlmPosture) -> &'static str {
    match p {
        g::LlmPosture::Open => "open",
        g::LlmPosture::SanctionedOnly => "sanctioned_only",
    }
}

fn egress_str(p: g::EgressPosture) -> &'static str {
    match p {
        g::EgressPosture::Open => "open",
        g::EgressPosture::CredentialedOnly => "credentialed_only",
    }
}

/// The standing of an organization whose stored document is `raw`; `None` for a document that
/// does not parse (it is skipped, the org's own page reports it).
pub fn standing(inst: &Instance, raw: &Value) -> Option<Standing> {
    let doc = g::from_stored(raw, inst).ok()?;
    let eff = g::effective(inst, &doc);
    let byo = g::effective_byo(
        &inst.byo,
        &g::ByoInputs {
            org: doc.byo.clone(),
            ..g::ByoInputs::default()
        },
    );
    let (days, _) = g::effective_agent_token_max_age(
        inst.agent_token_max_age_days,
        doc.agent_tokens.max_age_days,
    );
    let mut stricter = Vec::new();
    let mut clamped = Vec::new();
    let mut cmp = |key: &str, org: std::cmp::Ordering| match org {
        std::cmp::Ordering::Greater => stricter.push(key.to_owned()),
        std::cmp::Ordering::Less => clamped.push(key.to_owned()),
        std::cmp::Ordering::Equal => {}
    };
    cmp(
        "governance.llm.minimum_posture",
        doc.llm.posture.cmp(&inst.llm_minimum_posture),
    );
    cmp(
        "governance.egress.minimum_posture",
        doc.egress.posture.cmp(&inst.egress_minimum_posture),
    );
    cmp(
        "governance.egress.open_lane_minimum",
        doc.egress.open_lane.cmp(&inst.open_lane_minimum),
    );
    if let Some(p) = doc.byo.posture {
        cmp("byo.minimum_posture", p.cmp(&inst.byo.minimum_posture));
    }
    if let (Some(o), Some(i)) = (doc.agent_tokens.max_age_days, inst.agent_token_max_age_days) {
        // A smaller maximum is the stricter one.
        cmp("governance.agent_tokens.max_age_days", i.cmp(&o));
    }
    let mut key_scopes: Vec<String> = eff
        .endpoints
        .values()
        .flat_map(|e| e.key_scopes.iter().cloned())
        .collect();
    key_scopes.sort();
    key_scopes.dedup();
    Some(Standing {
        llm_posture: llm_str(eff.llm_posture).to_owned(),
        egress_posture: egress_str(eff.egress_posture).to_owned(),
        open_lane: eff.open_lane.as_str().to_owned(),
        byo_posture: byo.posture.as_str().to_owned(),
        token_max_age_days: days,
        baseline_allowlist: eff
            .baseline_allowlist
            .iter()
            .map(|a| match &a.path {
                Some(p) => format!("{}{}", a.host.as_str(), p.as_str()),
                None => a.host.as_str().to_owned(),
            })
            .collect(),
        key_scopes,
        stricter,
        clamped,
    })
}

/// Per-key before/after between two standings (the registry keys of the `governance` section).
pub fn changes(before: &Standing, after: &Standing) -> Vec<AffectedChange> {
    let mut out = Vec::new();
    let mut add = |key: &str, b: Value, a: Value| {
        if b != a {
            out.push(AffectedChange {
                key: key.to_owned(),
                before: b,
                after: a,
            });
        }
    };
    add(
        "governance.llm.minimum_posture",
        json!(before.llm_posture),
        json!(after.llm_posture),
    );
    add(
        "governance.egress.minimum_posture",
        json!(before.egress_posture),
        json!(after.egress_posture),
    );
    add(
        "governance.egress.open_lane_minimum",
        json!(before.open_lane),
        json!(after.open_lane),
    );
    add(
        "byo.minimum_posture",
        json!(before.byo_posture),
        json!(after.byo_posture),
    );
    add(
        "governance.agent_tokens.max_age_days",
        json!(before.token_max_age_days),
        json!(after.token_max_age_days),
    );
    add(
        "governance.egress.baseline_allowlist",
        json!(before.baseline_allowlist),
        json!(after.baseline_allowlist),
    );
    add(
        "governance.llm.allowed_key_scopes",
        json!(before.key_scopes),
        json!(after.key_scopes),
    );
    out
}

/// One row of `GET /v1/admin/settings/governance/orgs` (C-778).
#[derive(Debug, Serialize, ToSchema)]
pub struct OrgStandingRow {
    pub org_id: uuid::Uuid,
    pub slug: String,
    pub name: String,
    pub frozen: bool,
    pub llm_posture: String,
    pub egress_posture: String,
    pub open_lane: String,
    pub byo_posture: String,
    pub stricter: Vec<String>,
    pub clamped: Vec<String>,
}

/// The organizations page (newest first).
pub async fn list(
    pool: &aurora_db::PgPool,
    inst: &Instance,
    page: Pagination,
) -> Result<Page<OrgStandingRow>, ApiError> {
    let rows = repo::orgs_page(pool, page.after_key(), page.fetch()).await?;
    let limit = page.limit as usize;
    let next_cursor = (rows.len() > limit).then(|| {
        let r = &rows[limit - 1];
        Cursor {
            t: r.created_at,
            id: r.id,
        }
        .encode()
    });
    let items = rows
        .into_iter()
        .take(limit)
        .filter_map(|r| {
            let s = standing(inst, &r.governance)?;
            Some(OrgStandingRow {
                org_id: r.id,
                slug: r.slug,
                name: r.name,
                frozen: r.frozen,
                llm_posture: s.llm_posture,
                egress_posture: s.egress_posture,
                open_lane: s.open_lane,
                byo_posture: s.byo_posture,
                stricter: s.stricter,
                clamped: s.clamped,
            })
        })
        .collect();
    Ok(Page { items, next_cursor })
}

/// The organizations whose effective values differ between two instance ceilings: the first
/// `keep` of them in full and the total.
pub async fn affected(
    pool: &aurora_db::PgPool,
    before: &Instance,
    after: &Instance,
    keep: usize,
) -> Result<(Vec<AffectedOrg>, i64), ApiError> {
    let mut out = Vec::new();
    let mut total = 0i64;
    let mut cursor = None;
    loop {
        let rows = repo::orgs_page(pool, cursor, 200).await?;
        let Some(last) = rows.last() else {
            break;
        };
        cursor = Some((last.created_at, last.id));
        for r in &rows {
            let (Some(b), Some(a)) = (
                standing(before, &r.governance),
                standing(after, &r.governance),
            ) else {
                continue;
            };
            let changes = changes(&b, &a);
            if changes.is_empty() {
                continue;
            }
            total += 1;
            if out.len() < keep {
                out.push(AffectedOrg {
                    org_id: r.id,
                    slug: r.slug.clone(),
                    name: r.name.clone(),
                    changes,
                });
            }
        }
    }
    Ok((out, total))
}

/// What a change of the LLM endpoints does to the organizations' keys (SET-3, `08 §11.2`): the
/// organizations whose LLM keys stop being usable (`not_permitted`: the endpoint is gone, no
/// longer permitted or its key scope is; `mismatch`: its verified origin no longer complies),
/// the first `keep` of them with their counts, the number of organizations and of keys. Metadata
/// only. One pass over every organization, like [`affected`].
pub async fn keys_affected(
    pool: &aurora_db::PgPool,
    before: &Instance,
    after: &Instance,
    keep: usize,
) -> Result<(Vec<AffectedOrg>, i64, i64), ApiError> {
    use crate::domains::governance::service::classify;
    let usable = |eff: &g::EffectiveGovernance, s: &repo::LlmSecretRef| classify(eff, s).is_none();
    let (mut out, mut orgs, mut keys) = (Vec::new(), 0i64, 0i64);
    let mut cursor = None;
    loop {
        let rows = repo::orgs_page(pool, cursor, 200).await?;
        let Some(last) = rows.last() else {
            break;
        };
        cursor = Some((last.created_at, last.id));
        for r in &rows {
            let (Ok(db), Ok(da)) = (
                g::from_stored(&r.governance, before),
                g::from_stored(&r.governance, after),
            ) else {
                continue;
            };
            let (eb, ea) = (g::effective(before, &db), g::effective(after, &da));
            let secrets = repo::llm_secrets(pool, r.id).await?;
            let (mut now, mut then) = (0i64, 0i64);
            let mut lost = 0i64;
            for s in &secrets {
                let (b, a) = (usable(&eb, s), usable(&ea, s));
                now += i64::from(b);
                then += i64::from(a);
                lost += i64::from(b && !a);
            }
            if lost == 0 {
                continue;
            }
            orgs += 1;
            keys += lost;
            if out.len() < keep {
                out.push(AffectedOrg {
                    org_id: r.id,
                    slug: r.slug.clone(),
                    name: r.name.clone(),
                    changes: vec![AffectedChange {
                        key: "llm.endpoints".to_owned(),
                        before: json!({ "usable_keys": now }),
                        after: json!({ "usable_keys": then }),
                    }],
                });
            }
        }
    }
    Ok((out, orgs, keys))
}
