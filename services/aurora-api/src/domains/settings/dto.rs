//! Request and response shapes of `/v1/admin/settings` (C-771..C-776).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

/// `GET /v1/admin/settings` (C-771).
#[derive(Serialize, ToSchema)]
pub struct SettingsIndex {
    pub sections: Vec<SectionSummary>,
    /// Every key (and keyed-list entry) the deployment configuration sets: the page's banner.
    pub locked: Vec<LockedKey>,
}

#[derive(Serialize, ToSchema)]
pub struct SectionSummary {
    pub section: String,
    pub title: String,
    pub description: String,
    pub generation: i64,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by_label: Option<String>,
    /// (C-1102) The last writer by name; null when none or the user is gone.
    pub updated_by: Option<crate::domains::freeze::dto::NamedPerson>,
    pub keys_total: i64,
    pub keys_locked: i64,
    pub keys_dashboard: i64,
    /// The section's fields in words: All settings searches their labels and help (C-972).
    pub fields: Vec<FieldInfo>,
}

/// One choice of an enumeration field (C-972).
#[derive(Serialize, ToSchema)]
pub struct FieldChoice {
    pub value: String,
    pub label: String,
    pub description: String,
}

/// An operator document, by title (C-972).
#[derive(Serialize, ToSchema)]
pub struct FieldDoc {
    pub title: String,
    pub path: String,
}

/// A settings field in words (C-972, D-341): what the dashboard shows instead of a key.
#[derive(Serialize, ToSchema)]
pub struct FieldInfo {
    pub key: String,
    pub label: String,
    /// One sentence: what the setting does and when to change it.
    pub help: String,
    /// `bool`, `int`, `float`, `duration`, `enum`, `list`, `text`, `secret` or `keyed_list`.
    pub kind: String,
    pub choices: Option<Vec<FieldChoice>>,
    /// The default, in words.
    pub default_display: String,
    /// The unit the control edits in (a duration: `seconds`, `minutes`, `hours` or `days`).
    pub unit: Option<String>,
    pub doc: Option<FieldDoc>,
}

/// A key's current value, read-only (C-973).
#[derive(Serialize, ToSchema)]
pub struct CurrentValue {
    pub key: String,
    pub label: String,
    /// The value in words; a secret reads only "Set" or "Not set".
    pub value_display: String,
    /// `helm` or `default`.
    pub source: String,
}

#[derive(Serialize, ToSchema)]
pub struct LockedKey {
    pub section: String,
    pub key: String,
    pub entry_id: Option<String>,
    pub helm_key: String,
}

/// The value type of a setting, so a generic form can render it (additive to C-772, C-745).
#[derive(Serialize, ToSchema)]
pub struct SettingSchema {
    /// `bool`, `int`, `float`, `duration_secs`, `string`, `enum`, `list`, `object`.
    pub kind: String,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub max_len: Option<i64>,
    pub options: Option<Vec<String>>,
    /// The element type of a list (`kind` of its items).
    pub item_kind: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SecretState {
    pub set: bool,
    pub updated_at: Option<DateTime<Utc>>,
    /// The environment variable that supplies the secret (locks the field).
    pub from_env: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SettingEntry {
    pub id: String,
    #[schema(value_type = Object)]
    pub value: Value,
    /// `helm` or `dashboard`.
    pub source: String,
    pub locked: bool,
    /// Section-specific warnings about this entry (C-813), e.g. `needs_allowed_private`.
    pub warnings: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema)]
pub struct Setting {
    pub key: String,
    /// The effective value; `null` for a secret key (see `secret`).
    #[schema(value_type = Object)]
    pub value: Value,
    /// `helm`, `dashboard` or `default`.
    pub source: String,
    pub locked: bool,
    pub helm_key: String,
    #[schema(value_type = Object)]
    pub default: Value,
    pub schema: SettingSchema,
    /// `whole` or `keyed`.
    pub list: Option<String>,
    /// The field that identifies a keyed-list entry (`id`, or `class`); additive to C-772 (C-745).
    pub id_field: Option<String>,
    pub entries: Option<Vec<SettingEntry>>,
    pub secret: Option<SecretState>,
}

/// `GET`/`PUT /v1/admin/settings/{section}` (C-772, C-773).
#[derive(Serialize, ToSchema)]
pub struct SettingsSection {
    pub section: String,
    pub title: String,
    pub description: String,
    pub generation: i64,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by_label: Option<String>,
    /// (C-1102) The last writer by name; null when none or the user is gone.
    pub updated_by: Option<crate::domains::freeze::dto::NamedPerson>,
    pub settings: Vec<Setting>,
    /// The words of every setting above, in the same order (C-972).
    pub fields: Vec<FieldInfo>,
    /// `false` for a section whose keys are not editable yet: `current` holds its values (C-973).
    pub editable: bool,
    /// The running configuration's values, read-only; only when `editable` is `false` (C-973).
    pub current: Option<Vec<CurrentValue>>,
    /// Section-specific read-only data (C-778..C-782).
    #[schema(value_type = Option<Object>)]
    pub extra: Option<Value>,
}

/// `PUT` and `POST …/preview` body (C-773, C-774).
#[derive(Deserialize, ToSchema)]
pub struct PutSection {
    /// The generation the form was loaded at (compare-and-set).
    pub generation: i64,
    /// The section's dashboard values by dotted key; absent keys return to their default.
    #[schema(value_type = Object)]
    pub values: serde_json::Map<String, Value>,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct DiffItem {
    pub key: String,
    pub entry_id: Option<String>,
    #[schema(value_type = Object)]
    pub before: Value,
    #[schema(value_type = Object)]
    pub after: Value,
    pub before_source: String,
    pub after_source: String,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct LooseningItem {
    pub key: String,
    pub entry_id: Option<String>,
    pub reason: String,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct AffectedChange {
    pub key: String,
    #[schema(value_type = Object)]
    pub before: Value,
    #[schema(value_type = Object)]
    pub after: Value,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct AffectedOrg {
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub changes: Vec<AffectedChange>,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct SettingWarning {
    pub code: String,
    pub key: Option<String>,
    /// The keyed-list entry the warning is about (C-813 `endpoint_resolves_private`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    pub message: String,
}

/// `POST /v1/admin/settings/{section}/preview` (C-774).
#[derive(Serialize, ToSchema)]
pub struct SettingsPreview {
    pub generation: i64,
    pub diff: Vec<DiffItem>,
    pub loosening: Vec<LooseningItem>,
    pub affected_orgs: Vec<AffectedOrg>,
    pub affected_orgs_total: i64,
    pub warnings: Vec<SettingWarning>,
}

/// `POST /v1/admin/settings/import` (C-776).
#[derive(Deserialize, ToSchema)]
pub struct ImportRequest {
    pub document: String,
    /// `yaml` or `json`.
    pub format: String,
    pub dry_run: bool,
    /// `replace` (default) or `merge`.
    pub mode: Option<String>,
    /// Required to apply: the generation of every section the document touches.
    #[schema(value_type = Option<Object>)]
    pub generations: Option<serde_json::Map<String, Value>>,
}

#[derive(Serialize, ToSchema)]
pub struct ImportSection {
    pub section: String,
    pub generation: i64,
    pub diff: Vec<DiffItem>,
    pub loosening: Vec<LooseningItem>,
    pub warnings: Vec<SettingWarning>,
}

#[derive(Serialize, ToSchema)]
pub struct ImportError {
    pub section: String,
    pub code: String,
    pub message: String,
    #[schema(value_type = Option<Object>)]
    pub details: Option<Value>,
}

/// The dry-run result, or the applied sections of an import.
#[derive(Serialize, ToSchema)]
pub struct ImportResult {
    pub applied: bool,
    pub sections: Vec<ImportSection>,
    pub errors: Vec<ImportError>,
}

/// Query of the export route (C-775).
#[derive(Deserialize, utoipa::IntoParams)]
pub struct ExportQuery {
    /// `yaml` (default) or `json`.
    pub format: Option<String>,
    /// Comma-separated section ids; default all.
    pub sections: Option<String>,
}
