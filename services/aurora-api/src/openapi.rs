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

/// The complete generated document.
pub fn document() -> utoipa::openapi::OpenApi {
    crate::app::api_router().1
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
