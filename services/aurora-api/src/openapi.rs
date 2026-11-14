//! OpenAPI document assembly and `GET /v1/openapi.json`.

use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

struct Security;

impl Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let c = openapi.components.get_or_insert_with(Default::default);
        c.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("aurora_session"))),
        );
        c.add_security_scheme(
            "api_key",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
        c.add_security_scheme(
            "agent_token",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Aurora API", version = env!("CARGO_PKG_VERSION")),
    components(schemas(crate::http::error::ErrorEnvelope, crate::http::error::ErrorBody)),
    modifiers(&Security)
)]
struct ApiDoc;

pub fn base() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

/// The complete generated document. The SCIM plane is not part of the document and the
/// router build with `None` simply skips wiring it, so no `AppState` is needed here.
pub fn document() -> utoipa::openapi::OpenApi {
    let mut doc = crate::app::api_router(None).1;
    add_common_errors(&mut doc);
    doc
}

/// (FIX-119, `API_CONTRACTS` §conventions) Every authenticated route can answer **401** (no or an invalid
/// credential) and **429** (the per-principal limiter), whatever its handler's own annotations say: the
/// document used to list 401 on a handful of operations. Added at assembly, per operation that declares a
/// security requirement, never overriding a response the handler already documents. The body is the common
/// error envelope.
fn add_common_errors(doc: &mut utoipa::openapi::OpenApi) {
    use utoipa::openapi::path::Operation;
    use utoipa::openapi::response::ResponseBuilder;
    use utoipa::openapi::{Content, Ref};

    fn add(op: &mut Operation) {
        if op.security.as_ref().is_none_or(Vec::is_empty) {
            return;
        }
        for (code, description) in [
            (
                "401",
                "Unauthenticated: no credential, or an invalid or expired one",
            ),
            ("429", "Too many requests: the per-principal rate limit"),
        ] {
            if !op.responses.responses.contains_key(code) {
                let response = ResponseBuilder::new()
                    .description(description)
                    .content(
                        "application/json",
                        Content::new(Some(Ref::from_schema_name("ErrorEnvelope"))),
                    )
                    .build();
                op.responses
                    .responses
                    .insert(code.to_owned(), response.into());
            }
        }
    }

    for item in doc.paths.paths.values_mut() {
        for op in [
            item.get.as_mut(),
            item.put.as_mut(),
            item.post.as_mut(),
            item.delete.as_mut(),
            item.patch.as_mut(),
            item.options.as_mut(),
            item.head.as_mut(),
            item.trace.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            add(op);
        }
    }
}

/// Pretty JSON with a trailing newline, as checked in at `web/openapi.json`.
pub fn document_json() -> String {
    let mut s = serde_json::to_string_pretty(&document()).unwrap_or_default();
    s.push('\n');
    s
}

pub async fn serve_document() -> axum::response::Response {
    use axum::response::IntoResponse;
    static DOC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let body = DOC.get_or_init(document_json).clone();
    ([(http::header::CONTENT_TYPE, "application/json")], body).into_response()
}
