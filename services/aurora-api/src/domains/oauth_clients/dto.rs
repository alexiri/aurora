use aurora_db::repo::oauth_clients::ClientRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// An OAuth client. `fields` carries non-secret fields only; secret values are sealed.
#[derive(Debug, Serialize, ToSchema)]
pub struct OAuthClient {
    pub id: Uuid,
    /// `instance` | `organization` | `workspace`.
    pub scope: String,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub package_id: String,
    pub client_key: String,
    pub fields: BTreeMap<String, String>,
    pub enabled: bool,
    /// The self-hosted app instance this client is bound to (D-74); `null` = unbound.
    pub instance_id: Option<Uuid>,
    pub connection_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// (P5-REFINE-6, C-1070) The last Test; `null` until tested.
    pub test: Option<OAuthClientLastTest>,
    /// (C-1070) `disabled` when off; `ready` after an `ok` Test or once a connection was made
    /// with it; `test_failed` when the last Test failed and nothing connected since; else
    /// `ready_not_tested`.
    pub state: OAuthClientState,
}

/// (C-1070) How the provider answered a Test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestResult {
    /// The provider accepted the client and refused the made-up code.
    Ok,
    /// The client id or secret is wrong (`invalid_client`, `unauthorized_client`).
    Rejected,
    /// A network error, a timeout or the destination guard's refusal.
    Unreachable,
    /// Any other answer.
    Unexpected,
    /// The app's sign-in has no token endpoint to test, or the client is off. Never stored.
    NotTestable,
}

impl TestResult {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Rejected => "rejected",
            Self::Unreachable => "unreachable",
            Self::Unexpected => "unexpected",
            Self::NotTestable => "not_testable",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ok" => Self::Ok,
            "rejected" => Self::Rejected,
            "unreachable" => Self::Unreachable,
            "unexpected" => Self::Unexpected,
            _ => return None,
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OAuthClientLastTest {
    pub result: TestResult,
    pub at: DateTime<Utc>,
}

/// (C-1070) Where a sign-in app stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OAuthClientState {
    ReadyNotTested,
    Ready,
    TestFailed,
    Disabled,
}

/// (C-1070) The state rules, from the stored row.
#[must_use]
pub fn client_state(r: &ClientRow) -> OAuthClientState {
    if !r.enabled {
        return OAuthClientState::Disabled;
    }
    let last = r.last_test_result.as_deref().and_then(TestResult::parse);
    match (last, r.last_test_at) {
        (Some(TestResult::Ok), _) => OAuthClientState::Ready,
        (Some(_), Some(at)) => {
            // Verified by use after the failed Test.
            if r.last_connected_at.is_some_and(|c| c > at) {
                OAuthClientState::Ready
            } else {
                OAuthClientState::TestFailed
            }
        }
        _ if r.connection_count > 0 => OAuthClientState::Ready,
        _ => OAuthClientState::ReadyNotTested,
    }
}

/// (C-1070) `POST /v1/oauth-clients/{id}/test`. Nothing of the provider's answer but its host and
/// HTTP status.
#[derive(Debug, Serialize, ToSchema)]
pub struct OAuthClientTestResult {
    pub result: TestResult,
    pub checked_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<TestDetail>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TestDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl From<ClientRow> for OAuthClient {
    fn from(r: ClientRow) -> Self {
        let state = client_state(&r);
        let test = r
            .last_test_result
            .as_deref()
            .and_then(TestResult::parse)
            .zip(r.last_test_at)
            .map(|(result, at)| OAuthClientLastTest { result, at });
        let scope = match (r.org_id, r.workspace_id) {
            (None, _) => "instance",
            (Some(_), None) => "organization",
            (Some(_), Some(_)) => "workspace",
        };
        Self {
            id: r.id,
            scope: scope.into(),
            org_id: r.org_id,
            workspace_id: r.workspace_id,
            package_id: r.package_id,
            client_key: r.client_key,
            fields: r
                .fields
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
            enabled: r.enabled,
            instance_id: r.instance_id,
            connection_count: r.connection_count,
            created_at: r.created_at,
            updated_at: r.updated_at,
            test,
            state,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateOAuthClientRequest {
    pub package_id: String,
    pub client_key: String,
    /// Values for every field the manifest declares, secret ones included (sealed on write).
    pub fields: BTreeMap<String, String>,
    /// Bind the client to a self-hosted instance of the client's package visible to the scope
    /// (C-244, D-74).
    pub instance_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchOAuthClientRequest {
    /// Fields to replace; omitted fields keep their value.
    pub fields: Option<BTreeMap<String, String>>,
    pub enabled: Option<bool>,
}
