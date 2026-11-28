use crate::ModelRef;
use aurora_policy::json::{Lookup, Node};
use http::Method;
use serde_json::Value;

const MODEL_PATHS: &[&str] = &[
    "/v1/messages/count_tokens",
    "/v1/messages",
    "/v1/chat/completions",
    "/v1/completions",
    "/v1/responses",
    "/v1/embeddings",
];

/// (D-177, SEC-65, FIX-105) Whether a request to a `litellm` / `openai_compatible` endpoint is
/// one the sanctioned key may be injected into: `POST` to one of the six inference routes
/// (also under a route prefix, as [`model_from_request`] reads them), `GET /v1/models[/{id}]`, or a
/// path the operator listed in the endpoint's `allowed_paths`. Everything else, notably a LiteLLM
/// management route (`/key/generate`, `/user/new`, `/team/*`), the files, images and fine-tuning
/// APIs, is refused. Query and fragment do not count; a path with a `..` or empty segment never
/// matches a listed prefix.
pub fn inference_path_permitted(method: &Method, path: &str, allowed_paths: &[String]) -> bool {
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let clean = path.split('/').skip(1).all(|s| !s.is_empty() && s != "..");
    if method == Method::POST && MODEL_PATHS.iter().any(|p| path.ends_with(p)) && clean {
        return true;
    }
    if method == Method::GET && clean {
        let models = path.strip_suffix("/v1/models").is_some()
            || path
                .rsplit_once("/v1/models/")
                .is_some_and(|(_, id)| !id.is_empty() && !id.contains('/'));
        if models {
            return true;
        }
    }
    clean
        && allowed_paths.iter().any(|a| match a.strip_suffix("/*") {
            Some(prefix) => path == prefix || path.starts_with(&format!("{prefix}/")),
            None => path == a.trim_end_matches('/'),
        })
}

/// Locate the requested model. POSTs to model-invoking paths (also under any route
/// prefix, matched by path suffix) carry it at JSON pointer `/model`; a truncated,
/// unparsable or model-less body yields `Unknown` (fail closed). Anything else is `NoModel`.
pub fn model_from_request(
    method: &Method,
    path: &str,
    body_prefix: &[u8],
    truncated: bool,
) -> ModelRef {
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    if method != Method::POST || !MODEL_PATHS.iter().any(|p| path.ends_with(p)) {
        return ModelRef::NoModel;
    }
    if truncated {
        return ModelRef::Unknown;
    }
    body_model(body_prefix)
}

/// The `model` member of a JSON body. The allow-list reads it here and the provider gets the
/// bytes, so a name with more than one reading is no model.
pub(crate) fn body_model(body: &[u8]) -> ModelRef {
    let Some(doc) = aurora_policy::json::parse(body) else {
        return ModelRef::Unknown;
    };
    match doc.get("model") {
        Lookup::Found(Node::Scalar(Value::String(m))) if !m.is_empty() => {
            ModelRef::Model(m.clone())
        }
        _ => ModelRef::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(m: &Method, p: &str, allowed: &[&str]) -> bool {
        let a: Vec<String> = allowed.iter().map(|s| (*s).to_owned()).collect();
        inference_path_permitted(m, p, &a)
    }

    /// The model allow-list reads the parsed body and the provider gets the bytes: a body that
    /// two parsers may read differently names no model.
    #[test]
    fn a_model_with_two_readings_is_unknown() {
        let read =
            |body: &str| model_from_request(&Method::POST, "/v1/messages", body.as_bytes(), false);
        assert_eq!(
            read(r#"{"model":"small"}"#),
            ModelRef::Model("small".into())
        );
        for body in [
            r#"{"model":"large","model":"small"}"#,
            r#"{"model":"small","Model":"large"}"#,
            r#"{"MODEL":"small"}"#,
            r#"{"$serde_json::private::RawValue":"{\"model\":\"small\"}"}"#,
        ] {
            assert_eq!(read(body), ModelRef::Unknown, "{body}");
        }
        // A repeat elsewhere says nothing about the model.
        assert_eq!(
            read(r#"{"model":"small","messages":[{"role":"a","role":"b"}]}"#),
            ModelRef::Model("small".into())
        );
    }

    #[test]
    fn inference_set_and_allowed_paths() {
        for p in [
            "/v1/messages",
            "/v1/messages/count_tokens",
            "/v1/chat/completions",
            "/v1/completions",
            "/v1/responses",
            "/v1/embeddings",
            "/openai/v1/chat/completions",
            "/v1/chat/completions/",
            "/v1/chat/completions?stream=1",
        ] {
            assert!(ok(&Method::POST, p, &[]), "POST {p}");
            assert!(!ok(&Method::GET, p, &[]), "GET {p}");
        }
        for p in ["/v1/models", "/v1/models/gpt-x", "/x/v1/models"] {
            assert!(ok(&Method::GET, p, &[]), "GET {p}");
            assert!(!ok(&Method::POST, p, &[]), "POST {p}");
        }
        for p in [
            "/key/generate",
            "/user/new",
            "/team/new",
            "/v1/images/generations",
            "/v1/files",
            "/v1/fine_tuning/jobs",
            "/v1/models/a/b",
            "/",
            "",
        ] {
            assert!(!ok(&Method::POST, p, &[]), "POST {p}");
            assert!(!ok(&Method::GET, p, &[]), "GET {p}");
        }
        // Operator-declared paths: exact, or a subtree with a trailing `/*`.
        assert!(ok(&Method::GET, "/health", &["/health"]));
        assert!(!ok(&Method::GET, "/health/x", &["/health"]));
        assert!(ok(&Method::GET, "/health/x/y", &["/health/*"]));
        assert!(ok(&Method::GET, "/health", &["/health/*"]));
        assert!(!ok(&Method::GET, "/healthz", &["/health/*"]));
        assert!(!ok(&Method::GET, "/health/../key/generate", &["/health/*"]));
        assert!(!ok(&Method::POST, "/key//generate", &["/key/*"]));
    }
}
