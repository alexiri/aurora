//! (P5-UX-14, D-335, C-943, SEC-129) Test: check one account now, in order of what it has.
//!
//! 1. A connection whose token is refreshed or minted is refreshed first (through the gateway: the credential is
//!    only ever used there), then the connect method's `probe` runs from the API with the account's credentials.
//! 2. A `database` app: a gateway login up to `ReadyForQuery`, then close — no statement is sent.
//! 3. A hook-minted token with nothing else to check: one mint.
//! 4. An AI key: the existing key verification (C-502).
//! 5. Anything else: `not_testable`.
//!
//! The outcome is a code, at most a host, an HTTP status or a protocol code: never an upstream body, header or
//! credential. The result is stored (except `not_testable`) and audited as `account.test`.

use super::dto::{AccountRefType, AccountTestAnswer, AccountTestDetail, AccountTestResult};
use super::gateway::{Answer, Check};
use crate::app::AppState;
use crate::audit::{self, AuditEvent};
use crate::domains::connect::probe::{self, ProbeCause, Vars};
use crate::domains::connections::service as connections;
use crate::domains::plugins::registry;
use crate::domains::secrets::service as secrets;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::repo::app_instances;
use aurora_db::repo::connections::{self as conn_repo, ConnectionRow};
use aurora_db::repo::secrets as secret_repo;
use aurora_registry::manifest::{Connect, TokenKind};
use aurora_registry::snapshot::{AppEntry, Snapshot};
use aurora_secrets::{Purpose, SealContext, SecretRef};
use chrono::Utc;
use serde_json::{Map, Value, json};
use uuid::Uuid;

/// The outcome of one check, before it is stored.
struct Checked {
    result: AccountTestResult,
    detail: Option<AccountTestDetail>,
}

impl Checked {
    fn of(result: AccountTestResult) -> Self {
        Self {
            result,
            detail: None,
        }
    }

    fn with(result: AccountTestResult, detail: AccountTestDetail) -> Self {
        Self {
            result,
            detail: Some(detail),
        }
    }
}

impl From<Answer> for Checked {
    fn from(a: Answer) -> Self {
        let has_detail = a.host.is_some() || a.code.is_some();
        let detail = has_detail.then_some(AccountTestDetail {
            host: a.host,
            status: None,
            code: a.code,
        });
        Self {
            result: AccountTestResult::parse(&a.result),
            detail,
        }
    }
}

/// `POST /accounts/{type}/{id}/test`.
pub async fn test(
    state: &AppState,
    caller: &Caller,
    kind: AccountRefType,
    id: Uuid,
) -> Result<AccountTestAnswer, ApiError> {
    let (out, org, ws, target, meta) = match kind {
        AccountRefType::Connection => {
            let (row, access) = connections::visible(caller, state, id).await?;
            access.require_write()?;
            let out = test_connection(state, &row).await?;
            let meta = json!({"app_id": row.app_id, "label": row.label});
            (out, row.org_id, row.workspace_id, "connection", meta)
        }
        AccountRefType::Secret => {
            let row = secrets::load_authorized(caller, state, id).await?;
            let out = test_secret(state, &row).await;
            let meta = json!({"name": row.name, "kind": row.kind});
            (out, row.org_id, row.workspace_id, "secret", meta)
        }
    };
    let now = Utc::now();
    if let Some(stored) = out.result.stored() {
        match kind {
            AccountRefType::Connection => conn_repo::set_check(&state.pool, id, stored).await?,
            AccountRefType::Secret => secret_repo::set_check(&state.pool, id, stored).await?,
        }
    }
    let mut ev = AuditEvent::by(caller, "account.test")
        .org(org)
        .target(target, id)
        .metadata(json!({ "result": out.result, "account": meta }));
    if let Some(ws) = ws {
        ev = ev.workspace(ws);
    }
    audit::record(state, ev);
    Ok(AccountTestAnswer {
        result: out.result,
        detail: out.detail,
        checked_at: now,
    })
}

async fn test_secret(state: &AppState, row: &secret_repo::SecretRow) -> Checked {
    if row.kind != "llm" || row.status != "active" {
        return Checked::of(AccountTestResult::NotTestable);
    }
    match secrets::verify(state, row).await {
        Ok(_) => Checked::of(AccountTestResult::Ok),
        // `llm_key_invalid`: the provider refused the key; `llm_endpoint_unreachable`: nothing answered.
        Err(e) => match e.code() {
            "llm_key_invalid" => Checked::of(AccountTestResult::Rejected),
            "llm_endpoint_unreachable" => Checked::of(AccountTestResult::Unreachable),
            _ => Checked::of(AccountTestResult::Unexpected),
        },
    }
}

async fn test_connection(state: &AppState, row: &ConnectionRow) -> Result<Checked, ApiError> {
    if row.status != "active" {
        // A turned-off account runs nothing; an expired sign-in is already known and is fixed by Reconnect.
        return Ok(Checked::of(AccountTestResult::NotTestable));
    }
    let Some(snap) = registry::active_typed(state).await? else {
        return Ok(Checked::of(AccountTestResult::NotTestable));
    };
    let Some(app) = snap.app(&row.app_id) else {
        return Ok(Checked::of(AccountTestResult::NotTestable));
    };
    let method = row.connect_method.as_deref();
    let probe = probe_of(app, method);
    let token_kind = app.token_for(method).map(|t| t.kind);
    let minted = token_kind == Some(TokenKind::Hook);
    let rotating = matches!(token_kind, Some(TokenKind::Hook | TokenKind::Oauth2Refresh));

    // A token that is refreshed or minted is refreshed first: a probe must not judge a stale one.
    if rotating {
        let check = if probe.is_none() && minted && app.database.is_none() {
            Check::Mint
        } else {
            Check::Fresh
        };
        let a = state.account_gateway.check(row.org_id, row.id, check).await;
        if a.result != "ok" {
            return Ok(a.into());
        }
        if check == Check::Mint {
            return Ok(Checked::of(AccountTestResult::Ok));
        }
    }
    if let Some(p) = probe {
        return Ok(run_probe(state, &snap, app, row, p).await);
    }
    if app.database.is_some() {
        let a = state
            .account_gateway
            .check(row.org_id, row.id, Check::Database)
            .await;
        return Ok(a.into());
    }
    Ok(Checked::of(AccountTestResult::NotTestable))
}

/// The connect method's probe, with the connection's own credentials. A 1 MiB-capped read; the answer carries the
/// status and the host, never the body.
async fn run_probe(
    state: &AppState,
    snap: &Snapshot,
    app: &AppEntry,
    row: &ConnectionRow,
    p: &aurora_registry::manifest::Probe,
) -> Checked {
    let Some(creds) = open_credentials(state, row).await else {
        return Checked::of(AccountTestResult::Unexpected);
    };
    let metadata = row.metadata.as_object().cloned().unwrap_or_default();
    let fields = Map::new();
    let instance_host = row.instance_host.as_deref().or_else(|| {
        app.instances
            .as_ref()
            .and_then(|i| i.default_host.as_deref())
    });
    // (FIX-290, D-511) The registration as it is now: a removed or disabled one opens nothing.
    let registration = match row.instance_id {
        Some(id) => app_instances::get(&state.pool, id).await.ok().flatten(),
        None => None,
    };
    let vars = Vars {
        credentials: &creds,
        metadata: &metadata,
        tenant_host: row.tenant_host.as_deref(),
        fields: &fields,
        instance_host,
        instance_base_path: row.instance_base_path.as_deref(),
        instance_registered: row.instance_id.is_some(),
        app_host: registration
            .as_ref()
            .and_then(|i| state.connect.self_hosted(i)),
    };
    match probe::run(&state.connect, snap, app, p, &vars).await {
        Ok(_) => Checked::of(AccountTestResult::Ok),
        Err(f) => {
            let result = match f.cause {
                ProbeCause::Rejected => AccountTestResult::Rejected,
                ProbeCause::Unreachable => AccountTestResult::Unreachable,
                ProbeCause::Unexpected => AccountTestResult::Unexpected,
            };
            Checked::with(
                result,
                AccountTestDetail {
                    host: f.host,
                    status: f.status,
                    code: None,
                },
            )
        }
    }
}

/// The sealed credentials of a connection as a JSON object; `None` when they cannot be read.
async fn open_credentials(state: &AppState, row: &ConnectionRow) -> Option<Map<String, Value>> {
    let (cref, _) = conn_repo::credentials_state(&state.pool, row.id)
        .await
        .ok()??;
    let sref: SecretRef = cref.parse().ok()?;
    let ctx = SealContext {
        purpose: Purpose::ConnectionCredentials,
        owner_id: row.id,
        org_id: row.org_id,
        workspace_id: row.workspace_id,
    };
    let bytes = state.vault.open(&ctx, &sref).await.ok()?;
    match serde_json::from_slice::<Value>(bytes.expose()).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// The host an unreachable account could not be reached at, for its health line ("Could not be reached (<host>)").
/// A tenant or instance host when the connection has one, else the `host` the sealed credentials hold (a
/// database); `None` when neither is known. Read only for the rare unreachable row.
pub async fn unreachable_host(state: &AppState, id: Uuid) -> Option<String> {
    let row = conn_repo::get(&state.pool, id).await.ok()??;
    if let Some(h) = row
        .tenant_host
        .clone()
        .or_else(|| row.instance_host.clone())
    {
        return Some(h);
    }
    if let Some(h) = open_credentials(state, &row)
        .await
        .and_then(|c| c.get("host").and_then(Value::as_str).map(str::to_owned))
    {
        return Some(h);
    }
    // Else the host the connect method's probe asks (`https://api.example.com/me`).
    let snap = registry::active_typed(state).await.ok()??;
    let app = snap.app(&row.app_id)?;
    let probe = probe_of(app, row.connect_method.as_deref())?;
    let url = probe::render_url(&probe.url, row.tenant_host.as_deref())?;
    reqwest::Url::parse(&url)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

/// The probe of the connect method the connection was made with (any method with a probe when none is recorded).
fn probe_of<'a>(
    app: &'a AppEntry,
    method: Option<&str>,
) -> Option<&'a aurora_registry::manifest::Probe> {
    app.connect
        .iter()
        .filter(|c| method.is_none_or(|m| c.id() == m))
        .find_map(|c| match c {
            Connect::Oauth2(o) => o.probe.as_ref(),
            Connect::Fields(f) => f.probe.as_ref(),
            Connect::Hook(_) => None,
        })
}
