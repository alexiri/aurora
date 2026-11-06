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

const BASE: &str = "[server]\nexternal_url = \"https://aurora.example.org\"\n";

#[test]
fn toml_only_and_api_validates() {
    let raw = load_with(BASE, &[]).unwrap();
    let api = raw.api_config().unwrap();
    assert_eq!(api.external_url, "https://aurora.example.org");
    assert_eq!(api.max_connections, 20);
    assert_eq!(api.advertised_proxy_url, "http://aurora.example.org:4102");
    assert_eq!(api.keyring.active, "main");
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
        "https://x.example".into(),
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
    assert_eq!(r.telemetry.log_level, "info");
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
    let o = &a.auth.oidc_providers[0];
    assert_eq!(o.issuer, "https://idp.example.org");
    assert!(o.require_email_verified);
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
