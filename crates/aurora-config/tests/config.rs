#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use aurora_config::*;
use std::path::PathBuf;

const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="; // 32 zero bytes

fn secrets() -> Vec<(String, String)> {
    let k = format!("main:{KEY}");
    [
        ("AURORA_DATABASE_URL", "postgres://u:pw@db/aurora"),
        ("AURORA_ENCRYPTION_KEYS", k.as_str()),
        ("AURORA_ENCRYPTION_ACTIVE_KEY", "main"),
        ("AURORA_SIGNING_KEY", KEY),
        ("AURORA_INTERNAL_SECRET", KEY),
        ("AURORA_RUNNER_SESSION_KEY", KEY),
        ("AURORA_RUNNER_TOKEN", "aur_abc"),
        ("AURORA_CONNECTOR_TOKEN", "aun_abc"),
        ("BASE_IDP_SECRET", "base-secret"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

fn write(name: &str, text: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("aurora-config-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn load_with(toml: &str, extra: &[(&str, &str)]) -> Result<RawConfig> {
    let p = write("c.toml", toml);
    let mut e = secrets();
    e.extend(extra.iter().map(|(a, b)| (a.to_string(), b.to_string())));
    load(Some(&p), e.into_iter())
}

// Phase 1 (RFE-004): at least one OIDC provider is required, so every config carries one.
const BASE: &str = "[[auth.oidc_providers]]\nid = \"base\"\nissuer = \"https://idp.example.org\"\nclient_id = \"b\"\nclient_secret_env = \"BASE_IDP_SECRET\"\n[server]\nexternal_url = \"https://aurora.example.org\"\n";

#[test]
fn toml_only_and_api_validates() {
    let raw = load_with(BASE, &[]).unwrap();
    let api = raw.api_config().unwrap();
    assert_eq!(api.external_url, "https://aurora.example.org");
    assert_eq!(api.max_connections, 20);
    assert_eq!(api.advertised_proxy_url, "https://aurora.example.org:4102");
    assert_eq!(api.keyring.active, "main");
}

// P4-HAR (D-122): `hosted.enabled_harnesses` defaults to `["claude-code"]` (nothing changes until an
// operator opts in); an unknown id, an empty list, a default that is not enabled and an ungated
// `fake` refuse startup.
#[test]
fn hosted_enabled_harnesses_defaults_and_validation() {
    let ok = |extra: &str| {
        load_with(&format!("{BASE}{extra}"), &[])
            .unwrap()
            .api_config()
    };
    let api = ok("").unwrap();
    assert_eq!(api.hosted.enabled_harnesses, ["claude-code"]);
    assert_eq!(api.hosted.default_harness, "claude-code");
    let api = ok(
        "[hosted]\nenabled_harnesses = [\"claude-code\", \"codex\"]\ndefault_harness = \"codex\"\n",
    )
    .unwrap();
    assert_eq!(api.hosted.enabled_harnesses, ["claude-code", "codex"]);
    for (bad, why) in [
        ("[hosted]\nenabled_harnesses = [\"gemini\"]\n", "unknown id"),
        ("[hosted]\nenabled_harnesses = []\n", "empty"),
        (
            "[hosted]\nenabled_harnesses = [\"fake\"]\n",
            "fake without allow_fake_harness",
        ),
        (
            "[hosted]\ndefault_harness = \"codex\"\n",
            "default not enabled",
        ),
    ] {
        let e = ok(bad).unwrap_err();
        assert!(
            e.has_key("hosted.enabled_harnesses") || e.has_key("hosted.default_harness"),
            "{why}: {e:?}"
        );
    }
    // `fake` is fine when explicitly allowed.
    assert!(
        ok(
            "[hosted]\nallow_fake_harness = true\nenabled_harnesses = [\"claude-code\", \"fake\"]\n"
        )
        .is_ok()
    );
    // The Kubernetes runner names the runtimes of its image (it cannot read the label).
    let raw = load_with(
        &format!("{BASE}[runner]\nbackend = \"podman\"\napi_url = \"http://api:4101\"\nimage_harnesses = [\"claude-code\", \"codex\"]\n"),
        &[],
    )
    .unwrap();
    assert_eq!(
        raw.runner_config().unwrap().runner.image_harnesses,
        ["claude-code", "codex"]
    );
    let raw = load_with(
        &format!("{BASE}[runner]\nbackend = \"podman\"\napi_url = \"http://api:4101\"\nimage_harnesses = [\"gemini\"]\n"),
        &[],
    )
    .unwrap();
    assert!(
        raw.runner_config()
            .unwrap_err()
            .has_key("runner.image_harnesses")
    );
}

// P2-CAT-0 (C-210): the built-in catalogue directory defaults to the image path, can be moved
// and may not be empty.
#[test]
fn plugins_builtin_dir_default_override_and_empty() {
    let raw = load_with(BASE, &[]).unwrap();
    assert_eq!(raw.plugins.builtin_dir, "/usr/share/aurora/plugins");
    let raw = load_with(BASE, &[("AURORA__PLUGINS__BUILTIN_DIR", "/opt/builtin")]).unwrap();
    assert_eq!(raw.plugins.builtin_dir, "/opt/builtin");
    assert!(raw.api_config().is_ok());
    let raw = load_with(&format!("{BASE}[plugins]\nbuiltin_dir = \"\"\n"), &[]).unwrap();
    assert!(raw.api_config().unwrap_err().has_key("plugins.builtin_dir"));
}

// FIX-015 (RFE-016): `[events]` defaults, a valid window, both out-of-range ends, unknown bus id.
#[test]
fn events_delta_bus_and_coalesce_window() {
    let raw = load_with(BASE, &[]).unwrap();
    assert_eq!(raw.events.delta_bus, "notify");
    assert_eq!(raw.events.delta_coalesce_ms, 100);
    let api = raw.api_config().unwrap();
    assert_eq!(
        (api.events.delta_bus.as_str(), api.events.delta_coalesce_ms),
        ("notify", 100)
    );
    for ok in ["100", "250", "500"] {
        let raw = load_with(BASE, &[("AURORA__EVENTS__DELTA_COALESCE_MS", ok)]).unwrap();
        let api = raw.api_config().unwrap();
        assert_eq!(api.events.delta_coalesce_ms.to_string(), ok);
    }
    for bad in ["0", "99", "501"] {
        let raw = load_with(BASE, &[("AURORA__EVENTS__DELTA_COALESCE_MS", bad)]).unwrap();
        let e = raw.api_config().unwrap_err();
        assert!(e.has_key("events.delta_coalesce_ms"), "{bad}: {e}");
    }
    let raw = load_with(BASE, &[("AURORA__EVENTS__DELTA_BUS", "kafka")]).unwrap();
    let e = raw.api_config().unwrap_err();
    assert!(e.has_key("events.delta_bus"), "{e}");
    // The same keys in the TOML file.
    let toml = format!("{BASE}[events]\ndelta_bus = \"notify\"\ndelta_coalesce_ms = 300\n");
    let api = load_with(&toml, &[]).unwrap().api_config().unwrap();
    assert_eq!(api.events.delta_coalesce_ms, 300);
}

// P2-011A (RFE-011a, D-56): the instance maximum of approval timeouts is bounded and always
// below the turn ceiling minus its warning; startup refuses anything else.
#[test]
fn approvals_max_timeout_bounds_and_turn_ceiling() {
    let raw = load_with(BASE, &[]).unwrap();
    assert_eq!(raw.approvals.max_timeout_secs, 3600);
    assert!(raw.approvals.email_notifications);
    assert_eq!(raw.api_config().unwrap().approvals.max_timeout_secs, 3600);
    for ok in ["300", "14400"] {
        let raw = load_with(BASE, &[("AURORA__APPROVALS__MAX_TIMEOUT_SECS", ok)]).unwrap();
        assert!(raw.api_config().is_ok(), "{ok}");
    }
    for bad in ["299", "14401", "0"] {
        let raw = load_with(BASE, &[("AURORA__APPROVALS__MAX_TIMEOUT_SECS", bad)]).unwrap();
        let e = raw.api_config().unwrap_err();
        assert!(e.has_key("approvals.max_timeout_secs"), "{bad}: {e}");
    }
    // Maximum 3600 against a ceiling of 3600 + 900: refused; one second more ceiling: accepted.
    let env = |ceiling: &'static str| {
        load_with(
            BASE,
            &[
                ("AURORA__APPROVALS__MAX_TIMEOUT_SECS", "3600"),
                ("AURORA__HOSTED__TURN_CEILING_SECS", ceiling),
            ],
        )
        .unwrap()
    };
    let e = env("4500").api_config().unwrap_err();
    assert!(e.has_key("approvals.max_timeout_secs"), "{e}");
    assert!(env("4501").api_config().is_ok());
}

#[test]
fn env_overrides_nested_and_typed() {
    let raw = load_with(
        &format!("{BASE}[runner]\nbackend = \"podman\"\n"),
        &[
            ("AURORA__RUNNER__K8S__NAMESPACE", "aur"),
            ("AURORA__RUNNER__BACKEND", "fake"),
            ("AURORA__RUNNER__MAX_SANDBOXES", "8"),
            ("AURORA__AUTH__INSTANCE_ADMINS", "[\"a@b.c\"]"),
            ("AURORA__TELEMETRY__LOG_LEVEL", "debug"),
        ],
    )
    .unwrap();
    assert_eq!(raw.runner.k8s.namespace.as_deref(), Some("aur"));
    assert_eq!(raw.runner.backend.as_deref(), Some("fake"));
    assert_eq!(raw.runner.max_sandboxes, 8);
    assert_eq!(raw.auth.instance_admins, vec!["a@b.c"]);
    assert_eq!(raw.telemetry.log_level, "debug");
}

#[test]
fn env_only_without_file() {
    let mut e = secrets();
    e.push(("AURORA_CONFIG".into(), "/nonexistent/x.toml".into()));
    assert!(load(None, e.clone().into_iter()).is_err()); // explicit path must exist
    e.pop();
    e.push((
        "AURORA__SERVER__EXTERNAL_URL".into(),
        "http://localhost:4100".into(),
    ));
    e.push(("AURORA__AUTH__DEV_IDP__ENABLED".into(), "true".into()));
    e.push((
        "AURORA__AUTH__DEV_IDP__ISSUER".into(),
        "http://localhost:4107".into(),
    ));
    // default path normally absent in test environments
    if !std::path::Path::new(DEFAULT_CONFIG_PATH).exists() {
        let raw = load(None, e.into_iter()).unwrap();
        raw.api_config().unwrap();
    }
}

#[test]
fn file_secret() {
    let f = write("secret", &format!("{KEY}\n"));
    let mut e: Vec<(String, String)> = secrets()
        .into_iter()
        .filter(|(k, _)| k != "AURORA_SIGNING_KEY")
        .collect();
    e.push(("AURORA_SIGNING_KEY_FILE".into(), f.display().to_string()));
    let p = write("c2.toml", BASE);
    let raw = load(Some(&p), e.into_iter()).unwrap();
    assert_eq!(raw.env_secrets.signing_key.unwrap().expose(), KEY);
}

#[test]
fn unknown_key_names_path() {
    let e = load_with(&format!("{BASE}[gateway]\nbogus = 1\n"), &[]).unwrap_err();
    assert!(e.has_key("gateway.bogus"), "{e}");
    let e = load_with(BASE, &[("AURORA__SERVER__NOPE", "x")]).unwrap_err();
    assert!(e.has_key("server.nope"), "{e}");
    let e = load_with("top = 1\n", &[]).unwrap_err();
    assert!(e.has_key("top"), "{e}");
}

#[test]
fn collects_multiple_errors() {
    let raw = load_with(
        "[auth]\nsignup_mode = \"nope\"\n[gateway]\ncondition_body_bytes = 10\n",
        &[],
    )
    .unwrap();
    let mut raw = raw;
    raw.env_secrets.signing_key = None;
    let e = raw.api_config().unwrap_err();
    for k in [
        "server.external_url",
        "auth.signup_mode",
        "AURORA_SIGNING_KEY",
    ] {
        assert!(e.has_key(k), "missing {k}: {e}");
    }
    let e = raw.gateway_config().unwrap_err();
    assert!(e.has_key("gateway.condition_body_bytes"), "{e}");
}

#[test]
fn defaults_match_table() {
    let r = RawConfig::default();
    assert_eq!(r.server.bind, "0.0.0.0");
    assert_eq!(r.auth.signup_mode, "invite_only");
    assert_eq!(r.auth.org_creation, "instance_admins");
    assert_eq!(r.auth.session_idle_secs, 1_209_600);
    assert_eq!(r.auth.session_absolute_secs, 2_592_000);
    assert_eq!(r.secrets.default_store, "inline");
    assert_eq!(r.llm.enabled_providers, ["litellm", "anthropic"]);
    assert_eq!(
        r.governance.llm.allowed_key_scopes,
        ["organization", "workspace"]
    );
    assert_eq!(r.governance.llm.minimum_posture, "open");
    assert_eq!(r.governance.egress.minimum_posture, "open");
    assert_eq!(r.governance.egress.open_lane_minimum, "enabled");
    assert_eq!(r.agents.default_name, "Daneel");
    assert!(r.hosted.enabled);
    assert_eq!(r.hosted.idle_stop_secs, 1800);
    assert_eq!(r.gateway.condition_body_bytes, 262_144);
    assert_eq!(r.gateway.upstream_header_timeout_secs, 300);
    assert_eq!(r.gateway.ca_dir, "/var/lib/aurora/ca");
    assert_eq!(r.approvals.timeout_secs, 300);
    assert_eq!(r.runner.max_sandboxes, 4);
    assert_eq!(r.runner.sandbox_memory_mib, 2048);
    assert_eq!(r.runner.orphan_reap, "reap");
    assert_eq!(r.retention.request_log_days, 30);
    assert_eq!(r.attachments.default_org_quota_mib, 10240);
    assert_eq!(r.attachments.org_quota_mib_max, 51200);
    assert_eq!(r.attachments.days_max, None);
    assert_eq!(r.telemetry.log_level, "info");
}

// RFE-019 (D-63): the built-in default brief is neutral; empty = default; over-long fails naming the key.
#[test]
fn default_brief_is_neutral_and_validated() {
    let neutral = RawConfig::default().agents.default_brief;
    for banned in ["Daneel", "Olivaw", "CERN", "robot", "sassy", "positronic"] {
        assert!(!neutral.contains(banned), "default brief mentions {banned}");
    }
    assert!(neutral.chars().count() < 600);
    // unset and empty both resolve to the neutral default
    let cfg = load_with(BASE, &[]).unwrap().api_config().unwrap();
    assert_eq!(cfg.agents.default_brief, neutral);
    let cfg = load_with(&format!("{BASE}[agents]\ndefault_brief = \"\"\n"), &[])
        .unwrap()
        .api_config()
        .unwrap();
    assert_eq!(cfg.agents.default_brief, neutral);
    // a configured brief is used verbatim
    let cfg = load_with(
        &format!("{BASE}[agents]\ndefault_brief = \"Be brief.\"\n"),
        &[],
    )
    .unwrap()
    .api_config()
    .unwrap();
    assert_eq!(cfg.agents.default_brief, "Be brief.");
    // 20 000 chars is accepted, 20 001 is refused naming the key
    let ok = "x".repeat(20_000);
    assert!(
        load_with(&format!("{BASE}[agents]\ndefault_brief = \"{ok}\"\n"), &[])
            .unwrap()
            .api_config()
            .is_ok()
    );
    let long = "x".repeat(20_001);
    let err = load_with(
        &format!("{BASE}[agents]\ndefault_brief = \"{long}\"\n"),
        &[],
    )
    .unwrap()
    .api_config()
    .unwrap_err();
    assert!(err.has_key("agents.default_brief"));
}

#[test]
fn per_service_validation() {
    let raw = load_with("", &[]).unwrap();
    assert!(raw.api_config().unwrap_err().has_key("server.external_url"));
    assert!(
        raw.gateway_config()
            .unwrap_err()
            .has_key("server.external_url")
    );
    assert!(raw.hookhost_config().is_ok());
    assert!(raw.connector_config().is_ok());
    assert!(raw.runner_config().unwrap_err().has_key("runner.backend"));

    let raw = load_with(
        "[runner]\nbackend = \"docker\"\napi_url = \"http://a:1\"\n",
        &[],
    )
    .unwrap();
    assert!(raw.runner_config().unwrap_err().has_key("runner.backend"));
    let raw = load_with(
        "[runner]\nbackend = \"fake\"\napi_url = \"http://a:1\"\n",
        &[],
    )
    .unwrap();
    let r = raw.validate_for(Service::Runner).unwrap();
    assert!(matches!(r, ServiceConfig::Runner(r) if r.held_awake_ceiling == 3));
}

#[test]
fn secret_format_errors() {
    let mut raw = load_with(BASE, &[]).unwrap();
    raw.env_secrets.encryption_active_key = Some(Redacted::new("other".into()));
    raw.env_secrets.internal_secret = Some(Redacted::new("AAAA".into()));
    let e = raw.api_config().unwrap_err();
    assert!(e.has_key("AURORA_ENCRYPTION_ACTIVE_KEY"));
    assert!(e.has_key("AURORA_INTERNAL_SECRET"));
}

#[test]
fn governance_example_parses_and_validates() {
    let toml = format!(
        r#"{BASE}
[[llm.endpoints]]
id = "central"
provider = "litellm"
base_url = "https://llm.example.org"
key_origin = {{ method = "endpoint" }}

[[llm.endpoints]]
id = "anthropic"
provider = "anthropic"
key_origin = {{ method = "provider_account", allowed_organization_ids = ["abc"] }}

[governance.llm]
allowed_key_scopes = ["organization", "workspace"]
minimum_posture = "sanctioned_only"
require_key_origin = true

[governance.egress]
minimum_posture = "credentialed_only"
baseline_allowlist = [ {{ host = "pypi.org" }}, {{ host = "*.githubusercontent.com" }} ]
"#
    );
    let raw = load_with(&toml, &[]).unwrap();
    let api = raw.api_config().unwrap();
    assert_eq!(
        api.governance.llm_minimum_posture,
        LlmPosture::SanctionedOnly
    );
    assert_eq!(
        api.governance.egress_minimum_posture,
        EgressPosture::CredentialedOnly
    );
    assert!(
        !api.governance
            .known_llm_hosts
            .contains(&"api.anthropic.com".to_string())
    );
    assert!(
        api.governance
            .known_llm_hosts
            .contains(&"api.openai.com".to_string())
    );
    raw.gateway_config().unwrap();

    let bad = toml.replace("key_origin = { method = \"endpoint\" }", "");
    let e = load_with(&bad, &[]).unwrap().api_config().unwrap_err();
    assert!(e.has_key("llm.endpoints[0].key_origin"), "{e}");
}

// FIX-019 (D-136): the owner's endpoint shape starts, with a warning naming the endpoint.
#[test]
fn litellm_endpoint_without_default_model_warns_but_starts() {
    let toml = format!(
        "{BASE}[[llm.endpoints]]\nid = \"aigw\"\nprovider = \"litellm\"\nbase_url = \"https://aigw.example.org\"\nkey_origin = {{ method = \"endpoint\" }}\n"
    );
    let raw = load_with(&toml, &[]).unwrap();
    raw.api_config().unwrap();
    let w = llm_endpoint_warnings(&raw.llm);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(
        w[0].contains("`aigw`")
            && w[0].contains("default_model")
            && w[0].contains("no_default_model")
    );

    let with = toml.replace(
        "key_origin",
        "default_model = \"claude-sonnet\"\nkey_origin",
    );
    let raw = load_with(&with, &[]).unwrap();
    assert!(llm_endpoint_warnings(&raw.llm).is_empty());
    // The built-in provider is not live-listing: no warning.
    let raw = load_with(BASE, &[]).unwrap();
    assert!(llm_endpoint_warnings(&raw.llm).is_empty());
}

#[test]
fn unknown_key_origin_method_suggests_nearest() {
    let toml = format!(
        "{BASE}[[llm.endpoints]]\nid = \"aigw\"\nprovider = \"litellm\"\nbase_url = \"https://aigw.example.org\"\nkey_origin = {{ method = \"litellm-team\" }}\n"
    );
    let e = load_with(&toml, &[]).unwrap().api_config().unwrap_err();
    assert!(e.has_key("llm.endpoints[0].key_origin.method"), "{e}");
    assert!(
        e.to_string().contains("did you mean `litellm_team`?"),
        "{e}"
    );
    assert_eq!(
        nearest_origin_method("Provider-Account"),
        Some("provider_account")
    );
    assert_eq!(nearest_origin_method("zzzzzzzzzzzz"), None);
}

#[test]
fn known_hosts_extra_and_endpoint_removal() {
    let raw = load_with(
        "[governance.llm]\nknown_llm_hosts_extra = [\"llm.corp\"]\n[[llm.endpoints]]\nid = \"o\"\nprovider = \"openai\"\nbase_url = \"https://api.openai.com/v1\"\n",
        &[],
    )
    .unwrap();
    let h = raw.known_llm_hosts();
    assert!(h.contains(&"llm.corp".to_string()));
    assert!(!h.contains(&"api.openai.com".to_string()));
    assert!(h.contains(&"api.anthropic.com".to_string()));
}

#[test]
fn secrets_never_in_debug() {
    let mut raw = load_with(
        &format!("{BASE}[email]\nsmtp_url = \"smtp://u:smtppass@mail:25\"\nfrom = \"a@b.c\"\n[[secrets.stores]]\nid = \"v\"\nkind = \"vault\"\ntoken = \"vaulttoken\"\n"),
        &[],
    )
    .unwrap();
    raw.env_secrets.runner_token = Some(Redacted::new("aur_topsecret".into()));
    let api = raw.api_config().unwrap();
    let dbg = format!("{raw:?} {api:?}");
    for s in ["smtppass", "vaulttoken", "aur_topsecret", "pw@db", KEY] {
        assert!(!dbg.contains(s), "leaked {s}");
    }
    assert!(dbg.contains("[redacted]"));
}

#[test]
fn oidc_provider_defaults_alias_and_domain_validation() {
    let p = "[[auth.oidc_providers]]\nid = \"corp\"\nissuer_url = \"https://idp.example.org\"\nclient_id = \"c\"\nclient_secret_env = \"CORP_SECRET\"\n";
    let raw = load_with(
        &format!("{BASE}{p}allowed_email_domains = [\"example.org\"]\n"),
        &[("CORP_SECRET", "s3cret")],
    )
    .unwrap();
    let a = raw.api_config().unwrap();
    let o = a
        .auth
        .oidc_providers
        .iter()
        .find(|p| p.id == "corp")
        .unwrap();
    assert_eq!(o.issuer, "https://idp.example.org");
    assert_eq!(o.require_email_verified, None);
    assert_eq!(o.scopes, ["openid", "email", "profile"]);
    assert_eq!(a.oidc_client_secrets["corp"].expose(), "s3cret");

    let bad = load_with(
        &format!("{BASE}{p}allowed_email_domains = [\"@\"]\n"),
        &[("CORP_SECRET", "s3cret")],
    )
    .unwrap();
    let err = bad.api_config().unwrap_err().to_string();
    assert!(err.contains("allowed_email_domains"), "{err}");
}

/// D-118: `trusted_email_domains` takes exact domains only.
#[test]
fn oidc_trusted_email_domains_validation() {
    let p = "[[auth.oidc_providers]]\nid = \"corp\"\nissuer = \"https://idp.example.org\"\nclient_id = \"c\"\nclient_secret_env = \"CORP_SECRET\"\n";
    let ok = load_with(
        &format!("{BASE}{p}trusted_email_domains = [\"example.org\"]\n"),
        &[("CORP_SECRET", "s3cret")],
    )
    .unwrap();
    let a = ok.api_config().unwrap();
    let o = a
        .auth
        .oidc_providers
        .iter()
        .find(|p| p.id == "corp")
        .unwrap();
    assert_eq!(o.trusted_email_domains, ["example.org"]);

    for bad in [
        "",
        "@example.org",
        "*.example.org",
        "example",
        ".example.org",
        "a..org",
        "ex ample.org",
    ] {
        let raw = load_with(
            &format!("{BASE}{p}trusted_email_domains = [\"{bad}\"]\n"),
            &[("CORP_SECRET", "s3cret")],
        )
        .unwrap();
        let err = raw.api_config().unwrap_err().to_string();
        assert!(err.contains("trusted_email_domains"), "{bad}: {err}");
    }
}

/// P4-IDP-1 (D-139, SEC-78): role mappings are validated at startup, each error names its key.
#[test]
fn oidc_role_mappings_validation() {
    let p = "[[auth.oidc_providers]]\nid = \"corp\"\nissuer = \"https://idp.example.org\"\nclient_id = \"c\"\nclient_secret_env = \"CORP_SECRET\"\n";
    let ok = load_with(
        &format!(
            "{BASE}{p}role_claim = \"resource_access.aurora.roles\"\nrequire_role = true\n\
             [[auth.oidc_providers.role_mappings]]\nvalue = \"aurora-admins\"\ninstance_admin = true\n\
             [[auth.oidc_providers.role_mappings]]\nvalue = \"it\"\norg = {{ slug = \"it\", role = \"owner\" }}\nworkspaces = [{{ slug = \"infra\", role = \"approver\" }}]\n"
        ),
        &[("CORP_SECRET", "s3cret")],
    )
    .unwrap();
    let a = ok.api_config().unwrap();
    let o = a
        .auth
        .oidc_providers
        .iter()
        .find(|p| p.id == "corp")
        .unwrap();
    assert_eq!(o.role_mappings.len(), 2);
    assert!(o.require_role && !o.role_claim_from_userinfo());

    let m = |extra: &str| format!("{BASE}{p}role_claim = \"roles\"\n{extra}");
    let bad = [
        (m("require_role = true\n"), "require_role"),
        (
            format!(
                "{BASE}{p}[[auth.oidc_providers.role_mappings]]\nvalue = \"a\"\ninstance_admin = true\n"
            ),
            "role_claim",
        ),
        (m("role_claim_source = \"cookie\"\n"), "role_claim_source"),
        (
            m(
                "[[auth.oidc_providers.role_mappings]]\nvalue = \"a\"\ninstance_admin = true\n[[auth.oidc_providers.role_mappings]]\nvalue = \"a\"\ninstance_admin = true\n",
            ),
            "role_mappings[1].value",
        ),
        (
            m(
                "[[auth.oidc_providers.role_mappings]]\nvalue = \"a\"\norg = { slug = \"it\", role = \"root\" }\n",
            ),
            "role_mappings[0].org.role",
        ),
        (
            m(
                "[[auth.oidc_providers.role_mappings]]\nvalue = \"a\"\norg = { slug = \"IT\", role = \"member\" }\n",
            ),
            "role_mappings[0].org.slug",
        ),
        (
            m(
                "[[auth.oidc_providers.role_mappings]]\nvalue = \"a\"\nworkspaces = [{ slug = \"ws\", role = \"member\" }]\n",
            ),
            "role_mappings[0].org",
        ),
        (
            m(
                "[[auth.oidc_providers.role_mappings]]\nvalue = \"a\"\norg = { slug = \"it\", role = \"member\" }\nworkspaces = [{ slug = \"ws\", role = \"admin\" }]\n",
            ),
            "workspaces[0].role",
        ),
        (
            m("[[auth.oidc_providers.role_mappings]]\nvalue = \"a\"\nmystery = 1\n"),
            "mystery",
        ),
    ];
    for (toml, key) in bad {
        let err = match load_with(&toml, &[("CORP_SECRET", "s3cret")]) {
            Err(e) => e.to_string(),
            Ok(raw) => raw.api_config().unwrap_err().to_string(),
        };
        assert!(err.contains(key), "{key}: {err}");
    }
}

#[test]
fn internal_and_ops_bind_resolve_and_validate() {
    // Defaults fall back to server.bind.
    let raw = load_with(BASE, &[]).unwrap();
    assert_eq!(raw.gateway_config().unwrap().internal_bind, "0.0.0.0");
    let toml = format!(
        "{BASE}bind = \"127.0.0.2\"\n[gateway]\ninternal_bind = \"127.0.0.3\"\n[runner]\nbackend = \"fake\"\napi_url = \"http://a:1\"\nops_bind = \"127.0.0.3\"\n"
    );
    let raw = load_with(&toml, &[]).unwrap();
    assert_eq!(raw.gateway_config().unwrap().internal_bind, "127.0.0.3");
    assert_eq!(raw.runner_config().unwrap().ops_bind, "127.0.0.3");

    let bad = format!(
        "{BASE}[gateway]\ninternal_bind = \"not-an-ip\"\n[runner]\nbackend = \"fake\"\napi_url = \"http://a:1\"\nops_bind = \"nope\"\n"
    );
    let raw = load_with(&bad, &[]).unwrap();
    assert!(
        raw.gateway_config()
            .unwrap_err()
            .has_key("gateway.internal_bind")
    );
    assert!(raw.runner_config().unwrap_err().has_key("runner.ops_bind"));
}

#[test]
fn overlap_warning_logic() {
    assert!(bind_overlap_warning("0.0.0.0", "0.0.0.0").is_some());
    assert!(bind_overlap_warning("10.0.0.1", "10.0.0.1").is_some());
    assert!(bind_overlap_warning("10.0.0.1", "0.0.0.0").is_some());
    assert!(bind_overlap_warning("10.89.250.2", "10.89.249.2").is_none());
    assert!(bind_overlap_warning("127.0.0.2", "127.0.0.3").is_none());
    assert!(bind_overlap_warning("0.0.0.0", "127.0.0.1").is_none());
}

#[test]
fn k8s_core_namespace_is_accepted() {
    let raw = load_with(
        &format!(
            "{BASE}[runner]\nbackend = \"kubernetes\"\napi_url = \"http://api:4101\"\n\
             [runner.k8s]\nnamespace = \"sbx\"\ncore_namespace = \"core\"\n"
        ),
        &[],
    )
    .unwrap();
    assert_eq!(raw.runner.k8s.core_namespace.as_deref(), Some("core"));
    let r = raw.runner_config().unwrap();
    assert_eq!(r.backend, RunnerBackend::Kubernetes);
    assert_eq!(r.runner.k8s.core_namespace.as_deref(), Some("core"));
    // Unset by default (the backend resolves the pod's own namespace).
    let raw = load_with(BASE, &[]).unwrap();
    assert!(raw.runner.k8s.core_namespace.is_none());
}

#[test]
fn k8s_image_pull_secrets_are_validated() {
    let with = |list: &str| {
        load_with(
            &format!(
                "{BASE}[runner]\nbackend = \"kubernetes\"\napi_url = \"http://api:4101\"\n\
                 [runner.k8s]\nimage_pull_secrets = {list}\n"
            ),
            &[],
        )
        .unwrap()
    };
    let r = with("[\"regcred\", \"my.registry-cred\"]")
        .runner_config()
        .unwrap();
    assert_eq!(
        r.runner.k8s.image_pull_secrets,
        vec!["regcred".to_owned(), "my.registry-cred".to_owned()]
    );
    // Empty by default.
    let r = load_with(BASE, &[]).unwrap();
    assert!(r.runner.k8s.image_pull_secrets.is_empty());

    for bad in ["Reg_Cred", "-x", "x-", "a..b", ""] {
        let err = with(&format!("[\"ok\", \"{bad}\"]"))
            .runner_config()
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("runner.k8s.image_pull_secrets[1]"),
            "{bad}: {err}"
        );
    }
}

/// The ConfigMap TOML as rendered by the chart (`helm template t deploy/helm/aurora`).
const HELM_RENDERED_TOML: &str = r#"
[database]
  max_connections = 20

[gateway]
  ca_dir = "/var/lib/aurora/ca"
  internal_bind = "0.0.0.0"
  [gateway.proxy_tls]
    names = ["aurora-gateway", "t-aurora-gateway-proxy", "t-aurora-gateway-proxy.default.svc"]

[hosted]
  enabled = true

[runner]
  api_url = "http://t-aurora-api:4101"
  backend = "kubernetes"
  ops_bind = "0.0.0.0"
  [runner.k8s]
    core_namespace = "default"
    gateway_service = "t-aurora-gateway-proxy"
    namespace = "aurora-sandboxes"
    runner_service = "t-aurora-runner"

[server]
  bind = "0.0.0.0"
  external_url = "https://aurora.example.org"
  trusted_proxies = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"]

[telemetry]
  log_level = "info"
"#;

fn assert_helm_toml_loads(toml: &str) {
    let raw = load_with(toml, &[]).unwrap();
    let r = raw.runner_config().unwrap();
    assert_eq!(r.backend, RunnerBackend::Kubernetes);
    let k = &r.runner.k8s;
    assert_eq!(k.namespace.as_deref(), Some("aurora-sandboxes"));
    assert!(k.core_namespace.is_some());
    assert!(k.gateway_service.is_some() && k.runner_service.is_some());
}

#[test]
fn helm_rendered_runner_config_loads() {
    assert_helm_toml_loads(HELM_RENDERED_TOML);
}

/// Renders the chart with `helm template t <chart> --set-json <each arg>` and returns the
/// ConfigMap `aurora.toml`. `None` when `helm` is not installed.
fn helm_rendered_toml(set_json: &[&str]) -> Option<String> {
    let chart = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helm/aurora");
    let mut cmd = std::process::Command::new("helm");
    cmd.args(["template", "t"]).arg(&chart);
    for s in set_json {
        cmd.args(["--set-json", s]);
    }
    let Ok(out) = cmd.output() else {
        eprintln!("helm not installed: skipping live chart render");
        return None;
    };
    assert!(
        out.status.success(),
        "helm template failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let mut toml = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim_end() == "  aurora.toml: |" {
            inside = true;
        } else if inside {
            if let Some(l) = line.strip_prefix("    ") {
                toml.push_str(l);
                toml.push('\n');
            } else if line.trim().is_empty() {
                toml.push('\n');
            } else {
                break;
            }
        }
    }
    assert!(toml.contains("[runner.k8s]"), "ConfigMap TOML not found");
    Some(toml)
}

/// Renders the real chart when `helm` is installed and loads the ConfigMap TOML.
#[test]
fn live_helm_template_config_loads() {
    let Some(toml) = helm_rendered_toml(&[]) else {
        return;
    };
    assert_helm_toml_loads(&toml);
}

/// OPS-013: `runner.k8s.nodeSelector` / `tolerations` and the plugin directory render into
/// the ConfigMap TOML and load.
#[test]
fn helm_rendered_scheduling_and_plugins_load() {
    let Some(toml) = helm_rendered_toml(&[
        r#"runner.k8s.nodeSelector={"kubernetes.io/os":"linux","aurora.example.org/sandbox":"true"}"#,
        r#"runner.k8s.tolerations=[{"key":"sandbox","operator":"Exists","effect":"NoSchedule"}]"#,
        r#"plugins.source="pvc""#,
        r#"plugins.pvcName="plugins-data""#,
    ]) else {
        return;
    };
    assert_helm_toml_loads(&toml);
    let raw = load_with(&toml, &[]).unwrap();
    assert_eq!(raw.plugins.dir, "/var/lib/aurora/plugins");
    let r = raw.runner_config().unwrap();
    assert_eq!(
        r.runner
            .k8s
            .node_selector
            .get("kubernetes.io/os")
            .map(String::as_str),
        Some("linux")
    );
    assert_eq!(
        r.runner
            .k8s
            .node_selector
            .get("aurora.example.org/sandbox")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(r.runner.k8s.tolerations.len(), 1);
    assert_eq!(r.runner.k8s.tolerations[0]["key"].as_str(), Some("sandbox"));
    assert_eq!(
        r.runner.k8s.tolerations[0]["effect"].as_str(),
        Some("NoSchedule")
    );
}

/// OPS-013: `deploy/dev/aurora.dev.toml` (used by `just dev`) loads for the API, gateway and
/// hook host run as host processes, with their localhost URLs.
#[test]
fn dev_config_loads_for_host_processes() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/dev/aurora.dev.toml");
    let raw = load(Some(&path), secrets().into_iter()).unwrap();
    raw.api_config().unwrap();
    raw.gateway_config().unwrap();
    raw.hookhost_config().unwrap();
    assert_eq!(raw.gateway.ca_dir, "deploy/dev/ca");
    assert_eq!(raw.api.hookhost_url, "http://127.0.0.1:4104");
    assert_eq!(raw.gateway.hookhost_url, "http://127.0.0.1:4104");
    assert_eq!(raw.hookhost.api_internal_url, "http://127.0.0.1:4111");
}

#[test]
fn bootstrap_connector_host_token_must_be_an_auh_token() {
    let ok = load_with(
        BASE,
        &[("AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN", "auh_abc123")],
    )
    .unwrap()
    .api_config()
    .unwrap();
    assert!(ok.bootstrap_connector_host_token.is_some());
    let e = load_with(
        BASE,
        &[("AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN", "aun_abc123")],
    )
    .unwrap()
    .api_config()
    .unwrap_err();
    assert!(e.has_key("AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN"), "{e}");
    let none = load_with(BASE, &[]).unwrap().api_config().unwrap();
    assert!(none.bootstrap_connector_host_token.is_none());
}

#[test]
fn connector_host_config_defaults_token_and_overrides() {
    let c = load_with(BASE, &[("AURORA_CONNECTOR_HOST_TOKEN", "auh_abc123")])
        .unwrap()
        .connector_host_config()
        .unwrap();
    assert_eq!(c.api_url, "http://127.0.0.1:4101");
    assert_eq!(c.listen, "0.0.0.0:4110");
    assert_eq!(c.ops_port, 4192);
    assert!(!c.test_types);
    assert_eq!(c.token.expose(), "auh_abc123");
    let c = load_with(
        &format!(
            "{BASE}\n[connector_host]\napi_url = \"http://aurora-api:4101/\"\ntest_types = true\nops_port = 4200\n"
        ),
        &[("AURORA_CONNECTOR_HOST_TOKEN", "auh_abc123")],
    )
    .unwrap()
    .connector_host_config()
    .unwrap();
    assert_eq!(c.api_url, "http://aurora-api:4101");
    assert_eq!(c.ops_port, 4200);
    assert!(c.test_types);
    // The token is required and must be an `auh_` token (an `aun_` connector token is refused).
    let e = load_with(BASE, &[])
        .unwrap()
        .connector_host_config()
        .unwrap_err();
    assert!(e.has_key("AURORA_CONNECTOR_HOST_TOKEN"), "{e}");
    let e = load_with(BASE, &[("AURORA_CONNECTOR_HOST_TOKEN", "aun_abc")])
        .unwrap()
        .connector_host_config()
        .unwrap_err();
    assert!(e.has_key("AURORA_CONNECTOR_HOST_TOKEN"), "{e}");
    assert!(!format!("{c:?}").contains("auh_abc123"));
}

#[test]
fn bootstrap_connector_tokens_are_validated_and_resolved() {
    let ok = load_with(
        BASE,
        &[(
            "AURORA_BOOTSTRAP_CONNECTOR_TOKENS",
            "fake:aun_abc123, slack-2:aun_def456",
        )],
    )
    .unwrap()
    .api_config()
    .unwrap();
    let ids: Vec<&str> = ok
        .bootstrap_connector_tokens
        .iter()
        .map(|(i, _)| i.as_str())
        .collect();
    assert_eq!(ids, ["fake", "slack-2"]);
    // Ids follow the stored `connectors.id` shape; bad entries fail startup validation.
    for bad in [
        "Fake:aun_x1",
        "a_b:aun_x1",
        "x:aun_x1",
        "fake:notatoken",
        "fake",
    ] {
        let e = load_with(BASE, &[("AURORA_BOOTSTRAP_CONNECTOR_TOKENS", bad)])
            .unwrap()
            .api_config()
            .unwrap_err();
        assert!(e.has_key("AURORA_BOOTSTRAP_CONNECTOR_TOKENS"), "{bad}: {e}");
    }
}

/// SEC-13: the hook host is configured with its internal secret only: no database
/// URL, keyring, signing key or session keys are needed, and none are carried.
#[test]
fn sec13_hookhost_holds_no_keys_or_database_credentials() {
    let only_internal = [("AURORA_INTERNAL_SECRET".to_string(), KEY.to_string())];
    let raw = load(None, only_internal.into_iter()).unwrap();
    let c = raw
        .hookhost_config()
        .expect("hookhost needs only the internal secret");
    let dbg = format!("{c:?}");
    for needle in [
        "database",
        "encryption",
        "signing",
        "runner_session",
        "postgres://",
    ] {
        assert!(!dbg.to_lowercase().contains(needle), "{needle} in {dbg}");
    }
    // Even with every secret present in the environment, none leaks into its config.
    let raw = load_with("", &[]).unwrap();
    let dbg = format!("{:?}", raw.hookhost_config().unwrap());
    for needle in ["pw@db", KEY, "aur_abc", "aun_abc"] {
        assert!(!dbg.contains(needle), "{needle} in {dbg}");
    }
}

// ---- RFE-007: signing-key / internal-secret rings ----

const KEY2: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="; // 32 x 0x01

#[test]
fn bare_values_are_ring_v0_for_every_service() {
    let raw = load_with(BASE, &[]).unwrap();
    let api = raw.api_config().unwrap();
    assert_eq!(api.signing_ring.active_id(), "v0");
    assert_eq!(api.internal_ring.ids().collect::<Vec<_>>(), ["v0"]);
    assert_eq!(
        raw.gateway_config().unwrap().internal_ring.active_id(),
        "v0"
    );
    assert_eq!(
        raw.hookhost_config().unwrap().internal_ring.active_id(),
        "v0"
    );
    let (s, i) = raw.rings().unwrap();
    assert_eq!((s.active_id(), i.active_id()), ("v0", "v0"));
}

#[test]
fn lists_with_active_key_validate_per_service() {
    let sk = format!("a1:{KEY},b2:{KEY2}");
    let raw = load_with(
        BASE,
        &[
            ("AURORA_SIGNING_KEY", sk.as_str()),
            ("AURORA_SIGNING_ACTIVE_KEY", "b2"),
            ("AURORA_INTERNAL_SECRET", sk.as_str()),
            ("AURORA_INTERNAL_ACTIVE_KEY", "a1"),
        ],
    )
    .unwrap();
    let api = raw.api_config().unwrap();
    assert_eq!(api.signing_ring.active_id(), "b2");
    assert_eq!(api.internal_ring.active_id(), "a1");
    assert_eq!(api.internal_ring.ids().collect::<Vec<_>>(), ["a1", "b2"]);
    assert_eq!(
        raw.gateway_config().unwrap().internal_ring.active_id(),
        "a1"
    );
    assert_eq!(
        raw.hookhost_config().unwrap().internal_ring.active_id(),
        "a1"
    );
    // Values never appear in Debug output.
    assert!(!format!("{api:?}").contains(KEY2));
}

#[test]
fn list_without_active_key_fails_naming_the_variable() {
    let sk = format!("a1:{KEY},b2:{KEY2}");
    let raw = load_with(BASE, &[("AURORA_SIGNING_KEY", sk.as_str())]).unwrap();
    let e = raw.api_config().unwrap_err();
    assert!(e.has_key("AURORA_SIGNING_ACTIVE_KEY"), "{e}");
    let raw = load_with(BASE, &[("AURORA_INTERNAL_SECRET", sk.as_str())]).unwrap();
    for e in [
        raw.api_config().unwrap_err(),
        raw.gateway_config().unwrap_err(),
        raw.hookhost_config().unwrap_err(),
    ] {
        assert!(e.has_key("AURORA_INTERNAL_ACTIVE_KEY"), "{e}");
    }
}

#[test]
fn ring_limits_and_unknown_active_id_fail_startup() {
    let five = format!("a:{KEY},b:{KEY},c:{KEY},d:{KEY},e:{KEY}");
    let raw = load_with(
        BASE,
        &[
            ("AURORA_SIGNING_KEY", five.as_str()),
            ("AURORA_SIGNING_ACTIVE_KEY", "a"),
        ],
    )
    .unwrap();
    assert!(raw.api_config().unwrap_err().has_key("AURORA_SIGNING_KEY"));
    let two = format!("a:{KEY},b:{KEY2}");
    let raw = load_with(
        BASE,
        &[
            ("AURORA_INTERNAL_SECRET", two.as_str()),
            ("AURORA_INTERNAL_ACTIVE_KEY", "zz"),
        ],
    )
    .unwrap();
    assert!(
        raw.hookhost_config()
            .unwrap_err()
            .has_key("AURORA_INTERNAL_ACTIVE_KEY")
    );
    // A signing key must be exactly 32 bytes.
    let long = format!(
        "a:{}",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    );
    let raw = load_with(
        BASE,
        &[
            ("AURORA_SIGNING_KEY", long.as_str()),
            ("AURORA_SIGNING_ACTIVE_KEY", "a"),
        ],
    )
    .unwrap();
    assert!(raw.api_config().unwrap_err().has_key("AURORA_SIGNING_KEY"));
}

#[test]
fn api_ratelimit_defaults_overrides_and_validation() {
    // RFE-017: defaults (enabled, x1.0, org 3000/min), TOML class overrides, env multiplier.
    let api = load_with(BASE, &[]).unwrap().api_config().unwrap();
    let r = &api.ratelimit.api;
    assert!(r.enabled && r.classes.is_empty());
    assert_eq!((r.multiplier, r.org_per_minute), (1.0, 3000));
    let raw = load_with(
        &format!(
            "{BASE}[ratelimit.api]\norg_per_minute = 100\n[ratelimit.api.classes.export]\nper_hour = 3\nburst = 1\n"
        ),
        &[("AURORA__RATELIMIT__API__MULTIPLIER", "2.5")],
    )
    .unwrap();
    let r = raw.api_config().unwrap().ratelimit.api;
    assert_eq!((r.multiplier, r.org_per_minute), (2.5, 100));
    assert_eq!(r.classes["export"].per_hour, Some(3));
    // out-of-range multiplier, unknown class, both periods, zero values: all reported by key
    for (toml, key) in [
        (
            "[ratelimit.api]\nmultiplier = 0.05\n",
            "ratelimit.api.multiplier",
        ),
        (
            "[ratelimit.api]\nmultiplier = 101\n",
            "ratelimit.api.multiplier",
        ),
        (
            "[ratelimit.api]\norg_per_minute = 0\n",
            "ratelimit.api.org_per_minute",
        ),
        (
            "[ratelimit.api.classes.bogus]\nburst = 1\n",
            "ratelimit.api.classes.bogus",
        ),
        (
            "[ratelimit.api.classes.write]\nper_minute = 1\nper_hour = 1\n",
            "ratelimit.api.classes.write",
        ),
        (
            "[ratelimit.api.classes.write]\nburst = 0\n",
            "ratelimit.api.classes.write.burst",
        ),
        ("[ratelimit.api]\nnope = 1\n", "ratelimit.api.nope"),
    ] {
        let e = load_with(&format!("{BASE}{toml}"), &[])
            .and_then(|r| r.api_config().map(|_| ()))
            .unwrap_err();
        assert!(e.has_key(key), "{key}: {e}");
    }
}

// (phase 4, P4-CNT, D-119, SEC-62) Startup refusals of `ratelimit.counters.*`, each naming the key;
// the default is the `memory` backend with every class `local`.
#[test]
fn counters_defaults_and_startup_refusals() {
    let api = load_with(BASE, &[]).unwrap().api_config().unwrap();
    let c = &api.counters;
    assert_eq!(c.backend, CounterBackend::Memory);
    assert!(
        !c.api_shared && !c.gateway_shared,
        "memory: nothing is shared"
    );
    assert_eq!(c.security, aurora_counters::ClassPolicy::Local);
    assert_eq!(c.timeout, std::time::Duration::from_millis(50));
    let gw = load_with(BASE, &[]).unwrap().gateway_config().unwrap();
    assert_eq!(gw.counters.backend, CounterBackend::Memory);

    let ok = load_with(
        &format!("{BASE}[ratelimit.counters]\nbackend = \"postgres\"\n[ratelimit.counters.on_unavailable]\nsecurity = \"closed\"\n"),
        &[],
    )
    .unwrap()
    .api_config()
    .unwrap();
    assert!(ok.counters.api_shared && ok.counters.gateway_shared);
    assert_eq!(ok.counters.security, aurora_counters::ClassPolicy::Closed);

    for (toml, env, key) in [
        (
            "[ratelimit.counters.on_unavailable]\nsecurity = \"open\"\n",
            vec![],
            "ratelimit.counters.on_unavailable.security",
        ),
        (
            "[ratelimit.counters.on_unavailable]\nmanagement = \"OPEN\"\n",
            vec![],
            "ratelimit.counters.on_unavailable.management",
        ),
        (
            "[ratelimit.counters.on_unavailable]\ngateway = \"whatever\"\n",
            vec![],
            "ratelimit.counters.on_unavailable.gateway",
        ),
        (
            "[ratelimit.counters]\nbackend = \"memcached\"\n",
            vec![],
            "ratelimit.counters.backend",
        ),
        (
            "[ratelimit.counters]\nbackend = \"redis\"\n",
            vec![],
            "ratelimit.counters.redis.address",
        ),
        (
            "[ratelimit.counters]\nbackend = \"redis\"\n[ratelimit.counters.redis]\naddress = \"rediss://user:secret@redis.example.org\"\n",
            vec![],
            "ratelimit.counters.redis.address",
        ),
        (
            "[ratelimit.counters]\nbackend = \"redis\"\n[ratelimit.counters.redis]\naddress = \"redis://redis.example.org:6379\"\n",
            vec![],
            "ratelimit.counters.redis.address",
        ),
        (
            "[ratelimit.counters]\nbackend = \"redis\"\n[ratelimit.counters.redis]\naddress = \"rediss://r.example.org\"\npassword_env = \"NO_SUCH_REDIS_PW\"\n",
            vec![],
            "ratelimit.counters.redis.password_env",
        ),
        (
            "[ratelimit.counters]\ntimeout_ms = 5\n",
            vec![],
            "ratelimit.counters.timeout_ms",
        ),
        (
            "[ratelimit.counters.postgres]\npool_size = 0\n",
            vec![],
            "ratelimit.counters.postgres.pool_size",
        ),
        (
            "[ratelimit.counters.redis]\nkey_prefix = \"a b\"\n",
            vec![],
            "ratelimit.counters.redis.key_prefix",
        ),
        (
            "[ratelimit.counters]\nnope = 1\n",
            vec![],
            "ratelimit.counters.nope",
        ),
    ] {
        let _: Vec<(&str, &str)> = env;
        for svc in ["api", "gateway"] {
            let r = load_with(&format!("{BASE}{toml}"), &[]).and_then(|r| {
                if svc == "api" {
                    r.api_config().map(|_| ())
                } else {
                    r.gateway_config().map(|_| ())
                }
            });
            let e = r.unwrap_err();
            assert!(e.has_key(key), "{svc} {key}: {e}");
        }
    }
    // A plaintext remote Redis is accepted only with the explicit opt-in; credentials come from the
    // named variables and never appear in Debug output.
    let raw = load_with(
        &format!("{BASE}[ratelimit.counters]\nbackend = \"redis\"\n[ratelimit.counters.redis]\naddress = \"redis://redis.svc:6379\"\nallow_plaintext = true\npassword_env = \"CNT_REDIS_PW\"\n"),
        &[("CNT_REDIS_PW", "s3cret-value")],
    )
    .unwrap();
    let r = raw.api_config().unwrap();
    let rc = r.counters.redis.as_ref().unwrap();
    assert!(rc.plaintext_remote);
    assert_eq!(rc.password.as_ref().unwrap().expose(), "s3cret-value");
    assert!(!format!("{r:?}").contains("s3cret-value"));
}

// (phase 1, RFE-002) The proxy listener is TLS-only: no http:// advertised URL, cert/key in pairs.
#[test]
fn advertised_proxy_url_must_be_https() {
    let raw = load_with(
        &format!("{BASE}[gateway]\nadvertised_proxy_url = \"http://aurora-gateway:4102\"\n"),
        &[],
    )
    .unwrap();
    for e in [
        raw.api_config().unwrap_err().to_string(),
        raw.gateway_config().unwrap_err().to_string(),
    ] {
        assert!(e.contains("gateway.advertised_proxy_url"), "{e}");
        assert!(e.contains("https://"), "{e}");
    }
    let raw = load_with(
        &format!("{BASE}[gateway]\nadvertised_proxy_url = \"https://aurora-gateway:4102\"\n"),
        &[],
    )
    .unwrap();
    assert_eq!(
        raw.gateway_config().unwrap().advertised_proxy_url,
        "https://aurora-gateway:4102"
    );
}

// FIX-010 (C-301, D-87): the sandbox proxy address defaults by runner backend, an explicit value
// wins, and the advertised URL (BYO, container config) is never affected.
#[test]
fn sandbox_proxy_url_defaults_per_backend_and_override() {
    let run = |extra: &str| {
        load_with(&format!("{BASE}{extra}"), &[])
            .unwrap()
            .api_config()
            .unwrap()
    };
    // kubernetes: the in-cluster host alias; advertised stays external.
    let k = run("[runner]\nbackend = \"kubernetes\"\n");
    assert_eq!(k.sandbox_proxy_url, "https://aurora-gateway:4102");
    assert_eq!(k.advertised_proxy_url, "https://aurora.example.org:4102");
    // podman / fake / unset: unchanged behaviour = the advertised URL.
    for b in [
        "[runner]\nbackend = \"podman\"\n",
        "[runner]\nbackend = \"fake\"\n",
        "",
    ] {
        let c = run(b);
        assert_eq!(
            c.sandbox_proxy_url, "https://aurora.example.org:4102",
            "{b}"
        );
    }
    // Explicit override wins on every backend; advertised untouched.
    let o = run(
        "[runner]\nbackend = \"kubernetes\"\n[gateway]\nsandbox_proxy_url = \"https://gw.internal:9443\"\n",
    );
    assert_eq!(o.sandbox_proxy_url, "https://gw.internal:9443");
    assert_eq!(o.advertised_proxy_url, "https://aurora.example.org:4102");
    // The gateway sees the same resolved values (its certificate SANs include the host).
    let g = load_with(&format!("{BASE}[runner]\nbackend = \"kubernetes\"\n"), &[])
        .unwrap()
        .gateway_config()
        .unwrap();
    assert_eq!(g.sandbox_proxy_url, "https://aurora-gateway:4102");
}

#[test]
fn sandbox_proxy_url_must_be_https_host_and_port_only() {
    for bad in [
        "http://aurora-gateway:4102",
        "https://aurora-gateway:4102/path",
        "https://aurora-gateway:4102?x=1",
        "https://u:p@aurora-gateway:4102",
        "aurora-gateway:4102",
        "https://aurora-gateway:notaport",
    ] {
        let raw = load_with(
            &format!("{BASE}[gateway]\nsandbox_proxy_url = \"{bad}\"\n"),
            &[],
        )
        .unwrap();
        for e in [
            raw.api_config().unwrap_err().to_string(),
            raw.gateway_config().unwrap_err().to_string(),
        ] {
            assert!(e.contains("gateway.sandbox_proxy_url"), "{bad}: {e}");
        }
    }
    let ok = load_with(
        &format!("{BASE}[gateway]\nsandbox_proxy_url = \"https://aurora-gateway:4102/\"\n"),
        &[],
    )
    .unwrap();
    assert!(ok.api_config().is_ok());
}

#[test]
fn proxy_tls_cert_and_key_come_together() {
    let only_cert = load_with(
        &format!("{BASE}[gateway.proxy_tls]\ncert_file = \"/c.pem\"\n"),
        &[],
    )
    .unwrap();
    let e = only_cert.gateway_config().unwrap_err().to_string();
    assert!(e.contains("gateway.proxy_tls.key_file"), "{e}");
    let only_key = load_with(
        &format!("{BASE}[gateway.proxy_tls]\nkey_file = \"/k.pem\"\n"),
        &[],
    )
    .unwrap();
    let e = only_key.gateway_config().unwrap_err().to_string();
    assert!(e.contains("gateway.proxy_tls.cert_file"), "{e}");
    let ok = load_with(
        &format!(
            "{BASE}[gateway.proxy_tls]\ncert_file = \"/c.pem\"\nkey_file = \"/k.pem\"\nnames = [\"gw.example.org\", \"10.0.0.5\"]\n"
        ),
        &[],
    )
    .unwrap();
    let g = ok.gateway_config().unwrap().gateway;
    assert_eq!(g.proxy_tls.names, ["gw.example.org", "10.0.0.5"]);
    let bad = load_with(
        &format!("{BASE}[gateway.proxy_tls]\nnames = [\"bad name!\"]\n"),
        &[],
    )
    .unwrap();
    assert!(
        bad.gateway_config()
            .unwrap_err()
            .to_string()
            .contains("gateway.proxy_tls.names")
    );
}

#[test]
fn agent_token_max_age_is_optional_bounded_and_named_in_the_error() {
    let toml = |v: &str| format!("{BASE}[governance.agent_tokens]\nmax_age_days = {v}\n");
    // Default: no limit.
    let api = load_with(BASE, &[]).unwrap().api_config().unwrap();
    assert_eq!(api.governance.agent_token_max_age_days, None);
    for (v, want) in [("1", 1), ("90", 90), ("3650", 3650)] {
        let api = load_with(&toml(v), &[]).unwrap().api_config().unwrap();
        assert_eq!(api.governance.agent_token_max_age_days, Some(want));
    }
    for bad in ["0", "-1", "3651"] {
        let e = load_with(&toml(bad), &[])
            .unwrap()
            .api_config()
            .unwrap_err();
        assert!(e.has_key("governance.agent_tokens.max_age_days"), "{bad}");
    }
    // Not an integer: the load itself refuses, naming the key.
    let e = load_with(&toml("\"ten\""), &[])
        .err()
        .map(|e| e.to_string());
    assert!(
        e.is_some_and(|m| m.contains("max_age_days")),
        "a non-integer is refused at load"
    );
}

#[test]
fn open_lane_minimum_is_validated_and_defaults_to_enabled() {
    let toml = |v: &str| format!("{BASE}[governance.egress]\nopen_lane_minimum = \"{v}\"\n");
    let api = load_with(&toml("allowlist_only"), &[])
        .unwrap()
        .api_config()
        .unwrap();
    assert_eq!(
        api.governance.open_lane_minimum,
        OpenLanePosture::AllowlistOnly
    );
    let api = load_with(&toml("disabled"), &[])
        .unwrap()
        .api_config()
        .unwrap();
    assert_eq!(api.governance.open_lane_minimum, OpenLanePosture::Disabled);
    let e = load_with(&toml("sometimes"), &[])
        .unwrap()
        .api_config()
        .unwrap_err();
    assert!(e.has_key("governance.egress.open_lane_minimum"));
}

const DEV_SUP: &str = "dev_supervisor_binary = \"/home/dev/aurora/target/debug/aurora-supervisor\"";

fn dev_runner(server: &str, runner_extra: &str, backend: &str) -> String {
    format!(
        "[server]\n{server}\n[runner]\nbackend = \"{backend}\"\napi_url = \"http://localhost:4101\"\n{runner_extra}\n"
    )
}

#[test]
fn dev_supervisor_binary_is_accepted_only_in_dev_mode() {
    let dev_server = "bind = \"127.0.0.1\"\nexternal_url = \"http://localhost:4100\"";
    let ok = load_with(&dev_runner(dev_server, DEV_SUP, "podman"), &[]).unwrap();
    let r = ok.runner_config().unwrap();
    assert_eq!(
        r.runner.dev_supervisor_binary.as_deref(),
        Some("/home/dev/aurora/target/debug/aurora-supervisor")
    );
    // Unset by default.
    let off = load_with(&dev_runner(dev_server, "", "podman"), &[]).unwrap();
    assert!(
        off.runner_config()
            .unwrap()
            .runner
            .dev_supervisor_binary
            .is_none()
    );

    // Refused: public bind, public external URL, non-loopback runner API, wrong backend, bad path.
    let refused = [
        dev_runner(
            "bind = \"0.0.0.0\"\nexternal_url = \"http://localhost:4100\"",
            DEV_SUP,
            "podman",
        ),
        dev_runner("bind = \"127.0.0.1\"", DEV_SUP, "podman"),
        dev_runner(
            "bind = \"127.0.0.1\"\nexternal_url = \"https://aurora.example.org\"",
            DEV_SUP,
            "podman",
        ),
        dev_runner(dev_server, DEV_SUP, "kubernetes"),
        dev_runner(dev_server, DEV_SUP, "fake"),
        dev_runner(
            dev_server,
            "dev_supervisor_binary = \"target/debug/aurora-supervisor\"",
            "podman",
        ),
        dev_runner(
            dev_server,
            "dev_supervisor_binary = \"/a:/etc/passwd:rw\"",
            "podman",
        ),
        dev_runner(
            dev_server,
            "dev_supervisor_binary = \"/a/../etc/shadow\"",
            "podman",
        ),
    ];
    for toml in refused {
        let raw = load_with(&toml, &[]).unwrap();
        assert!(
            raw.runner_config()
                .unwrap_err()
                .has_key("runner.dev_supervisor_binary"),
            "must be refused: {toml}"
        );
    }
    // A non-loopback runner API is refused too.
    let raw = load_with(
        &format!(
            "[server]\n{dev_server}\n[runner]\nbackend = \"podman\"\napi_url = \"http://aurora-api:4101\"\n{DEV_SUP}\n"
        ),
        &[],
    )
    .unwrap();
    assert!(
        raw.runner_config()
            .unwrap_err()
            .has_key("runner.dev_supervisor_binary")
    );
}

#[test]
fn byo_section_defaults_and_validation() {
    // Defaults: enabled, floor owners_only, no default list, context not required.
    let api = load_with(BASE, &[]).unwrap().api_config().unwrap();
    let b = &api.governance.byo;
    assert!(b.enabled);
    assert_eq!(b.minimum_posture, ByoPosture::OwnersOnly);
    assert!(b.default_source_cidrs.is_empty());
    assert!(!b.require_context_credentials);

    let ok = format!(
        "{BASE}[byo]\nenabled = false\nminimum_posture = \"enabled\"\n\
         default_source_cidrs = [\"203.0.113.0/24\", \"2001:db8::/32\"]\n\
         require_context_credentials = true\n"
    );
    let gw = load_with(&ok, &[]).unwrap().gateway_config().unwrap();
    assert!(!gw.governance.byo.enabled);
    assert_eq!(gw.governance.byo.minimum_posture, ByoPosture::Enabled);
    assert_eq!(gw.governance.byo.default_source_cidrs.len(), 2);
    assert!(gw.governance.byo.require_context_credentials);

    for (bad, key) in [
        ("minimum_posture = \"never\"", "byo.minimum_posture"),
        (
            "default_source_cidrs = [\"0.0.0.0/0\"]",
            "byo.default_source_cidrs[0]",
        ),
        (
            "default_source_cidrs = [\"::/0\"]",
            "byo.default_source_cidrs[0]",
        ),
        (
            "default_source_cidrs = [\"10.0.0.1/8\"]",
            "byo.default_source_cidrs[0]",
        ),
        (
            "default_source_cidrs = [\"nonsense\"]",
            "byo.default_source_cidrs[0]",
        ),
    ] {
        let e = load_with(&format!("{BASE}[byo]\n{bad}\n"), &[])
            .unwrap()
            .api_config()
            .unwrap_err();
        assert!(e.has_key(key), "{bad}: {e:?}");
    }
    let many: Vec<String> = (0..33).map(|i| format!("\"10.{i}.0.0/16\"")).collect();
    let e = load_with(
        &format!("{BASE}[byo]\ndefault_source_cidrs = [{}]\n", many.join(",")),
        &[],
    )
    .unwrap()
    .api_config()
    .unwrap_err();
    assert!(e.has_key("byo.default_source_cidrs"));
}

#[test]
fn gateway_trusted_proxies_are_validated_cidrs() {
    let ok = format!("{BASE}[gateway]\ntrusted_proxies = [\"10.0.0.0/8\"]\n");
    let gw = load_with(&ok, &[]).unwrap().gateway_config().unwrap();
    assert_eq!(gw.gateway.trusted_proxies, ["10.0.0.0/8"]);
    for bad in ["0.0.0.0/0", "10.0.0.1", "x"] {
        let t = format!("{BASE}[gateway]\ntrusted_proxies = [\"{bad}\"]\n");
        let e = load_with(&t, &[]).unwrap().gateway_config().unwrap_err();
        assert!(e.has_key("gateway.trusted_proxies[0]"), "{bad}");
    }
}

#[test]
fn phase1_oidc_only_startup_rules() {
    // No provider and no dev IdP: startup fails (`no_oidc_provider`).
    let raw = load_with(
        "[server]\nexternal_url = \"https://aurora.example.org\"\n",
        &[],
    )
    .unwrap();
    let e = raw.api_config().unwrap_err();
    assert!(e.has_key("auth.oidc_providers"), "{e}");
    assert!(e.to_string().contains("no_oidc_provider"), "{e}");

    // `require_email_verified = false` is refused naming the key; `true` is accepted and ignored.
    let p = |v: &str| {
        format!(
            "[[auth.oidc_providers]]\nid = \"c\"\nissuer = \"https://i.example.org\"\nclient_id = \"c\"\nclient_secret_env = \"BASE_IDP_SECRET\"\nrequire_email_verified = {v}\n[server]\nexternal_url = \"https://aurora.example.org\"\n"
        )
    };
    let e = load_with(&p("false"), &[])
        .unwrap()
        .api_config()
        .unwrap_err();
    assert!(
        e.has_key("auth.oidc_providers[0].require_email_verified"),
        "{e}"
    );
    load_with(&p("true"), &[]).unwrap().api_config().unwrap();

    // The dev IdP registers the provider `dev` on a loopback origin ...
    let dev = |url: &str| {
        format!(
            "[auth.dev_idp]\nenabled = true\nissuer = \"http://localhost:4107\"\n[server]\nexternal_url = \"{url}\"\n"
        )
    };
    let api = load_with(&dev("http://localhost:4100"), &[])
        .unwrap()
        .api_config()
        .unwrap();
    let d = api
        .auth
        .oidc_providers
        .iter()
        .find(|p| p.id == DEV_IDP_PROVIDER_ID)
        .unwrap();
    assert_eq!(d.client_id, DEV_IDP_CLIENT_ID);
    assert_eq!(d.display_name.as_deref(), Some("Development sign-in"));
    assert_eq!(
        api.oidc_client_secrets["dev"].expose(),
        DEV_IDP_CLIENT_SECRET
    );
    for ok in [
        "http://127.0.0.1:4100",
        "http://app.localhost:4000",
        "http://[::1]:4100",
    ] {
        load_with(&dev(ok), &[]).unwrap().api_config().unwrap();
    }
    // ... and is refused anywhere else, https or not.
    for bad in [
        "https://aurora.example.org",
        "http://aurora.example.org",
        "https://localhost.evil.example",
        "https://127.0.0.1.example.org",
    ] {
        let e = load_with(&dev(bad), &[]).unwrap().api_config().unwrap_err();
        assert!(e.has_key("auth.dev_idp.enabled"), "{bad}: {e}");
    }
}

// ---- Vault store (P3-VLT-1; 15 Vault rows, C-370..C-375, SEC-61) ----

const ORG: &str = "0190f6a2-7c1e-7b3a-9d4e-5f6a7b8c9d0e";

fn vault(extra: &str) -> String {
    format!(
        "{BASE}[[secrets.stores]]\nid = \"vault-prod\"\nkind = \"hashicorp-vault\"\naddress = \"https://vault.example.org:8200\"\nauth = {{ method = \"kubernetes\", role = \"aurora\" }}\n{extra}"
    )
}

fn vault_err(extra: &str) -> aurora_config::ConfigErrors {
    load_with(&vault(extra), &[])
        .unwrap()
        .api_config()
        .unwrap_err()
}

#[test]
fn vault_store_defaults_and_valid_scopes() {
    let raw = load_with(
        &vault(&format!(
            "[[secrets.stores.reference_scopes]]\norg_id = \"{ORG}\"\npaths = [\"teams/physics/*\", \"shared/openai\"]\n[[secrets.stores.reference_scopes]]\norg_id = \"{ORG}\"\npaths = [\"more/*\"]\n"
        )),
        &[],
    )
    .unwrap();
    raw.api_config().unwrap();
    raw.gateway_config().unwrap();
    let s = aurora_config::VaultSettings::parse(&raw.secrets.stores[0]).unwrap();
    assert_eq!(s.kv_mount, "secret");
    assert_eq!(s.managed_prefix, "aurora");
    assert_eq!(
        s.auth,
        aurora_config::VaultAuthSettings::Kubernetes {
            mount: "kubernetes".into(),
            role: "aurora".into(),
            jwt_path: "/var/run/secrets/kubernetes.io/serviceaccount/token".into()
        }
    );
    // Several entries for one org id are unioned.
    assert_eq!(s.reference_scopes.len(), 1);
    assert_eq!(s.reference_scopes[0].paths.len(), 3);
}

#[test]
fn vault_reference_scope_covering_managed_prefix_refuses_startup() {
    for glob in [
        "*",
        "au*",
        "aurora*",
        "aurora/*",
        "aurora/0190f6a2/*",
        "aurora",
        "aurora/x/y",
    ] {
        let e = vault_err(&format!(
            "[[secrets.stores.reference_scopes]]\norg_id = \"{ORG}\"\npaths = [\"{glob}\"]\n"
        ));
        assert!(
            e.has_key("secrets.stores[0].reference_scopes[0].paths[0]"),
            "{glob}: {e}"
        );
    }
    // The prefix is configurable; the rule follows it. A sibling name is fine.
    let ok = format!(
        "managed_prefix = \"vault-managed\"\n[[secrets.stores.reference_scopes]]\norg_id = \"{ORG}\"\npaths = [\"aurora/*\", \"vault-managedx/*\"]\n"
    );
    load_with(&vault(&ok), &[]).unwrap().api_config().unwrap();
    let e = vault_err(&format!(
        "managed_prefix = \"vault-managed\"\n[[secrets.stores.reference_scopes]]\norg_id = \"{ORG}\"\npaths = [\"vault-managed/*\"]\n"
    ));
    assert!(e.has_key("secrets.stores[0].reference_scopes[0].paths[0]"));
}

#[test]
fn vault_reference_scope_non_uuid_org_id_refuses_startup() {
    for bad in [
        "acme",
        "",
        "0190f6a27c1e7b3a9d4e5f6a7b8c9d0e",
        "urn:uuid:0190f6a2-7c1e-7b3a-9d4e-5f6a7b8c9d0e",
    ] {
        let e = vault_err(&format!(
            "[[secrets.stores.reference_scopes]]\norg_id = \"{bad}\"\npaths = [\"a/*\"]\n"
        ));
        assert!(
            e.has_key("secrets.stores[0].reference_scopes[0].org_id"),
            "{bad:?}: {e}"
        );
    }
    let e = vault_err("[[secrets.stores.reference_scopes]]\npaths = [\"a/*\"]\n");
    assert!(e.has_key("secrets.stores[0].reference_scopes[0].org_id"));
}

#[test]
fn vault_scope_glob_rules() {
    for bad in [
        "a/*/b", "a/b*c", "/a", "a//b", "a/../b", "a/./b", "a/", "**", "a/b/**",
    ] {
        let e = vault_err(&format!(
            "[[secrets.stores.reference_scopes]]\norg_id = \"{ORG}\"\npaths = [\"{bad}\"]\n"
        ));
        assert!(
            e.has_key("secrets.stores[0].reference_scopes[0].paths[0]"),
            "{bad:?}: {e}"
        );
    }
    let e = vault_err(&format!(
        "[[secrets.stores.reference_scopes]]\norg_id = \"{ORG}\"\npaths = []\n"
    ));
    assert!(e.has_key("secrets.stores[0].reference_scopes[0].paths"));
}

#[test]
fn vault_address_auth_and_mount_rules() {
    let with = |address: &str, auth: &str, extra: &str| {
        load_with(
            &format!(
                "{BASE}[[secrets.stores]]\nid = \"v\"\nkind = \"hashicorp-vault\"\naddress = \"{address}\"\nauth = {auth}\n{extra}"
            ),
            &[],
        )
        .unwrap()
        .api_config()
    };
    let k8s = "{ method = \"kubernetes\", role = \"r\" }";
    with("https://vault:8200", k8s, "").unwrap();
    with(
        "http://127.0.0.1:8200",
        "{ method = \"token\", token_env = \"VAULT_TOKEN\" }",
        "",
    )
    .unwrap();
    with("http://localhost:8200", k8s, "").unwrap();
    for bad in [
        "http://vault.example.org:8200",
        "https://u:p@vault:8200",
        "https://vault:8200/v1",
        "https://vault:8200?x=1",
        "vault:8200",
        "ftp://vault",
    ] {
        let e = with(bad, k8s, "").unwrap_err();
        assert!(e.has_key("secrets.stores[0].address"), "{bad}: {e}");
    }
    for (auth, key) in [
        ("{ method = \"kubernetes\" }", "secrets.stores[0].auth.role"),
        ("{ method = \"ldap\" }", "secrets.stores[0].auth.method"),
        ("{ role = \"r\" }", "secrets.stores[0].auth.method"),
        (
            "{ method = \"approle\", role_id_env = \"A\" }",
            "secrets.stores[0].auth.secret_id_env",
        ),
        (
            "{ method = \"approle\", role = \"r\", role_id_env = \"A\", secret_id_env = \"B\" }",
            "secrets.stores[0].auth.role",
        ),
        ("{ method = \"token\" }", "secrets.stores[0].auth.token_env"),
        (
            "{ method = \"token\", token_env = \"T\", mount = \"m\" }",
            "secrets.stores[0].auth.mount",
        ),
        (
            "{ method = \"kubernetes\", role = \"r\", mount = \"/k8s\" }",
            "secrets.stores[0].auth.mount",
        ),
    ] {
        let e = with("https://vault:8200", auth, "").unwrap_err();
        assert!(e.has_key(key), "{auth}: {e}");
    }
    for (extra, key) in [
        ("kv_mount = \"/secret\"\n", "secrets.stores[0].kv_mount"),
        ("kv_mount = \"secret/\"\n", "secrets.stores[0].kv_mount"),
        (
            "managed_prefix = \"a b\"\n",
            "secrets.stores[0].managed_prefix",
        ),
        (
            "managed_prefix = \"a/../b\"\n",
            "secrets.stores[0].managed_prefix",
        ),
        (
            "managed_prefix = \"\"\n",
            "secrets.stores[0].managed_prefix",
        ),
    ] {
        let e = with("https://vault:8200", k8s, extra).unwrap_err();
        assert!(e.has_key(key), "{extra}: {e}");
    }
    // Unknown keys are refused, not ignored.
    assert!(with("https://vault:8200", k8s, "adress = \"x\"\n").is_err());
}
