//! (P4-LLM-1, SEC-65, D-260(b)/(c)) The inference path sets of `08 §Phase 4 providers`, as pure functions.
//!
//! A provider's credential is injected only on its own inference routes; everything else on the endpoint host is
//! refused by the gateway with 403 `llm_path_not_permitted`. `allowed_paths` of an endpoint **adds** paths (an
//! absolute path, or a subtree ending in `/*`); it is never accepted for `bedrock` and `vertex`. Query and fragment do
//! not count; a path with an empty or `..` segment (also percent-encoded) never matches anything.
use crate::Dialect;
use http::Method;

/// The path without query, fragment and trailing slash, or `None` when it is not clean (an empty, `.`, `..` or
/// percent-encoded dot segment, or a missing leading `/`).
fn clean(path: &str) -> Option<&str> {
    let p = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let rest = p.strip_prefix('/')?;
    let ok = rest.split('/').all(|s| {
        let l = s.to_ascii_lowercase();
        !s.is_empty()
            && s != "."
            && s != ".."
            && !l.contains("%2e")
            && !l.contains("%2f")
            && !l.contains("%5c")
            && !s.contains('\\')
    });
    ok.then_some(p)
}

/// `allowed_paths`: exact, or a subtree with a trailing `/*` (`["/*"]` admits everything clean).
pub fn allowed_path_match(path: &str, allowed: &[String]) -> bool {
    let Some(path) = clean(path) else {
        return false;
    };
    allowed.iter().any(|a| match a.strip_suffix("/*") {
        Some(prefix) => path == prefix || path.starts_with(&format!("{prefix}/")),
        None => path == a.trim_end_matches('/'),
    })
}

/// Segments of `rest` (a clean path without its leading `/`).
fn segs(path: &str) -> Vec<&str> {
    path.trim_start_matches('/').split('/').collect()
}

/// `GET /v1/models` and `GET /v1/models/{id}` below `prefix` (`""` or e.g. `/openai`).
fn models_route(method: &Method, s: &[&str]) -> bool {
    *method == Method::GET
        && match s {
            ["v1", "models"] => true,
            ["v1", "models", id] => !id.is_empty(),
            _ => false,
        }
}

fn responses_route(method: &Method, s: &[&str]) -> bool {
    matches!(
        (method.as_str(), s),
        ("POST", ["v1", "responses"])
            | ("GET" | "DELETE", ["v1", "responses", _])
            | ("GET", ["v1", "responses", _, "input_items"])
            | ("POST", ["v1", "responses", _, "cancel"])
    )
}

fn chat_route(method: &Method, s: &[&str]) -> bool {
    *method == Method::POST && matches!(s, ["v1", "chat", "completions"] | ["v1", "completions"])
}

fn embeddings_route(method: &Method, s: &[&str]) -> bool {
    *method == Method::POST && matches!(s, ["v1", "embeddings"])
}

fn messages_route(method: &Method, s: &[&str]) -> bool {
    *method == Method::POST && matches!(s, ["v1", "messages"] | ["v1", "messages", "count_tokens"])
}

/// Strips `prefix` (`""`, or a base path such as `/openai`, no trailing slash) and returns the remaining clean segments.
fn below<'a>(path: &'a str, prefix: &str) -> Option<Vec<&'a str>> {
    let p = clean(path)?;
    let rest = if prefix.is_empty() {
        p
    } else {
        p.strip_prefix(prefix)?
    };
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    Some(segs(rest))
}

/// `anthropic` (D-260(c)): `POST /v1/messages`, `POST /v1/messages/count_tokens`, `GET /v1/models[/{id}]`, plus `allowed_paths`.
pub fn anthropic_path_permitted(method: &Method, path: &str, allowed: &[String]) -> bool {
    below(path, "").is_some_and(|s| messages_route(method, &s) || models_route(method, &s))
        || allowed_path_match(path, allowed)
}

/// `openai`: chat, completions, responses (and its `{id}` family), embeddings; `GET /v1/models[/{id}]`; plus `allowed_paths`.
pub fn openai_path_permitted(method: &Method, path: &str, allowed: &[String]) -> bool {
    below(path, "").is_some_and(|s| {
        chat_route(method, &s)
            || responses_route(method, &s)
            || embeddings_route(method, &s)
            || models_route(method, &s)
    }) || allowed_path_match(path, allowed)
}

/// `azure_openai`: the same routes under `/openai/v1/…` (written here as `/openai` + the `/v1` routes).
pub fn azure_path_permitted(method: &Method, path: &str, allowed: &[String]) -> bool {
    below(path, "/openai").is_some_and(|s| {
        chat_route(method, &s)
            || responses_route(method, &s)
            || embeddings_route(method, &s)
            || models_route(method, &s)
    }) || allowed_path_match(path, allowed)
}

/// `bedrock` (runtime host): `POST /model/{modelId}/{invoke|invoke-with-response-stream|converse|converse-stream|count-tokens}`
/// and, only for a declared OpenAI dialect, `POST /openai/v1/{chat/completions|responses}`. No `allowed_paths`, ever.
pub fn bedrock_path_permitted(method: &Method, path: &str, openai_dialect: bool) -> bool {
    let Some(p) = clean(path) else { return false };
    if *method != Method::POST {
        return false;
    }
    match segs(p).as_slice() {
        ["model", id, op] => {
            !id.is_empty()
                && matches!(
                    *op,
                    "invoke"
                        | "invoke-with-response-stream"
                        | "converse"
                        | "converse-stream"
                        | "count-tokens"
                )
        }
        ["openai", "v1", "chat", "completions"] | ["openai", "v1", "responses"] => openai_dialect,
        _ => false,
    }
}

/// `vertex`: `POST /v1/projects/<project_id>/locations/<location>/publishers/{anthropic|google}/models/<model>:{rawPredict|
/// streamRawPredict|generateContent|streamGenerateContent|countTokens}` and, when declared, `POST …/endpoints/openapi/chat/completions`.
/// The project and location in the path must equal the endpoint's. No `allowed_paths`, ever.
pub fn vertex_path_permitted(
    method: &Method,
    path: &str,
    project_id: &str,
    location: &str,
    openai_dialect: bool,
) -> bool {
    let Some(p) = clean(path) else { return false };
    if *method != Method::POST {
        return false;
    }
    match segs(p).as_slice() {
        [
            "v1",
            "projects",
            pr,
            "locations",
            loc,
            "publishers",
            publisher,
            "models",
            model_op,
        ] => {
            let Some((model, op)) = model_op.split_once(':') else {
                return false;
            };
            *pr == project_id
                && *loc == location
                && matches!(*publisher, "anthropic" | "google")
                && !model.is_empty()
                && matches!(
                    op,
                    "rawPredict"
                        | "streamRawPredict"
                        | "generateContent"
                        | "streamGenerateContent"
                        | "countTokens"
                )
        }
        [
            "v1",
            "projects",
            pr,
            "locations",
            loc,
            "endpoints",
            "openapi",
            "chat",
            "completions",
        ] => openai_dialect && *pr == project_id && *loc == location,
        _ => false,
    }
}

/// `openai_compatible`: derived from the declared dialects relative to the base path (`base_path` is the path of the
/// endpoint's `base_url`, `""` when none): `openai_chat` → chat and completions; `openai_responses` → the responses
/// family; `anthropic_messages` → messages and count_tokens; always `GET /v1/models[/{id}]`, `POST /v1/embeddings`, and
/// `GET <models_path>`; plus `allowed_paths` (`["/*"]` is the opt-out).
pub fn compatible_path_permitted(
    method: &Method,
    path: &str,
    base_path: &str,
    dialects: &[Dialect],
    models_path: Option<&str>,
    allowed: &[String],
) -> bool {
    let base = base_path.trim_end_matches('/');
    let derived = below(path, base).is_some_and(|s| {
        models_route(method, &s)
            || embeddings_route(method, &s)
            || (dialects.contains(&Dialect::OpenaiChat) && chat_route(method, &s))
            || (dialects.contains(&Dialect::OpenaiResponses) && responses_route(method, &s))
            || (dialects.contains(&Dialect::AnthropicMessages) && messages_route(method, &s))
    });
    let listing = *method == Method::GET
        && models_path
            .is_some_and(|m| clean(path) == clean(&format!("{base}{m}")) && clean(path).is_some());
    derived || listing || allowed_path_match(path, allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GET: Method = Method::GET;
    const POST: Method = Method::POST;
    const DELETE: Method = Method::DELETE;
    fn none() -> Vec<String> {
        vec![]
    }
    fn a(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn anthropic_inference_only() {
        for (m, p) in [
            (POST, "/v1/messages"),
            (POST, "/v1/messages/count_tokens"),
            (GET, "/v1/models"),
            (GET, "/v1/models/claude-x"),
            (POST, "/v1/messages?beta=true"),
        ] {
            assert!(anthropic_path_permitted(&m, p, &none()), "{m} {p}");
        }
        for (m, p) in [
            (POST, "/v1/files"),
            (GET, "/v1/files"),
            (POST, "/v1/messages/batches"),
            (GET, "/v1/messages/batches/x"),
            (POST, "/v1/skills"),
            (GET, "/v1/organizations/me"),
            (GET, "/v1/messages"),
            (POST, "/v1/models"),
            (POST, "/v1/chat/completions"),
        ] {
            assert!(!anthropic_path_permitted(&m, p, &none()), "{m} {p}");
        }
        assert!(anthropic_path_permitted(
            &GET,
            "/v1/files",
            &a(&["/v1/files"])
        ));
        assert!(!anthropic_path_permitted(
            &GET,
            "/v1/files/x",
            &a(&["/v1/files"])
        ));
        assert!(anthropic_path_permitted(
            &GET,
            "/v1/files/x",
            &a(&["/v1/files/*"])
        ));
    }

    #[test]
    fn openai_inference_only() {
        for (m, p) in [
            (POST, "/v1/chat/completions"),
            (POST, "/v1/completions"),
            (POST, "/v1/responses"),
            (POST, "/v1/embeddings"),
            (GET, "/v1/models"),
            (GET, "/v1/models/gpt-5"),
            (GET, "/v1/responses/resp_1"),
            (DELETE, "/v1/responses/resp_1"),
            (GET, "/v1/responses/resp_1/input_items"),
            (POST, "/v1/responses/resp_1/cancel"),
        ] {
            assert!(openai_path_permitted(&m, p, &none()), "{m} {p}");
        }
        for (m, p) in [
            (POST, "/v1/files"),
            (GET, "/v1/files"),
            (POST, "/v1/fine_tuning/jobs"),
            (POST, "/v1/images/generations"),
            (POST, "/v1/audio/speech"),
            (POST, "/v1/batches"),
            (GET, "/v1/organization/admin_api_keys"),
            (GET, "/v1/chat/completions"),
            (DELETE, "/v1/models/gpt-5"),
            (POST, "/v1/messages"),
            (GET, "/openai/v1/models"),
        ] {
            assert!(!openai_path_permitted(&m, p, &none()), "{m} {p}");
        }
        assert!(openai_path_permitted(
            &POST,
            "/v1/files",
            &a(&["/v1/files"])
        ));
    }

    #[test]
    fn azure_is_the_openai_set_under_openai_v1() {
        assert!(azure_path_permitted(
            &POST,
            "/openai/v1/chat/completions",
            &none()
        ));
        assert!(azure_path_permitted(&POST, "/openai/v1/responses", &none()));
        assert!(azure_path_permitted(&GET, "/openai/v1/models", &none()));
        assert!(!azure_path_permitted(
            &POST,
            "/v1/chat/completions",
            &none()
        ));
        assert!(!azure_path_permitted(
            &POST,
            "/openai/deployments/x/chat/completions",
            &none()
        ));
        assert!(!azure_path_permitted(&POST, "/openai/v1/files", &none()));
        assert!(!azure_path_permitted(&GET, "/openai/v1/../../x", &none()));
    }

    #[test]
    fn bedrock_runtime_routes_only() {
        for op in [
            "invoke",
            "invoke-with-response-stream",
            "converse",
            "converse-stream",
            "count-tokens",
        ] {
            assert!(
                bedrock_path_permitted(
                    &POST,
                    &format!("/model/eu.anthropic.claude-sonnet-4-5-v1%3A0/{op}"),
                    false
                ),
                "{op}"
            );
        }
        for p in [
            "/guardrail/g/version/1/apply",
            "/async-invoke",
            "/model/x/unknown",
            "/model//invoke",
            "/model/x/invoke/y",
            "/agents/x",
            "/model/../x/invoke",
            "/model/%2e%2e/invoke",
        ] {
            assert!(!bedrock_path_permitted(&POST, p, true), "{p}");
        }
        assert!(!bedrock_path_permitted(&GET, "/model/x/invoke", false));
        assert!(!bedrock_path_permitted(
            &POST,
            "/openai/v1/chat/completions",
            false
        ));
        assert!(bedrock_path_permitted(
            &POST,
            "/openai/v1/chat/completions",
            true
        ));
        assert!(bedrock_path_permitted(&POST, "/openai/v1/responses", true));
        assert!(!bedrock_path_permitted(
            &POST,
            "/openai/v1/embeddings",
            true
        ));
    }

    #[test]
    fn vertex_requires_the_endpoint_project_and_location() {
        let ok =
            |p: &str, openai| vertex_path_permitted(&POST, p, "my-proj-1", "europe-west1", openai);
        for op in [
            "rawPredict",
            "streamRawPredict",
            "generateContent",
            "streamGenerateContent",
            "countTokens",
        ] {
            assert!(
                ok(
                    &format!(
                        "/v1/projects/my-proj-1/locations/europe-west1/publishers/anthropic/models/claude-sonnet-4-5@20250929:{op}"
                    ),
                    false
                ),
                "{op}"
            );
        }
        assert!(ok(
            "/v1/projects/my-proj-1/locations/europe-west1/publishers/google/models/gemini-2.5-pro:generateContent",
            false
        ));
        // another project or location, another publisher, other operations
        assert!(!ok(
            "/v1/projects/other-proj/locations/europe-west1/publishers/anthropic/models/m:rawPredict",
            false
        ));
        assert!(!ok(
            "/v1/projects/my-proj-1/locations/us-central1/publishers/anthropic/models/m:rawPredict",
            false
        ));
        assert!(!ok(
            "/v1/projects/my-proj-1/locations/europe-west1/publishers/meta/models/m:rawPredict",
            false
        ));
        assert!(!ok(
            "/v1/projects/my-proj-1/locations/europe-west1/publishers/anthropic/models/m:tuneModel",
            false
        ));
        assert!(!ok(
            "/v1/projects/my-proj-1/locations/europe-west1/publishers/anthropic/models/m",
            false
        ));
        assert!(!ok(
            "/v1/projects/my-proj-1/locations/europe-west1/endpoints/x:predict",
            false
        ));
        assert!(!ok("/v1/projects/my-proj-1", false));
        // the OpenAI surface only when declared
        let oa = "/v1/projects/my-proj-1/locations/europe-west1/endpoints/openapi/chat/completions";
        assert!(!ok(oa, false));
        assert!(ok(oa, true));
        assert!(!vertex_path_permitted(
            &POST,
            "/v1/projects/p2/locations/europe-west1/endpoints/openapi/chat/completions",
            "my-proj-1",
            "europe-west1",
            true
        ));
        assert!(!vertex_path_permitted(
            &GET,
            oa,
            "my-proj-1",
            "europe-west1",
            true
        ));
    }

    #[test]
    fn compatible_is_derived_from_dialects_and_the_base_path() {
        let chat = [Dialect::OpenaiChat];
        let ok = |m: &Method, p: &str, base: &str, d: &[Dialect], mp: Option<&str>, al: &[&str]| {
            compatible_path_permitted(m, p, base, d, mp, &a(al))
        };
        assert!(ok(&POST, "/v1/chat/completions", "", &chat, None, &[]));
        assert!(ok(&POST, "/v1/completions", "", &chat, None, &[]));
        assert!(ok(&POST, "/v1/embeddings", "", &chat, None, &[]));
        assert!(ok(&GET, "/v1/models", "", &chat, None, &[]));
        // not declared: the responses and messages families
        assert!(!ok(&POST, "/v1/responses", "", &chat, None, &[]));
        assert!(!ok(&POST, "/v1/messages", "", &chat, None, &[]));
        let all = [
            Dialect::OpenaiChat,
            Dialect::OpenaiResponses,
            Dialect::AnthropicMessages,
        ];
        assert!(ok(&POST, "/v1/responses", "", &all, None, &[]));
        assert!(ok(&GET, "/v1/responses/r1", "", &all, None, &[]));
        assert!(ok(&POST, "/v1/messages/count_tokens", "", &all, None, &[]));
        // a base path prefix
        assert!(ok(
            &POST,
            "/llm/v1/chat/completions",
            "/llm",
            &chat,
            None,
            &[]
        ));
        assert!(ok(
            &POST,
            "/llm/v1/chat/completions",
            "/llm/",
            &chat,
            None,
            &[]
        ));
        assert!(!ok(&POST, "/v1/chat/completions", "/llm", &chat, None, &[]));
        assert!(!ok(
            &POST,
            "/llmx/v1/chat/completions",
            "/llm",
            &chat,
            None,
            &[]
        ));
        // other routes are refused; allowed_paths adds; `/*` opts out
        for p in [
            "/v1/files",
            "/v1/fine_tuning/jobs",
            "/key/generate",
            "/v1/images/generations",
        ] {
            assert!(!ok(&POST, p, "", &all, None, &[]), "{p}");
            assert!(ok(&POST, p, "", &all, None, &["/*"]), "{p}");
        }
        assert!(ok(&GET, "/health", "", &chat, None, &["/health"]));
        assert!(!ok(
            &GET,
            "/health/../key/generate",
            "",
            &chat,
            None,
            &["/*"]
        ));
        // models_path
        assert!(ok(&GET, "/api/tags", "", &chat, Some("/api/tags"), &[]));
        assert!(!ok(&POST, "/api/tags", "", &chat, Some("/api/tags"), &[]));
        assert!(!ok(&GET, "/api/tags", "", &chat, None, &[]));
    }
}
