//! Onboarding (P4-ONB-2, D-141): the caller's progress and the instance setup status.

use super::dto::*;
use crate::app::AppState;
use crate::domains::admin::dto::IdpRoles;
use crate::domains::placement::service as placement;
use aurora_core::{ApiError, FieldError, limits};
use aurora_db::repo::onboarding as repo;
use aurora_db::repo::{idp_roles, runners};
use chrono::Utc;
use std::collections::HashSet;
use std::time::Duration;
use uuid::Uuid;

/// A gateway heartbeats every 15 s; four missed beats count as down.
const GATEWAY_ONLINE_SECS: f64 = 60.0;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DISMISSED: usize = 200;

fn view(r: Option<repo::OnboardingRow>) -> MeOnboarding {
    match r {
        Some(r) => MeOnboarding {
            welcome_seen: r.welcome_seen,
            guide_dismissed: r.guide_dismissed,
            tour_completed_at: r.tour_completed_at,
        },
        None => MeOnboarding {
            welcome_seen: false,
            guide_dismissed: vec![],
            tour_completed_at: None,
        },
    }
}

pub async fn get_me(state: &AppState, user: Uuid) -> Result<MeOnboarding, ApiError> {
    Ok(view(repo::get(&state.pool, user).await?))
}

pub async fn patch_me(
    state: &AppState,
    user: Uuid,
    req: PatchOnboarding,
) -> Result<MeOnboarding, ApiError> {
    let guide_dismissed = match req.guide_dismissed {
        Some(list) => {
            let mut seen = HashSet::new();
            let list: Vec<Uuid> = list.into_iter().filter(|id| seen.insert(*id)).collect();
            if list.len() > MAX_DISMISSED {
                return Err(ApiError::invalid_request(
                    "Too many dismissed guides.",
                    vec![FieldError::new(
                        "/guide_dismissed",
                        "must have at most 200 entries",
                    )],
                ));
            }
            Some(list)
        }
        None => None,
    };
    let patch = repo::Patch {
        welcome_seen: req.welcome_seen,
        guide_dismissed,
        tour_completed_at: req.tour_completed.map(|done| done.then(Utc::now)),
    };
    Ok(view(Some(repo::apply(&state.pool, user, &patch).await?)))
}

fn check(
    id: impl Into<String>,
    status: SetupState,
    reason: &str,
    message: impl Into<String>,
) -> SetupCheck {
    SetupCheck {
        id: id.into(),
        status,
        reason: reason.to_owned(),
        message: message.into(),
        value: None,
        helm: None,
    }
}

impl SetupCheck {
    fn value(mut self, v: impl Into<String>) -> Self {
        self.value = Some(v.into());
        self
    }
    fn helm(mut self, v: &str) -> Self {
        self.helm = Some(v.to_owned());
        self
    }
}

/// The worst check decides: `todo` over `warning` over `ok`.
fn step(id: &str, ok_reason: &str, checks: Vec<SetupCheck>) -> SetupStep {
    let rank = |s: SetupState| match s {
        SetupState::Ok => 0,
        SetupState::Warning => 1,
        SetupState::Todo => 2,
    };
    // `rev` so the first of several equally bad checks is the one reported.
    let worst = checks.iter().rev().max_by_key(|c| rank(c.status));
    let (status, reason) = match worst {
        Some(c) if c.status != SetupState::Ok => (c.status, c.reason.clone()),
        _ => (SetupState::Ok, ok_reason.to_owned()),
    };
    SetupStep {
        id: id.to_owned(),
        status,
        reason,
        checks,
    }
}

/// Any HTTP answer in time counts as reachable; the body is never read or shown.
async fn reachable(http: &reqwest::Client, url: &str) -> bool {
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, http.get(url).send()).await,
        Ok(Ok(_))
    )
}

pub async fn setup(state: &AppState, admin: Uuid) -> Result<SetupStatus, ApiError> {
    let placed = placement::view(state).await?;
    let providers = &state.config.auth.oidc_providers;

    // 1. Sign-in and roles.
    let you = match idp_roles::get_state(&state.pool, admin).await? {
        Some(st) => {
            let p = providers.iter().find(|p| p.id == st.provider_id);
            let roles = IdpRoles::from(st);
            Some(SetupYou {
                provider_name: p.and_then(|p| p.display_name.clone()),
                role_claim: p.and_then(|p| p.role_claim.clone()),
                provider: roles.provider,
                values: roles.values,
                granted: roles.granted,
            })
        }
        None => None,
    };
    let mut roles = Vec::new();
    if providers.is_empty() {
        roles.push(
            check(
                "providers",
                SetupState::Todo,
                "no_provider",
                "No OIDC provider is configured, so roles cannot come from your sign-in.",
            )
            .helm("auth.oidc_providers"),
        );
    } else {
        let mapped = providers.iter().any(|p| !p.role_mappings.is_empty());
        if mapped {
            roles.push(
                check(
                    "role_mappings",
                    SetupState::Ok,
                    "role_mappings_set",
                    "Roles in the sign-in token decide who is an admin.",
                )
                .helm("auth.oidc_providers[].role_mappings"),
            );
        } else {
            roles.push(
                check(
                    "role_mappings",
                    SetupState::Warning,
                    "no_role_mappings",
                    "No role mappings: only the admin e-mail list makes admins.",
                )
                .helm("auth.oidc_providers[].role_claim, role_mappings"),
            );
        }
    }

    // 2. Placement.
    let mut place = Vec::new();
    if placed.saved {
        place.push(
            check(
                "placement",
                SetupState::Ok,
                "placement_saved",
                "New users are placed as you chose.",
            )
            .value(placed.mode.as_str()),
        );
        if placed.warnings.iter().any(|w| w == "default_org_missing") {
            place.push(check(
                "default_org",
                SetupState::Warning,
                "default_org_missing",
                "The default organization no longer exists.",
            ));
        }
    } else {
        place.push(check(
            "placement",
            SetupState::Todo,
            "placement_unsaved",
            "Choose where new users land.",
        ));
    }

    // 3. LLM access.
    let default_org = placed.default_org.as_ref().map(|o| o.id);
    let counts = repo::setup_counts(&state.pool, default_org, GATEWAY_ONLINE_SECS).await?;
    let endpoints = state.llm.endpoints();
    let probes = futures::future::join_all(
        endpoints
            .iter()
            .map(|e| reachable(&state.llm_http, &e.base_url)),
    )
    .await;
    let mut llm = Vec::new();
    if endpoints.is_empty() {
        llm.push(
            check(
                "endpoints",
                SetupState::Todo,
                "no_endpoints",
                "No LLM endpoint is configured.",
            )
            .helm("llm.endpoints"),
        );
    }
    for (e, up) in endpoints.iter().zip(probes) {
        let name = if e.display_name.is_empty() {
            e.id.as_str()
        } else {
            e.display_name.as_str()
        };
        llm.push(if up {
            check(
                format!("endpoint:{}", e.id),
                SetupState::Ok,
                "endpoint_reachable",
                format!("{name} answers."),
            )
            .value(e.base_url.clone())
        } else {
            check(
                format!("endpoint:{}", e.id),
                SetupState::Warning,
                "endpoint_unreachable",
                format!("{name} did not answer from the API."),
            )
            .value(e.base_url.clone())
            .helm("llm.endpoints[].base_url")
        });
        // FIX-019: a LiteLLM endpoint without a default model leaves agents without a model.
        if e.provider == "litellm" {
            llm.push(match &e.default_model {
                Some(m) => check(
                    format!("default_model:{}", e.id),
                    SetupState::Ok,
                    "default_model_set",
                    format!("{name} has a default model."),
                )
                .value(m.clone()),
                None => check(
                    format!("default_model:{}", e.id),
                    SetupState::Warning,
                    "no_default_model",
                    format!(
                        "{name} has no default model. Agents would need a model chosen one by one."
                    ),
                )
                .helm("llm.endpoints[].default_model"),
            });
        }
    }
    if let Some(org) = &placed.default_org {
        let has_key = counts.org_llm_keys > 0;
        llm.push(
            check(
                "org_key",
                if has_key {
                    SetupState::Ok
                } else {
                    SetupState::Todo
                },
                if has_key {
                    "org_key_present"
                } else {
                    "no_org_key"
                },
                if has_key {
                    format!("{} has an LLM key.", org.name)
                } else {
                    format!("{} has no LLM key yet.", org.name)
                },
            )
            .value(counts.org_llm_keys.to_string()),
        );
        let starter = placed.starter_grants.iter().any(|g| g.detail == "llm");
        llm.push(if starter {
            check(
                "starter_grant",
                SetupState::Ok,
                "starter_grant_set",
                "Every new user's agent gets the key.",
            )
        } else {
            check(
                "starter_grant",
                SetupState::Todo,
                "no_starter_grant",
                "New users' agents get no key yet.",
            )
        });
    }

    // 4. Agents can run.
    let online = limits::REPLICA_ONLINE_THRESHOLD.as_secs_f64();
    let rows = runners::list(&state.pool).await?;
    let stats = runners::stats(&state.pool, online).await?;
    let live: Vec<&runners::RunnerRow> = rows
        .iter()
        .filter(|r| {
            stats
                .iter()
                .any(|s| s.runner_id == r.id && s.live_replicas > 0)
        })
        .collect();
    let mut run = Vec::new();
    if rows.is_empty() {
        run.push(check(
            "runners",
            SetupState::Todo,
            "no_runner",
            "No runner is registered, so no agent can start.",
        ));
    } else if live.is_empty() {
        run.push(
            check(
                "runners",
                SetupState::Warning,
                "runner_offline",
                "Runners are registered but none is online.",
            )
            .value(rows.len().to_string()),
        );
    } else {
        run.push(
            check(
                "runners",
                SetupState::Ok,
                "runner_online",
                format!("{} runner(s) online.", live.len()),
            )
            .value(live.len().to_string()),
        );
        let cap = |r: &runners::RunnerRow, k: &str| {
            r.capabilities.as_ref().and_then(|c| c.get(k)).cloned()
        };
        if let Some(image) = live
            .iter()
            .find_map(|r| cap(r, "image").and_then(|v| v.as_str().map(str::to_owned)))
        {
            run.push(
                check(
                    "image",
                    SetupState::Ok,
                    "image_known",
                    "The sandbox image agents start from.",
                )
                .value(image)
                .helm("runner.image"),
            );
        }
        let mut harnesses: Vec<String> = live
            .iter()
            .filter_map(|r| cap(r, "harnesses"))
            .filter_map(|v| v.as_array().cloned())
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        harnesses.sort();
        harnesses.dedup();
        if !harnesses.is_empty() {
            run.push(
                check(
                    "harnesses",
                    SetupState::Ok,
                    "harnesses_known",
                    "The agent programs the runners can start.",
                )
                .value(harnesses.join(", ")),
            );
        }
    }
    run.push(if counts.gateways_live > 0 {
        check(
            "gateway",
            SetupState::Ok,
            "gateway_online",
            "The gateway is up: agents reach the internet through it.",
        )
        .value(counts.gateways_live.to_string())
    } else {
        check(
            "gateway",
            SetupState::Todo,
            "no_gateway",
            "No gateway is running. Agents cannot reach the internet or the LLM without it.",
        )
    });

    // 5. Optional.
    let mut opt = vec![
        check(
            "oauth_clients",
            if counts.instance_oauth_clients > 0 {
                SetupState::Ok
            } else {
                SetupState::Todo
            },
            if counts.instance_oauth_clients > 0 {
                "oauth_clients_set"
            } else {
                "no_oauth_clients"
            },
            "OAuth clients let everyone connect apps.",
        )
        .value(counts.instance_oauth_clients.to_string()),
        check(
            "connectors",
            if counts.managed_connectors > 0 {
                SetupState::Ok
            } else {
                SetupState::Todo
            },
            if counts.managed_connectors > 0 {
                "connectors_set"
            } else {
                "no_connectors"
            },
            "Connectors bring chat tools to your agents.",
        )
        .value(counts.managed_connectors.to_string()),
    ];
    if counts.plugins_pending > 0 {
        opt.push(
            check(
                "plugins",
                SetupState::Warning,
                "plugins_pending",
                "Plugin versions are waiting for your approval.",
            )
            .value(counts.plugins_pending.to_string()),
        );
    }
    let mut optional = step("optional", "optional_ok", opt);
    // Nothing here blocks: a missing optional item is a suggestion, not a to-do.
    if optional.status == SetupState::Todo {
        optional.status = SetupState::Ok;
        optional.reason = "optional".to_owned();
    }

    Ok(SetupStatus {
        placement_saved: placed.saved,
        you,
        steps: vec![
            step("roles", "roles_ok", roles),
            step("placement", "placement_saved", place),
            step("llm", "llm_ok", llm),
            step("runners", "runners_ok", run),
            optional,
        ],
    })
}
