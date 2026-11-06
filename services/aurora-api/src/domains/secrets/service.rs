use super::dto::*;
use super::{gov, llm};
use crate::app::AppState;
use crate::http::auth::{Caller, require_own};
use crate::http::pagination::Pagination;
use aurora_core::{ApiError, FieldError, Secret as Plain};
use aurora_db::repo::{credentials, derived_rules, secrets as repo};
use aurora_db::{DbError, conflict_code};
use aurora_policy::{HostPattern, PathPattern};
use aurora_secrets::{Purpose, SealContext, SecretError, SecretRef, ValueSource};
use serde_json::Value;
use uuid::Uuid;

use repo::SecretRow;

const MAX_VALUE: usize = 16 * 1024;

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid secret.", vec![FieldError::new(path, msg)])
}

pub fn ctx(row: &SecretRow) -> SealContext {
    SealContext {
        purpose: Purpose::SecretValue,
        owner_id: row.id,
        org_id: row.org_id,
        workspace_id: row.workspace_id,
    }
}

pub fn secret_err(e: SecretError) -> ApiError {
    match e {
        SecretError::StoreUnavailable => ApiError::unavailable(
            "secret_store_unavailable",
            "The secret store is unavailable.",
        ),
        SecretError::ReferenceNotPermitted
        | SecretError::NotFound
        | SecretError::SecretRefMismatch => ApiError::unprocessable(
            "reference_not_permitted",
            "The referenced secret path is not permitted or does not exist.",
        ),
        SecretError::StoreNotConfigured
        | SecretError::ReferencesNotSupported
        | SecretError::ManagedNotSupported
        | SecretError::Malformed(_) => {
            bad("/reference", "unsupported store or malformed reference")
        }
        other => {
            tracing::error!(error = %other, "secret store error");
            ApiError::internal("Internal error.")
        }
    }
}

/// Writes need `can_own` (workspace) or owner/admin (organization) (I-27). A caller who cannot
/// see the scope gets 404; a visible caller with too low a role gets 403.
pub async fn authorize(
    caller: &Caller,
    state: &AppState,
    org: Uuid,
    ws: Option<Uuid>,
) -> Result<(), ApiError> {
    match ws {
        Some(ws) => require_own(&caller.workspace(state, ws).await?)?,
        None => caller.org(state, org).await?.require_admin()?,
    }
    Ok(())
}

/// Reads: like [`authorize`], except that organization-scope metadata is also readable by an
/// org member who is `can_own` on at least one workspace of the org (I-27 amendment: they
/// discover org secrets to grant them to their agents). Values are never returned.
pub async fn authorize_read(
    caller: &Caller,
    state: &AppState,
    org: Uuid,
    ws: Option<Uuid>,
) -> Result<(), ApiError> {
    match ws {
        Some(_) => authorize(caller, state, org, ws).await,
        None => caller.org_credential_reader(state, org).await.map(|_| ()),
    }
}

pub async fn load(state: &AppState, id: Uuid) -> Result<SecretRow, ApiError> {
    repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Secret not found."))
}

/// Load and authorize a write (`can_own` / owner-admin); invisible secrets are 404, visible but
/// insufficient role 403.
pub async fn load_authorized(
    caller: &Caller,
    state: &AppState,
    id: Uuid,
) -> Result<SecretRow, ApiError> {
    let row = load(state, id).await?;
    authorize(caller, state, row.org_id, row.workspace_id).await?;
    Ok(row)
}

/// Load and authorize a metadata read (see [`authorize_read`]).
pub async fn load_readable(
    caller: &Caller,
    state: &AppState,
    id: Uuid,
) -> Result<SecretRow, ApiError> {
    let row = load(state, id).await?;
    authorize_read(caller, state, row.org_id, row.workspace_id).await?;
    Ok(row)
}

pub async fn list(
    state: &AppState,
    org: Uuid,
    ws: Option<Uuid>,
    q: &ListQuery,
) -> Result<crate::http::pagination::Page<Secret>, ApiError> {
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    if q.kind
        .as_deref()
        .is_some_and(|k| k != "generic" && k != "llm")
    {
        return Err(bad("/kind", "must be generic or llm"));
    }
    if q.status
        .as_deref()
        .is_some_and(|s| !["active", "proposed", "rejected"].contains(&s))
    {
        return Err(bad("/status", "must be active, proposed or rejected"));
    }
    let rows = repo::list_scope(
        &state.pool,
        org,
        ws,
        q.kind.as_deref(),
        q.status.as_deref(),
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    let page = crate::http::pagination::Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(crate::http::pagination::Page {
        items: page.items.into_iter().map(Secret::from).collect(),
        next_cursor: page.next_cursor,
    })
}

fn clean_name(n: &str) -> Result<String, ApiError> {
    let t = n.trim();
    if !(1..=100).contains(&t.chars().count()) || t.chars().any(char::is_control) {
        return Err(bad(
            "/name",
            "must be 1-100 characters without control characters",
        ));
    }
    Ok(t.to_owned())
}

fn recipe_json(r: &InjectionRecipe) -> Result<Value, ApiError> {
    let v = serde_json::to_value(r).map_err(|_| ApiError::internal("Internal error."))?;
    let rec: aurora_secrets::InjectionRecipe =
        serde_json::from_value(v.clone()).map_err(|_| bad("/injection", "invalid recipe"))?;
    rec.validate().map_err(|m| bad("/injection", &m))?;
    Ok(v)
}

fn validate_generic_target(
    state: &AppState,
    host: &str,
    path: Option<&str>,
) -> Result<(), ApiError> {
    HostPattern::parse(host).map_err(|e| bad("/host_pattern", &e.to_string()))?;
    if let Some(p) = path {
        PathPattern::parse(p).map_err(|e| bad("/path_pattern", &format!("{e}")))?;
    }
    if gov::governed_hosts(&state.gov_instance)
        .iter()
        .any(|g| gov::patterns_overlap(host, g))
    {
        return Err(ApiError::unprocessable(
            "llm_host_requires_llm_key",
            "This host pattern can match an LLM provider host; create an LLM key instead.",
        ));
    }
    Ok(())
}

enum Input {
    Managed(String),
    Reference(SecretReference),
}

fn input(value: Option<String>, reference: Option<SecretReference>) -> Result<Input, ApiError> {
    match (value, reference) {
        (Some(v), None) => {
            if v.is_empty() || v.len() > MAX_VALUE {
                return Err(bad("/value", "must be 1 to 16384 bytes"));
            }
            Ok(Input::Managed(v))
        }
        (None, Some(r)) => Ok(Input::Reference(r)),
        _ => Err(bad("/value", "provide exactly one of value or reference")),
    }
}

/// A value ready to persist: the ref, whether we sealed it (to discard on failure), plaintext.
struct Prepared {
    vref: SecretRef,
    source: ValueSource,
}

/// Check a reference or capture a managed value (not yet sealed).
enum Staged {
    Managed(Plain<String>),
    Reference(SecretRef, Option<Plain<String>>),
}

async fn stage(
    state: &AppState,
    ctx: &SealContext,
    input: Input,
    need_plain: bool,
) -> Result<Staged, ApiError> {
    match input {
        Input::Managed(v) => Ok(Staged::Managed(Plain::new(v))),
        Input::Reference(r) => {
            let sref = state
                .vault
                .check_reference(ctx, &r.store_id, &r.path, r.field.as_deref())
                .await
                .map_err(secret_err)?;
            let plain = if need_plain {
                let bytes = state.vault.open(ctx, &sref).await.map_err(secret_err)?;
                let s = String::from_utf8(bytes.expose().clone())
                    .map_err(|_| bad("/reference", "referenced value is not valid UTF-8"))?;
                Some(Plain::new(s))
            } else {
                None
            };
            Ok(Staged::Reference(sref, plain))
        }
    }
}

fn plain_of(s: &Staged) -> Option<&Plain<String>> {
    match s {
        Staged::Managed(p) => Some(p),
        Staged::Reference(_, p) => p.as_ref(),
    }
}

async fn finalize(state: &AppState, ctx: &SealContext, s: Staged) -> Result<Prepared, ApiError> {
    match s {
        Staged::Managed(p) => {
            let vref = state
                .vault
                .seal(ctx, Plain::new(p.expose().clone().into_bytes()))
                .await
                .map_err(secret_err)?;
            Ok(Prepared {
                vref,
                source: ValueSource::Managed,
            })
        }
        Staged::Reference(r, _) => Ok(Prepared {
            vref: r,
            source: ValueSource::Reference,
        }),
    }
}

fn source_str(s: ValueSource) -> &'static str {
    match s {
        ValueSource::Managed => "managed",
        ValueSource::Reference => "reference",
    }
}

async fn discard(state: &AppState, ctx: &SealContext, r: &SecretRef, src: ValueSource) {
    if let Err(e) = state.vault.discard(ctx, r, src).await {
        tracing::warn!(error = %e, "discarding secret material failed");
    }
}

fn scope_invalidation(org: Uuid, ws: Option<Uuid>) -> (&'static str, Uuid) {
    match ws {
        Some(w) => ("workspace", w),
        None => ("org", org),
    }
}

/// LLM prerequisites: endpoint, provider, admission and the effective endpoint.
struct LlmCtx<'a> {
    ep: &'a aurora_llm::Endpoint,
    provider: &'a dyn aurora_llm::LlmProvider,
    eff: aurora_llm::governance::EffectiveEndpoint,
}

async fn llm_ctx<'a>(
    state: &'a AppState,
    org: Uuid,
    ws: Option<Uuid>,
    endpoint_id: &str,
    admit: bool,
) -> Result<LlmCtx<'a>, ApiError> {
    let ep = state
        .llm
        .endpoint(endpoint_id)
        .ok_or_else(|| bad("/llm/endpoint_id", "unknown endpoint"))?;
    let provider = state
        .llm
        .provider_for(ep)
        .ok_or_else(|| bad("/llm/endpoint_id", "provider not enabled"))?
        .as_ref();
    let eff = gov::effective_for(state, org).await?;
    if admit
        && !eff.key_permitted(
            endpoint_id,
            if ws.is_some() {
                "workspace"
            } else {
                "organization"
            },
        )
    {
        return Err(llm::not_permitted());
    }
    let eff_ep = eff
        .endpoints
        .get(endpoint_id)
        .cloned()
        .ok_or_else(llm::not_permitted)?;
    Ok(LlmCtx {
        ep,
        provider,
        eff: eff_ep,
    })
}

pub struct CreateScope {
    pub org: Uuid,
    pub ws: Option<Uuid>,
}

pub async fn create(
    state: &AppState,
    caller: &Caller,
    scope: CreateScope,
    req: CreateSecretRequest,
) -> Result<SecretRow, ApiError> {
    let name = clean_name(&req.name)?;
    let id = Uuid::now_v7();
    let ctx = SealContext {
        purpose: Purpose::SecretValue,
        owner_id: id,
        org_id: scope.org,
        workspace_id: scope.ws,
    };
    let inp = input(req.value, req.reference)?;
    let is_llm = match req.kind.as_str() {
        "generic" => false,
        "llm" => true,
        _ => return Err(bad("/kind", "must be generic or llm")),
    };

    let (host, path, injection, llm_cols, origin_json, staged);
    if is_llm {
        let spec = req
            .llm
            .ok_or_else(|| bad("/llm", "required for llm secrets"))?;
        if req.host_pattern.is_some() || req.path_pattern.is_some() || req.injection.is_some() {
            return Err(bad(
                "/host_pattern",
                "derived and read-only for llm secrets",
            ));
        }
        let mode = spec.auth_mode.as_deref().unwrap_or("api_key");
        if mode != "api_key" {
            return Err(bad("/llm/auth_mode", "unsupported auth mode"));
        }
        let lc = llm_ctx(state, scope.org, scope.ws, &spec.endpoint_id, true).await?;
        host = lc
            .provider
            .hosts(lc.ep)
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::internal("Internal error."))?;
        path = None;
        injection = llm_injection(lc.provider);
        let st = stage(state, &ctx, inp, true).await?;
        let key = plain_of(&st)
            .map(|p| Plain::new(p.expose().trim().to_owned()))
            .ok_or_else(|| ApiError::internal("Internal error."))?;
        let o = llm::check_key(&state.llm_http, lc.provider, lc.ep, &lc.eff, &key, 1)
            .await
            .map_err(llm::KeyFail::into_error)?;
        llm::require_compliant(&o)?;
        origin_json =
            Some(serde_json::to_value(&o).map_err(|_| ApiError::internal("Internal error."))?);
        llm_cols = Some((
            lc.provider.id().to_owned(),
            spec.endpoint_id,
            mode.to_owned(),
        ));
        staged = st;
    } else {
        if req.llm.is_some() {
            return Err(bad("/llm", "only valid for llm secrets"));
        }
        let h = req
            .host_pattern
            .ok_or_else(|| bad("/host_pattern", "required for generic secrets"))?;
        validate_generic_target(state, &h, req.path_pattern.as_deref())?;
        injection = match &req.injection {
            Some(r) => recipe_json(r)?,
            None => serde_json::json!({"kind": "bearer"}),
        };
        host = h;
        path = req.path_pattern;
        origin_json = None;
        llm_cols = None;
        staged = stage(state, &ctx, inp, false).await?;
    }

    let prepared = finalize(state, &ctx, staged).await?;
    let col = prepared.vref.to_column();
    let res = insert_tx(
        state,
        &repo::NewSecret {
            id,
            org_id: scope.org,
            workspace_id: scope.ws,
            name: &name,
            kind: &req.kind,
            llm_provider: llm_cols.as_ref().map(|c| c.0.as_str()),
            llm_auth_mode: llm_cols.as_ref().map(|c| c.2.as_str()),
            llm_endpoint_id: llm_cols.as_ref().map(|c| c.1.as_str()),
            host_pattern: &host,
            path_pattern: path.as_deref(),
            injection: &injection,
            value_source: source_str(prepared.source),
            value_ref: &col,
            status: "active",
            origin: origin_json.as_ref(),
            proposed_by_agent_id: None,
            created_by: Some(caller.user_id),
        },
    )
    .await;
    if let Err(e) = res {
        discard(state, &ctx, &prepared.vref, prepared.source).await;
        return Err(e);
    }
    load(state, id).await
}

async fn insert_tx(state: &AppState, n: &repo::NewSecret<'_>) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    repo::insert(&mut *tx, n)
        .await
        .map_err(|e| conflict_code(e, &[(repo::NAME_CONSTRAINT, "name_taken")]))?;
    let (s, i) = scope_invalidation(n.org_id, n.workspace_id);
    credentials::notify_gateway_invalidate(&mut *tx, s, i).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

fn llm_injection(p: &dyn aurora_llm::LlmProvider) -> Value {
    match p.injection(&aurora_llm::AuthMode::ApiKey).header {
        aurora_llm::InjectionHeader::Bearer => serde_json::json!({"kind": "bearer"}),
        aurora_llm::InjectionHeader::XApiKey => {
            serde_json::json!({"kind": "header", "name": "x-api-key", "format": "{value}"})
        }
    }
}

pub async fn patch(
    state: &AppState,
    row: &SecretRow,
    req: PatchSecretRequest,
) -> Result<SecretRow, ApiError> {
    if row.status != "active" {
        return Err(ApiError::conflict(
            "secret_not_active",
            "The secret is not active.",
        ));
    }
    let name = req.name.as_deref().map(clean_name).transpose()?;
    let mut injection = None;
    if row.kind == "llm" {
        if req.host_pattern.is_some() || req.path_pattern.is_some() || req.injection.is_some() {
            return Err(ApiError::unprocessable(
                "llm_secret_read_only",
                "Host, path and injection of LLM secrets are derived and read-only.",
            ));
        }
    } else {
        let host = req.host_pattern.as_deref().unwrap_or(&row.host_pattern);
        let path = match &req.path_pattern {
            Some(p) => p.as_deref(),
            None => row.path_pattern.as_deref(),
        };
        validate_generic_target(state, host, path)?;
        if let Some(r) = &req.injection {
            injection = Some(recipe_json(r)?);
        }
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    repo::update_meta(
        &mut *tx,
        row.id,
        name.as_deref(),
        req.host_pattern.as_deref(),
        req.path_pattern.as_ref().map(Option::as_deref),
        injection.as_ref(),
    )
    .await
    .map_err(|e| conflict_code(e, &[(repo::NAME_CONSTRAINT, "name_taken")]))?;
    let (s, i) = scope_invalidation(row.org_id, row.workspace_id);
    credentials::notify_gateway_invalidate(&mut *tx, s, i).await?;
    tx.commit().await.map_err(DbError::from)?;
    load(state, row.id).await
}

pub async fn delete(state: &AppState, row: &SecretRow) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    derived_rules::delete_for_secret_grants(&mut *tx, row.id).await?;
    repo::delete(&mut *tx, row.id).await?;
    let (s, i) = scope_invalidation(row.org_id, row.workspace_id);
    credentials::notify_gateway_invalidate(&mut *tx, s, i).await?;
    tx.commit().await.map_err(DbError::from)?;
    if let Ok(r) = row.value_ref.parse::<SecretRef>() {
        let src = if row.value_source == "reference" {
            ValueSource::Reference
        } else {
            ValueSource::Managed
        };
        discard(state, &ctx(row), &r, src).await;
    }
    Ok(())
}

pub async fn set_value(
    state: &AppState,
    row: &SecretRow,
    req: SetValueRequest,
) -> Result<SecretRow, ApiError> {
    if row.status != "active" {
        return Err(ApiError::conflict(
            "secret_not_active",
            "The secret is not active.",
        ));
    }
    let c = ctx(row);
    let inp = input(req.value, req.reference)?;
    let is_llm = row.kind == "llm";
    let new_version = i64::from(row.value_version) + 1;
    let mut origin_json = None;
    let staged = stage(state, &c, inp, is_llm).await?;
    if is_llm {
        let endpoint = row.llm_endpoint_id.as_deref().unwrap_or_default();
        let lc = llm_ctx(state, row.org_id, row.workspace_id, endpoint, true).await?;
        let key = plain_of(&staged)
            .map(|p| Plain::new(p.expose().trim().to_owned()))
            .ok_or_else(|| ApiError::internal("Internal error."))?;
        let o = llm::check_key(
            &state.llm_http,
            lc.provider,
            lc.ep,
            &lc.eff,
            &key,
            new_version,
        )
        .await
        .map_err(llm::KeyFail::into_error)?;
        llm::require_compliant(&o)?;
        origin_json =
            Some(serde_json::to_value(&o).map_err(|_| ApiError::internal("Internal error."))?);
    }
    let prepared = finalize(state, &c, staged).await?;
    let col = prepared.vref.to_column();
    let swap = async {
        let mut tx = state.pool.begin().await.map_err(DbError::from)?;
        let ok = repo::replace_value(
            &mut *tx,
            row.id,
            &row.value_ref,
            row.value_version,
            &col,
            source_str(prepared.source),
            origin_json.as_ref(),
        )
        .await?;
        if !ok {
            return Ok::<bool, ApiError>(false);
        }
        let (s, i) = scope_invalidation(row.org_id, row.workspace_id);
        credentials::notify_gateway_invalidate(&mut *tx, s, i).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(true)
    };
    match swap.await {
        Ok(true) => {}
        Ok(false) => {
            discard(state, &c, &prepared.vref, prepared.source).await;
            return Err(ApiError::conflict(
                "conflict",
                "The secret changed concurrently; retry.",
            ));
        }
        Err(e) => {
            discard(state, &c, &prepared.vref, prepared.source).await;
            return Err(e);
        }
    }
    if let Ok(old) = row.value_ref.parse::<SecretRef>() {
        let src = if row.value_source == "reference" {
            ValueSource::Reference
        } else {
            ValueSource::Managed
        };
        discard(state, &c, &old, src).await;
    }
    load(state, row.id).await
}

/// Re-verify the stored key; persists mismatch/unverifiable results.
pub async fn verify(state: &AppState, row: &SecretRow) -> Result<StoredOrigin, ApiError> {
    if row.kind != "llm" || row.status != "active" {
        return Err(ApiError::unprocessable(
            "not_llm_secret",
            "Only active LLM secrets can be verified.",
        ));
    }
    let c = ctx(row);
    let key = open_key(state, row).await?;
    let lc = llm_ctx(
        state,
        row.org_id,
        row.workspace_id,
        row.llm_endpoint_id.as_deref().unwrap_or_default(),
        false,
    )
    .await?;
    let o = llm::check_key(
        &state.llm_http,
        lc.provider,
        lc.ep,
        &lc.eff,
        &key,
        i64::from(row.value_version),
    )
    .await
    .map_err(llm::KeyFail::into_error)?;
    let _ = c;
    let j = serde_json::to_value(&o).map_err(|_| ApiError::internal("Internal error."))?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    repo::set_origin(&mut *tx, row.id, &j).await?;
    let (s, i) = scope_invalidation(row.org_id, row.workspace_id);
    credentials::notify_gateway_invalidate(&mut *tx, s, i).await?;
    aurora_db::bus::notify(
        &mut *tx,
        aurora_db::bus::Channel::SecretOrigin,
        &serde_json::json!({"secret_id": row.id, "status": j["status"]}),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "origin notify failed");
        ApiError::internal("Internal error.")
    })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(o.into())
}

async fn open_key(state: &AppState, row: &SecretRow) -> Result<Plain<String>, ApiError> {
    let r: SecretRef = row
        .value_ref
        .parse()
        .map_err(|_| ApiError::internal("Internal error."))?;
    let bytes = state.vault.open(&ctx(row), &r).await.map_err(secret_err)?;
    let s = String::from_utf8(bytes.expose().clone())
        .map_err(|_| ApiError::internal("Internal error."))?;
    Ok(Plain::new(s.trim().to_owned()))
}

pub async fn models(state: &AppState, row: &SecretRow) -> Result<Models, ApiError> {
    if row.kind != "llm" || row.status != "active" {
        return Err(ApiError::unprocessable(
            "not_llm_secret",
            "Only active LLM secrets have models.",
        ));
    }
    let key = open_key(state, row).await?;
    let lc = llm_ctx(
        state,
        row.org_id,
        row.workspace_id,
        row.llm_endpoint_id.as_deref().unwrap_or_default(),
        false,
    )
    .await?;
    let list = lc
        .provider
        .list_models(lc.ep, &key, &state.llm_http)
        .await
        .map_err(|e| match e {
            aurora_llm::LlmError::InvalidKey => {
                ApiError::unprocessable("llm_key_invalid", "The provider rejected this API key.")
            }
            _ => ApiError::bad_gateway(
                "llm_endpoint_unreachable",
                "The LLM endpoint could not be reached.",
            ),
        })?;
    let allowed = &lc.eff.allowed_models;
    Ok(Models {
        items: list
            .into_iter()
            .filter(|m| allowed.is_empty() || allowed.contains(&m.id))
            .map(|m| ModelItem {
                id: m.id,
                display_name: m.display_name,
            })
            .collect(),
    })
}

pub async fn accept(
    state: &AppState,
    caller: &Caller,
    row: &SecretRow,
    value: String,
) -> Result<SecretRow, ApiError> {
    if row.status != "proposed" {
        return Err(ApiError::conflict(
            "not_proposed",
            "The secret is not a pending proposal.",
        ));
    }
    let Some(ws) = row.workspace_id else {
        return Err(ApiError::conflict(
            "not_proposed",
            "The secret is not a pending proposal.",
        ));
    };
    let Input::Managed(v) = input(Some(value), None)? else {
        return Err(bad("/value", "required"));
    };
    validate_generic_target(state, &row.host_pattern, row.path_pattern.as_deref())?;
    let c = ctx(row);
    let prepared = finalize(state, &c, Staged::Managed(Plain::new(v))).await?;
    let col = prepared.vref.to_column();
    let run = async {
        let mut tx = state.pool.begin().await.map_err(DbError::from)?;
        if !repo::activate(&mut *tx, row.id, &col, source_str(prepared.source)).await? {
            return Ok::<bool, ApiError>(false);
        }
        if let Some(agent) = row.proposed_by_agent_id {
            let g = aurora_db::repo::grants::upsert_secret_grant(
                &mut *tx,
                ws,
                agent,
                row.id,
                Some(caller.user_id),
            )
            .await?;
            crate::domains::grants::compile::compile_secret_grant(
                &mut tx, row.org_id, ws, g.id, agent, row.id, &row.name,
            )
            .await?;
            credentials::notify_gateway_invalidate(&mut *tx, "agent", agent).await?;
        }
        credentials::notify_gateway_invalidate(&mut *tx, "workspace", ws).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(true)
    };
    match run.await {
        Ok(true) => load(state, row.id).await,
        Ok(false) => {
            discard(state, &c, &prepared.vref, prepared.source).await;
            Err(ApiError::conflict(
                "not_proposed",
                "The secret is not a pending proposal.",
            ))
        }
        Err(e) => {
            discard(state, &c, &prepared.vref, prepared.source).await;
            Err(e)
        }
    }
}

pub async fn reject(state: &AppState, row: &SecretRow) -> Result<(), ApiError> {
    if !repo::reject(&state.pool, row.id).await? {
        return Err(ApiError::conflict(
            "not_proposed",
            "The secret is not a pending proposal.",
        ));
    }
    Ok(())
}
