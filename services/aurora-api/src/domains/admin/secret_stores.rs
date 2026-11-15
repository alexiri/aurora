//! Admin › Secret stores (phase 5, FIX-124, D-195, D-236, D-237; C-746..C-749): the store
//! registry as an instance setting. Every configuration write is the settings service's
//! compare-and-set on the `secrets` section (locks, merged-view validation, critical audit,
//! re-authentication, e-mail, `aurora_settings` reload); this module adds the status view with
//! cached health and counts, Test connection, and the value-migration job.

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::settings::service::{self, Kind, SectionRequest, Via};
use crate::http::auth::InstanceAdmin;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::secrets_migrate::{self, Counts, Options};
use aurora_config::{Source, VaultSettings};
use aurora_core::{ApiError, limits};
use aurora_secrets::{Purpose, SealContext, StoreHealth};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

const SECTION: &str = "secrets";
const STORES_KEY: &str = "secrets.stores";
const DEFAULT_KEY: &str = "secrets.default_store";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_status))
        .routes(routes!(put_store, delete_store))
        .routes(routes!(put_default))
        .routes(routes!(put_scopes))
        .routes(routes!(test_store))
        .routes(routes!(start_migration))
        .routes(routes!(current_migration))
}

// ------------------------------------------------------------------------------------- DTOs

/// A value and where it comes from.
#[derive(Serialize, ToSchema)]
pub struct ValueWithSource {
    pub value: String,
    /// `helm`, `dashboard` or `default`.
    pub source: String,
}

#[derive(Serialize, ToSchema)]
pub struct StoreHealthView {
    pub ok: bool,
    pub checked_at: DateTime<Utc>,
    /// `unavailable` (the store did not answer) or `init_failed` (it could not be built).
    pub error_code: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ReferenceScopeView {
    pub org_id: Uuid,
    pub org_slug: Option<String>,
    pub paths: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct StoreView {
    pub id: String,
    pub kind: String,
    /// `builtin` (inline), `helm` (locked) or `dashboard`.
    pub source: String,
    /// The Helm values key that locks it, for a Helm store.
    pub helm_key: Option<String>,
    /// The store's settings: no credential, a sealed one as `{ "$secret": { "set": true } }`.
    #[schema(value_type = Object)]
    pub settings: Value,
    /// Organizations may reference values in it.
    pub references: bool,
    pub default: bool,
    pub health: StoreHealthView,
    /// Why it could not be built, when it could not.
    pub error: Option<String>,
    pub managed_values: i64,
    pub reference_values: i64,
    pub reference_scopes: Vec<ReferenceScopeView>,
}

/// `GET /v1/admin/secret-stores` (C-746).
#[derive(Serialize, ToSchema)]
pub struct StoresStatus {
    pub default_store: ValueWithSource,
    /// The store new managed values go to right now (differs from `default_store` only while
    /// the configured default cannot be built).
    pub effective_default: String,
    /// Generation of the `secrets` settings section: send it back on every write.
    pub generation: i64,
    pub stores: Vec<StoreView>,
    pub migration: MigrationView,
}

#[derive(Deserialize, ToSchema)]
pub struct PutStore {
    /// The generation the page loaded.
    pub generation: i64,
    /// `kind`, `address`, `kv_mount`, `managed_prefix`, `auth`, `ca_pem`, `reference_scopes`:
    /// the entry without its `id`. A secret is `{ "$secret": "<value>" }`, or
    /// `{ "$secret": "keep" }` to keep the stored one.
    #[schema(value_type = Object)]
    pub store: Value,
}

#[derive(Deserialize, ToSchema)]
pub struct PutDefault {
    pub generation: i64,
    pub store_id: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ScopeInput {
    pub org_id: Uuid,
    pub paths: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PutScopes {
    pub generation: i64,
    pub scopes: Vec<ScopeInput>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct GenerationQuery {
    pub generation: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct TestStore {
    /// The store id (an existing store keeps its stored secrets for `{ "$secret": "keep" }`).
    pub id: String,
    #[schema(value_type = Object)]
    pub store: Value,
}

#[derive(Serialize, ToSchema)]
pub struct TestStep {
    /// `configuration`, `authenticate`, `write`, `read`, `delete`.
    pub name: String,
    pub ok: bool,
    pub error_code: Option<String>,
}

/// `POST /v1/admin/secret-stores/test` (C-746).
#[derive(Serialize, ToSchema)]
pub struct TestResult {
    pub ok: bool,
    pub steps: Vec<TestStep>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Deserialize, ToSchema)]
pub struct StartMigration {
    /// The store the managed values move to (managed, built, not `inline` unless moving back).
    pub target: String,
    /// Count what would move and write nothing.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct PurposeCounts {
    pub purpose: String,
    /// Moved (dry run: would move).
    pub moved: u64,
    pub skipped: u64,
    pub failed: u64,
    pub already: u64,
    /// Values to move, from the dry run taken at the start.
    pub total: u64,
}

/// A migration job, or the report of a dry run (C-747).
#[derive(Serialize, Clone, ToSchema)]
pub struct MigrationView {
    /// `none`, `running`, `succeeded`, `failed` or `dry_run`.
    pub state: String,
    pub target: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub started_by: Option<String>,
    pub per_purpose: Vec<PurposeCounts>,
    pub error: Option<String>,
    /// Another replica (or the CLI) holds the migration lock: progress is not visible here.
    pub running_elsewhere: bool,
}

impl MigrationView {
    fn none() -> Self {
        Self {
            state: "none".into(),
            target: None,
            started_at: None,
            finished_at: None,
            started_by: None,
            per_purpose: Vec::new(),
            error: None,
            running_elsewhere: false,
        }
    }
}

// ------------------------------------------------------------------- cached health and counts

struct Probed {
    at: Instant,
    checked_at: DateTime<Utc>,
    ok: bool,
}

/// `(managed, reference)` value counts per store id.
type StoreCounts = BTreeMap<String, (i64, i64)>;

#[derive(Default)]
struct HealthCache {
    /// Per store instance (a rebuilt registry has new instances, so a change re-probes).
    stores: HashMap<usize, Probed>,
    counts: Option<(Instant, StoreCounts)>,
}

fn caches() -> &'static tokio::sync::Mutex<HashMap<String, HealthCache>> {
    static C: OnceLock<tokio::sync::Mutex<HashMap<String, HealthCache>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// One cache per database (a process serving several, as the test harness does, never mixes).
fn db_key(state: &AppState) -> String {
    state
        .pool
        .connect_options()
        .get_database()
        .unwrap_or_default()
        .to_owned()
}

async fn probe_health(store: &Arc<dyn aurora_secrets::SecretStore>) -> bool {
    matches!(
        tokio::time::timeout(limits::SECRET_STORE_PROBE_TIMEOUT, store.health()).await,
        Ok(StoreHealth::Healthy)
    )
}

// ------------------------------------------------------------------------------- the status

fn redact_secrets(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("$secret") => json!({ "$secret": { "set": true } }),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, c)| (k.clone(), redact_secrets(c)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(redact_secrets).collect()),
        other => other.clone(),
    }
}

async fn org_slugs(state: &AppState, ids: &[Uuid]) -> HashMap<Uuid, String> {
    if ids.is_empty() {
        return HashMap::new();
    }
    sqlx::query_as::<_, (Uuid, String)>("SELECT id, slug FROM organizations WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(&state.pool)
        .await
        .map(|rows| rows.into_iter().collect())
        .unwrap_or_default()
}

fn source_name(s: Source) -> &'static str {
    match s {
        Source::Helm => "helm",
        Source::Dashboard => "dashboard",
        Source::Default => "default",
    }
}

async fn build_status(state: &AppState) -> Result<StoresStatus, ApiError> {
    let snap = state.settings.snapshot();
    let entries = snap
        .effective
        .keys
        .get(STORES_KEY)
        .and_then(|k| k.entries.clone())
        .unwrap_or_default();
    let default_key = snap.effective.keys.get(DEFAULT_KEY);
    let default_value = default_key
        .and_then(|k| k.value.as_str())
        .unwrap_or("inline")
        .to_owned();
    let registry = state.vault.registry();
    let failures = state.vault.failures();
    let helm_key = aurora_config::registry()
        .get(STORES_KEY)
        .map(|d| d.helm_key.clone());

    // Health and counts: at most one probe per store per interval, whatever the page loads.
    let mut guard = caches().lock().await;
    let cache = guard.entry(db_key(state)).or_default();
    let live: Vec<usize> = registry
        .stores()
        .map(|s| Arc::as_ptr(s).cast::<()>() as usize)
        .collect();
    cache.stores.retain(|k, _| live.contains(k));
    let mut health: HashMap<String, (bool, DateTime<Utc>)> = HashMap::new();
    for store in registry.stores() {
        let key = Arc::as_ptr(store).cast::<()>() as usize;
        let fresh = cache
            .stores
            .get(&key)
            .filter(|p| p.at.elapsed() < limits::SECRET_STORE_HEALTH_INTERVAL);
        let (ok, at) = match fresh {
            Some(p) => (p.ok, p.checked_at),
            None => {
                let ok = probe_health(store).await;
                let now = Utc::now();
                cache.stores.insert(
                    key,
                    Probed {
                        at: Instant::now(),
                        checked_at: now,
                        ok,
                    },
                );
                (ok, now)
            }
        };
        health.insert(store.id().to_owned(), (ok, at));
    }
    let counts = match cache
        .counts
        .as_ref()
        .filter(|(at, _)| at.elapsed() < limits::SECRET_STORE_HEALTH_INTERVAL)
    {
        Some((_, c)) => c.clone(),
        None => {
            let c = secrets_migrate::tally(&state.pool)
                .await
                .map_err(|e| ApiError::internal(format!("counting stored values: {e}")))?;
            cache.counts = Some((Instant::now(), c.clone()));
            c
        }
    };
    drop(guard);

    let mut org_ids = Vec::new();
    for e in &entries {
        for sc in e
            .value
            .get("reference_scopes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = sc
                .get("org_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                org_ids.push(id);
            }
        }
    }
    let slugs = org_slugs(state, &org_ids).await;
    let now = Utc::now();
    let count_of = |id: &str| counts.get(id).copied().unwrap_or((0, 0));

    let mut stores = Vec::new();
    let inline_counts = count_of("inline");
    stores.push(StoreView {
        id: "inline".into(),
        kind: "inline".into(),
        source: "builtin".into(),
        helm_key: None,
        settings: json!({}),
        references: false,
        default: registry.default_store_id() == "inline",
        health: StoreHealthView {
            ok: true,
            checked_at: now,
            error_code: None,
        },
        error: None,
        managed_values: inline_counts.0,
        reference_values: inline_counts.1,
        reference_scopes: Vec::new(),
    });
    for e in &entries {
        let built = registry.get(&e.id);
        let failure = failures.iter().find(|f| f.id == e.id);
        let (ok, checked_at, error_code) = match (&built, failure) {
            (Some(_), _) => match health.get(&e.id) {
                Some((true, at)) => (true, *at, None),
                Some((false, at)) => (false, *at, Some("unavailable".to_owned())),
                None => (false, now, Some("unavailable".to_owned())),
            },
            (None, _) => (false, now, Some("init_failed".to_owned())),
        };
        let c = count_of(&e.id);
        let scopes: Vec<ReferenceScopeView> = e
            .value
            .get("reference_scopes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|sc| {
                let org_id = sc
                    .get("org_id")
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok())?;
                Some(ReferenceScopeView {
                    org_id,
                    org_slug: slugs.get(&org_id).cloned(),
                    paths: sc
                        .get("paths")
                        .and_then(Value::as_array)
                        .map(|p| {
                            p.iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect();
        let mut settings = redact_secrets(&e.value);
        if let Some(m) = settings.as_object_mut() {
            m.remove("id");
            m.remove("kind");
            m.remove("reference_scopes");
        }
        stores.push(StoreView {
            id: e.id.clone(),
            kind: e
                .value
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            source: source_name(e.source).to_owned(),
            helm_key: (e.source == Source::Helm)
                .then(|| helm_key.clone())
                .flatten(),
            settings,
            references: built.as_ref().is_some_and(|s| s.capabilities().references),
            default: registry.default_store_id() == e.id,
            health: StoreHealthView {
                ok,
                checked_at,
                error_code,
            },
            error: failure.map(|f| f.error.clone()),
            managed_values: c.0,
            reference_values: c.1,
            reference_scopes: scopes,
        });
    }
    Ok(StoresStatus {
        default_store: ValueWithSource {
            value: default_value,
            source: default_key
                .map_or("default", |k| source_name(k.source))
                .to_owned(),
        },
        effective_default: registry.default_store_id().to_owned(),
        generation: snap.generation(SECTION),
        stores,
        migration: current_view(state).await,
    })
}

/// The store registry, its health and its counts (C-746).
#[utoipa::path(get, path = "/admin/secret-stores", operation_id = "adminListSecretStores",
    tag = "admin",
    responses((status = 200, body = StoresStatus), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_status(
    _c: InstanceAdmin,
    axum::extract::State(s): axum::extract::State<AppState>,
) -> Result<Json<StoresStatus>, ApiError> {
    Ok(Json(build_status(&s).await?))
}

// ----------------------------------------------------------------------------------- writes

/// The `secrets` section's dashboard values as a PUT would send them back (secrets as `keep`).
fn current_values(state: &AppState) -> Map<String, Value> {
    let snap = state.settings.snapshot();
    let stored: Map<String, Value> = snap
        .stored
        .sections
        .get(SECTION)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    service::dashboard_values(&state.settings, &stored, SECTION, "keep")
}

fn dashboard_entries(values: &Map<String, Value>) -> Vec<Value> {
    values
        .get(STORES_KEY)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn entry_id(e: &Value) -> &str {
    e.get("id").and_then(Value::as_str).unwrap_or_default()
}

async fn write_section(
    state: &AppState,
    c: InstanceAdmin,
    generation: i64,
    values: Map<String, Value>,
) -> Result<StoresStatus, ApiError> {
    service::apply(
        state,
        &Via::Admin(c.0),
        Kind::Update,
        vec![SectionRequest {
            section: SECTION.into(),
            expected: Some(generation),
            values,
        }],
        false,
    )
    .await?;
    build_status(state).await
}

fn not_found(id: &str) -> ApiError {
    ApiError::new(
        http::StatusCode::NOT_FOUND,
        "secret_store_not_found",
        format!("There is no secret store `{id}`."),
    )
}

fn helm_locked(id: &str) -> ApiError {
    let helm_key = aurora_config::registry()
        .get(STORES_KEY)
        .map(|d| d.helm_key.clone())
        .unwrap_or_default();
    ApiError::conflict(
        "setting_locked",
        format!("Store `{id}` is configured in Helm (`{helm_key}`) and cannot be changed here."),
    )
    .with_details(json!({ "key": STORES_KEY, "entry_id": id, "helm_key": helm_key }))
}

fn source_of(state: &AppState, id: &str) -> Option<Source> {
    state
        .settings
        .snapshot()
        .effective
        .keys
        .get(STORES_KEY)
        .and_then(|k| k.entries.as_ref())
        .and_then(|es| es.iter().find(|e| e.id == id).map(|e| e.source))
}

/// Add or replace a dashboard-defined store (C-746). Same validation as the configuration's
/// (`aurora-config`'s `VaultSettings`), run on the merged view by the settings service.
#[utoipa::path(put, path = "/admin/secret-stores/{id}", operation_id = "adminPutSecretStore",
    tag = "admin",
    params(("id" = String, Path, description = "Store id")),
    request_body = PutStore,
    responses((status = 200, body = StoresStatus), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 412, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn put_store(
    c: InstanceAdmin,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PutStore>,
) -> Result<Json<StoresStatus>, ApiError> {
    let Value::Object(mut entry) = req.store else {
        return Err(ApiError::unprocessable(
            "settings_invalid",
            "The store must be an object.",
        ));
    };
    entry.insert("id".into(), json!(id));
    // A Helm store's id taken by a dashboard entry is refused by the settings service
    // (422 `entry_id_in_helm`, C-773).
    let mut values = current_values(&s);
    let mut list = dashboard_entries(&values);
    match list.iter_mut().find(|e| entry_id(e) == id) {
        Some(slot) => *slot = Value::Object(entry),
        None => list.push(Value::Object(entry)),
    }
    values.insert(STORES_KEY.into(), Value::Array(list));
    Ok(Json(write_section(&s, c, req.generation, values).await?))
}

/// Remove a dashboard-defined store; refused while it still holds values or is the default.
#[utoipa::path(delete, path = "/admin/secret-stores/{id}", operation_id = "adminDeleteSecretStore",
    tag = "admin",
    params(("id" = String, Path, description = "Store id"), GenerationQuery),
    responses((status = 200, body = StoresStatus), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 412, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_store(
    c: InstanceAdmin,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<GenerationQuery>,
) -> Result<Json<StoresStatus>, ApiError> {
    match source_of(&s, &id) {
        None => return Err(not_found(&id)),
        Some(Source::Helm) => return Err(helm_locked(&id)),
        Some(_) => {}
    }
    let snap = s.settings.snapshot();
    let configured_default = snap
        .effective
        .keys
        .get(DEFAULT_KEY)
        .and_then(|k| k.value.as_str())
        .unwrap_or("inline");
    if configured_default == id {
        return Err(ApiError::conflict(
            "store_is_default",
            "This store is the default. Choose another default store first.",
        ));
    }
    // Counted fresh (never from the page's cache): this decides whether values would be lost.
    let counts = secrets_migrate::tally(&s.pool)
        .await
        .map_err(|e| ApiError::internal(format!("counting stored values: {e}")))?;
    let (managed, reference) = counts.get(&id).copied().unwrap_or((0, 0));
    // The page shows what this just found.
    caches().lock().await.entry(db_key(&s)).or_default().counts = Some((Instant::now(), counts));
    if managed + reference > 0 {
        return Err(ApiError::conflict(
            "store_not_empty",
            format!(
                "This store still holds {managed} managed and {reference} referenced value(s). Move them first."
            ),
        )
        .with_details(json!({ "managed_values": managed, "reference_values": reference })));
    }
    let mut values = current_values(&s);
    let list: Vec<Value> = dashboard_entries(&values)
        .into_iter()
        .filter(|e| entry_id(e) != id)
        .collect();
    if list.is_empty() {
        values.remove(STORES_KEY);
    } else {
        values.insert(STORES_KEY.into(), Value::Array(list));
    }
    Ok(Json(write_section(&s, c, q.generation, values).await?))
}

/// Choose where new managed values go (C-746).
#[utoipa::path(put, path = "/admin/secret-store-default", operation_id = "adminPutSecretStoreDefault",
    tag = "admin",
    request_body = PutDefault,
    responses((status = 200, body = StoresStatus), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 412, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn put_default(
    c: InstanceAdmin,
    axum::extract::State(s): axum::extract::State<AppState>,
    Json(req): Json<PutDefault>,
) -> Result<Json<StoresStatus>, ApiError> {
    let registry = s.vault.registry();
    let usable = registry
        .get(&req.store_id)
        .is_some_and(|st| st.capabilities().managed);
    if !usable {
        return Err(ApiError::unprocessable(
            "store_not_usable",
            "That store is not available, or does not hold managed values. Check it on this page first.",
        ));
    }
    let mut values = current_values(&s);
    values.insert(DEFAULT_KEY.into(), json!(req.store_id));
    Ok(Json(write_section(&s, c, req.generation, values).await?))
}

/// Replace a dashboard store's reference scopes (SEC-61): per organization, path globs relative
/// to the KV mount (a prefix glob ends in one `*`).
#[utoipa::path(put, path = "/admin/secret-stores/{id}/reference-scopes",
    operation_id = "adminPutSecretStoreScopes", tag = "admin",
    params(("id" = String, Path, description = "Store id")),
    request_body = PutScopes,
    responses((status = 200, body = StoresStatus), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 412, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn put_scopes(
    c: InstanceAdmin,
    axum::extract::State(s): axum::extract::State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PutScopes>,
) -> Result<Json<StoresStatus>, ApiError> {
    match source_of(&s, &id) {
        None => return Err(not_found(&id)),
        Some(Source::Helm) => return Err(helm_locked(&id)),
        Some(_) => {}
    }
    let ids: Vec<Uuid> = req.scopes.iter().map(|x| x.org_id).collect();
    let known = org_slugs(&s, &ids).await;
    if let Some(missing) = ids.iter().find(|i| !known.contains_key(i)) {
        return Err(ApiError::unprocessable(
            "org_not_found",
            format!("There is no organization with id {missing}."),
        ));
    }
    let mut values = current_values(&s);
    let mut list = dashboard_entries(&values);
    let Some(slot) = list.iter_mut().find(|e| entry_id(e) == id) else {
        return Err(not_found(&id));
    };
    if let Some(m) = slot.as_object_mut() {
        let scopes: Vec<Value> = req
            .scopes
            .iter()
            .map(|x| json!({ "org_id": x.org_id.to_string(), "paths": x.paths }))
            .collect();
        if scopes.is_empty() {
            m.remove("reference_scopes");
        } else {
            m.insert("reference_scopes".into(), Value::Array(scopes));
        }
    }
    values.insert(STORES_KEY.into(), Value::Array(list));
    Ok(Json(write_section(&s, c, req.generation, values).await?))
}

// -------------------------------------------------------------------------- Test connection

fn error_code(e: &aurora_secrets::SecretError) -> &'static str {
    use aurora_secrets::SecretError as E;
    match e {
        E::StoreUnavailable => "unreachable",
        E::InvalidConfig(_) | E::StoreNotConfigured => "invalid_config",
        E::SecretRefMismatch | E::Malformed(_) => "unexpected_value",
        E::NotFound => "not_found",
        E::ManagedNotSupported | E::ReferencesNotSupported | E::ReferenceNotPermitted => {
            "not_permitted"
        }
        E::Internal => "internal",
    }
}

/// Replace the `$secret` words of `entry` before building a store to test: `keep` takes the
/// stored sealed value of the same store, a string is sealed in memory for this test only.
fn resolve_for_test(
    keyring: &Arc<aurora_crypto::Keyring>,
    entry: &mut Value,
    stored: Option<&Value>,
    path: &mut Vec<String>,
) -> Result<(), &'static str> {
    match entry {
        Value::Object(m) if m.len() == 1 && m.contains_key("$secret") => {
            let word = m["$secret"].as_str().map(str::to_owned);
            match word.as_deref() {
                Some("keep") => {
                    let mut cur = stored;
                    for p in path.iter() {
                        cur = cur.and_then(|v| v.get(p));
                    }
                    match cur {
                        Some(v) if v.get("$secret").is_some_and(Value::is_object) => {
                            *entry = v.clone();
                            Ok(())
                        }
                        _ => Err("no_stored_secret"),
                    }
                }
                Some(plain) if !plain.is_empty() => {
                    let owner = Uuid::now_v7();
                    let ctx = SealContext {
                        purpose: Purpose::InstanceSetting,
                        owner_id: owner,
                        org_id: Uuid::nil(),
                        workspace_id: None,
                    };
                    let sealed = aurora_crypto::Aad {
                        purpose: ctx.purpose.as_str(),
                        org_id: ctx.org_id,
                        owner_id: ctx.owner_id,
                    };
                    let blob = keyring
                        .seal(&sealed, plain.as_bytes())
                        .map_err(|_| "internal")?;
                    *entry = json!({ "$secret": { "ref": blob, "owner": owner } });
                    Ok(())
                }
                _ => Err("invalid_config"),
            }
        }
        Value::Object(m) => {
            for (k, v) in m.iter_mut() {
                path.push(k.clone());
                resolve_for_test(keyring, v, stored, path)?;
                path.pop();
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Try a store definition before saving it: build it, authenticate, then write, read and delete
/// a probe value under its managed prefix. Writes nothing to the settings (C-746).
#[utoipa::path(post, path = "/admin/secret-store-test", operation_id = "adminTestSecretStore",
    tag = "admin",
    request_body = TestStore,
    responses((status = 200, body = TestResult), (status = 403, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn test_store(
    _c: InstanceAdmin,
    axum::extract::State(s): axum::extract::State<AppState>,
    Json(req): Json<TestStore>,
) -> Result<Json<TestResult>, ApiError> {
    let fail = |name: &str, code: &str| {
        Json(TestResult {
            ok: false,
            steps: vec![TestStep {
                name: name.into(),
                ok: false,
                error_code: Some(code.into()),
            }],
            checked_at: Utc::now(),
        })
    };
    let Value::Object(mut entry) = req.store else {
        return Ok(fail("configuration", "invalid_config"));
    };
    entry.insert("id".into(), json!(req.id));
    let kind = entry
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if !aurora_config::dashboard_store_kind_allowed(&kind) {
        return Ok(fail("configuration", "store_kind_not_allowed"));
    }
    let Ok(keyring) = crate::app::build_keyring(&s.config) else {
        return Ok(fail("configuration", "internal"));
    };
    // The stored entry of the same id supplies `keep`ed secrets.
    let snap_entry = s
        .settings
        .snapshot()
        .stored
        .get(SECTION, STORES_KEY)
        .and_then(Value::as_array)
        .and_then(|l| l.iter().find(|e| entry_id(e) == req.id).cloned());
    let mut entry = Value::Object(entry);
    if let Err(code) = resolve_for_test(&keyring, &mut entry, snap_entry.as_ref(), &mut Vec::new())
    {
        return Ok(fail("configuration", code));
    }
    let cfg: aurora_config::SecretStoreConfig = match serde_json::from_value(entry) {
        Ok(c) => c,
        Err(_) => return Ok(fail("configuration", "invalid_config")),
    };
    if kind == aurora_config::VAULT_KIND
        && let Err(errs) = VaultSettings::parse(&cfg)
    {
        let mut r = fail("configuration", "invalid_config");
        r.0.steps[0].error_code = Some(format!(
            "invalid_config:{}",
            errs.iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ));
        return Ok(r);
    }
    let store = match aurora_secrets::StoreRegistry::build_single(
        &cfg,
        keyring,
        s.config.tls.extra_ca_bundle.as_deref(),
    ) {
        Ok(st) => st,
        Err(e) => return Ok(fail("configuration", error_code(&e))),
    };
    let ctx = SealContext {
        purpose: Purpose::InstanceSetting,
        owner_id: Uuid::now_v7(),
        org_id: Uuid::nil(),
        workspace_id: None,
    };
    let probe = aurora_core::Secret::new(b"aurora-connection-test".to_vec());
    let mut steps = vec![TestStep {
        name: "configuration".into(),
        ok: true,
        error_code: None,
    }];
    let timeout = limits::SECRET_STORE_PROBE_TIMEOUT;
    let put = tokio::time::timeout(timeout, store.put(&ctx, &probe)).await;
    let put_ref = match put {
        Ok(Ok(r)) => {
            steps.push(step("authenticate", None));
            steps.push(step("write", None));
            Some(r)
        }
        Ok(Err(e)) => {
            steps.push(step("authenticate", Some(error_code(&e))));
            None
        }
        Err(_) => {
            steps.push(step("authenticate", Some("timeout")));
            None
        }
    };
    if let Some(r) = put_ref {
        match tokio::time::timeout(timeout, store.get(&ctx, &r)).await {
            Ok(Ok(v)) if v.expose().as_slice() == probe.expose().as_slice() => {
                steps.push(step("read", None));
            }
            Ok(Ok(_)) => steps.push(step("read", Some("unexpected_value"))),
            Ok(Err(e)) => steps.push(step("read", Some(error_code(&e)))),
            Err(_) => steps.push(step("read", Some("timeout"))),
        }
        match tokio::time::timeout(timeout, store.delete(&ctx, &r)).await {
            Ok(Ok(())) => steps.push(step("delete", None)),
            Ok(Err(e)) => steps.push(step("delete", Some(error_code(&e)))),
            Err(_) => steps.push(step("delete", Some("timeout"))),
        }
    }
    Ok(Json(TestResult {
        ok: steps.iter().all(|x| x.ok),
        steps,
        checked_at: Utc::now(),
    }))
}

fn step(name: &str, code: Option<&str>) -> TestStep {
    TestStep {
        name: name.into(),
        ok: code.is_none(),
        error_code: code.map(str::to_owned),
    }
}

// ----------------------------------------------------------------------- the migration job

struct Job {
    view: MigrationView,
}

fn jobs() -> &'static Mutex<HashMap<String, Job>> {
    static J: OnceLock<Mutex<HashMap<String, Job>>> = OnceLock::new();
    J.get_or_init(Default::default)
}

fn lock_jobs() -> std::sync::MutexGuard<'static, HashMap<String, Job>> {
    jobs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn counts_view(purpose: &str, c: Counts, total: u64) -> PurposeCounts {
    PurposeCounts {
        purpose: purpose.to_owned(),
        moved: c.moved,
        skipped: c.skipped,
        failed: c.failed,
        already: c.already,
        total,
    }
}

/// Is a migration (the CLI's, or a job on another replica) holding the lock right now?
async fn lock_held(state: &AppState) -> bool {
    let Ok(mut tx) = state.pool.begin().await else {
        return false;
    };
    let got: Result<bool, _> = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(secrets_migrate::MIGRATION_LOCK_KEY)
        .fetch_one(&mut *tx)
        .await;
    matches!(got, Ok(false))
}

/// The last job this replica ran, else the last finished one from the audit log.
async fn current_view(state: &AppState) -> MigrationView {
    let key = db_key(state);
    let local = lock_jobs().get(&key).map(|j| j.view.clone());
    let mut view = match local {
        Some(v) => v,
        None => last_finished(state)
            .await
            .unwrap_or_else(MigrationView::none),
    };
    if view.state != "running" {
        view.running_elsewhere = lock_held(state).await;
    }
    view
}

async fn last_finished(state: &AppState) -> Option<MigrationView> {
    let row: Option<(DateTime<Utc>, Value, String)> = sqlx::query_as(
        "SELECT created_at, metadata, actor_label FROM audit_log \
         WHERE action = 'secret_stores.migrate_finish' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await
    .ok()?;
    let (at, meta, actor) = row?;
    let per: Vec<PurposeCounts> = meta
        .get("per_purpose")
        .and_then(|v| serde_json::from_value::<Vec<PurposeCountsIn>>(v.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|p| PurposeCounts {
            purpose: p.purpose,
            moved: p.moved,
            skipped: p.skipped,
            failed: p.failed,
            already: p.already,
            total: p.total,
        })
        .collect();
    Some(MigrationView {
        state: meta
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("succeeded")
            .to_owned(),
        target: meta
            .get("target")
            .and_then(Value::as_str)
            .map(str::to_owned),
        started_at: None,
        finished_at: Some(at),
        started_by: Some(actor),
        per_purpose: per,
        error: meta.get("error").and_then(Value::as_str).map(str::to_owned),
        running_elsewhere: false,
    })
}

#[derive(Deserialize)]
struct PurposeCountsIn {
    purpose: String,
    moved: u64,
    skipped: u64,
    failed: u64,
    already: u64,
    total: u64,
}

fn dry_options(target: &str) -> Options {
    let mut o = Options::new(target);
    o.dry_run = true;
    o.keep_instance_settings_inline = true;
    o
}

fn summary_view(
    summary: &secrets_migrate::Summary,
    totals: &BTreeMap<String, u64>,
) -> Vec<PurposeCounts> {
    summary
        .per_purpose
        .iter()
        .map(|(p, c)| counts_view(p, *c, totals.get(*p).copied().unwrap_or(0)))
        .collect()
}

/// Dry run (the counts per purpose) or start the background job that moves every managed value
/// to `target` (C-747). One job at a time, on any replica or the CLI.
#[utoipa::path(post, path = "/admin/secret-stores/migrations", operation_id = "adminStartSecretMigration",
    tag = "admin",
    request_body = StartMigration,
    responses((status = 200, body = MigrationView), (status = 202, body = MigrationView),
        (status = 403, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 412, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn start_migration(
    c: InstanceAdmin,
    axum::extract::State(s): axum::extract::State<AppState>,
    Json(req): Json<StartMigration>,
) -> Result<(http::StatusCode, Json<MigrationView>), ApiError> {
    let registry = s.vault.registry();
    let usable = registry
        .get(&req.target)
        .is_some_and(|st| st.capabilities().managed);
    if !usable {
        return Err(ApiError::unprocessable(
            "store_not_usable",
            "That store is not available, or does not hold managed values.",
        ));
    }
    let who = c.0.email.clone();
    if req.dry_run {
        let summary = secrets_migrate::run(&s.pool, &s.vault, &dry_options(&req.target))
            .await
            .map_err(|e| ApiError::internal(format!("dry run: {e}")))?;
        let per = summary
            .per_purpose
            .iter()
            .map(|(p, cnt)| counts_view(p, *cnt, cnt.moved))
            .collect();
        return Ok((
            http::StatusCode::OK,
            Json(MigrationView {
                state: "dry_run".into(),
                target: Some(req.target),
                started_at: None,
                finished_at: None,
                started_by: Some(who),
                per_purpose: per,
                error: None,
                running_elsewhere: false,
            }),
        ));
    }
    service::require_reauth(&s, &Via::Admin(c.0.clone())).await?;
    let key = db_key(&s);
    if lock_jobs()
        .get(&key)
        .is_some_and(|j| j.view.state == "running")
        || lock_held(&s).await
    {
        return Err(ApiError::conflict(
            "migration_running",
            "A secrets migration is already running.",
        ));
    }
    // The totals the progress is measured against.
    let dry = secrets_migrate::run(&s.pool, &s.vault, &dry_options(&req.target))
        .await
        .map_err(|e| ApiError::internal(format!("counting values to move: {e}")))?;
    let totals: BTreeMap<String, u64> = dry
        .per_purpose
        .iter()
        .map(|(p, cnt)| ((*p).to_owned(), cnt.moved))
        .collect();
    let started_at = Utc::now();
    let actor = AuditActor::instance_admin(&c.0);
    let ev = AuditEvent::new(
        actor.clone(),
        "secret_stores.migrate_start",
        Outcome::Success,
    )
    .target("secret_store", &req.target)
    .metadata(json!({ "target": req.target, "totals": totals }));
    audit::commit_critical(audit::begin(&s).await?, &ev).await?;
    let view = MigrationView {
        state: "running".into(),
        target: Some(req.target.clone()),
        started_at: Some(started_at),
        finished_at: None,
        started_by: Some(who),
        per_purpose: totals
            .iter()
            .map(|(p, t)| counts_view(p, Counts::default(), *t))
            .collect(),
        error: None,
        running_elsewhere: false,
    };
    lock_jobs().insert(key.clone(), Job { view: view.clone() });
    let state = s.clone();
    let target = req.target.clone();
    tokio::spawn(async move {
        let progress_key = key.clone();
        let progress_totals = totals.clone();
        let mut opts = Options::new(&target);
        opts.keep_instance_settings_inline = true;
        opts.discard_grace = limits::SECRET_MIGRATION_DISCARD_GRACE;
        opts.progress = Some(secrets_migrate::Progress(Arc::new(
            move |purpose, counts| {
                if let Some(j) = lock_jobs().get_mut(&progress_key) {
                    let total = progress_totals.get(purpose).copied().unwrap_or(0);
                    match j.view.per_purpose.iter_mut().find(|p| p.purpose == purpose) {
                        Some(p) => *p = counts_view(purpose, counts, total),
                        None => j.view.per_purpose.push(counts_view(purpose, counts, total)),
                    }
                }
            },
        )));
        let result = secrets_migrate::run(&state.pool, &state.vault, &opts).await;
        let (outcome, error, per) = match &result {
            Ok(summary) => (
                if summary.failed() == 0 {
                    "succeeded"
                } else {
                    "failed"
                },
                (summary.failed() > 0).then(|| {
                    format!(
                        "{} value(s) could not be moved; run it again",
                        summary.failed()
                    )
                }),
                summary_view(summary, &totals),
            ),
            Err(e) => ("failed", Some(e.clone()), Vec::new()),
        };
        // The cached counts are stale now.
        if let Some(cache) = caches().lock().await.get_mut(&key) {
            cache.counts = None;
        }
        let finished = Utc::now();
        {
            let mut jobs = lock_jobs();
            if let Some(j) = jobs.get_mut(&key) {
                j.view.state = outcome.to_owned();
                j.view.finished_at = Some(finished);
                j.view.error.clone_from(&error);
                if !per.is_empty() {
                    j.view.per_purpose.clone_from(&per);
                }
            }
        }
        let ev = AuditEvent::new(actor, "secret_stores.migrate_finish", Outcome::Success)
            .target("secret_store", &target)
            .metadata(json!({
                "target": target,
                "outcome": outcome,
                "error": error,
                "per_purpose": per,
            }));
        match audit::begin(&state).await {
            Ok(tx) => {
                if let Err(e) = audit::commit_critical(tx, &ev).await {
                    tracing::error!(error = %e, "secret migration finish not audited");
                }
            }
            Err(e) => tracing::error!(error = %e, "secret migration finish not audited"),
        }
    });
    Ok((http::StatusCode::ACCEPTED, Json(view)))
}

/// The running or last migration (C-747).
#[utoipa::path(get, path = "/admin/secret-stores/migrations/current",
    operation_id = "adminGetSecretMigration", tag = "admin",
    responses((status = 200, body = MigrationView), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn current_migration(
    _c: InstanceAdmin,
    axum::extract::State(s): axum::extract::State<AppState>,
) -> Result<Json<MigrationView>, ApiError> {
    Ok(Json(current_view(&s).await))
}
