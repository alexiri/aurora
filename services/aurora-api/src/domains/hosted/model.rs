//! Model selection for hosted agents (docs/08 §Model selection for hosted agents).

use crate::app::AppState;
use crate::domains::secrets::gov;
use aurora_core::ApiError;
use aurora_db::repo::hosted::{self, LlmCandidate};
use aurora_llm::governance::{EffectiveGovernance, OriginStatus, StoredOrigin};
use aurora_llm::{AuthMode, Dialect, Endpoint, ModelRef, ProviderRegistry};
use aurora_protocol::runner::{ModelChoice, ModelMeta, ModelRoleIds};
use serde_json::Value;
use sqlx::PgConnection;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Dialects each harness speaks, **in preference order** (`12 §Harness selection and adapter
/// contract`, declared by the adapters): the dialect used is the first one the endpoint serves.
pub fn harness_dialects(harness: &str) -> &'static [Dialect] {
    match harness {
        // The pinned Codex CLI (0.167.0) dropped `wire_api = "chat"`: a version that drops a
        // dialect removes it here (`12 §Harness selection`, D-153).
        "codex" => &[Dialect::OpenaiResponses],
        // (phase 5, P5-HARN, D-362(c)) Declared by the adapters; Part A of the brief checks each
        // list against the pinned binary. None declares Bedrock or Vertex: their SDKs sign with
        // cloud credentials, so `litellm` serves those.
        "pi" | "opencode" => &[
            Dialect::AnthropicMessages,
            Dialect::OpenaiResponses,
            Dialect::OpenaiChat,
        ],
        "goose" => &[Dialect::AnthropicMessages, Dialect::OpenaiChat],
        // (D-260(a)) Claude Code reaches Anthropic, and Bedrock and Vertex through their own dialects.
        _ => &[
            Dialect::AnthropicMessages,
            Dialect::AnthropicBedrock,
            Dialect::AnthropicVertex,
        ],
    }
}

/// The dialect used: the **first of the harness's own list** that the endpoint also serves
/// (`12 §Harness selection`, `08 §Model selection`); `None` = the key is `no_shared_dialect`.
pub fn first_shared(harness: &str, endpoint: &[Dialect]) -> Option<Dialect> {
    harness_dialects(harness)
        .iter()
        .copied()
        .find(|d| endpoint.contains(d))
}

#[cfg(test)]
mod dialect_tests {
    use super::*;

    #[test]
    fn dialect_is_the_first_of_the_harness_list_the_endpoint_serves() {
        use Dialect::{AnthropicMessages as A, OpenaiChat as C, OpenaiResponses as R};
        // LiteLLM serves all three (chat listed first): codex still gets `responses`.
        assert_eq!(first_shared("codex", &[C, R, A]), Some(R));
        assert_eq!(first_shared("claude-code", &[C, R, A]), Some(A));
        // Chat-only and anthropic-only endpoints do not serve the pinned Codex.
        assert_eq!(first_shared("codex", &[C]), None);
        assert_eq!(first_shared("codex", &[A]), None);
        assert_eq!(first_shared("claude-code", &[C, R]), None);
        assert_eq!(first_shared("codex", &[]), None);
    }

    #[test]
    fn registry_names_every_known_harness_and_dialects_are_declared() {
        for (id, name) in HARNESSES {
            assert!(!name.is_empty() && !harness_dialects(id).is_empty(), "{id}");
        }
        assert_eq!(harness_dialects("codex"), &[Dialect::OpenaiResponses]);
    }

    #[test]
    fn harness_choices_in_the_settings_words_equal_the_table_without_fake() {
        let table: Vec<(&str, &str)> = HARNESSES
            .iter()
            .filter(|(id, _)| *id != "fake")
            .copied()
            .collect();
        let choices: Vec<(&str, &str)> = aurora_config::HARNESS_CHOICES
            .iter()
            .map(|c| (c.value, c.label))
            .collect();
        assert_eq!(choices, table);
    }

    #[test]
    fn new_harnesses_serve_chat_only_endpoints_and_the_registry_matches_config() {
        use Dialect::{AnthropicMessages as A, OpenaiChat as C, OpenaiResponses as R};
        // `openai_chat`-only endpoints are usable by an enabled harness for the first time.
        for h in ["pi", "goose", "opencode"] {
            assert_eq!(first_shared(h, &[C]), Some(C), "{h}");
            assert_eq!(first_shared(h, &[A, C, R]), Some(A), "{h}");
        }
        assert_eq!(first_shared("goose", &[R]), None);
        assert_eq!(first_shared("pi", &[R]), Some(R));
        let ids: Vec<&str> = HARNESSES.iter().map(|(i, _)| *i).collect();
        let mut known: Vec<&str> = aurora_config::KNOWN_HARNESSES.to_vec();
        known.sort_unstable();
        let mut mine = ids.clone();
        mine.sort_unstable();
        assert_eq!(mine, known);
    }
}

#[cfg(test)]
mod listing_tests {
    //! (FIX-170, D-372) The resolution order of `08 §Model selection`, step by step.
    use super::*;

    fn ep(default: Option<&str>, models: &[&str]) -> Endpoint {
        Endpoint {
            id: "aigw".into(),
            provider: "litellm".into(),
            display_name: String::new(),
            base_url: "http://x".into(),
            dialects: vec![],
            default_model: default.map(str::to_owned),
            models: models.iter().map(|m| (*m).to_owned()).collect(),
            allowed_models: vec![],
            key_origin: None,
            allowed_paths: vec![],
            organization: None,
            project: None,
            auth_header: None,
            models_path: None,
            region: None,
            project_id: None,
            location: None,
            model_roles: Default::default(),
            model_info: Default::default(),
        }
    }

    fn listing(models: &[&str], modes: &[(&str, &str)]) -> KeyListing {
        KeyListing {
            models: models.iter().map(|m| (*m).to_owned()).collect(),
            modes: Some(
                modes
                    .iter()
                    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                    .collect(),
            ),
        }
    }

    fn pick(
        e: &Endpoint,
        allowed: &[&str],
        l: Option<&KeyListing>,
    ) -> (Option<(String, Source)>, bool) {
        let allowed: Vec<String> = allowed.iter().map(|m| (*m).to_owned()).collect();
        let p = pick_default(e, e.default_model.clone(), &allowed, l);
        (p.pick, p.not_served)
    }

    #[test]
    fn a_served_default_is_used() {
        let l = listing(&["a", "b"], &[]);
        let (p, warn) = pick(&ep(Some("b"), &[]), &[], Some(&l));
        assert_eq!(p, Some(("b".into(), Source::EndpointDefault)));
        assert!(!warn);
    }

    #[test]
    fn a_default_the_key_does_not_serve_is_skipped_with_a_warning() {
        // The owner's case: `hosted_vllm/glm53-flash-int4` is the other namespace's name.
        let l = listing(&["glm-5.3-flash-int4", "b"], &[]);
        let (p, warn) = pick(
            &ep(Some("hosted_vllm/glm53-flash-int4"), &[]),
            &[],
            Some(&l),
        );
        assert_eq!(p, Some(("glm-5.3-flash-int4".into(), Source::KeyListing)));
        assert!(warn);
    }

    #[test]
    fn the_listing_order_is_kept_and_the_allow_list_filters_it() {
        let l = listing(&["z", "m", "a"], &[]);
        let e = ep(None, &[]);
        assert_eq!(
            pick(&e, &[], Some(&l)).0,
            Some(("z".into(), Source::KeyListing))
        );
        assert_eq!(
            pick(&e, &["a", "m"], Some(&l)).0,
            Some(("m".into(), Source::KeyListing))
        );
    }

    #[test]
    fn a_non_chat_model_is_skipped_and_an_unknown_mode_counts_as_chat() {
        let e = ep(None, &[]);
        let l = listing(
            &["emb", "chat-a"],
            &[("emb", "embedding"), ("chat-a", "chat")],
        );
        assert_eq!(
            pick(&e, &[], Some(&l)).0,
            Some(("chat-a".into(), Source::KeyListing))
        );
        // `/model/info` failed (no modes): the first allowed model is taken.
        let no_modes = KeyListing {
            models: vec!["emb".into(), "chat-a".into()],
            modes: None,
        };
        assert_eq!(
            pick(&e, &[], Some(&no_modes)).0,
            Some(("emb".into(), Source::KeyListing))
        );
        // Not covered by `/model/info`: chat.
        let partial = listing(&["x", "y"], &[("x", "image_generation")]);
        assert_eq!(
            pick(&e, &[], Some(&partial)).0,
            Some(("y".into(), Source::KeyListing))
        );
        let responses = listing(&["r"], &[("r", "responses")]);
        assert_eq!(
            pick(&e, &[], Some(&responses)).0,
            Some(("r".into(), Source::KeyListing))
        );
    }

    #[test]
    fn a_listing_with_no_allowed_chat_model_resolves_nothing_even_with_a_static_list() {
        let e = ep(None, &["static"]);
        let l = listing(&["emb"], &[("emb", "embedding")]);
        assert_eq!(pick(&e, &[], Some(&l)).0, None);
        assert_eq!(pick(&e, &["other"], Some(&listing(&["a"], &[]))).0, None);
    }

    #[test]
    fn an_unreadable_listing_falls_back_to_the_allow_list_then_the_static_list_then_nothing() {
        let e = ep(None, &["static"]);
        assert_eq!(
            pick(&e, &["al"], None).0,
            Some(("al".into(), Source::AllowList))
        );
        assert_eq!(
            pick(&e, &[], None).0,
            Some(("static".into(), Source::StaticList))
        );
        assert_eq!(pick(&ep(None, &[]), &[], None).0, None);
        // An unknown listing never marks the default as not served.
        assert_eq!(
            pick(&ep(Some("d"), &[]), &[], None),
            (Some(("d".into(), Source::EndpointDefault)), false)
        );
    }

    #[test]
    fn only_litellm_uses_a_listing() {
        let mut e = ep(None, &[]);
        e.provider = "openai_compatible".into();
        // Another provider's listing is never read, so none is passed; the order is unchanged.
        assert_eq!(pick(&e, &[], None).0, None);
    }

    fn key<'a>(name: &str, ws: bool, ep: &'a Endpoint) -> KeyState<'a> {
        KeyState {
            secret: LlmCandidate {
                id: Uuid::now_v7(),
                name: name.into(),
                status: "active".into(),
                workspace_id: ws.then(Uuid::now_v7),
                llm_provider: "litellm".into(),
                llm_endpoint_id: ep.id.clone(),
                llm_auth_mode: None,
                origin: None,
                value_version: 1,
            },
            ep: Some(ep),
            reason: None,
            dialect: None,
            listing: None,
        }
    }

    #[test]
    fn on_one_endpoint_the_workspace_key_comes_first_like_the_gateways_pick() {
        let (a, mut b) = (ep(None, &[]), ep(None, &[]));
        b.id = "other".into();
        // Oldest first: org key of A, key of B, workspace key of A.
        let keys = vec![
            key("org-a", false, &a),
            key("b", true, &b),
            key("ws-a", true, &a),
        ];
        let names: Vec<String> = prefer_workspace_keys(keys)
            .into_iter()
            .map(|k| k.secret.name)
            .collect();
        // A's two keys keep A's positions (0 and 2), workspace first; B stays between them.
        assert_eq!(names, ["ws-a", "b", "org-a"]);
        // Two keys of one scope stay oldest first.
        let keys = vec![key("old", false, &a), key("new", false, &a)];
        let names: Vec<String> = prefer_workspace_keys(keys)
            .into_iter()
            .map(|k| k.secret.name)
            .collect();
        assert_eq!(names, ["old", "new"]);
    }

    #[test]
    fn the_listing_reads_the_alias_only() {
        let l = listing(&["glm-5.3-flash-int4"], &[]);
        assert!(l.serves("glm-5.3-flash-int4") && !l.serves("hosted_vllm/glm53-flash-int4"));
    }
}

/// Registry of the harnesses this build knows, `(id, display name)`; `hosted.enabled_harnesses`
/// selects among them (`fake` only with `hosted.allow_fake_harness`).
pub const HARNESSES: [(&str, &str); 6] = [
    ("claude-code", "Claude Code"),
    ("codex", "OpenAI Codex"),
    ("pi", "Pi"),
    ("goose", "goose"),
    ("opencode", "OpenCode"),
    ("fake", "Scripted test harness"),
];

/// Harness ids agents may use on this instance: `hosted.enabled_harnesses`, plus `fake` when it
/// is allowed (tests and dev).
pub fn allowed_harnesses(state: &AppState) -> Vec<String> {
    let h = &state.ops().hosted;
    let mut v = h.enabled_harnesses.clone();
    if h.allow_fake_harness && !v.iter().any(|x| x == "fake") {
        v.push("fake".into());
    }
    v
}

/// A usable granted key together with its endpoint.
pub struct Usable<'a> {
    pub secret: LlmCandidate,
    pub ep: &'a Endpoint,
    /// First dialect both the harness and the endpoint speak.
    pub dialect: Dialect,
}

#[allow(clippy::large_enum_variant)] // one short-lived value per dispatch; the other variants are tiny
pub enum Selection {
    Chosen {
        choice: ModelChoice,
        /// Platform notices for the next turn (e.g. a dropped model override).
        notices: Vec<String>,
        /// (P5-MODELS-1) Set when agent roles were dropped: the dispatcher stores this back
        /// (`Some(None)` clears the agent's roles). Selection itself writes nothing.
        stored_roles: Option<Option<Value>>,
    },
    /// No usable key. The message lists reason counts, never key names (C-501).
    NoKey { message: String },
    /// A usable key but no model resolves (D-136, D-372); the message says why.
    NoDefaultModel { message: String },
}

impl Selection {
    /// `(error_code, message)` of the turn failure this selection implies, if any.
    pub fn failure(&self) -> Option<(&'static str, String)> {
        match self {
            Self::Chosen { .. } => None,
            Self::NoKey { message } => Some(("no_model_key", message.clone())),
            Self::NoDefaultModel { message } => Some(("no_default_model", message.clone())),
        }
    }
}

/// (D-372) No model resolves and the key's listing could not be read (the endpoint is unreachable or refused it).
pub const NO_DEFAULT_MODEL_LISTING_FAILED_MSG: &str = "No model is chosen for this agent, its endpoint has no usable default, \
     and the endpoint's model list could not be read with this agent's key.";
/// (D-372) No model resolves and the key's listing has no allowed chat model.
pub const NO_DEFAULT_MODEL_NONE_SERVED_MSG: &str = "No model is chosen for this agent, and the models this agent's key serves \
     include no allowed chat model.";
/// (D-372) No model resolves on a provider whose models are not picked automatically.
pub const NO_DEFAULT_MODEL_MSG: &str = "An LLM key is usable but no model is chosen for this agent \
                                        and its endpoint has no default model.";

/// Why a granted key is not usable, in the `08 §Model readiness` check order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NotActive,
    EndpointUnknown,
    ProviderDisabled,
    ScopeNotPermitted,
    OriginUnverified,
    OriginNotCompliant,
    NoSharedDialect,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotActive => "not_active",
            Self::EndpointUnknown => "endpoint_unknown",
            Self::ProviderDisabled => "provider_disabled",
            Self::ScopeNotPermitted => "scope_not_permitted",
            Self::OriginUnverified => "origin_unverified",
            Self::OriginNotCompliant => "origin_not_compliant",
            Self::NoSharedDialect => "no_shared_dialect",
        }
    }
}

/// One granted key with its readiness. `reason = None` means usable.
pub struct KeyState<'a> {
    pub secret: LlmCandidate,
    pub ep: Option<&'a Endpoint>,
    pub reason: Option<Reason>,
    /// First dialect both the harness and the endpoint speak (set when usable).
    pub dialect: Option<Dialect>,
    /// (D-372) What the key serves, for the key the gateway injects on a `litellm` endpoint (the first
    /// usable key of the endpoint); `None` = not read (other providers, other keys, or the read failed).
    pub listing: Option<Arc<KeyListing>>,
}

/// (D-372) The models one key serves, in the order the provider returned them, and, for LiteLLM, the
/// `mode` of each public model name (`None` = `/model/info` could not be read: every model counts as chat).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyListing {
    pub models: Vec<String>,
    pub modes: Option<HashMap<String, String>>,
}

impl KeyListing {
    pub fn serves(&self, model: &str) -> bool {
        self.models.iter().any(|m| m == model)
    }
    /// A model whose mode is known and is not chat-capable is not a chat model.
    pub fn chat(&self, model: &str) -> bool {
        self.modes
            .as_ref()
            .and_then(|m| m.get(model))
            .is_none_or(|mode| aurora_llm::is_chat_mode(mode))
    }
}

/// Where the default model came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Override,
    EndpointDefault,
    ProviderDefault,
    /// (D-372) The first allowed chat model of the key's own listing (`litellm` only).
    KeyListing,
    AllowList,
    StaticList,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::EndpointDefault => "endpoint_default",
            Self::ProviderDefault => "provider_default",
            Self::KeyListing => "key_listing",
            Self::AllowList => "allow_list",
            Self::StaticList => "static_list",
        }
    }
}

/// What the next turn would use without further input.
pub struct Resolved {
    /// Index into [`Readiness::keys`] of the key serving the model.
    pub key: usize,
    pub model: String,
    pub source: Source,
}

/// Every granted key's readiness plus the default selection (D-136). The dispatcher and
/// `GET /v1/agents/{agent}/model-readiness` both read this.
pub struct Readiness<'a> {
    /// In `provider_preference` order, oldest first within a provider.
    pub keys: Vec<KeyState<'a>>,
    pub default: Option<Resolved>,
    /// The agent has a model override that is not usable (dropped, with a notice, next turn).
    pub override_dropped: Option<String>,
    pub effort: String,
    /// (P5-MODELS-1, C-878) The role resolution of the default selection (`None` with no default).
    pub roles: Option<RoleResolution>,
    /// (D-372) Readiness warning `default_model_not_served`: the endpoint's default is allowed but the key's
    /// known listing does not contain it, so it was skipped.
    pub default_not_served: bool,
    /// (D-372) Readiness warning `override_not_served`: the override is kept (never re-pointed).
    pub override_not_served: bool,
}

/// (C-878) One role: its model, where it came from, and why it is not sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleView {
    pub model_id: Option<String>,
    /// `agent` | `endpoint` | `none`.
    pub source: &'static str,
    /// `role_unset` | `role_not_allowed` | `role_not_served` (D-372: the key's known listing lacks it; still sent).
    pub warning: Option<&'static str>,
}

impl RoleView {
    fn none() -> Self {
        Self {
            model_id: None,
            source: "none",
            warning: None,
        }
    }
}

/// The roles in resolution order; also the vocabulary.
pub const ROLES: [&str; 3] = ["large", "medium", "small"];

/// What `08 §Model roles` makes of the agent's and the endpoint's roles for the turn's endpoint.
#[derive(Debug, Clone)]
pub struct RoleResolution {
    /// `large`, `medium`, `small`.
    pub roles: [RoleView; 3],
    /// Agent roles dropped for this turn (their endpoint is no longer the turn's, or the model is no
    /// longer usable); never re-pointed.
    pub dropped: Vec<&'static str>,
    /// What to store back after a drop: `Some(None)` clears the agent's roles, `Some(Some(v))` keeps the rest.
    pub stored_after_drop: Option<Option<Value>>,
    /// The agent's `subagent_model` (`inherit` or a role).
    pub subagent_model: String,
    /// `ModelChoice.subagent_role`.
    pub subagent_role: Option<&'static str>,
    /// `subagent_role_unset`.
    pub subagent_warning: Option<&'static str>,
    /// Metadata of the default model and of the resolved roles only.
    pub model_meta: Vec<ModelMeta>,
}

impl RoleResolution {
    /// The resolved role models, for `ModelChoice.roles`.
    pub fn role_ids(&self) -> ModelRoleIds {
        let get = |i: usize| self.roles[i].model_id.clone();
        ModelRoleIds {
            large: get(0),
            medium: get(1),
            small: get(2),
        }
    }
}

/// Whether a role model may be sent: a valid id (SEC-122), allowed for the org and on a static list when the
/// endpoint has one.
fn role_usable(allowed: &dyn Fn(&str) -> bool, ep: &Endpoint, model: &str) -> bool {
    aurora_core::model_id::validate_model_id(model)
        && allowed(model)
        && (ep.models.is_empty() || ep.models.iter().any(|m| m == model))
}

/// (P5-POLISH-3, C-1034) The roles an endpoint leaves unset or not allowed, as the instance's admin sees
/// them: the same resolution as an agent's turn on the endpoint (`resolve_roles`) with no agent roles and
/// only the endpoint's own allow-list (no organization is involved). `(role, warning)` pairs in role order;
/// `role_not_served` needs a key's listing and is therefore never reported here.
pub fn endpoint_role_warnings(ep: &Endpoint, harness: &str) -> Vec<(&'static str, &'static str)> {
    let allowed =
        |m: &str| ep.allowed_models.is_empty() || ep.allowed_models.iter().any(|a| a == m);
    let r = resolve_roles_with(&allowed, ep, harness, "", None, "inherit", None);
    ROLES
        .iter()
        .zip(r.roles.iter())
        .filter_map(|(role, v)| v.warning.map(|w| (*role, w)))
        .collect()
}

/// The one role resolution (`08 §Model roles`): the agent's stamped role, then the endpoint's, then none;
/// the allow-list applies at selection; an agent role that is not usable is dropped, never re-pointed.
fn resolve_roles(
    eff: &EffectiveGovernance,
    ep: &Endpoint,
    harness: &str,
    default_model: &str,
    agent_roles: Option<&Value>,
    subagent_model: &str,
    listing: Option<&KeyListing>,
) -> RoleResolution {
    let allowed = |m: &str| model_ok(eff, &ep.id, m);
    resolve_roles_with(
        &allowed,
        ep,
        harness,
        default_model,
        agent_roles,
        subagent_model,
        listing,
    )
}

fn resolve_roles_with(
    allowed: &dyn Fn(&str) -> bool,
    ep: &Endpoint,
    harness: &str,
    default_model: &str,
    agent_roles: Option<&Value>,
    subagent_model: &str,
    listing: Option<&KeyListing>,
) -> RoleResolution {
    let maps_roles = !aurora_protocol::runner::harness_model_roles(harness).is_empty();
    let (ep_roles, info) = (&ep.model_roles, &ep.model_info);
    let stamped = agent_roles
        .and_then(|v| v.get("endpoint_id"))
        .and_then(Value::as_str);
    let mut views = [RoleView::none(), RoleView::none(), RoleView::none()];
    let mut dropped: Vec<&'static str> = Vec::new();
    let mut kept = serde_json::Map::new();
    for (i, role) in ROLES.iter().enumerate() {
        if let Some(m) = agent_roles
            .and_then(|v| v.get(*role))
            .and_then(Value::as_str)
        {
            if stamped == Some(ep.id.as_str()) && role_usable(allowed, ep, m) {
                views[i] = RoleView {
                    model_id: Some(m.to_owned()),
                    source: "agent",
                    warning: None,
                };
                kept.insert((*role).into(), m.into());
            } else {
                dropped.push(role);
            }
        }
        if views[i].model_id.is_some() {
            continue;
        }
        match ep_roles.get(*role) {
            Some(m) if role_usable(allowed, ep, m) => {
                views[i] = RoleView {
                    model_id: Some(m.clone()),
                    source: "endpoint",
                    warning: None,
                };
            }
            // Not allowed for this org (or not a valid id): counts as unset, with a warning, and is not sent.
            Some(_) => views[i].warning = Some("role_not_allowed"),
            None if maps_roles && ep.provider != "anthropic" => {
                views[i].warning = Some("role_unset");
            }
            None => {}
        }
        // (D-372) A role the key's known listing does not contain: warned, still sent, never re-pointed.
        if let (Some(m), Some(l)) = (views[i].model_id.as_deref(), listing)
            && !l.serves(m)
        {
            views[i].warning = Some("role_not_served");
        }
    }
    let stored_after_drop = (!dropped.is_empty()).then(|| {
        (!kept.is_empty()).then(|| {
            let mut m = serde_json::Map::new();
            m.insert("endpoint_id".into(), ep.id.clone().into());
            m.extend(kept);
            Value::Object(m)
        })
    });
    let (mut subagent_role, mut subagent_warning) = (None, None);
    if maps_roles && let Some(i) = ROLES.iter().position(|r| *r == subagent_model) {
        if views[i].model_id.is_some() || ep.provider == "anthropic" {
            subagent_role = Some(ROLES[i]);
        } else {
            subagent_warning = Some("subagent_role_unset");
        }
    }
    let mut ids: Vec<&str> = vec![default_model];
    ids.extend(views.iter().filter_map(|v| v.model_id.as_deref()));
    let mut model_meta: Vec<ModelMeta> = Vec::new();
    for id in ids {
        if model_meta.iter().any(|m| m.model == id) {
            continue;
        }
        if let Some(i) = info.get(id) {
            model_meta.push(ModelMeta {
                model: id.to_owned(),
                context_window: i.context_window.and_then(|w| u32::try_from(w).ok()),
                capabilities: i.capabilities.clone(),
            });
        }
    }
    RoleResolution {
        roles: views,
        dropped,
        stored_after_drop,
        subagent_model: subagent_model.to_owned(),
        subagent_role,
        subagent_warning,
        model_meta,
    }
}

impl Readiness<'_> {
    /// The endpoint of the default selection.
    pub fn default_endpoint(&self) -> Option<&str> {
        let d = self.default.as_ref()?;
        self.keys.get(d.key)?.ep.map(|e| e.id.as_str())
    }

    pub fn any_usable(&self) -> bool {
        self.keys.iter().any(|k| k.reason.is_none())
    }

    /// `No LLM key is granted…` or `No usable LLM key: 1 granted key not usable (reason)`.
    /// Reason counts only; never key names (participants who are not owners read it).
    pub fn no_key_message(&self) -> String {
        if self.keys.is_empty() {
            return "No LLM key is granted to this agent.".to_owned();
        }
        let n = self.keys.len();
        let order = [
            Reason::NotActive,
            Reason::EndpointUnknown,
            Reason::ProviderDisabled,
            Reason::ScopeNotPermitted,
            Reason::OriginUnverified,
            Reason::OriginNotCompliant,
            Reason::NoSharedDialect,
        ];
        let parts: Vec<String> = order
            .iter()
            .filter_map(|r| {
                let c = self.keys.iter().filter(|k| k.reason == Some(*r)).count();
                match c {
                    0 => None,
                    1 => Some(r.as_str().to_owned()),
                    c => Some(format!("{} ×{c}", r.as_str())),
                }
            })
            .collect();
        format!(
            "No usable LLM key: {n} granted key{} not usable ({}).",
            if n == 1 { "" } else { "s" },
            parts.join(", ")
        )
    }
}

/// Readiness of one granted key, first failing check in the `08` order.
fn key_reason<'a>(
    state: &AppState,
    llm: &'a ProviderRegistry,
    eff: &EffectiveGovernance,
    harness: &str,
    c: &LlmCandidate,
) -> (Option<&'a Endpoint>, Option<Reason>, Option<Dialect>) {
    let ep = llm.endpoint(&c.llm_endpoint_id);
    if c.status != "active" {
        return (ep, Some(Reason::NotActive), None);
    }
    let Some(ep) = ep else {
        return (None, Some(Reason::EndpointUnknown), None);
    };
    // The key's provider is no longer the one this endpoint serves, or is not enabled.
    let enabled = state
        .settings
        .snapshot()
        .effective
        .raw
        .llm
        .enabled_providers
        .clone();
    let provider = llm
        .provider_for(ep)
        .filter(|_| ep.provider == c.llm_provider)
        .filter(|_| enabled.contains(&c.llm_provider));
    let Some(provider) = provider else {
        return (Some(ep), Some(Reason::ProviderDisabled), None);
    };
    let scope = if c.workspace_id.is_some() {
        "workspace"
    } else {
        "organization"
    };
    if !eff.key_permitted(&ep.id, scope) {
        return (Some(ep), Some(Reason::ScopeNotPermitted), None);
    }
    if eff
        .endpoints
        .get(&ep.id)
        .is_some_and(|e| e.origin_rule.is_some())
    {
        let origin = c.origin.clone().unwrap_or(serde_json::Value::Null);
        let stored = serde_json::from_value::<StoredOrigin>(origin.clone()).ok();
        // A stored `mismatch` is a verified-but-wrong account (governance re-evaluation or a
        // re-verify), which the owner fixes with another key, not with Re-verify.
        let judged = stored.as_ref().is_some_and(|o| {
            o.value_version == i64::from(c.value_version)
                && matches!(o.status, OriginStatus::Verified | OriginStatus::Mismatch)
        });
        if !judged {
            return (Some(ep), Some(Reason::OriginUnverified), None);
        }
        if !eff.origin_complies(&ep.id, &origin, i64::from(c.value_version)) {
            return (Some(ep), Some(Reason::OriginNotCompliant), None);
        }
    }
    let eps = provider.dialects(ep);
    let Some(dialect) = first_shared(harness, &eps) else {
        return (Some(ep), Some(Reason::NoSharedDialect), None);
    };
    (Some(ep), None, Some(dialect))
}

/// Usable keys in `provider_preference` order, for the model pickers.
pub fn usable<'a>(
    state: &AppState,
    llm: &'a ProviderRegistry,
    eff: &EffectiveGovernance,
    harness: &str,
    cands: Vec<LlmCandidate>,
) -> Vec<Usable<'a>> {
    keys_of(state, llm, eff, harness, cands)
        .into_iter()
        .filter_map(|k| match (k.reason, k.ep, k.dialect) {
            (None, Some(ep), Some(dialect)) => Some(Usable {
                secret: k.secret,
                ep,
                dialect,
            }),
            _ => None,
        })
        .collect()
}

fn keys_of<'a>(
    state: &AppState,
    llm: &'a ProviderRegistry,
    eff: &EffectiveGovernance,
    harness: &str,
    cands: Vec<LlmCandidate>,
) -> Vec<KeyState<'a>> {
    // enabled providers the list does not name are appended (`08 §Phase 4 providers`)
    let pref = state
        .settings
        .snapshot()
        .effective
        .raw
        .llm
        .effective_provider_preference();
    let mut out: Vec<(usize, KeyState<'a>)> = cands
        .into_iter()
        .map(|c| {
            let (ep, reason, dialect) = key_reason(state, llm, eff, harness, &c);
            let rank = ep
                .and_then(|e| pref.iter().position(|p| *p == e.provider))
                .unwrap_or(usize::MAX);
            (
                rank,
                KeyState {
                    secret: c,
                    ep,
                    reason,
                    dialect,
                    listing: None,
                },
            )
        })
        .collect();
    out.sort_by_key(|(r, _)| *r); // stable: oldest key first within a provider
    prefer_workspace_keys(out.into_iter().map(|(_, k)| k).collect())
}

/// (D-372 step 0) On one endpoint the gateway injects the workspace-scoped key over the organization's
/// (`06` step 6), so that key comes first among the endpoint's keys here, too: readiness, the model lists
/// and the dispatcher read the key whose value the gateway will inject. The endpoint's keys keep the
/// positions they had, so the order across endpoints (provider preference, oldest first) is unchanged.
fn prefer_workspace_keys(keys: Vec<KeyState<'_>>) -> Vec<KeyState<'_>> {
    let ids: Vec<Option<String>> = keys.iter().map(|k| k.ep.map(|e| e.id.clone())).collect();
    let mut slots: Vec<Option<KeyState<'_>>> = keys.into_iter().map(Some).collect();
    let mut out: Vec<Option<KeyState<'_>>> = (0..slots.len()).map(|_| None).collect();
    let mut done: Vec<&String> = Vec::new();
    for id in ids.iter().flatten() {
        if done.contains(&id) {
            continue;
        }
        done.push(id);
        let at: Vec<usize> = ids
            .iter()
            .enumerate()
            .filter(|(_, i)| i.as_ref() == Some(id))
            .map(|(i, _)| i)
            .collect();
        let mut group: Vec<KeyState<'_>> = at.iter().filter_map(|i| slots[*i].take()).collect();
        // Stable: workspace keys first, each group still oldest first.
        group.sort_by_key(|k| k.secret.workspace_id.is_none());
        for (slot, k) in at.iter().zip(group) {
            out[*slot] = Some(k);
        }
    }
    for (o, s) in out.iter_mut().zip(slots.iter_mut()) {
        if o.is_none() {
            *o = s.take();
        }
    }
    out.into_iter().flatten().collect()
}

fn model_ok(eff: &EffectiveGovernance, ep: &str, model: &str) -> bool {
    eff.model_allowed(ep, &ModelRef::Model(model.to_owned()))
        .is_ok()
}

/// The endpoint's default model (or the first allowed one when the default is not allowed),
/// with where it came from.
fn default_model(
    llm: &ProviderRegistry,
    eff: &EffectiveGovernance,
    ep: &Endpoint,
    listing: Option<&KeyListing>,
) -> DefaultPick {
    let none = DefaultPick::default();
    let Some(provider) = llm.provider_for(ep) else {
        return none;
    };
    let allowed = eff
        .endpoints
        .get(&ep.id)
        .map(|e| e.allowed_models.clone())
        .unwrap_or_default();
    pick_default(ep, provider.default_model(ep), &allowed, listing)
}

/// The order of `08 §Model selection` (D-372), without the registry: the endpoint's default, the provider
/// default, the first allowed chat model of the key's listing (`litellm`), the allow-list, the static list.
fn pick_default(
    ep: &Endpoint,
    provider_default: Option<String>,
    allowed: &[String],
    listing: Option<&KeyListing>,
) -> DefaultPick {
    let none = DefaultPick::default();
    let ok = |m: &String| allowed.is_empty() || allowed.contains(m);
    // (D-372) Only a `litellm` key's listing is used; for the others `listing` is never set.
    let litellm = ep.provider == "litellm";
    let served = |m: &String| listing.is_none_or(|l| l.serves(m));
    let mut not_served = false;
    // 1. The endpoint's default, unless the key's known listing lacks it (warning, not a failure).
    if let Some(m) = ep.default_model.clone().filter(ok) {
        if served(&m) {
            return DefaultPick::of(m, Source::EndpointDefault, false);
        }
        not_served = true;
    }
    // 2. The provider default (for `litellm` the same field).
    if let Some(m) = provider_default.filter(ok).filter(served) {
        return DefaultPick::of(m, Source::ProviderDefault, not_served);
    }
    // 3. The first allowed chat model of the key's listing, in the provider's order.
    if litellm && let Some(l) = listing {
        return match l.models.iter().find(|m| ok(m) && l.chat(m)) {
            Some(m) => DefaultPick::of(m.clone(), Source::KeyListing, not_served),
            None => DefaultPick { not_served, ..none },
        };
    }
    // 4. Listing unknown (or another provider): the allow-list, then the static list.
    if let Some(m) = allowed.first() {
        return DefaultPick::of(m.clone(), Source::AllowList, not_served);
    }
    match ep.models.first() {
        Some(m) => DefaultPick::of(m.clone(), Source::StaticList, not_served),
        None => DefaultPick { not_served, ..none },
    }
}

/// What [`default_model`] found.
#[derive(Default)]
struct DefaultPick {
    pick: Option<(String, Source)>,
    /// The endpoint's default was skipped because the key's listing does not contain it.
    not_served: bool,
}

impl DefaultPick {
    fn of(model: String, source: Source, not_served: bool) -> Self {
        Self {
            pick: Some((model, source)),
            not_served,
        }
    }
}

// ---- (D-372) what a key serves: one read per (key, value version), cached like the model lists ----

type ListingKey = (Uuid, i32, String);
type ListingMap = HashMap<ListingKey, (Instant, Option<Arc<KeyListing>>)>;

/// A failed read is remembered briefly so an unreachable endpoint is not asked once per agent per poll.
const LISTING_FAILED_TTL: Duration = Duration::from_secs(30);

fn listings() -> &'static Mutex<ListingMap> {
    static C: std::sync::OnceLock<Mutex<ListingMap>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn listing_key(c: &LlmCandidate) -> ListingKey {
    (c.id, c.value_version, c.llm_endpoint_id.clone())
}

/// `Some(Some(l))` cached, `Some(None)` a recent failure, `None` unknown.
fn listing_cached(k: &ListingKey) -> Option<Option<Arc<KeyListing>>> {
    let mut m = listings().lock().unwrap_or_else(|p| p.into_inner());
    match m.get(k) {
        Some((at, v)) => {
            let ttl = if v.is_some() {
                crate::domains::secrets::service::MODEL_CACHE_TTL
            } else {
                LISTING_FAILED_TTL
            };
            if at.elapsed() < ttl {
                return Some(v.clone());
            }
            m.remove(k);
            None
        }
        None => None,
    }
}

fn listing_put(k: ListingKey, v: Option<Arc<KeyListing>>) {
    let mut m = listings().lock().unwrap_or_else(|p| p.into_inner());
    if m.len() > 1_000 {
        m.retain(|_, (at, _)| at.elapsed() < crate::domains::secrets::service::MODEL_CACHE_TTL);
    }
    m.insert(k, (Instant::now(), v));
}

/// What the key serves: from the cache; with `fetch`, read from the endpoint with the key (on a miss) and
/// cached. Never called with a transaction open when `fetch` is set (D-372); `None` = unknown.
async fn key_listing(state: &AppState, c: &LlmCandidate, fetch: bool) -> Option<Arc<KeyListing>> {
    let k = listing_key(c);
    if let Some(hit) = listing_cached(&k) {
        return hit;
    }
    if !fetch {
        return None;
    }
    let read = async {
        let row = aurora_db::repo::secrets::get(&state.pool, c.id)
            .await
            .ok()??;
        crate::domains::secrets::service::fetch_listing(state, &row)
            .await
            .ok()
    }
    .await
    .map(|(models, modes)| Arc::new(KeyListing { models, modes }));
    listing_put(k, read.clone());
    read
}

/// The one readiness function (D-136, `08 §Model readiness`): every granted key's `usable`
/// or first failing reason, plus the default selection with its source. Keeps the selection
/// semantics: the override first, never re-pointed, dropped when no longer usable.
///
/// (D-372) For `litellm` this reads what the endpoint's gateway key serves, from the cache or, in this
/// variant, from the endpoint. Use [`readiness_cached`] while a database transaction is open: it never
/// makes a network call.
pub async fn readiness<'a>(
    state: &AppState,
    llm: &'a ProviderRegistry,
    conn: &mut PgConnection,
    agent: Uuid,
    org: Uuid,
    workspace: Uuid,
    harness: &str,
) -> Result<Readiness<'a>, ApiError> {
    readiness_with(state, llm, conn, agent, org, workspace, harness, true).await
}

/// [`readiness`] that reads the key listings from the cache only (a miss is "listing unknown"), for callers
/// that hold a transaction. The dispatcher's poll warms the cache before it opens its transaction.
pub async fn readiness_cached<'a>(
    state: &AppState,
    llm: &'a ProviderRegistry,
    conn: &mut PgConnection,
    agent: Uuid,
    org: Uuid,
    workspace: Uuid,
    harness: &str,
) -> Result<Readiness<'a>, ApiError> {
    readiness_with(state, llm, conn, agent, org, workspace, harness, false).await
}

#[allow(clippy::too_many_arguments)]
async fn readiness_with<'a>(
    state: &AppState,
    llm: &'a ProviderRegistry,
    conn: &mut PgConnection,
    agent: Uuid,
    org: Uuid,
    workspace: Uuid,
    harness: &str,
    fetch: bool,
) -> Result<Readiness<'a>, ApiError> {
    let eff = gov::effective_for(state, org).await?;
    let cands = hosted::granted_llm_secrets(conn, agent, org, workspace).await?;
    let (prov, epid, mid, effort) = hosted::model_prefs(conn, agent).await?;
    let (agent_roles, subagent_model) = hosted::model_role_prefs(conn, agent).await?;
    let effort = effort.unwrap_or_else(|| "medium".to_owned());
    let mut keys = keys_of(state, llm, &eff, harness, cands);
    // (D-372 step 0) The key the gateway injects is the first usable one of its endpoint: read what it serves.
    let mut seen: Vec<&str> = Vec::new();
    let mut first_of_endpoint = vec![false; keys.len()];
    for (i, k) in keys.iter().enumerate() {
        if let (None, Some(ep)) = (k.reason, k.ep)
            && !seen.contains(&ep.id.as_str())
        {
            seen.push(ep.id.as_str());
            first_of_endpoint[i] = true;
        }
    }
    for (i, k) in keys.iter_mut().enumerate() {
        if first_of_endpoint[i] && k.ep.is_some_and(|ep| ep.provider == "litellm") {
            k.listing = key_listing(state, &k.secret, fetch).await;
        }
    }
    let mut override_dropped = None;
    let mut override_not_served = false;
    let mut default = None;
    let mut default_not_served = false;
    if let (Some(p), Some(e), Some(m)) = (prov, epid, mid) {
        // An explicit model is stamped with provider + endpoint; never re-pointed.
        let hit = keys.iter().position(|k| {
            k.reason.is_none() && k.ep.is_some_and(|ep| ep.provider == p && ep.id == e)
        });
        match hit {
            Some(i) if model_ok(&eff, &e, &m) => {
                // (D-372) Warned when the key's known listing lacks it; the override is still used.
                override_not_served = keys[i].listing.as_ref().is_some_and(|l| !l.serves(&m));
                default = Some(Resolved {
                    key: i,
                    model: m,
                    source: Source::Override,
                });
            }
            _ => override_dropped = Some(m),
        }
    }
    if default.is_none() {
        for (i, k) in keys.iter().enumerate() {
            let (None, Some(ep)) = (k.reason, k.ep) else {
                continue;
            };
            if !first_of_endpoint[i] {
                continue;
            }
            let found = default_model(llm, &eff, ep, k.listing.as_deref());
            if let Some((model, source)) = found.pick {
                default_not_served = found.not_served;
                default = Some(Resolved {
                    key: i,
                    model,
                    source,
                });
                break;
            }
            default_not_served |= found.not_served;
        }
    }
    // (P5-MODELS-1) Roles of the default selection: the same function serves the dispatcher and the report.
    let roles = default.as_ref().and_then(|d| {
        let k = keys.get(d.key)?;
        Some(resolve_roles(
            &eff,
            k.ep?,
            harness,
            &d.model,
            agent_roles.as_ref(),
            &subagent_model,
            k.listing.as_deref(),
        ))
    });
    Ok(Readiness {
        keys,
        default,
        override_dropped,
        effort,
        roles,
        default_not_served,
        override_not_served,
    })
}

impl Readiness<'_> {
    /// (D-372) Why no model resolves (call only when [`Self::default`] is `None` and a key is usable): the
    /// listing could not be read, or the key serves no allowed chat model.
    pub fn no_default_message(&self) -> String {
        let first = self
            .keys
            .iter()
            .find(|k| k.reason.is_none() && k.ep.is_some_and(|e| e.provider == "litellm"));
        match first {
            Some(k) if k.listing.is_none() => NO_DEFAULT_MODEL_LISTING_FAILED_MSG.to_owned(),
            Some(_) => NO_DEFAULT_MODEL_NONE_SERVED_MSG.to_owned(),
            None => NO_DEFAULT_MODEL_MSG.to_owned(),
        }
    }
}

/// (D-372) Reads what each usable `litellm` key of the agents with queued hosted turns serves into the
/// cache, **before** the dispatcher's poll opens its transaction (no database transaction spans the network
/// call). One read per (key, value version) per cache lifetime, however many agents share the key.
pub async fn prewarm_queued(state: &AppState, pages: usize, page: i64) -> Result<(), ApiError> {
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let mut cursor = None;
    let mut agents: Vec<Uuid> = Vec::new();
    for _ in 0..pages {
        let batch = hosted::queued_hosted_agents(&mut conn, cursor, page).await?;
        let Some(last) = batch.last().copied() else {
            break;
        };
        cursor = Some((last.1, last.0));
        agents.extend(batch.iter().map(|p| p.0));
        if (batch.len() as i64) < page {
            break;
        }
    }
    let llm = state.llm();
    for agent in agents {
        let Some(a) = aurora_db::repo::agents::get(&mut *conn, agent).await? else {
            continue;
        };
        let harness = a
            .harness
            .clone()
            .unwrap_or_else(|| state.ops().hosted.default_harness.clone());
        // A failure here is the same failure the dispatcher's own pass reports; it must not stop the poll.
        if let Err(e) = readiness(
            state,
            &llm,
            &mut conn,
            agent,
            a.org_id,
            a.workspace_id,
            &harness,
        )
        .await
        {
            tracing::warn!(%agent, error = %e.code(), "model listing prewarm skipped");
        }
    }
    Ok(())
}

fn choice(
    llm: &ProviderRegistry,
    ep: &Endpoint,
    dialect: Dialect,
    mode: AuthMode,
    model: String,
    effort: &str,
    roles: Option<(&RoleResolution, &str)>,
) -> ModelChoice {
    let client_env = llm
        .provider_for(ep)
        .map(|p| p.client_env(ep, dialect, &mode))
        .unwrap_or_default();
    // (C-877) Roles travel only to a harness that maps them (`harness_model_roles`), and only the resolved ones.
    let mapped = roles.filter(|(_, h)| !aurora_protocol::runner::harness_model_roles(h).is_empty());
    let role_ids = mapped.map(|(r, _)| r.role_ids()).filter(|r| !r.is_empty());
    let meta = mapped
        .map(|(r, _)| r.model_meta.clone())
        .filter(|m| !m.is_empty());
    ModelChoice {
        roles: role_ids,
        subagent_role: mapped.and_then(|(r, _)| r.subagent_role).map(str::to_owned),
        model_meta: meta,
        provider: ep.provider.clone(),
        endpoint_id: ep.id.clone(),
        base_url: ep.base_url.clone(),
        dialect: dialect.as_str().to_owned(),
        model,
        effort: effort.to_owned(),
        client_env,
    }
}

/// Pick the model for an agent's next turn.
pub async fn select(
    state: &AppState,
    conn: &mut PgConnection,
    agent: Uuid,
    org: Uuid,
    workspace: Uuid,
    harness: &str,
) -> Result<Selection, ApiError> {
    // The scripted `fake` harness talks to no LLM: no key, no model (12 §Adapter fake).
    if harness == "fake" && state.ops().hosted.allow_fake_harness {
        return Ok(Selection::Chosen {
            choice: ModelChoice {
                provider: "fake".into(),
                endpoint_id: "fake".into(),
                base_url: String::new(),
                dialect: Dialect::AnthropicMessages.as_str().into(),
                model: "fake".into(),
                effort: "medium".into(),
                client_env: vec![],
                roles: None,
                subagent_role: None,
                model_meta: None,
            },
            notices: vec![],
            stored_roles: None,
        });
    }
    let llm = state.llm();
    // (D-372) `conn` may be the poll's transaction: the listings come from the cache the poll warmed first.
    let r = readiness_cached(state, &llm, conn, agent, org, workspace, harness).await?;
    if !r.any_usable() {
        return Ok(Selection::NoKey {
            message: r.no_key_message(),
        });
    }
    let Some(d) = &r.default else {
        return Ok(Selection::NoDefaultModel {
            message: r.no_default_message(),
        });
    };
    let k = &r.keys[d.key];
    let (Some(ep), Some(dialect)) = (k.ep, k.dialect) else {
        return Ok(Selection::NoDefaultModel {
            message: r.no_default_message(),
        });
    };
    let mut notices = Vec::new();
    if let Some(m) = &r.override_dropped {
        notices.push(format!(
            "The model override `{m}` is no longer usable (key, endpoint or model not permitted); \
             the default model is used instead."
        ));
    }
    if let Some(rr) = r.roles.as_ref().filter(|rr| !rr.dropped.is_empty()) {
        let names = rr
            .dropped
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ");
        notices.push(format!(
            "The model role(s) {names} of this agent are no longer usable (endpoint or model not permitted) \
             and were removed; the endpoint's roles apply instead."
        ));
    }
    Ok(Selection::Chosen {
        choice: choice(
            &llm,
            ep,
            dialect,
            k.secret
                .llm_auth_mode
                .as_deref()
                .and_then(AuthMode::parse)
                .unwrap_or(AuthMode::ApiKey),
            d.model.clone(),
            &r.effort,
            r.roles.as_ref().map(|rr| (rr, harness)),
        ),
        notices,
        stored_roles: r.roles.as_ref().and_then(|rr| rr.stored_after_drop.clone()),
    })
}

/// (P5-MODELS-2, D-304) What the dispatcher chose for a turn, stored with it (`turns.model_basis`): the
/// trust rule at turn end reads the default and the roles from here, never from the harness.
pub fn basis_of(c: &ModelChoice) -> Value {
    let mut b = serde_json::json!({
        "provider": c.provider, "endpoint_id": c.endpoint_id, "model": c.model,
    });
    if let Some(r) = &c.roles {
        b["roles"] = serde_json::json!(r.set().map(|(_, m)| m).collect::<Vec<_>>());
    }
    b
}

/// Token counters of one `by_model` entry: input, output, cache read, cache write.
type Counts = [i64; 4];

fn counts_of(e: &Value) -> Counts {
    let n = |k: &str| e.get(k).and_then(Value::as_i64).unwrap_or(0).max(0);
    [
        n("input_tokens"),
        n("output_tokens"),
        n("cache_read_tokens"),
        n("cache_write_tokens"),
    ]
}

/// (D-304, C-879, `08` *Error classification and usage*) The usage the API stores for a turn: the harness's
/// claim, reduced by the trust rule. `provider` and `endpoint_id` come from the turn's basis, never from the
/// claim. An entry is kept when its model is the default, a resolved role, or accepted by `allowed` (the
/// effective allow-list at turn end; any pattern-valid id when none applies). Every other entry, and every
/// entry past `USAGE_BY_MODEL_MAX`, is folded into the default model's entry and sets `unverified_models`.
/// The totals are the sum of the kept entries. A claim with no breakdown becomes one entry for the default.
pub fn fold_usage(basis: &Value, claim: &Value, allowed: impl Fn(&str) -> bool) -> Value {
    let text = |k: &str| basis.get(k).and_then(Value::as_str).unwrap_or_default();
    let default = text("model").to_owned();
    let roles: Vec<&str> = basis
        .get("roles")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let trusted = |m: &str| {
        m == default
            || roles.contains(&m)
            || (aurora_core::model_id::validate_model_id(m) && allowed(m))
    };
    let claimed: Vec<(String, Counts)> = match claim
        .get("by_model")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
    {
        Some(a) => a
            .iter()
            .map(|e| {
                let m = e.get("model").and_then(Value::as_str).unwrap_or_default();
                (m.to_owned(), counts_of(e))
            })
            .collect(),
        None => vec![(default.clone(), counts_of(claim))],
    };
    let add = |into: &mut Counts, c: Counts| {
        for (t, d) in into.iter_mut().zip(c) {
            *t = t.saturating_add(d);
        }
    };
    let (mut kept, mut folded): (Vec<(String, Counts)>, Counts) = (Vec::new(), [0; 4]);
    let mut unverified = false;
    for (m, c) in claimed {
        if !trusted(&m) {
            add(&mut folded, c);
            unverified = true;
        } else if let Some((_, k)) = kept.iter_mut().find(|(km, _)| *km == m) {
            add(k, c);
        } else if m == default {
            kept.insert(0, (m, c)); // the default is first, so the limit never drops it
        } else {
            kept.push((m, c));
        }
    }
    // The limit counts the default's entry, which exists whenever something was folded.
    let room = aurora_core::limits::USAGE_BY_MODEL_MAX
        - usize::from(!kept.iter().any(|(m, _)| *m == default));
    while kept.len() > room {
        if let Some((_, c)) = kept.pop() {
            add(&mut folded, c);
            unverified = true;
        }
    }
    if unverified {
        match kept.iter_mut().find(|(m, _)| *m == default) {
            Some((_, c)) => add(c, folded),
            None => kept.insert(0, (default.clone(), folded)),
        }
    }
    let mut total = [0_i64; 4];
    for (_, c) in &kept {
        add(&mut total, *c);
    }
    let mut out = serde_json::json!({
        "input_tokens": total[0], "output_tokens": total[1],
        "cache_read_tokens": total[2], "cache_write_tokens": total[3],
        "provider": text("provider"), "endpoint_id": text("endpoint_id"), "model": default,
        "by_model": kept.iter().map(|(m, c)| serde_json::json!({
            "model": m, "input_tokens": c[0], "output_tokens": c[1],
            "cache_read_tokens": c[2], "cache_write_tokens": c[3],
        })).collect::<Vec<_>>(),
    });
    if let Some(r) = claim.get("reasoning_tokens").and_then(Value::as_i64) {
        out["reasoning_tokens"] = r.max(0).into();
    }
    if unverified {
        out["unverified_models"] = true.into();
    }
    out
}

/// The usage to store at turn end (see [`fold_usage`]): the turn's basis and the effective allow-list now.
/// A turn without a recorded basis keeps its totals and loses the breakdown it cannot verify.
pub async fn usage_for_turn(
    state: &AppState,
    conn: &mut PgConnection,
    turn: Uuid,
    claim: &Value,
) -> Result<Value, ApiError> {
    let Some((org, Some(basis))) = hosted::model_basis(conn, turn).await? else {
        let mut u = claim.clone();
        if let Some(o) = u.as_object_mut() {
            o.remove("by_model");
        }
        return Ok(u);
    };
    let eff = gov::effective_for(state, org).await?;
    let ep = basis
        .get("endpoint_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(fold_usage(&basis, claim, |m| model_ok(&eff, &ep, m)))
}
