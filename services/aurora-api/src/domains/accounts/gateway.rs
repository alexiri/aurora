//! The API's side of `POST /internal/v1/account-test` (P5-UX-14, C-943, SEC-129): the checks only the gateway
//! can make, because the credential is used only there (a gateway login to a database up to `ReadyForQuery`, a
//! token mint of a hook-minted token, a refresh). The route sits on the internal listener the API already uses
//! for the gateway (`gateway_internal_url`), authenticated with the same internal secret. A trait so
//! tests can stand in for the gateway.

use async_trait::async_trait;
use aurora_core::telemetry::InternalRequest;
use serde_json::{Value, json};
use uuid::Uuid;

/// What the gateway is asked to check on an app connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// A login to the connection's database up to `ReadyForQuery`, then close; no statement.
    Database,
    /// One mint of a hook-minted token.
    Mint,
    /// Refresh a token that is expiring, so a probe does not judge a stale one.
    Fresh,
}

impl Check {
    fn as_str(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Mint => "mint",
            Self::Fresh => "fresh",
        }
    }
}

/// The gateway's answer: a result code, at most a host, a status or a protocol code (never upstream content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// `ok` | `rejected` | `unreachable` | `unexpected` | `not_testable`.
    pub result: String,
    pub host: Option<String>,
    pub code: Option<String>,
}

impl Answer {
    pub fn unexpected() -> Self {
        Self {
            result: "unexpected".into(),
            host: None,
            code: None,
        }
    }
}

#[async_trait]
pub trait AccountGateway: Send + Sync {
    async fn check(&self, org_id: Uuid, connection_id: Uuid, check: Check) -> Answer;
}

/// The production client: HTTP to the gateway's internal listener with the internal secret.
pub struct HttpAccountGateway {
    base: String,
    bearer: String,
    key_id: String,
    http: reqwest::Client,
}

impl HttpAccountGateway {
    pub fn new(
        base: &str,
        ring: &aurora_crypto::SecretRing,
        extra_ca_bundle: Option<&str>,
    ) -> Result<Self, String> {
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            bearer: ring.active_bearer(),
            key_id: ring.active_id().to_owned(),
            http: aurora_secrets::tls::with_extra_roots(
                reqwest::Client::builder()
                    // The gateway's own cap is 20 s; a little more for the hop.
                    .timeout(std::time::Duration::from_secs(25))
                    .redirect(reqwest::redirect::Policy::none()),
                extra_ca_bundle,
            )?
            .build()
            .map_err(|e| format!("account gateway client: {e}"))?,
        })
    }
}

#[async_trait]
impl AccountGateway for HttpAccountGateway {
    async fn check(&self, org_id: Uuid, connection_id: Uuid, check: Check) -> Answer {
        let body =
            json!({ "org_id": org_id, "connection_id": connection_id, "check": check.as_str() });
        let Ok(resp) = self
            .http
            .post(format!("{}/internal/v1/account-test", self.base))
            .bearer_auth(&self.bearer)
            .header(aurora_core::limits::KEY_ID_HEADER, &self.key_id)
            .json(&body)
            .send_internal("aurora-gateway")
            .await
        else {
            return Answer::unexpected();
        };
        let Ok(v) = resp.json::<Value>().await else {
            return Answer::unexpected();
        };
        let text = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_owned);
        match text("/result") {
            Some(result)
                if [
                    "ok",
                    "rejected",
                    "unreachable",
                    "unexpected",
                    "not_testable",
                ]
                .contains(&result.as_str()) =>
            {
                Answer {
                    result,
                    host: text("/detail/host"),
                    code: text("/detail/code"),
                }
            }
            _ => Answer::unexpected(),
        }
    }
}
