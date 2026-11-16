//! (P4-LLM-1, C-800) Endpoint-entry rules for every LLM provider, as one function.
//!
//! The same function serves the startup check (`check_llm`, message = the code) and the settings writes of SET-3
//! (`422 settings_invalid`, `details.fields[] = { path: "/llm.endpoints/<id>/<key>", code }`), so a rule is written once.
//! The entry's JSON keys are classified by [`unknown_key_code`] before it is deserialized into [`LlmEndpoint`].
use crate::LlmEndpoint;

/// One refusal: the offending key and the stable code of C-800.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointFieldError {
    pub field: &'static str,
    pub code: &'static str,
}

/// Where an entry comes from: a Helm entry keeps today's rules (`http` for `litellm`/`openai_compatible` as the
/// operator's choice); a dashboard entry may use `http` only to a private host (D-262(b)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrySource {
    Helm,
    Dashboard {
        /// The host is covered by `gateway.allowed_private` (the caller knows; this crate does not).
        host_allowed_private: bool,
    },
}

/// Every key an entry may carry (the typed struct's fields).
pub const ENDPOINT_KEYS: &[&str] = &[
    "id",
    "provider",
    "display_name",
    "base_url",
    "dialects",
    "default_model",
    "models",
    "allowed_models",
    "key_origin",
    "allowed_paths",
    "organization",
    "project",
    "auth_header",
    "models_path",
    "region",
    "project_id",
    "location",
];

/// The code for a key that is not part of an entry: `credential_in_entry` for anything named like a credential
/// (`api_key`, `access_key*`, `secret*`, `private_key`, `password`, `token`, `client_secret`, …), else `key_unknown`.
/// `None` when the key is a known one.
pub fn unknown_key_code(key: &str) -> Option<&'static str> {
    if ENDPOINT_KEYS.contains(&key) {
        return None;
    }
    let k = key.to_ascii_lowercase();
    let credential = k == "api_key"
        || k == "apikey"
        || k.starts_with("access_key")
        || k.starts_with("secret")
        || k.ends_with("_secret")
        || k == "private_key"
        || k == "password"
        || k == "token"
        || k.ends_with("_token")
        || k == "client_secret";
    Some(if credential {
        "credential_in_entry"
    } else {
        "key_unknown"
    })
}

const OPENAI_DIALECTS: &[&str] = &["openai_chat", "openai_responses"];
const ALL_DIALECTS: &[&str] = &[
    "anthropic_messages",
    "openai_chat",
    "openai_responses",
    "anthropic_bedrock",
    "anthropic_vertex",
];
/// Most `allowed_paths` entries (D-177).
pub const ALLOWED_PATHS_MAX: usize = 32;

fn is_loopback(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']);
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `scheme://host[:port]` parts of a bare origin (no userinfo, path other than `/`, query or fragment).
fn bare_origin(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains(['/', '?', '#', '@', ' ', '\\']) {
        return None;
    }
    authority_host(rest).map(|h| (scheme, h))
}

/// The host of `host[:port]` (or `[v6][:port]`), when the authority is well formed.
fn authority_host(authority: &str) -> Option<&str> {
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split_once(']')?.0
    } else {
        authority.split(':').next()?
    };
    let port = if authority.starts_with('[') {
        authority.split_once("]:")
    } else {
        authority.split_once(':')
    }
    .map(|x| x.1);
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    (!host.is_empty()
        && port_ok
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':'))
    .then_some(host)
}

/// `base_url` of a compatible endpoint: `http(s)://host[:port][/prefix]`, no userinfo, query or fragment.
fn origin_with_prefix(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if rest.contains(['?', '#', '@', ' ', '\\']) {
        return None;
    }
    let authority = rest.split('/').next()?;
    let host = authority_host(authority)?;
    // no `..` or empty segments in the prefix
    let prefix = &rest[authority.len()..];
    if prefix
        .trim_end_matches('/')
        .split('/')
        .skip(1)
        .any(|s| s.is_empty() || s == "..")
    {
        return None;
    }
    Some((scheme, host))
}

/// `^[a-z]{2}(-gov)?-[a-z]+-[0-9]$`
pub fn region_valid(r: &str) -> bool {
    let p: Vec<&str> = r.split('-').collect();
    let (head, word, digit) = match p.as_slice() {
        [a, w, d] => (*a, *w, *d),
        [a, "gov", w, d] => (*a, *w, *d),
        _ => return false,
    };
    head.len() == 2
        && head.bytes().all(|b| b.is_ascii_lowercase())
        && !word.is_empty()
        && word.bytes().all(|b| b.is_ascii_lowercase())
        && digit.len() == 1
        && digit.bytes().all(|b| b.is_ascii_digit())
}

/// `^[a-z][a-z0-9-]{4,28}[a-z0-9]$`
pub fn project_id_valid(p: &str) -> bool {
    let b = p.as_bytes();
    (6..=30).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && (b[b.len() - 1].is_ascii_lowercase() || b[b.len() - 1].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `global` or `^[a-z]+-[a-z]+[0-9]+$`
pub fn location_valid(l: &str) -> bool {
    if l == "global" {
        return true;
    }
    let Some((a, rest)) = l.split_once('-') else {
        return false;
    };
    let letters = rest.bytes().take_while(u8::is_ascii_lowercase).count();
    let digits = &rest[letters..];
    !a.is_empty()
        && a.bytes().all(|b| b.is_ascii_lowercase())
        && letters > 0
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
}

fn allowed_paths_shape_ok(paths: &[String]) -> bool {
    paths.len() <= ALLOWED_PATHS_MAX
        && paths.iter().all(|p| {
            let body = p.strip_suffix("/*").unwrap_or(p);
            !(!p.starts_with('/')
                || p.len() > 200
                || body.contains(['*', '?', '#', ' ', '\\'])
                || body
                    .trim_end_matches('/')
                    .split('/')
                    .skip(1)
                    .any(|s| s.is_empty() || s == ".."))
        })
}

/// All refusals of one entry, in a stable order. Empty = the entry is acceptable. Providers outside the five new ones
/// (`litellm`, `anthropic`) keep their existing checks elsewhere; only the common rules apply here.
pub fn endpoint_field_errors(e: &LlmEndpoint, source: EntrySource) -> Vec<EndpointFieldError> {
    let mut out = Vec::new();
    let mut err =
        |field: &'static str, code: &'static str| out.push(EndpointFieldError { field, code });
    let provider = e.provider.as_str();
    let is_new = matches!(
        provider,
        "openai" | "azure_openai" | "bedrock" | "vertex" | "openai_compatible"
    );

    // keys that belong to another provider
    let own: &[&str] = match provider {
        "openai" => &["organization", "project"],
        "azure_openai" => &[],
        "bedrock" => &["region"],
        "vertex" => &["project_id", "location"],
        "openai_compatible" => &["auth_header", "models_path"],
        _ => &[],
    };
    let present = [
        ("organization", e.organization.is_some()),
        ("project", e.project.is_some()),
        ("auth_header", e.auth_header.is_some()),
        ("models_path", e.models_path.is_some()),
        ("region", e.region.is_some()),
        ("project_id", e.project_id.is_some()),
        ("location", e.location.is_some()),
    ];
    for (k, set) in present {
        if set && !own.contains(&k) {
            err(
                match k {
                    "organization" => "organization",
                    "project" => "project",
                    "auth_header" => "auth_header",
                    "models_path" => "models_path",
                    "region" => "region",
                    "project_id" => "project_id",
                    _ => "location",
                },
                "key_unknown",
            );
        }
    }

    // dialects
    let allowed_dialects: &[&str] = match provider {
        "openai" | "azure_openai" => OPENAI_DIALECTS,
        "bedrock" => &["anthropic_bedrock", "openai_chat", "openai_responses"],
        "vertex" => &["anthropic_vertex", "openai_chat"],
        "openai_compatible" => &["anthropic_messages", "openai_chat", "openai_responses"],
        _ => ALL_DIALECTS,
    };
    if e.dialects
        .iter()
        .any(|d| !allowed_dialects.contains(&d.as_str()))
    {
        err("dialects", "dialect_unsupported");
    }

    // base_url
    match provider {
        "openai" | "vertex" => {
            if e.base_url.is_some() {
                err("base_url", "base_url_not_allowed");
            }
        }
        // `https`; plain `http` only to a loopback host (`08 §Configuration`: tests and development, e.g. the mock
        // resources of the e2e suites), never a reachable one.
        "azure_openai" => match e.base_url.as_deref().and_then(bare_origin) {
            Some(("https", _)) => {}
            Some(("http", host)) if is_loopback(host) => {}
            _ => err("base_url", "base_url_invalid"),
        },
        "bedrock" => {
            // An optional VPC endpoint; `https`, or plain `http` to a loopback host as for `azure_openai`.
            if let Some(u) = e.base_url.as_deref()
                && !match bare_origin(u) {
                    Some(("https", _)) => true,
                    Some(("http", host)) => is_loopback(host),
                    _ => false,
                }
            {
                err("base_url", "base_url_invalid");
            }
        }
        "openai_compatible" => match e.base_url.as_deref().and_then(origin_with_prefix) {
            Some(("https", _)) => {}
            Some(("http", host)) => {
                if let EntrySource::Dashboard {
                    host_allowed_private,
                } = source
                    && !(is_loopback(host) || host_allowed_private)
                {
                    err("base_url", "http_needs_private_host");
                }
            }
            _ => err("base_url", "base_url_invalid"),
        },
        _ => {}
    }

    // provider-specific values
    if provider == "openai" {
        for (f, v) in [("organization", &e.organization), ("project", &e.project)] {
            if v.as_deref()
                .is_some_and(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
            {
                err(
                    if f == "organization" {
                        "organization"
                    } else {
                        "project"
                    },
                    "value_invalid",
                );
            }
        }
    }
    if provider == "bedrock" && !e.region.as_deref().is_some_and(region_valid) {
        err("region", "region_invalid");
    }
    if provider == "vertex" {
        if !e.project_id.as_deref().is_some_and(project_id_valid) {
            err("project_id", "project_id_invalid");
        }
        if !e.location.as_deref().is_some_and(location_valid) {
            err("location", "location_invalid");
        }
    }
    if provider == "openai_compatible" {
        if e.auth_header
            .as_deref()
            .is_some_and(|h| !matches!(h, "authorization" | "api-key" | "x-api-key"))
        {
            err("auth_header", "value_invalid");
        }
        if e.models_path.as_deref().is_some_and(|m| {
            !m.starts_with('/')
                || m.len() > 256
                || m.contains(['?', '#', ' ', '\\'])
                || m.split('/').skip(1).any(|s| s == "..")
        }) {
            err("models_path", "value_invalid");
        }
    }

    // models
    if matches!(provider, "azure_openai" | "vertex") {
        if e.models.is_empty() {
            err("models", "models_required");
        }
        if e.default_model.as_deref().is_none_or(str::is_empty) {
            err("default_model", "default_model_required");
        }
    }

    // allowed_paths
    if !e.allowed_paths.is_empty() {
        if matches!(provider, "bedrock" | "vertex") {
            err("allowed_paths", "allowed_paths_not_supported");
        } else if !allowed_paths_shape_ok(&e.allowed_paths) {
            err("allowed_paths", "allowed_paths_invalid");
        }
    }

    // key-origin method per provider
    if is_new && let Some(o) = &e.key_origin {
        let ok: &[&str] = match provider {
            "openai" | "vertex" => &["provider_account"],
            "azure_openai" | "bedrock" => &["endpoint", "provider_account"],
            _ => &["endpoint"],
        };
        if !ok.contains(&o.method.as_str()) {
            err("key_origin", "key_origin_method_unsupported");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyOrigin;

    fn ep(provider: &str) -> LlmEndpoint {
        LlmEndpoint {
            id: "e".into(),
            provider: provider.into(),
            ..LlmEndpoint::default()
        }
    }
    fn codes(e: &LlmEndpoint) -> Vec<(&'static str, &'static str)> {
        endpoint_field_errors(e, EntrySource::Helm)
            .into_iter()
            .map(|x| (x.field, x.code))
            .collect()
    }
    fn s(x: &str) -> Option<String> {
        Some(x.into())
    }
    fn good_vertex() -> LlmEndpoint {
        LlmEndpoint {
            project_id: s("my-project-1"),
            location: s("europe-west1"),
            models: vec!["claude-sonnet-4-5@20250929".into()],
            default_model: s("claude-sonnet-4-5@20250929"),
            ..ep("vertex")
        }
    }

    #[test]
    fn minimal_valid_entries() {
        assert!(codes(&ep("openai")).is_empty());
        assert!(
            codes(&LlmEndpoint {
                base_url: s("https://acme.openai.azure.com"),
                models: vec!["gpt-5".into()],
                default_model: s("gpt-5"),
                ..ep("azure_openai")
            })
            .is_empty()
        );
        assert!(
            codes(&LlmEndpoint {
                region: s("eu-central-1"),
                ..ep("bedrock")
            })
            .is_empty()
        );
        assert!(
            codes(&LlmEndpoint {
                region: s("us-gov-west-1"),
                base_url: s("https://vpce-1.bedrock-runtime.us-gov-west-1.vpce.amazonaws.com"),
                ..ep("bedrock")
            })
            .is_empty()
        );
        assert!(codes(&good_vertex()).is_empty());
        assert!(
            codes(&LlmEndpoint {
                location: s("global"),
                ..good_vertex()
            })
            .is_empty()
        );
        assert!(
            codes(&LlmEndpoint {
                base_url: s("https://llm.example.com/prefix"),
                auth_header: s("api-key"),
                models_path: s("/api/tags"),
                dialects: vec!["openai_chat".into(), "anthropic_messages".into()],
                ..ep("openai_compatible")
            })
            .is_empty()
        );
    }

    #[test]
    fn every_refusal_code() {
        let b = |f: fn(&mut LlmEndpoint), p: &str| {
            let mut e = ep(p);
            f(&mut e);
            codes(&e)
        };
        assert_eq!(
            b(|e| e.base_url = s("https://x.example"), "openai"),
            [("base_url", "base_url_not_allowed")]
        );
        assert_eq!(
            b(|e| e.base_url = s("https://x.example"), "vertex").first(),
            Some(&("base_url", "base_url_not_allowed"))
        );
        assert_eq!(
            b(|e| e.region = s("eu-central-1"), "openai"),
            [("region", "key_unknown")]
        );
        for bad in [
            "http://acme.openai.azure.com",
            "https://acme.openai.azure.com/openai",
            "https://u:p@acme.openai.azure.com",
            "https://acme.openai.azure.com?x=1",
            "acme.openai.azure.com",
            "",
        ] {
            let e = LlmEndpoint {
                base_url: s(bad),
                models: vec!["m".into()],
                default_model: s("m"),
                ..ep("azure_openai")
            };
            assert_eq!(codes(&e), [("base_url", "base_url_invalid")], "{bad}");
        }
        // plain http only to a loopback host (tests and development)
        for ok in [
            "http://127.0.0.1:8080",
            "http://localhost:9",
            "http://[::1]:7",
        ] {
            let e = LlmEndpoint {
                base_url: s(ok),
                models: vec!["m".into()],
                default_model: s("m"),
                ..ep("azure_openai")
            };
            assert!(codes(&e).is_empty(), "{ok}: {:?}", codes(&e));
        }
        // (P4-LLM-4) a Bedrock VPC endpoint: `https`, or plain `http` to a loopback host only
        for (url, ok) in [
            ("http://127.0.0.1:9", true),
            (
                "https://vpce-1.bedrock-runtime.us-east-1.vpce.amazonaws.com",
                true,
            ),
            ("http://bedrock.example.com", false),
        ] {
            let e = LlmEndpoint {
                base_url: s(url),
                region: s("us-east-1"),
                ..ep("bedrock")
            };
            assert_eq!(codes(&e).is_empty(), ok, "{url}: {:?}", codes(&e));
        }
        assert_eq!(
            codes(&ep("azure_openai")),
            [
                ("base_url", "base_url_invalid"),
                ("models", "models_required"),
                ("default_model", "default_model_required")
            ]
        );
        for bad in [
            "",
            "eu-central",
            "EU-central-1",
            "eu-central-12",
            "us-gov-gov-west-1",
            "europe-central-1",
            "us-west-",
        ] {
            assert_eq!(
                codes(&LlmEndpoint {
                    region: s(bad),
                    ..ep("bedrock")
                }),
                [("region", "region_invalid")],
                "{bad}"
            );
        }
        assert_eq!(codes(&ep("bedrock")), [("region", "region_invalid")]);
        for bad in [
            "MyProject",
            "a-b",
            "1abcdef",
            "my-project-",
            "my_project_1",
            &"a".repeat(31),
        ] {
            assert_eq!(
                codes(&LlmEndpoint {
                    project_id: s(bad),
                    ..good_vertex()
                }),
                [("project_id", "project_id_invalid")],
                "{bad}"
            );
        }
        for bad in [
            "",
            "us",
            "us-central",
            "US-central1",
            "europe-west",
            "us-central1-a",
        ] {
            assert_eq!(
                codes(&LlmEndpoint {
                    location: s(bad),
                    ..good_vertex()
                }),
                [("location", "location_invalid")],
                "{bad}"
            );
        }
        assert_eq!(
            codes(&LlmEndpoint {
                models: vec![],
                default_model: None,
                ..good_vertex()
            }),
            [
                ("models", "models_required"),
                ("default_model", "default_model_required")
            ]
        );
        assert_eq!(
            codes(&LlmEndpoint {
                dialects: vec!["openai_responses".into()],
                ..good_vertex()
            }),
            [("dialects", "dialect_unsupported")]
        );
        assert_eq!(
            codes(&LlmEndpoint {
                dialects: vec!["anthropic_messages".into()],
                region: s("eu-west-1"),
                ..ep("bedrock")
            }),
            [("dialects", "dialect_unsupported")]
        );
        assert_eq!(
            codes(&LlmEndpoint {
                dialects: vec!["anthropic_vertex".into()],
                ..ep("openai")
            }),
            [("dialects", "dialect_unsupported")]
        );
        assert_eq!(
            codes(&LlmEndpoint {
                allowed_paths: vec!["/v1/x".into()],
                region: s("eu-west-1"),
                ..ep("bedrock")
            }),
            [("allowed_paths", "allowed_paths_not_supported")]
        );
        assert_eq!(
            codes(&LlmEndpoint {
                allowed_paths: vec!["/v1/x".into()],
                ..good_vertex()
            }),
            [("allowed_paths", "allowed_paths_not_supported")]
        );
        for bad in [
            vec!["v1/x"],
            vec!["/a/../b"],
            vec!["/a?x"],
            vec!["/a*"],
            vec!["//a"],
        ] {
            let e = LlmEndpoint {
                allowed_paths: bad.iter().map(|x| (*x).into()).collect(),
                ..ep("openai")
            };
            assert_eq!(
                codes(&e),
                [("allowed_paths", "allowed_paths_invalid")],
                "{bad:?}"
            );
        }
        assert_eq!(
            codes(&LlmEndpoint {
                allowed_paths: (0..33).map(|i| format!("/p{i}")).collect(),
                ..ep("openai")
            }),
            [("allowed_paths", "allowed_paths_invalid")]
        );
        assert!(
            codes(&LlmEndpoint {
                allowed_paths: vec!["/*".into()],
                ..ep("openai")
            })
            .is_empty()
        );
        let ko = |m: &str| {
            Some(KeyOrigin {
                method: m.into(),
                ..KeyOrigin::default()
            })
        };
        assert_eq!(
            codes(&LlmEndpoint {
                key_origin: ko("endpoint"),
                ..ep("openai")
            }),
            [("key_origin", "key_origin_method_unsupported")]
        );
        assert_eq!(
            codes(&LlmEndpoint {
                key_origin: ko("litellm_team"),
                region: s("eu-west-1"),
                ..ep("bedrock")
            }),
            [("key_origin", "key_origin_method_unsupported")]
        );
        assert_eq!(
            codes(&LlmEndpoint {
                key_origin: ko("endpoint"),
                ..good_vertex()
            }),
            [("key_origin", "key_origin_method_unsupported")]
        );
        assert!(
            codes(&LlmEndpoint {
                key_origin: ko("provider_account"),
                region: s("eu-west-1"),
                ..ep("bedrock")
            })
            .is_empty()
        );
        assert_eq!(
            codes(&LlmEndpoint {
                key_origin: ko("provider_account"),
                base_url: s("https://x.example"),
                ..ep("openai_compatible")
            }),
            [("key_origin", "key_origin_method_unsupported")]
        );
    }

    #[test]
    fn openai_compatible_rules() {
        let c = |url: &str, src| {
            endpoint_field_errors(
                &LlmEndpoint {
                    base_url: s(url),
                    ..ep("openai_compatible")
                },
                src,
            )
            .into_iter()
            .map(|x| x.code)
            .collect::<Vec<_>>()
        };
        let dash = |allowed| EntrySource::Dashboard {
            host_allowed_private: allowed,
        };
        assert!(c("http://llm.internal:8000/v1", EntrySource::Helm).is_empty());
        assert_eq!(
            c("http://llm.internal:8000", dash(false)),
            ["http_needs_private_host"]
        );
        assert!(c("http://llm.internal:8000", dash(true)).is_empty());
        assert!(c("http://127.0.0.1:8000", dash(false)).is_empty());
        assert!(c("http://localhost:8000", dash(false)).is_empty());
        assert!(c("http://[::1]:8000", dash(false)).is_empty());
        assert!(c("https://llm.example.com", dash(false)).is_empty());
        for bad in [
            "",
            "ftp://x.example",
            "https://u:p@x.example",
            "https://x.example/a?b=1",
            "https://x.example/a/../b",
            "x.example",
        ] {
            assert_eq!(c(bad, EntrySource::Helm), ["base_url_invalid"], "{bad}");
        }
        assert_eq!(
            codes(&LlmEndpoint {
                base_url: s("https://x.example"),
                auth_header: s("cookie"),
                models_path: s("api/tags"),
                ..ep("openai_compatible")
            }),
            [
                ("auth_header", "value_invalid"),
                ("models_path", "value_invalid")
            ]
        );
        assert_eq!(
            codes(&LlmEndpoint {
                base_url: s("https://x.example"),
                dialects: vec!["anthropic_bedrock".into()],
                ..ep("openai_compatible")
            }),
            [("dialects", "dialect_unsupported")]
        );
        // `http` is for the compatible provider only
        assert_eq!(
            codes(&LlmEndpoint {
                base_url: s("http://x.example"),
                models: vec!["m".into()],
                default_model: s("m"),
                ..ep("azure_openai")
            }),
            [("base_url", "base_url_invalid")]
        );
    }

    #[test]
    fn credential_named_keys_are_refused_everything_else_unknown() {
        for k in [
            "api_key",
            "access_key_id",
            "access_key",
            "secret",
            "secret_access_key",
            "private_key",
            "password",
            "token",
            "client_secret",
            "bearer_token",
            "SECRET_X",
        ] {
            assert_eq!(unknown_key_code(k), Some("credential_in_entry"), "{k}");
        }
        for k in ["colour", "base-url", "timeout"] {
            assert_eq!(unknown_key_code(k), Some("key_unknown"), "{k}");
        }
        for k in ENDPOINT_KEYS {
            assert_eq!(unknown_key_code(k), None, "{k}");
        }
    }

    #[test]
    fn provider_preference_appends_missing_enabled_providers() {
        let l = crate::LlmSection {
            enabled_providers: vec![
                "litellm".into(),
                "anthropic".into(),
                "openai".into(),
                "vertex".into(),
            ],
            provider_preference: vec!["anthropic".into(), "litellm".into()],
            ..crate::LlmSection::default()
        };
        assert_eq!(
            l.effective_provider_preference(),
            ["anthropic", "litellm", "openai", "vertex"]
        );
        assert_eq!(
            crate::LlmSection::default().effective_provider_preference(),
            ["litellm", "anthropic"]
        );
    }
}
