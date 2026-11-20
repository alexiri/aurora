//! (P5-REFINE-6, C-1070, D-407) Test a sign-in app (an app's OAuth client): one authorization-code
//! request with a made-up code to the provider's token endpoint. A provider that knows the client
//! refuses the code (`invalid_grant`); one that does not refuses the client (`invalid_client`).
//! Nothing of the answer is returned or stored but the result, the host and the HTTP status.

use super::dto::{OAuthClientTestResult, TestDetail, TestResult};
use super::routes::{actor, writable};
use crate::app::AppState;
use crate::audit::{self, AuditEvent, Outcome};
use crate::domains::connect::service::{TokenProbe, probe_token_endpoint};
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use aurora_db::repo::oauth_clients as repo;
use axum::extract::State;
use serde_json::json;
use uuid::Uuid;

/// How the provider's answer maps to a result (C-1070). `invalid_grant` means the client was
/// accepted and the made-up code refused; GitHub answers the same with `bad_verification_code`
/// (HTTP 200) and a wrong client with `incorrect_client_credentials`.
#[must_use]
pub fn classify(status: u16, error: Option<&str>) -> TestResult {
    match (status, error) {
        (200 | 400 | 401, Some("invalid_grant" | "bad_verification_code")) => TestResult::Ok,
        (
            200 | 400 | 401,
            Some("invalid_client" | "unauthorized_client" | "incorrect_client_credentials"),
        ) => TestResult::Rejected,
        _ => TestResult::Unexpected,
    }
}

/// Test a sign-in app against its provider (the callers who may `PATCH` it).
#[utoipa::path(
    post, path = "/oauth-clients/{id}/test", operation_id = "testOauthClient", tag = "oauth-clients",
    params(("id" = Uuid, Path, description = "OAuth client id")),
    responses((status = 200, description = "How the provider answered", body = OAuthClientTestResult),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 429, description = "Too many tests", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
pub(super) async fn test_client(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<OAuthClientTestResult>, ApiError> {
    let row = writable(&caller, &state, id).await?;
    let (result, detail) = if row.enabled {
        match probe_token_endpoint(&state, &row).await? {
            TokenProbe::NotTestable => (TestResult::NotTestable, None),
            TokenProbe::Unreachable { host } => (
                TestResult::Unreachable,
                Some(TestDetail { host, status: None }),
            ),
            TokenProbe::Answered {
                host,
                status,
                error,
            } => (
                classify(status, error.as_deref()),
                Some(TestDetail {
                    host,
                    status: Some(status),
                }),
            ),
        }
    } else {
        (TestResult::NotTestable, None)
    };
    let mut tx = audit::begin(&state).await?;
    // `not_testable` is answered, never stored.
    let checked_at = if result == TestResult::NotTestable {
        chrono::Utc::now()
    } else {
        repo::record_test(&mut *tx, row.id, result.as_str()).await?
    };
    let mut ev = AuditEvent::new(actor(&caller, &row), "oauth_client.test", Outcome::Success)
        .target("oauth_client", row.id)
        .metadata(json!({
            "result": result.as_str(),
            "package_id": row.package_id,
            "client_key": row.client_key,
        }));
    if let Some(o) = row.org_id {
        ev = ev.org(o);
    }
    if let Some(w) = row.workspace_id {
        ev = ev.workspace(w);
    }
    audit::commit_critical(tx, &ev).await?;
    Ok(Json(OAuthClientTestResult {
        result,
        checked_at,
        detail,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_providers_answers_map_to_results() {
        assert_eq!(classify(400, Some("invalid_grant")), TestResult::Ok);
        assert_eq!(classify(401, Some("invalid_grant")), TestResult::Ok);
        assert_eq!(classify(200, Some("bad_verification_code")), TestResult::Ok);
        assert_eq!(classify(401, Some("invalid_client")), TestResult::Rejected);
        assert_eq!(
            classify(400, Some("unauthorized_client")),
            TestResult::Rejected
        );
        assert_eq!(
            classify(200, Some("incorrect_client_credentials")),
            TestResult::Rejected
        );
        assert_eq!(classify(500, Some("invalid_grant")), TestResult::Unexpected);
        assert_eq!(classify(400, Some("invalid_request")), TestResult::Unexpected);
        assert_eq!(classify(200, None), TestResult::Unexpected);
        assert_eq!(classify(404, None), TestResult::Unexpected);
    }
}
