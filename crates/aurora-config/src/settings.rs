//! Instance settings (phase 5, SET-1, D-236..D-238, C-770): the registry of movable keys and the
//! effective view `helm > stored > default` over the deployment configuration.
//!
//! A *movable key* is a configuration key an instance admin may edit in the dashboard unless the
//! deployment configuration sets it. Everything else is not a setting (SEC-89). The merge works on
//! the configuration's TOML table, so the **existing** validation functions (`api_config`,
//! `gateway_config`) run on the merged [`RawConfig`] and no rule exists twice.

use crate::error::{ConfigError, ConfigErrors, Result};
use crate::load::{build_raw, read_table, set_path};
use crate::raw::RawConfig;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

/// The instance-settings sections (C-770). `export` and `import` are never section ids.
pub const SECTION_IDS: [&str; 13] = [
    "governance",
    "llm",
    "sign_in",
    "email",
    "secrets",
    "agents",
    "approvals",
    "retention",
    "skills",
    "scim",
    "previews",
    "terminal",
    "limits",
];

/// The test-only section (feature `test-registry` or this crate's unit tests): its keys have no
/// `RawConfig` field, so they exercise the mechanism without a real setting.
pub const TEST_SECTION: &str = "settings_test";
/// A second test-only section.
pub const TEST_SECTION_B: &str = "settings_test_b";

/// Keys (or key prefixes ending in `.`) that are **never** settings (D-237(b), SEC-89): roots,
/// topology, `auth.instance_admins` and every development or test switch.
pub const NOT_SETTINGS: &[&str] = &[
    "database.",
    "server.",
    "auth.instance_admins",
    "auth.trusted_origins",
    "auth.dev_idp.",
    "hosted.allow_fake_harness",
    "gateway.insecure_loopback_injection",
    "plugins.",
    "runner.",
    "connector_host.test_types",
    "ratelimit.api.enabled",
    "ratelimit.counters.",
    "email.log_links",
    "llm.dev_upstream_overrides",
    "push.dev_endpoint_overrides",
    "events.",
    "telemetry.",
    "tls.",
    "hookhost.",
    "relay.",
    "previews.enabled",
    "previews.domain",
    "terminal.available",
    "scim.available",
    "governance.llm.known_llm_hosts",
];

/// One section's display data.
#[derive(Debug, Clone, Copy)]
pub struct SectionDef {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
}

/// The value type of a setting (C-770).
#[derive(Debug, Clone)]
pub enum SettingType {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    /// A decimal number (`ratelimit.api.multiplier`).
    Float {
        min: f64,
        max: f64,
    },
    /// A number of seconds.
    DurationSecs {
        min: i64,
        max: i64,
    },
    Str {
        max_len: usize,
    },
    Enum(Vec<&'static str>),
    List {
        item: Box<SettingType>,
        max_items: usize,
    },
    /// A list entry (keyed lists): validated by the section's own rules on the merged view.
    Object,
}

/// How a list setting is locked and merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// Locked as a whole: Helm's list or the dashboard's, never merged.
    Whole,
    /// Entries identified by `id_field`: a Helm entry is locked, dashboard entries sit beside it.
    Keyed { id_field: &'static str },
}

/// How a change of a value is judged looser (C-770, shown by the preview).
#[derive(Debug, Clone)]
pub enum Loosening {
    /// Not a ceiling or floor: a change is never flagged.
    None,
    /// `true`: turning the switch on loosens; `false`: turning it off loosens.
    Bool { loosens_when: bool },
    /// The numeric value going up (`true`) or down (`false`) loosens.
    Number { up_loosens: bool },
    /// Higher loosens, and `0` means "no limit": the loosest value (`auth.cli_key_ttl_days`).
    NumberZeroIsMax,
    /// An order, strictest first: moving toward the end loosens.
    Order(Vec<&'static str>),
    /// A list gaining entries (`true`) or losing entries (`false`) loosens.
    List { superset_loosens: bool },
}

/// One registry entry.
#[derive(Debug, Clone)]
pub struct SettingDef {
    pub section: &'static str,
    /// The dotted configuration path.
    pub key: &'static str,
    pub ty: SettingType,
    /// The application's default (what the effective view shows as `default`).
    pub default: Value,
    /// The Helm values key that sets it.
    pub helm_key: String,
    pub list: Option<ListKind>,
    /// A write-only value (sealed in the inline store, never returned).
    pub secret: bool,
    pub loosening: Loosening,
    /// A test-only key without a `RawConfig` field.
    pub test_only: bool,
    /// While neither the configuration nor the store sets it, the value of this other (boolean,
    /// configuration-only) key: `terminal.enabled` follows `terminal.available` (D-237(d)).
    pub default_from: Option<&'static str>,
    /// A keyed list whose configuration form is a table keyed by id (`ratelimit.api.classes`);
    /// stored and shown as a list of entries carrying their `id`.
    pub as_table: bool,
}

impl SettingDef {
    /// A scalar or whole-list setting with the default `helm_key` (`config.<key>`).
    pub fn new(section: &'static str, key: &'static str, ty: SettingType, default: Value) -> Self {
        Self {
            section,
            key,
            ty,
            default,
            helm_key: format!("config.{key}"),
            list: None,
            secret: false,
            loosening: Loosening::None,
            test_only: false,
            default_from: None,
            as_table: false,
        }
    }
    /// The default follows another configuration-only boolean key (see [`Self::default_from`]).
    #[must_use]
    pub fn default_from(mut self, key: &'static str) -> Self {
        self.default_from = Some(key);
        self
    }
    /// A keyed list whose configuration form is a table keyed by `id_field`'s value.
    #[must_use]
    pub fn keyed_table(mut self, id_field: &'static str) -> Self {
        self.list = Some(ListKind::Keyed { id_field });
        self.as_table = true;
        self
    }
    #[must_use]
    pub fn helm(mut self, key: &str) -> Self {
        self.helm_key = key.to_string();
        self
    }
    #[must_use]
    pub fn whole_list(mut self) -> Self {
        self.list = Some(ListKind::Whole);
        self
    }
    #[must_use]
    pub fn keyed(mut self, id_field: &'static str) -> Self {
        self.list = Some(ListKind::Keyed { id_field });
        self
    }
    #[must_use]
    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
    #[must_use]
    pub fn loosening(mut self, l: Loosening) -> Self {
        self.loosening = l;
        self
    }

    /// Check one value against the type (shape and bounds). `Err` is a short message.
    pub fn check(&self, v: &Value) -> std::result::Result<(), String> {
        if self.secret {
            return Ok(());
        }
        check_type(&self.ty, v)
    }

    /// Is `after` looser than `before` (C-770 `loosening`)?
    pub fn is_loosening(&self, before: &Value, after: &Value) -> bool {
        if before == after {
            return false;
        }
        match &self.loosening {
            Loosening::None => false,
            Loosening::Bool { loosens_when } => after.as_bool() == Some(*loosens_when),
            Loosening::Number { up_loosens } => match (before.as_f64(), after.as_f64()) {
                (Some(b), Some(a)) => (a > b) == *up_loosens,
                // An optional maximum that is removed is unbounded: the loosest value.
                (Some(_), None) if after.is_null() => *up_loosens,
                _ => false,
            },
            Loosening::NumberZeroIsMax => {
                let rank = |v: &Value| v.as_f64().map(|n| if n == 0.0 { f64::INFINITY } else { n });
                matches!((rank(before), rank(after)), (Some(b), Some(a)) if a > b)
            }
            Loosening::Order(order) => {
                let pos = |v: &Value| v.as_str().and_then(|s| order.iter().position(|o| *o == s));
                matches!((pos(before), pos(after)), (Some(b), Some(a)) if a > b)
            }
            Loosening::List { superset_loosens } => {
                let items = |v: &Value| -> BTreeSet<String> {
                    v.as_array()
                        .map(|a| a.iter().map(Value::to_string).collect())
                        .unwrap_or_default()
                };
                let (b, a) = (items(before), items(after));
                if *superset_loosens {
                    a.difference(&b).next().is_some()
                } else {
                    b.difference(&a).next().is_some()
                }
            }
        }
    }
}

fn check_type(ty: &SettingType, v: &Value) -> std::result::Result<(), String> {
    match ty {
        SettingType::Bool => v
            .is_boolean()
            .then_some(())
            .ok_or_else(|| "must be true or false".into()),
        SettingType::Int { min, max } | SettingType::DurationSecs { min, max } => {
            match v.as_i64() {
                Some(n) if (*min..=*max).contains(&n) => Ok(()),
                Some(_) => Err(format!("must be between {min} and {max}")),
                None => Err("must be a whole number".into()),
            }
        }
        SettingType::Float { min, max } => match v.as_f64() {
            Some(n) if (*min..=*max).contains(&n) => Ok(()),
            Some(_) => Err(format!("must be between {min} and {max}")),
            None => Err("must be a number".into()),
        },
        SettingType::Str { max_len } => match v.as_str() {
            Some(s) if s.chars().count() <= *max_len => Ok(()),
            Some(_) => Err(format!("at most {max_len} characters")),
            None => Err("must be a string".into()),
        },
        SettingType::Enum(allowed) => match v.as_str() {
            Some(s) if allowed.contains(&s) => Ok(()),
            _ => Err(format!("must be one of: {}", allowed.join(", "))),
        },
        SettingType::List { item, max_items } => match v.as_array() {
            Some(a) if a.len() <= *max_items => a.iter().try_for_each(|x| check_type(item, x)),
            Some(_) => Err(format!("at most {max_items} entries")),
            None => Err("must be a list".into()),
        },
        SettingType::Object => v
            .is_object()
            .then_some(())
            .ok_or_else(|| "must be an object".into()),
    }
}

/// The registry: every movable key, by section. Each section brief registers its own entries
/// (SET-2..SET-5, FIX-124); SET-1 registers none besides the test section.
#[derive(Debug)]
pub struct Registry {
    defs: Vec<SettingDef>,
}

impl Registry {
    fn build() -> Self {
        #[allow(unused_mut)]
        let mut defs: Vec<SettingDef> = Vec::new();
        defs.extend(secrets_defs());
        defs.extend(governance_defs());
        defs.extend(llm_defs());
        defs.extend(sign_in_defs());
        defs.extend(email_defs());
        defs.extend(agents_defs());
        defs.extend(approvals_defs());
        defs.extend(retention_defs());
        defs.extend(skills_defs());
        defs.extend(previews_defs());
        defs.extend(scim_defs());
        defs.extend(terminal_defs());
        defs.extend(limits_defs());
        #[cfg(any(test, feature = "test-registry"))]
        defs.extend(test_defs());
        Self { defs }
    }

    pub fn defs(&self) -> &[SettingDef] {
        &self.defs
    }

    pub fn get(&self, key: &str) -> Option<&SettingDef> {
        self.defs.iter().find(|d| d.key == key)
    }

    pub fn section(&self, section: &str) -> impl Iterator<Item = &SettingDef> {
        let section = section.to_string();
        self.defs.iter().filter(move |d| d.section == section)
    }

    /// The registered (non-test) keys, sorted: `deploy/helm/aurora/movable-keys.txt`.
    pub fn movable_keys(&self) -> Vec<&'static str> {
        let mut keys: Vec<&'static str> = self
            .defs
            .iter()
            .filter(|d| !d.test_only)
            .map(|d| d.key)
            .collect();
        keys.sort_unstable();
        keys
    }

    /// Is `id` a known section (the reserved thirteen, plus the test section when enabled)?
    pub fn has_section(&self, id: &str) -> bool {
        sections().iter().any(|s| s.id == id)
    }
}

/// The process-wide registry.
pub fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Registry::build)
}

/// The sections in display order.
pub fn sections() -> &'static [SectionDef] {
    static S: OnceLock<Vec<SectionDef>> = OnceLock::new();
    S.get_or_init(|| {
        #[allow(unused_mut)]
        let mut v = vec![
            SectionDef {
                id: "governance",
                title: "Instance rules",
                description: "What organizations may not go below: AI keys, Internet access and agents people run themselves.",
            },
            SectionDef {
                id: "llm",
                title: "LLM endpoints",
                description: "Providers and endpoints agents may reach.",
            },
            SectionDef {
                id: "sign_in",
                title: "Sign-in",
                description: "Sign-up, sessions and identity providers.",
            },
            SectionDef {
                id: "email",
                title: "Email",
                description: "Outgoing mail.",
            },
            SectionDef {
                id: "secrets",
                title: "Secret stores",
                description: "Where secret values are kept.",
            },
            SectionDef {
                id: "agents",
                title: "Agents & sandboxes",
                description: "Agent defaults and hosted-agent timeouts.",
            },
            SectionDef {
                id: "approvals",
                title: "Approvals",
                description: "Approval deadlines and notifications.",
            },
            SectionDef {
                id: "retention",
                title: "Retention & storage",
                description: "How long data is kept and storage quotas.",
            },
            SectionDef {
                id: "skills",
                title: "Skills",
                description: "The git skill library.",
            },
            SectionDef {
                id: "scim",
                title: "Directory sync",
                description: "SCIM provisioning.",
            },
            SectionDef {
                id: "previews",
                title: "Previews",
                description: "Port preview quotas and caps.",
            },
            SectionDef {
                id: "terminal",
                title: "Terminal",
                description: "The web terminal.",
            },
            SectionDef {
                id: "limits",
                title: "Limits",
                description: "Rate limits and gateway limits.",
            },
        ];
        #[cfg(any(test, feature = "test-registry"))]
        {
            v.push(SectionDef {
                id: TEST_SECTION,
                title: "Test section",
                description: "Exercises the settings mechanism.",
            });
            v.push(SectionDef {
                id: TEST_SECTION_B,
                title: "Second test section",
                description: "A second section for multi-section imports.",
            });
        }
        v
    })
}

/// The `secrets` section (FIX-124, D-237): where secret values are kept. The store definitions are
/// a keyed list by `id` (a store the chart renders from `vault.*` is one locked entry, dashboard
/// stores sit beside it); the default store is a scalar.
fn secrets_defs() -> Vec<SettingDef> {
    use serde_json::json;
    vec![
        SettingDef::new(
            "secrets",
            "secrets.default_store",
            SettingType::Str { max_len: 64 },
            json!("inline"),
        )
        .helm("vault.makeDefault"),
        SettingDef::new(
            "secrets",
            "secrets.stores",
            SettingType::List {
                item: Box::new(SettingType::Object),
                max_items: 16,
            },
            json!([]),
        )
        .keyed("id")
        .helm("vault.*"),
    ]
}

/// The `llm` section (SET-3, D-236, D-237): providers, their preference order and the endpoints.
/// The provider list and the order are whole values (locked together when Helm sets them);
/// endpoints are a keyed list by `id` (a Helm endpoint is one locked entry, dashboard endpoints
/// with other ids sit beside it). `llm.dev_upstream_overrides` is never a setting (SEC-89).
/// Defaults are those of [`crate::raw::LlmSection`].
fn llm_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "llm";
    let providers = || SettingType::List {
        item: Box::new(SettingType::Str { max_len: 64 }),
        max_items: 16,
    };
    vec![
        SettingDef::new(
            S,
            "llm.enabled_providers",
            providers(),
            json!(["litellm", "anthropic"]),
        )
        .whole_list(),
        SettingDef::new(
            S,
            "llm.provider_preference",
            providers(),
            json!(["litellm", "anthropic"]),
        )
        .whole_list(),
        SettingDef::new(
            S,
            "llm.endpoints",
            SettingType::List {
                item: Box::new(SettingType::Object),
                max_items: LLM_ENDPOINTS_MAX,
            },
            json!([]),
        )
        .keyed("id"),
    ]
}

/// Most `auth.oidc_providers` entries (Helm and dashboard together).
pub const OIDC_PROVIDERS_MAX: usize = 16;

/// The `sign_in` section (SET-4, D-235(b), D-237): sign-up, session lifetimes and the OIDC
/// providers (a keyed list by `id`: a Helm provider is one locked entry, dashboard providers sit
/// beside it). `auth.instance_admins` and `auth.trusted_origins` stay in Helm (break-glass).
fn sign_in_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "sign_in";
    vec![
        SettingDef::new(
            S,
            "auth.signup_mode",
            SettingType::Enum(vec!["invite_only", "sso_jit", "open"]),
            json!("invite_only"),
        )
        .loosening(Loosening::Order(vec!["invite_only", "sso_jit", "open"])),
        SettingDef::new(
            S,
            "auth.org_creation",
            SettingType::Enum(vec!["instance_admins", "anyone"]),
            json!("instance_admins"),
        )
        .loosening(Loosening::Order(vec!["instance_admins", "anyone"])),
        SettingDef::new(
            S,
            "auth.session_idle_secs",
            SettingType::DurationSecs {
                min: 60,
                max: 31_536_000,
            },
            json!(aurora_core::limits::SESSION_IDLE.as_secs()),
        )
        .loosening(Loosening::Number { up_loosens: true }),
        SettingDef::new(
            S,
            "auth.session_absolute_secs",
            SettingType::DurationSecs {
                min: 300,
                max: 31_536_000,
            },
            json!(aurora_core::limits::SESSION_ABSOLUTE.as_secs()),
        )
        .loosening(Loosening::Number { up_loosens: true }),
        // 0 = never expires: the loosest value.
        SettingDef::new(
            S,
            "auth.cli_key_ttl_days",
            SettingType::Int { min: 0, max: 3650 },
            json!(90),
        )
        .loosening(Loosening::NumberZeroIsMax),
        SettingDef::new(
            S,
            "auth.oidc_providers",
            SettingType::List {
                item: Box::new(SettingType::Object),
                max_items: OIDC_PROVIDERS_MAX,
            },
            json!([]),
        )
        .keyed("id"),
    ]
}

/// The `email` section (SET-4, D-237(f)): the mailer. `email.log_links` is never a setting; the
/// password is the dashboard counterpart of `AURORA_SMTP_PASSWORD` (locked while that is set).
fn email_defs() -> Vec<SettingDef> {
    const S: &str = "email";
    vec![
        SettingDef::new(
            S,
            "email.smtp_url",
            SettingType::Str { max_len: 512 },
            Value::Null,
        ),
        SettingDef::new(
            S,
            "email.from",
            SettingType::Str { max_len: 320 },
            Value::Null,
        ),
        SettingDef::new(
            S,
            "email.smtp_username",
            SettingType::Str { max_len: 256 },
            Value::Null,
        ),
        SettingDef::new(
            S,
            "email.smtp_password",
            SettingType::Str { max_len: 1024 },
            Value::Null,
        )
        .helm("api.extraEnv: AURORA_SMTP_PASSWORD")
        .secret(),
    ]
}

/// A number of seconds or a count that loosens when it goes up (deadlines, caps, quotas). The
/// bounds are wide: the section's own validation (`validate.rs`) names the precise rule.
fn up(s: &'static str, key: &'static str, max: i64, default: impl Into<Value>) -> SettingDef {
    SettingDef::new(s, key, SettingType::Int { min: 0, max }, default.into())
        .loosening(Loosening::Number { up_loosens: true })
}

/// The `agents` section (SET-5, D-237): agent defaults and the hosted-agent deadlines. Defaults are
/// those of `raw.rs`; `hosted.allow_fake_harness` is a development switch and stays configuration.
fn agents_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "agents";
    vec![
        SettingDef::new(
            S,
            "agents.default_name",
            SettingType::Str { max_len: 100 },
            json!("Daneel"),
        ),
        SettingDef::new(
            S,
            "agents.default_brief",
            SettingType::Str { max_len: 20_000 },
            json!(crate::raw::DEFAULT_AGENT_BRIEF),
        ),
        SettingDef::new(S, "hosted.enabled", SettingType::Bool, json!(true))
            .loosening(Loosening::Bool { loosens_when: true }),
        SettingDef::new(
            S,
            "hosted.default_harness",
            SettingType::Str { max_len: 64 },
            json!("claude-code"),
        ),
        SettingDef::new(
            S,
            "hosted.enabled_harnesses",
            SettingType::List {
                item: Box::new(SettingType::Str { max_len: 64 }),
                max_items: 16,
            },
            json!(["claude-code"]),
        )
        .whole_list()
        .loosening(Loosening::List {
            superset_loosens: true,
        }),
        SettingDef::new(
            S,
            "hosted.default_timezone",
            SettingType::Str { max_len: 64 },
            json!("UTC"),
        ),
        up(S, "hosted.idle_stop_secs", 604_800, 30 * 60),
        up(S, "hosted.background_cap_secs", 2_592_000, 12 * 3600),
        up(S, "hosted.turn_ceiling_secs", 2_592_000, 6 * 3600),
        up(S, "hosted.turn_stall_secs", 604_800, 10 * 60),
        up(S, "hosted.sandbox_ready_timeout_secs", 86_400, 300),
        // (P5-GUIDE-7, C-657) read at each home sync: no new generation trigger.
        SettingDef::new(
            S,
            "hosted.platform_tools",
            SettingType::Enum(vec!["on_demand", "all"]),
            json!("on_demand"),
        )
        .loosening(Loosening::Order(vec!["on_demand", "all"])),
    ]
}

/// The `approvals` section (SET-5, D-237), with `approvals.park.*` (P4-011B-2).
fn approvals_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "approvals";
    vec![
        up(
            S,
            "approvals.timeout_secs",
            2_592_000,
            aurora_core::limits::APPROVAL_TIMEOUT.as_secs(),
        ),
        up(S, "approvals.max_timeout_secs", 2_592_000, 3600),
        SettingDef::new(
            S,
            "approvals.email_notifications",
            SettingType::Bool,
            json!(true),
        ),
        // (P4-011B-2, D-265(c), C-829) Parking. The cross-key rule applies only while
        // `approvals.park.enabled` is true (D-357); `validate.rs` names it.
        SettingDef::new(S, "approvals.park.enabled", SettingType::Bool, json!(false))
            .loosening(Loosening::Bool { loosens_when: true }),
        // A shorter threshold parks more requests: a looser setting is a lower number.
        SettingDef::new(
            S,
            "approvals.park.threshold_secs",
            SettingType::Int {
                min: 60,
                max: 604_800,
            },
            json!(900),
        )
        .loosening(Loosening::Number { up_loosens: false }),
        up(S, "approvals.park.max_timeout_secs", 604_800, 86_400),
        up(S, "approvals.park.resume_window_secs", 86_400, 1800),
        SettingDef::new(
            S,
            "approvals.park.deny_cooldown_secs",
            SettingType::Int {
                min: 0,
                max: 86_400,
            },
            json!(900),
        )
        .loosening(Loosening::Number { up_loosens: false }),
        up(S, "approvals.park.max_body_bytes", 1_048_576, 262_144),
        up(S, "approvals.park.max_pending_per_agent", 200, 20),
    ]
}

/// The `retention` section (SET-5): retention windows and attachment ceilings (`0` = no ceiling,
/// the loosest value; an unset window maximum is unbounded).
fn retention_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "retention";
    let days = |key: &'static str| {
        SettingDef::new(
            S,
            key,
            SettingType::Int {
                min: 1,
                max: 100_000,
            },
            Value::Null,
        )
        .loosening(Loosening::Number { up_loosens: true })
    };
    let mib = |key: &'static str, default: u64| {
        SettingDef::new(
            S,
            key,
            SettingType::Int {
                min: 0,
                max: 100_000_000,
            },
            json!(default),
        )
        .loosening(Loosening::NumberZeroIsMax)
    };
    vec![
        up(S, "retention.request_log_days", 100_000, 30),
        days("retention.conversation_days_max"),
        mib("attachments.default_org_quota_mib", 10 * 1024),
        mib("attachments.org_quota_mib_max", 50 * 1024),
        days("attachments.days_max"),
    ]
}

/// The `skills` section (SET-5): the git skill library.
fn skills_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "skills";
    vec![
        SettingDef::new(S, "skills.library.enabled", SettingType::Bool, json!(true)),
        SettingDef::new(
            S,
            "skills.library.refresh_interval_secs",
            SettingType::Int {
                min: 0,
                max: 604_800,
            },
            json!(900),
        ),
        up(
            S,
            "skills.library.bytes_per_agent",
            1_073_741_824,
            1024 * 1024,
        ),
    ]
}

/// The `previews` section (SET-5); `previews.enabled` and `previews.domain` stay in Helm.
fn previews_defs() -> Vec<SettingDef> {
    const S: &str = "previews";
    vec![
        up(S, "previews.session_ttl_secs", 2_592_000, 8 * 3600),
        up(S, "previews.session_idle_secs", 2_592_000, 3600),
        up(
            S,
            "previews.bytes_per_share_day",
            1_099_511_627_776,
            5 * 1024 * 1024 * 1024u64,
        ),
        up(S, "previews.shares_per_agent", 100_000, 10),
        up(S, "previews.shares_per_org", 1_000_000, 200),
        up(S, "previews.awake_cap_secs", 2_592_000, 4 * 3600),
    ]
}

/// The `scim` section (SET-5, D-237(d)): `scim.enabled` follows `scim.available` (configuration
/// only) until set, and cannot be turned on while the deployment does not route `/scim/v2`.
fn scim_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "scim";
    vec![
        SettingDef::new(S, "scim.enabled", SettingType::Bool, json!(false))
            .loosening(Loosening::Bool { loosens_when: true })
            .default_from("scim.available"),
        SettingDef::new(
            S,
            "scim.allowed_email_domains",
            SettingType::List {
                item: Box::new(SettingType::Str { max_len: 253 }),
                max_items: 256,
            },
            json!([]),
        )
        .whole_list()
        .loosening(Loosening::List {
            superset_loosens: false,
        }),
        SettingDef::new(
            S,
            "scim.rate_per_minute",
            SettingType::Int { min: 30, max: 6000 },
            json!(600),
        )
        .loosening(Loosening::Number { up_loosens: true })
        .helm("scim.ratePerMinute"),
        up(S, "scim.token_max_days", 730, 730),
    ]
}

/// The `terminal` section (SET-5, D-237(d)); `terminal.enabled` follows `terminal.available`.
fn terminal_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "terminal";
    vec![
        SettingDef::new(S, "terminal.enabled", SettingType::Bool, json!(false))
            .loosening(Loosening::Bool { loosens_when: true })
            .default_from("terminal.available"),
        up(S, "terminal.awake_cap_secs", 2_592_000, 4 * 3600),
        up(S, "terminal.wake_window_secs", 3600, 60),
        up(S, "terminal.wake_wait_secs", 600, 45),
    ]
}

/// The `limits` section (SET-5): the API's rate limits and the gateway's address and size limits.
fn limits_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "limits";
    vec![
        SettingDef::new(
            S,
            "ratelimit.api.multiplier",
            SettingType::Float {
                min: 0.1,
                max: 100.0,
            },
            json!(1.0),
        )
        .loosening(Loosening::Number { up_loosens: true }),
        up(S, "ratelimit.api.org_per_minute", 10_000_000, 3000),
        SettingDef::new(
            S,
            "ratelimit.api.classes",
            SettingType::List {
                item: Box::new(SettingType::Object),
                max_items: 16,
            },
            json!([]),
        )
        .keyed_table("class")
        .loosening(Loosening::None),
        SettingDef::new(
            S,
            "gateway.allowed_private",
            SettingType::List {
                item: Box::new(SettingType::Str { max_len: 64 }),
                max_items: 256,
            },
            json!([]),
        )
        .whole_list()
        .loosening(Loosening::List {
            superset_loosens: true,
        }),
        SettingDef::new(
            S,
            "gateway.condition_body_bytes",
            SettingType::Int {
                min: 4096,
                max: 8_388_608,
            },
            json!(262_144),
        )
        .loosening(Loosening::Number { up_loosens: true }),
        up(S, "gateway.upstream_header_timeout_secs", 86_400, 300),
    ]
}

/// Most `llm.endpoints` entries (Helm and dashboard together are checked against this on a write).
pub const LLM_ENDPOINTS_MAX: usize = 64;

/// The `governance` section (SET-2, D-237): the instance ceiling of `08 §11.1`. Defaults are those
/// of `raw.rs`; the replacement `governance.llm.known_llm_hosts` list stays configuration-only.
fn governance_defs() -> Vec<SettingDef> {
    use serde_json::json;
    const S: &str = "governance";
    let strs = |max_items| SettingType::List {
        item: Box::new(SettingType::Str { max_len: 256 }),
        max_items,
    };
    vec![
        SettingDef::new(
            S,
            "governance.llm.allowed_key_scopes",
            strs(8),
            json!(["organization", "workspace"]),
        )
        .whole_list()
        .loosening(Loosening::List {
            superset_loosens: true,
        }),
        SettingDef::new(
            S,
            "governance.llm.minimum_posture",
            SettingType::Enum(vec!["open", "sanctioned_only"]),
            json!("open"),
        )
        .loosening(Loosening::Order(vec!["sanctioned_only", "open"])),
        SettingDef::new(
            S,
            "governance.llm.require_key_origin",
            SettingType::Bool,
            json!(false),
        )
        .loosening(Loosening::Bool {
            loosens_when: false,
        }),
        SettingDef::new(
            S,
            "governance.llm.known_llm_hosts_extra",
            strs(200),
            json!([]),
        )
        .whole_list()
        .loosening(Loosening::List {
            superset_loosens: true,
        }),
        SettingDef::new(
            S,
            "governance.egress.minimum_posture",
            SettingType::Enum(vec!["open", "credentialed_only"]),
            json!("open"),
        )
        .loosening(Loosening::Order(vec!["credentialed_only", "open"])),
        SettingDef::new(
            S,
            "governance.egress.baseline_allowlist",
            SettingType::List {
                item: Box::new(SettingType::Object),
                max_items: 200,
            },
            json!([]),
        )
        .whole_list()
        .loosening(Loosening::List {
            superset_loosens: true,
        }),
        SettingDef::new(
            S,
            "governance.egress.open_lane_minimum",
            SettingType::Enum(vec!["enabled", "allowlist_only", "disabled"]),
            json!("enabled"),
        )
        .loosening(Loosening::Order(vec![
            "disabled",
            "allowlist_only",
            "enabled",
        ])),
        SettingDef::new(
            S,
            "governance.agent_tokens.max_age_days",
            SettingType::Int { min: 1, max: 3650 },
            Value::Null,
        )
        .loosening(Loosening::Number { up_loosens: true }),
        SettingDef::new(S, "byo.enabled", SettingType::Bool, json!(true))
            .loosening(Loosening::Bool { loosens_when: true }),
        SettingDef::new(
            S,
            "byo.minimum_posture",
            SettingType::Enum(vec!["enabled", "owners_only", "disabled"]),
            json!("owners_only"),
        )
        .loosening(Loosening::Order(vec!["disabled", "owners_only", "enabled"])),
        SettingDef::new(S, "byo.default_source_cidrs", strs(200), json!([]))
            .whole_list()
            .loosening(Loosening::List {
                superset_loosens: true,
            }),
        SettingDef::new(
            S,
            "byo.require_context_credentials",
            SettingType::Bool,
            json!(false),
        )
        .loosening(Loosening::Bool {
            loosens_when: false,
        }),
    ]
}

/// May a dashboard-defined store have this `kind`? Only Vault; the test store only where the
/// test registry is compiled in (SEC-89: no test switch is reachable from a browser session).
pub fn dashboard_store_kind_allowed(kind: &str) -> bool {
    kind == crate::VAULT_KIND || (cfg!(any(test, feature = "test-registry")) && kind == "memory")
}

#[cfg(any(test, feature = "test-registry"))]
fn test_defs() -> Vec<SettingDef> {
    use serde_json::json;
    let mut v = vec![
        SettingDef::new(
            TEST_SECTION,
            "settings_test.flag",
            SettingType::Bool,
            json!(false),
        )
        .loosening(Loosening::Bool { loosens_when: true }),
        SettingDef::new(
            TEST_SECTION,
            "settings_test.limit",
            SettingType::Int { min: 1, max: 100 },
            json!(10),
        )
        .loosening(Loosening::Number { up_loosens: true }),
        SettingDef::new(
            TEST_SECTION,
            "settings_test.mode",
            SettingType::Enum(vec!["strict", "standard", "open"]),
            json!("standard"),
        )
        .loosening(Loosening::Order(vec!["strict", "standard", "open"])),
        SettingDef::new(
            TEST_SECTION,
            "settings_test.names",
            SettingType::List {
                item: Box::new(SettingType::Str { max_len: 64 }),
                max_items: 20,
            },
            json!([]),
        )
        .whole_list()
        .loosening(Loosening::List {
            superset_loosens: true,
        }),
        SettingDef::new(
            TEST_SECTION,
            "settings_test.token",
            SettingType::Str { max_len: 256 },
            Value::Null,
        )
        .secret(),
        SettingDef::new(
            TEST_SECTION,
            "settings_test.items",
            SettingType::List {
                item: Box::new(SettingType::Object),
                max_items: 20,
            },
            json!([]),
        )
        .keyed("id"),
        // A second section, so an import can span two and prove it is all-or-nothing.
        SettingDef::new(
            TEST_SECTION_B,
            "settings_test_b.count",
            SettingType::Int { min: 1, max: 10 },
            json!(1),
        ),
    ];
    for d in &mut v {
        d.test_only = true;
    }
    v
}

// ---------------------------------------------------------------------------------------------
// Presence and the effective view
// ---------------------------------------------------------------------------------------------

/// Where an effective value comes from (C-772).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Helm,
    Dashboard,
    Default,
}

/// Values stored in `instance_settings`: section → dotted key → value.
#[derive(Debug, Clone, Default)]
pub struct Stored {
    pub sections: BTreeMap<String, BTreeMap<String, Value>>,
}

impl Stored {
    pub fn get(&self, section: &str, key: &str) -> Option<&Value> {
        self.sections.get(section).and_then(|s| s.get(key))
    }

    /// Stored values from `(section, value object)` rows, reading only `only` sections when
    /// given. A row of an unknown section and a key that is not a registered key of its section
    /// (written by a newer release, or hand-edited: SEC-89, D-238(c)) are ignored; the second
    /// value lists them for the caller to log.
    pub fn from_rows<'a>(
        rows: impl IntoIterator<Item = (&'a str, &'a Value)>,
        only: Option<&[&str]>,
    ) -> (Self, Vec<String>) {
        let reg = registry();
        let mut stored = Stored::default();
        let mut ignored = Vec::new();
        for (section, value) in rows {
            if only.is_some_and(|o| !o.contains(&section)) {
                continue;
            }
            if !reg.has_section(section) {
                ignored.push(format!("section {section}"));
                continue;
            }
            let mut keys = BTreeMap::new();
            if let Value::Object(map) = value {
                for (k, v) in map {
                    match reg.get(k) {
                        Some(d) if d.section == section => {
                            keys.insert(k.clone(), v.clone());
                        }
                        _ => ignored.push(format!("key {k} in section {section}")),
                    }
                }
            }
            stored.sections.insert(section.to_string(), keys);
        }
        (stored, ignored)
    }

    /// Stored values of the wrong shape or out of their bounds: `(key, message)`.
    pub fn type_problems(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (section, keys) in &self.sections {
            for (key, v) in keys {
                if let Some(d) = registry().get(key).filter(|d| d.section == section)
                    && let Err(m) = d.check(v)
                {
                    out.push((key.clone(), m));
                }
            }
        }
        out
    }
}

/// One entry of a keyed list with its source.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryView {
    pub id: String,
    pub value: Value,
    pub source: Source,
}

/// The effective value of one registered key.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveKey {
    pub value: Value,
    pub source: Source,
    /// Keyed lists only.
    pub entries: Option<Vec<EntryView>>,
}

/// The merged configuration and the per-key view.
#[derive(Debug)]
pub struct Effective {
    pub raw: RawConfig,
    pub keys: BTreeMap<String, EffectiveKey>,
}

/// The deployment configuration before deserialization, with the set of dotted keys it sets.
#[derive(Debug, Clone)]
pub struct ConfigSource {
    table: toml::Table,
    env: BTreeMap<String, String>,
    /// Everything the configuration sets, test-only keys included (they have no `RawConfig` field).
    json: Value,
    /// The dotted keys the file and `AURORA__…` overrides set (keyed-list entries as `key.<id>`).
    pub presence: BTreeSet<String>,
}

impl ConfigSource {
    /// Load like [`crate::load`], keeping the table and the presence set.
    pub fn load(path: Option<&Path>, env: impl Iterator<Item = (String, String)>) -> Result<Self> {
        let (table, env) = read_table(path, env)?;
        Self::from_table(table, env)
    }

    /// An empty configuration (tests: nothing is locked).
    pub fn empty() -> Self {
        Self {
            table: toml::Table::new(),
            env: BTreeMap::new(),
            json: Value::Object(serde_json::Map::new()),
            presence: BTreeSet::new(),
        }
    }

    /// Build from an already merged table (and the environment for the secret variables).
    #[allow(unused_mut)]
    pub fn from_table(mut table: toml::Table, env: BTreeMap<String, String>) -> Result<Self> {
        let json = serde_json::to_value(&table)
            .map_err(|e| ConfigErrors(vec![ConfigError::new("config", e.to_string())]))?;
        // Test-only keys have no `RawConfig` field: lift their table out before deserializing.
        #[cfg(any(test, feature = "test-registry"))]
        {
            table.remove(TEST_SECTION);
        }
        let mut presence = BTreeSet::new();
        collect_presence(&json, "", &mut presence);
        add_entry_presence(&json, &mut presence);
        Ok(Self {
            table,
            env,
            json,
            presence,
        })
    }

    /// Parse TOML text (tests).
    pub fn from_toml(text: &str, env: BTreeMap<String, String>) -> Result<Self> {
        let table = text
            .parse::<toml::Table>()
            .map_err(|e| ConfigErrors(vec![ConfigError::new("config", e.message().to_string())]))?;
        Self::from_table(table, env)
    }

    /// The configuration alone (no stored values): what [`crate::load`] returns.
    pub fn raw(&self) -> Result<RawConfig> {
        build_raw(&self.table, &self.env)
    }

    /// The value the configuration sets for `key` (any dotted path), if it does.
    pub fn helm_value(&self, key: &str) -> Option<&Value> {
        let mut cur = &self.json;
        for part in key.split('.') {
            cur = cur.as_object()?.get(part)?;
        }
        Some(cur)
    }

    /// The environment variable that supplies a **secret** key (`AURORA_SMTP_PASSWORD` or its
    /// `_FILE` form, C-772 `secret.from_env`): while it is set the key is locked.
    pub fn secret_env(&self, key: &str) -> Option<&'static str> {
        let name = match key {
            "email.smtp_password" => "AURORA_SMTP_PASSWORD",
            _ => return None,
        };
        let set = |n: &str| self.env.get(n).is_some_and(|v| !v.is_empty());
        (set(name) || set(&format!("{name}_FILE"))).then_some(name)
    }

    /// Does the configuration set this registered key (a keyed list counts when it has entries)?
    pub fn sets(&self, key: &str) -> bool {
        self.presence.contains(key)
    }

    /// `helm > stored > default` over the registry, then the merged [`RawConfig`] (not validated:
    /// the caller runs `api_config()` / `gateway_config()` on it, D-236(d)).
    pub fn effective(&self, stored: &Stored) -> Result<Effective> {
        let reg = registry();
        let mut table = self.table.clone();
        let mut keys = BTreeMap::new();
        let mut errors = Vec::new();
        for d in reg.defs() {
            let stored_v = stored.get(d.section, d.key).filter(|_| !d.secret);
            let helm_v = self.helm_value(d.key).filter(|_| self.sets(d.key));
            let eff = match d.list {
                Some(ListKind::Keyed { id_field }) => {
                    let mut entries: Vec<EntryView> = Vec::new();
                    for e in helm_v.map(|v| list_entries(d, v)).into_iter().flatten() {
                        entries.push(EntryView {
                            id: entry_id(&e, id_field),
                            value: e,
                            source: Source::Helm,
                        });
                    }
                    for e in stored_v.and_then(Value::as_array).into_iter().flatten() {
                        let id = entry_id(e, id_field);
                        // An entry the configuration defines replaces a stored one (D-236(b)).
                        if entries.iter().any(|x| x.id == id) {
                            continue;
                        }
                        entries.push(EntryView {
                            id,
                            value: e.clone(),
                            source: Source::Dashboard,
                        });
                    }
                    let source = if entries.iter().any(|e| e.source == Source::Helm) {
                        Source::Helm
                    } else if entries.is_empty() {
                        Source::Default
                    } else {
                        Source::Dashboard
                    };
                    let value = Value::Array(entries.iter().map(|e| e.value.clone()).collect());
                    if !entries.is_empty() && !d.test_only {
                        let wire = if d.as_table {
                            entries_to_table(id_field, &entries)
                        } else {
                            value.clone()
                        };
                        put(&mut table, d.key, &wire, &mut errors);
                    }
                    EffectiveKey {
                        value: if entries.is_empty() {
                            d.default.clone()
                        } else {
                            value
                        },
                        source,
                        entries: Some(entries),
                    }
                }
                _ => match (helm_v, stored_v) {
                    (Some(v), _) => EffectiveKey {
                        value: v.clone(),
                        source: Source::Helm,
                        entries: None,
                    },
                    (None, Some(v)) => {
                        if !d.test_only {
                            put(&mut table, d.key, v, &mut errors);
                        }
                        EffectiveKey {
                            value: v.clone(),
                            source: Source::Dashboard,
                            entries: None,
                        }
                    }
                    (None, None) => {
                        // A default that follows another configuration key (D-237(d)).
                        let followed = d
                            .default_from
                            .and_then(|k| self.helm_value(k))
                            .and_then(Value::as_bool);
                        let value = followed.map_or_else(|| d.default.clone(), Value::Bool);
                        if let Some(b) = followed.filter(|_| !d.test_only) {
                            put(&mut table, d.key, &Value::Bool(b), &mut errors);
                        }
                        EffectiveKey {
                            value,
                            source: Source::Default,
                            entries: None,
                        }
                    }
                },
            };
            keys.insert(d.key.to_string(), eff);
        }
        if !errors.is_empty() {
            return Err(ConfigErrors(errors));
        }
        let mut raw = build_raw(&table, &self.env)?;
        // A secret key is not part of the merged table: the sealed SMTP password rides in the
        // typed configuration so validation sees it (the environment's password wins, D-237(f)).
        if self.secret_env("email.smtp_password").is_none()
            && let Some(v) = stored.get("email", "email.smtp_password")
            && let Ok(s) = serde_json::from_value::<crate::raw::SealedSecret>(v.clone())
        {
            raw.email.smtp_password_sealed = Some(s);
        }
        Ok(Effective { raw, keys })
    }
}

/// The entries a configuration value of a keyed list holds: the array itself, or for a table-form
/// list (`ratelimit.api.classes`) one entry per table key, carrying that key as its id field.
pub fn list_entries(d: &SettingDef, v: &Value) -> Vec<Value> {
    let id_field = match d.list {
        Some(ListKind::Keyed { id_field }) => id_field,
        _ => "id",
    };
    match v {
        Value::Array(a) => a.clone(),
        Value::Object(m) if d.as_table => m
            .iter()
            .map(|(id, body)| {
                let mut e = body.as_object().cloned().unwrap_or_default();
                e.insert(id_field.into(), Value::String(id.clone()));
                Value::Object(e)
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The table form of entries (the inverse of [`list_entries`]): each entry's id is the key.
fn entries_to_table(id_field: &str, entries: &[EntryView]) -> Value {
    let mut out = serde_json::Map::new();
    for e in entries {
        let mut body = e.value.as_object().cloned().unwrap_or_default();
        body.remove(id_field);
        out.insert(e.id.clone(), Value::Object(body));
    }
    Value::Object(out)
}

fn entry_id(e: &Value, id_field: &str) -> String {
    e.get(id_field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn put(table: &mut toml::Table, key: &str, v: &Value, errors: &mut Vec<ConfigError>) {
    let parts: Vec<String> = key.split('.').map(str::to_string).collect();
    let Some((last, parents)) = parts.split_last() else {
        return;
    };
    let tv = match toml::Value::try_from(v) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigError::new(
                key,
                format!("stored value cannot be used: {e}"),
            ));
            return;
        }
    };
    if let Err(p) = set_path(table, parents, last, tv) {
        errors.push(ConfigError::new(
            key,
            format!("`{p}` is not a table in the configuration"),
        ));
    }
}

fn collect_presence(v: &Value, prefix: &str, out: &mut BTreeSet<String>) {
    let Value::Object(map) = v else { return };
    for (k, child) in map {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match child {
            Value::Object(_) => collect_presence(child, &path, out),
            // An empty array still counts as set: the operator pinned an empty list.
            _ => {
                out.insert(path);
            }
        }
    }
}

/// `key.<id>` for the entries of every registered keyed list the configuration defines.
fn add_entry_presence(json: &Value, out: &mut BTreeSet<String>) {
    for d in registry().defs() {
        let Some(ListKind::Keyed { id_field }) = d.list else {
            continue;
        };
        let mut cur = json;
        let mut found = true;
        for part in d.key.split('.') {
            match cur.as_object().and_then(|o| o.get(part)) {
                Some(n) => cur = n,
                None => {
                    found = false;
                    break;
                }
            }
        }
        if !found {
            continue;
        }
        let entries = list_entries(d, cur);
        if d.as_table && !entries.is_empty() {
            // A table does not count as set on its own (its members are the presence records).
            out.insert(d.key.to_string());
        }
        for e in &entries {
            let id = entry_id(e, id_field);
            if !id.is_empty() {
                out.insert(format!("{}.{id}", d.key));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn src(toml: &str) -> ConfigSource {
        ConfigSource::from_toml(toml, BTreeMap::new()).unwrap()
    }

    fn stored(pairs: &[(&str, Value)]) -> Stored {
        let mut s = Stored::default();
        for (k, v) in pairs {
            s.sections
                .entry(TEST_SECTION.to_string())
                .or_default()
                .insert((*k).to_string(), v.clone());
        }
        s
    }

    fn stored_at(section: &str, pairs: &[(&str, Value)]) -> Stored {
        let mut s = Stored::default();
        for (k, v) in pairs {
            s.sections
                .entry(section.to_string())
                .or_default()
                .insert((*k).to_string(), v.clone());
        }
        s
    }

    #[test]
    fn presence_records_file_keys_and_keyed_entry_ids() {
        let s = src(
            "[settings_test]\nflag = true\nnames = [\"a\"]\n[[settings_test.items]]\nid = \"x\"\n[server]\nbind = \"0.0.0.0:1\"\n",
        );
        assert!(s.sets("settings_test.flag"));
        assert!(s.sets("settings_test.names"));
        assert!(s.sets("settings_test.items.x"));
        assert!(s.sets("server.bind"));
        assert!(!s.sets("settings_test.limit"));
    }

    #[test]
    fn presence_records_env_overrides() {
        let dir =
            std::env::temp_dir().join(format!("aurora-settings-presence-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c.toml");
        std::fs::write(&file, "[settings_test]\nflag = true\n").unwrap();
        let env = [("AURORA__SETTINGS_TEST__LIMIT".to_string(), "7".to_string())];
        let s = ConfigSource::load(Some(&file), env.into_iter()).unwrap();
        assert!(s.sets("settings_test.limit"), "override sets the key");
        assert!(s.sets("settings_test.flag"), "file sets the key");
        let e = s.effective(&Stored::default()).unwrap();
        assert_eq!(e.keys["settings_test.limit"].value, json!(7));
        assert_eq!(e.keys["settings_test.limit"].source, Source::Helm);
    }

    #[test]
    fn helm_beats_stored_beats_default() {
        let s = src("[settings_test]\nlimit = 5\n");
        let e = s
            .effective(&stored(&[
                ("settings_test.limit", json!(50)),
                ("settings_test.mode", json!("strict")),
            ]))
            .unwrap();
        assert_eq!(e.keys["settings_test.limit"].source, Source::Helm);
        assert_eq!(e.keys["settings_test.limit"].value, json!(5));
        assert_eq!(e.keys["settings_test.mode"].source, Source::Dashboard);
        assert_eq!(e.keys["settings_test.mode"].value, json!("strict"));
        assert_eq!(e.keys["settings_test.flag"].source, Source::Default);
        assert_eq!(e.keys["settings_test.flag"].value, json!(false));
    }

    #[test]
    fn whole_list_locks_as_a_whole() {
        let s = src("[settings_test]\nnames = [\"helm\"]\n");
        let e = s
            .effective(&stored(&[("settings_test.names", json!(["dash"]))]))
            .unwrap();
        assert_eq!(e.keys["settings_test.names"].source, Source::Helm);
        assert_eq!(e.keys["settings_test.names"].value, json!(["helm"]));
    }

    #[test]
    fn keyed_list_merges_by_id_with_helm_winning() {
        let s = src("[[settings_test.items]]\nid = \"h\"\nvalue = 1\n");
        let e = s
            .effective(&stored(&[(
                "settings_test.items",
                json!([{"id": "h", "value": 9}, {"id": "d", "value": 2}]),
            )]))
            .unwrap();
        let k = &e.keys["settings_test.items"];
        let entries = k.entries.as_ref().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            (entries[0].id.as_str(), entries[0].source),
            ("h", Source::Helm)
        );
        assert_eq!(entries[0].value["value"], json!(1));
        assert_eq!(
            (entries[1].id.as_str(), entries[1].source),
            ("d", Source::Dashboard)
        );
        assert_eq!(k.source, Source::Helm);
    }

    /// SET-5 (D-237(d)): `terminal.enabled` / `scim.enabled` follow the configuration-only
    /// `available` key until set, a stored value wins over that default, and `available` is
    /// never a setting.
    #[test]
    fn availability_is_the_default_of_the_terminal_and_scim_switches() {
        let off = src("").effective(&Stored::default()).unwrap();
        assert_eq!(off.keys["terminal.enabled"].value, json!(false));
        assert!(!off.raw.terminal.enabled && !off.raw.scim.enabled);

        let on = src("[terminal]\navailable = true\n[scim]\navailable = true\n")
            .effective(&Stored::default())
            .unwrap();
        assert_eq!(on.keys["terminal.enabled"].value, json!(true));
        assert_eq!(on.keys["terminal.enabled"].source, Source::Default);
        assert!(on.raw.terminal.enabled && on.raw.terminal.available);
        assert!(on.raw.scim.enabled && on.raw.scim.available);

        // A stored `false` turns it off although the deployment provides the wiring.
        let stored_off = src("[terminal]\navailable = true\n")
            .effective(&stored_at(
                "terminal",
                &[("terminal.enabled", json!(false))],
            ))
            .unwrap();
        assert!(!stored_off.raw.terminal.enabled && stored_off.raw.terminal.available);

        // A configuration that sets the key itself locks it.
        let helm = src("[terminal]\navailable = true\nenabled = false\n")
            .effective(&Stored::default())
            .unwrap();
        assert_eq!(helm.keys["terminal.enabled"].source, Source::Helm);
        assert!(!helm.raw.terminal.enabled);

        let r = registry();
        for k in ["terminal.available", "scim.available"] {
            assert!(r.get(k).is_none(), "{k} must not be a setting");
        }
    }

    /// SET-5 (C-782): `ratelimit.api.classes` is a table in the configuration and a keyed list
    /// (by `class`) in settings; a configuration class is locked, stored classes sit beside it.
    #[test]
    fn rate_classes_are_a_keyed_list_over_a_table() {
        let s = src("[ratelimit.api.classes.write]\nper_minute = 5\n");
        assert!(s.sets("ratelimit.api.classes") && s.sets("ratelimit.api.classes.write"));
        let e = s
            .effective(&stored_at(
                "limits",
                &[(
                    "ratelimit.api.classes",
                    json!([{"class": "write", "per_minute": 99}, {"class": "export", "per_hour": 3}]),
                )],
            ))
            .unwrap();
        let k = &e.keys["ratelimit.api.classes"];
        let entries = k.entries.as_ref().unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|x| (x.id.as_str(), x.source))
                .collect::<Vec<_>>(),
            [("write", Source::Helm), ("export", Source::Dashboard)]
        );
        // The merged configuration has the table form with the Helm class winning.
        let api = &e.raw.ratelimit.api;
        assert_eq!(api.classes["write"].per_minute, Some(5));
        assert_eq!(api.classes["export"].per_hour, Some(3));
        assert_eq!(api.classes.len(), 2);
        // Nothing set: no entries, the default (an empty list).
        let none = src("").effective(&Stored::default()).unwrap();
        assert_eq!(none.keys["ratelimit.api.classes"].value, json!([]));
        assert!(none.raw.ratelimit.api.classes.is_empty());
    }

    #[test]
    fn a_decimal_setting_checks_its_bounds_and_the_multiplier_loosens_upward() {
        let d = registry().get("ratelimit.api.multiplier").unwrap();
        assert!(d.check(&json!(0.5)).is_ok() && d.check(&json!(100)).is_ok());
        assert!(d.check(&json!(0.05)).is_err() && d.check(&json!(101)).is_err());
        assert!(d.check(&json!("2")).is_err());
        assert!(d.is_loosening(&json!(1.0), &json!(2.0)));
        assert!(!d.is_loosening(&json!(2.0), &json!(1.0)));
        let p = registry().get("gateway.allowed_private").unwrap();
        assert!(p.is_loosening(&json!([]), &json!(["10.0.0.0/8"])));
        assert!(!p.is_loosening(&json!(["10.0.0.0/8"]), &json!([])));
    }

    #[test]
    fn the_llm_section_locks_endpoints_per_entry_and_the_lists_as_a_whole() {
        let r = registry();
        let keys: Vec<&str> = r.section("llm").map(|d| d.key).collect();
        assert_eq!(
            keys,
            [
                "llm.enabled_providers",
                "llm.provider_preference",
                "llm.endpoints"
            ]
        );
        assert!(matches!(
            r.get("llm.endpoints").unwrap().list,
            Some(ListKind::Keyed { id_field: "id" })
        ));
        for k in ["llm.enabled_providers", "llm.provider_preference"] {
            assert!(matches!(r.get(k).unwrap().list, Some(ListKind::Whole)));
        }
        // `llm.dev_upstream_overrides` is never a setting (SEC-89).
        assert!(r.get("llm.dev_upstream_overrides").is_none());
        // Defaults are `LlmSection`'s, so an unset key shows what the application does.
        let d = crate::LlmSection::default();
        assert_eq!(
            r.get("llm.enabled_providers").unwrap().default,
            json!(d.enabled_providers)
        );
        assert_eq!(
            r.get("llm.provider_preference").unwrap().default,
            json!(d.provider_preference)
        );
        // A Helm endpoint is one locked entry; a dashboard endpoint sits beside it.
        let s = src(
            "[[llm.endpoints]]\nid = \"central\"\nprovider = \"litellm\"\nbase_url = \"https://l.example\"\n",
        );
        let mut st = Stored::default();
        st.sections.entry("llm".to_string()).or_default().insert(
            "llm.endpoints".to_string(),
            json!([{"id": "team", "provider": "litellm", "base_url": "https://t.example"}]),
        );
        let e = s.effective(&st).unwrap();
        let entries = e.keys["llm.endpoints"].entries.as_ref().unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|x| (x.id.as_str(), x.source))
                .collect::<Vec<_>>(),
            [("central", Source::Helm), ("team", Source::Dashboard)]
        );
        assert_eq!(e.raw.llm.endpoints.len(), 2);
    }

    #[test]
    fn secret_values_never_enter_the_merge() {
        let s = src("");
        let e = s
            .effective(&stored(&[(
                "settings_test.token",
                json!({"$secret": {"ref": "x"}}),
            )]))
            .unwrap();
        assert_eq!(e.keys["settings_test.token"].source, Source::Default);
    }

    #[test]
    fn loosening_orders() {
        let r = registry();
        let flag = r.get("settings_test.flag").unwrap();
        assert!(flag.is_loosening(&json!(false), &json!(true)));
        assert!(!flag.is_loosening(&json!(true), &json!(false)));
        let mode = r.get("settings_test.mode").unwrap();
        assert!(mode.is_loosening(&json!("strict"), &json!("open")));
        assert!(!mode.is_loosening(&json!("open"), &json!("strict")));
        let names = r.get("settings_test.names").unwrap();
        assert!(names.is_loosening(&json!(["a"]), &json!(["a", "b"])));
        assert!(!names.is_loosening(&json!(["a", "b"]), &json!(["a"])));
        let limit = r.get("settings_test.limit").unwrap();
        assert!(limit.is_loosening(&json!(1), &json!(2)));
    }

    #[test]
    fn the_sign_in_section_orders_what_loosens_and_the_cli_key_ttl_treats_zero_as_never() {
        let r = registry();
        let signup = r.get("auth.signup_mode").unwrap();
        assert!(signup.is_loosening(&json!("invite_only"), &json!("sso_jit")));
        assert!(signup.is_loosening(&json!("sso_jit"), &json!("open")));
        assert!(!signup.is_loosening(&json!("open"), &json!("invite_only")));
        let org = r.get("auth.org_creation").unwrap();
        assert!(org.is_loosening(&json!("instance_admins"), &json!("anyone")));
        let abs = r.get("auth.session_absolute_secs").unwrap();
        assert!(abs.is_loosening(&json!(3600), &json!(7200)));
        assert!(!abs.is_loosening(&json!(7200), &json!(3600)));
        let ttl = r.get("auth.cli_key_ttl_days").unwrap();
        assert!(ttl.is_loosening(&json!(90), &json!(0)), "0 = never expires");
        assert!(ttl.is_loosening(&json!(30), &json!(90)));
        assert!(!ttl.is_loosening(&json!(0), &json!(90)));
        // Providers are a keyed list by id; the password is a write-only secret.
        let p = r.get("auth.oidc_providers").unwrap();
        assert_eq!(p.list, Some(ListKind::Keyed { id_field: "id" }));
        assert!(r.get("email.smtp_password").unwrap().secret);
        assert!(
            r.get("email.log_links").is_none(),
            "a development switch is never a setting"
        );
    }

    #[test]
    fn a_secret_the_environment_supplies_locks_its_key() {
        let env = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect::<BTreeMap<_, _>>()
        };
        let none = ConfigSource::from_toml("", env(&[])).unwrap();
        assert_eq!(none.secret_env("email.smtp_password"), None);
        let var = ConfigSource::from_toml("", env(&[("AURORA_SMTP_PASSWORD", "x")])).unwrap();
        assert_eq!(
            var.secret_env("email.smtp_password"),
            Some("AURORA_SMTP_PASSWORD")
        );
        let file =
            ConfigSource::from_toml("", env(&[("AURORA_SMTP_PASSWORD_FILE", "/p")])).unwrap();
        assert_eq!(
            file.secret_env("email.smtp_password"),
            Some("AURORA_SMTP_PASSWORD")
        );
        assert_eq!(var.secret_env("email.from"), None);
    }

    #[test]
    fn type_checks_bound_values() {
        let r = registry();
        let limit = r.get("settings_test.limit").unwrap();
        assert!(limit.check(&json!(100)).is_ok());
        assert!(limit.check(&json!(101)).is_err());
        assert!(limit.check(&json!("x")).is_err());
        let mode = r.get("settings_test.mode").unwrap();
        assert!(mode.check(&json!("nope")).is_err());
    }

    #[test]
    fn registry_keys_equal_the_checked_in_movable_list_and_exclude_non_settings() {
        let listed = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/helm/aurora/movable-keys.txt"
        ))
        .unwrap_or_default();
        let listed: Vec<&str> = listed
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .collect();
        assert_eq!(
            listed,
            registry().movable_keys(),
            "movable-keys.txt must equal the registry"
        );
        for d in registry().defs() {
            for n in NOT_SETTINGS {
                let hit = if n.ends_with('.') {
                    d.key.starts_with(n)
                } else {
                    d.key == *n
                };
                assert!(!hit || d.test_only, "{} is a non-setting (SEC-89)", d.key);
            }
            assert!(!d.key.starts_with("env_secrets"), "{}", d.key);
        }
        for s in SECTION_IDS {
            assert!(registry().has_section(s));
        }
    }

    #[test]
    fn governance_loosening_orders_and_the_removed_token_age() {
        let r = registry();
        let d = |k: &str| r.get(k).unwrap();
        let (open, sanc) = (json!("open"), json!("sanctioned_only"));
        assert!(d("governance.llm.minimum_posture").is_loosening(&sanc, &open));
        assert!(!d("governance.llm.minimum_posture").is_loosening(&open, &sanc));
        assert!(d("byo.enabled").is_loosening(&json!(false), &json!(true)));
        assert!(d("byo.minimum_posture").is_loosening(&json!("owners_only"), &json!("enabled")));
        let age = d("governance.agent_tokens.max_age_days");
        assert!(age.is_loosening(&json!(30), &json!(90)));
        assert!(
            age.is_loosening(&json!(30), &Value::Null),
            "removing the maximum loosens"
        );
        assert!(!age.is_loosening(&Value::Null, &json!(30)));
    }
}
