//! The shapes of the template-source routes (C-1107). Built from stored rows, never a live
//! read; the token is never in any of them.

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// One C-1141 problem, as data (`code` is the fixed code, `path` where it applies).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TemplateProblemView {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl From<&aurora_template::TemplateProblem> for TemplateProblemView {
    fn from(p: &aurora_template::TemplateProblem) -> Self {
        Self {
            code: p.code.as_str().to_owned(),
            path: p.path.clone(),
            message: p.message.clone(),
        }
    }
}

/// What a ref resolved to (C-1107, as C-1124's `resolved`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ResolvedRefView {
    /// `branch` | `tag` | `commit`.
    pub kind: String,
    pub name: String,
    /// Whether this is the repository's default branch.
    pub default: bool,
}

/// The state of one template of a source (C-1107).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SourceTemplateView {
    pub template_id: String,
    pub dir: String,
    pub name: String,
    pub summary: String,
    pub head_version: Option<String>,
    /// `ok` | `needs_newer_aurora` | `invalid` (the head's state).
    pub head_state: String,
    /// `new` | `ingested` | `id_taken` | `invalid`.
    pub state: String,
}

/// A template source (C-1107): the stored rows, never a live read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AgentTemplateSource {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    /// The repository's display form (`host[:port]/path`, as C-1122 shows it).
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: String,
    /// Whether a sealed access token is set (the value is never returned).
    pub token_set: bool,
    pub token_updated_at: Option<DateTime<Utc>>,
    pub homepage: Option<String>,
    pub contact: Option<String>,
    /// The manifest's `packages.url` template.
    pub packages_url: String,
    pub update_interval: String,
    /// `ok` | `check_failed`.
    pub status: String,
    pub check_failed_at: Option<DateTime<Utc>>,
    pub check_failed_reason: Option<String>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_head_sha: Option<String>,
    pub templates: Vec<SourceTemplateView>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AgentTemplateSourceList {
    pub items: Vec<AgentTemplateSource>,
}

/// The repository part of a manifest (C-1103).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ManifestRepoView {
    pub name: String,
    pub description: String,
}

/// One template the preview found in the folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PreviewTemplateView {
    pub template_id: String,
    pub dir: String,
    pub name: String,
    pub summary: String,
    pub version: Option<String>,
    /// `new` | `id_taken` (a stored row's states cannot apply to a source that is not saved).
    pub state: String,
    pub problems: Vec<TemplateProblemView>,
}

/// What a preview read found (C-1107): check without saving, nothing stored, no audit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AgentTemplateSourcePreview {
    /// The normalized address.
    pub url: String,
    /// The repository's display form.
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub resolved: ResolvedRefView,
    pub commit_sha: String,
    pub path: String,
    pub manifest: ManifestRepoView,
    pub templates: Vec<PreviewTemplateView>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewTemplateSourceRequest {
    pub url: String,
    /// A branch, tag or commit; default the repository's default branch.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// The folder in the repository; default the root.
    pub path: Option<String>,
    /// An access token used for this read only (never stored).
    pub token: Option<String>,
    pub token_username: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTemplateSourceRequest {
    pub url: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: Option<String>,
    /// A read-only access token, sealed (C-1126); `token_username` defaults to `x-access-token`.
    pub token: Option<String>,
    pub token_username: Option<String>,
    /// `6h|12h|1d|1w|2w|1mo|never`; default `agents.templates.update_interval`.
    pub update_interval: Option<String>,
}

/// Absent = unchanged, `null` = removed, a string = replaced (the agents' patch convention).
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    // A field present with any value (including `null`) lands here: the inner option is the
    // value's own optionality.
    Ok(Some(serde::de::Deserialize::deserialize(d)?))
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTemplateSourceRequest {
    /// `6h|12h|1d|1w|2w|1mo|never`.
    pub update_interval: Option<String>,
    /// A branch, tag or commit; `""` clears (the default branch again).
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// The folder in the repository.
    pub path: Option<String>,
    /// A new token (replaces), or `null` (removes); absent = unchanged.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub token: Option<Option<String>>,
    pub token_username: Option<String>,
}

// ---- the stored content -----------------------------------------------------------------------

/// The `agent_template_versions.content` JSON (03: the parsed `TemplateContent`, files base64,
/// the avatar re-encoded to a 256×256 PNG). `TemplateContent` has no `Serialize` of its own —
/// this is the one encoding, and the review views (C-1108) read it back; the byte-for-byte
/// files are what creation copies (P6-TPL-3).
pub fn content_json(c: &aurora_template::TemplateContent) -> serde_json::Value {
    use aurora_template::{AppEntry, Automation, Effort, InputKind, Level, SkillRef, Verification};
    use serde_json::json;

    fn file(f: &aurora_template::TemplateFile) -> serde_json::Value {
        json!({
            "path": f.path,
            "size": f.bytes.len(),
            "executable": f.executable,
            "sha256": hex(&f.bytes),
            "bytes": B64.encode(&f.bytes),
        })
    }
    fn level_str(l: Level) -> &'static str {
        match l {
            Level::Read => "read",
            Level::Ask => "ask",
            Level::Act => "act",
        }
    }
    let effort = c.meta.effort.map(|e| match e {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Max => "max",
    });
    let inputs: Vec<_> = c
        .meta
        .inputs
        .iter()
        .map(|i| {
            json!({
                "id": i.id,
                "label": i.label,
                "help": i.help,
                "kind": match i.kind {
                    InputKind::Text => "text",
                    InputKind::Choice => "choice",
                },
                "required": i.required,
                "pattern": i.pattern,
                "max_length": i.max_length,
                "options": i.options,
                "default": i.default,
            })
        })
        .collect();
    let apps: Vec<_> = c
        .meta
        .apps
        .iter()
        .map(|a| match a {
            AppEntry::App {
                app,
                required,
                level,
                tools,
                why,
            } => json!({
                "app": app,
                "required": required,
                "level": level_str(*level),
                "tools": tools.as_ref().map(|t| json!({"allow": t.allow, "ask": t.ask})),
                "why": why,
            }),
            AppEntry::AnyOf {
                any_of,
                required,
                level,
                why,
            } => json!({
                "any_of": any_of,
                "required": required,
                "level": level_str(*level),
                "why": why,
            }),
        })
        .collect();
    let skill_refs: Vec<_> = c
        .meta
        .skill_refs
        .iter()
        .map(|r| match r {
            SkillRef::Library {
                address,
                ref_,
                path,
                skill,
                required,
                why,
            } => json!({
                "kind": "library",
                "address": address.url,
                "ref": ref_,
                "path": path,
                "skill": skill,
                "required": required,
                "why": why,
            }),
            SkillRef::Organization {
                name,
                required,
                why,
            } => json!({
                "kind": "organization",
                "name": name,
                "required": required,
                "why": why,
            }),
        })
        .collect();
    let automations: Vec<_> = c
        .automations
        .iter()
        .map(|a| match a {
            Automation::Schedule { name, prompt, cron } => {
                json!({"kind": "schedule", "name": name, "prompt": prompt, "cron": cron})
            }
            Automation::Webhook {
                name,
                instructions,
                verification,
            } => json!({
                "kind": "webhook",
                "name": name,
                "instructions": instructions,
                "verification": match verification {
                    Verification::Signed => "signed",
                    Verification::Password => "password",
                    Verification::None => "none",
                },
            }),
        })
        .collect();
    json!({
        "meta": {
            "format": c.meta.format,
            "id": c.meta.id,
            "version": c.meta.version,
            "name": c.meta.name,
            "summary": c.meta.summary,
            "min_core": c.meta.min_core,
            "effort": effort,
            "inputs": inputs,
            "apps": apps,
            "skill_refs": skill_refs,
        },
        "agent_name": c.agent_name,
        "may_message_first": c.may_message_first,
        "instructions": c.instructions,
        "readme": c.readme,
        "memories": c.memories.iter().map(|m| json!({
            "key": m.key, "content": m.content, "locked": m.locked,
        })).collect::<Vec<_>>(),
        "skills": c.skills.iter().map(|s| json!({
            "name": s.name,
            "use_it_when": s.use_it_when,
            "body": s.body,
            "files": s.files.iter().map(file).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "helpers": c.helpers.iter().map(file).collect::<Vec<_>>(),
        "automations": automations,
        "avatar": c.avatar.as_ref().map(|(name, bytes)| json!({
            "name": name, "bytes": B64.encode(bytes),
        })),
    })
}

fn hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let d = h.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The base64 engine the content column uses (standard, with padding).
pub(crate) use base64::engine::general_purpose::STANDARD as B64;

// ---- the check result (C-1108) ------------------------------------------------------------------

/// One version that left `awaiting_package` in the check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CheckIngestedView {
    pub template_id: String,
    pub version: String,
    /// `pending_approval` | `invalid`.
    pub status: String,
    pub status_reason: Option<String>,
}

/// One version still `awaiting_package` after the check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CheckWaitingView {
    pub template_id: String,
    pub version: String,
    pub since: DateTime<Utc>,
    pub last_reason: Option<String>,
}

/// A problem the check found (the head's `template.yaml`, an id another source holds, a head
/// version that is not newer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CheckProblemView {
    pub template_id: Option<String>,
    pub dir: String,
    pub code: String,
    pub message: String,
}

/// The result of a check of one source (C-1108): a read failure is a result, not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TemplateSourceCheckResult {
    pub source_id: Uuid,
    /// `updated` | `up_to_date` | `failed`.
    pub status: String,
    pub commit_sha: String,
    pub ingested: Vec<CheckIngestedView>,
    pub waiting: Vec<CheckWaitingView>,
    pub problems: Vec<CheckProblemView>,
    pub check_failed_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TemplateSourceCheckAll {
    pub items: Vec<TemplateSourceCheckResult>,
}
