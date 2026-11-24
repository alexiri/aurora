//! The shapes of the template-source routes (C-1107). Built from stored rows, never a live
//! read; the token is never in any of them.

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domains::accounts::dto::Account;

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

/// (P6-TPL-3, C-1112) Lowercase hex of the SHA-256 of `bytes`: the parts table's content hash
/// (the create and the offer/apply diff agree on it).
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(bytes)
}

fn hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let d = h.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The stored content JSON decoded back into the parsed template (P6-TPL-3): creation and the
/// apply copy from exactly what was approved, and the part hashes are the C-1104 hashes of this.
/// `None` for a shape this version's encoder never wrote (a corrupted column); every caller
/// turns that into a refusal, never into a partial agent (SEC-146).
pub(crate) fn content_from_json(v: &Value) -> Option<aurora_template::TemplateContent> {
    use aurora_template::{
        AppEntry, Automation, Effort, InputDecl, InputKind, Level, MemoryFile, SkillRef,
        TemplateContent, TemplateFile, TemplateMeta, TemplateSkill, Verification,
    };

    fn level(k: &str) -> Level {
        match k {
            "ask" => Level::Ask,
            "act" => Level::Act,
            _ => Level::Read,
        }
    }
    fn file(x: &Value) -> Option<TemplateFile> {
        Some(TemplateFile {
            path: x.get("path")?.as_str()?.to_owned(),
            bytes: B64.decode(x.get("bytes")?.as_str()?).ok()?,
            executable: x
                .get("executable")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
    fn input(x: &Value) -> Option<InputDecl> {
        let arr = |k: &str| {
            x.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Some(InputDecl {
            id: x.get("id")?.as_str()?.to_owned(),
            label: x.get("label")?.as_str()?.to_owned(),
            help: x.get("help").and_then(Value::as_str).map(str::to_owned),
            kind: match x.get("kind").and_then(Value::as_str) {
                Some("choice") => InputKind::Choice,
                _ => InputKind::Text,
            },
            required: x.get("required").and_then(Value::as_bool).unwrap_or(true),
            pattern: x.get("pattern").and_then(Value::as_str).map(str::to_owned),
            max_length: x
                .get("max_length")
                .and_then(Value::as_u64)
                .and_then(|m| u32::try_from(m).ok()),
            options: arr("options"),
            default: x.get("default").and_then(Value::as_str).map(str::to_owned),
        })
    }
    fn app(x: &Value) -> Option<AppEntry> {
        let l = x.get("level").and_then(Value::as_str).map(level);
        match (x.get("app"), x.get("any_of")) {
            (Some(Value::String(a)), _) => Some(AppEntry::App {
                app: a.clone(),
                required: x.get("required").and_then(Value::as_bool).unwrap_or(true),
                level: l.unwrap_or_default(),
                tools: x.get("tools").filter(|t| !t.is_null()).map(|t| {
                    let arr = |k: &str| {
                        t.get(k)
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default()
                    };
                    aurora_template::AppTools {
                        allow: arr("allow"),
                        ask: arr("ask"),
                    }
                }),
                why: x
                    .get("why")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }),
            (_, Some(Value::Array(ids))) => Some(AppEntry::AnyOf {
                any_of: ids
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                required: x.get("required").and_then(Value::as_bool).unwrap_or(true),
                level: l.unwrap_or_default(),
                why: x
                    .get("why")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }),
            _ => None,
        }
    }
    fn skill_ref(x: &Value) -> Option<SkillRef> {
        match x.get("kind").and_then(Value::as_str)? {
            "library" => Some(SkillRef::Library {
                // The stored address is already normalized (C-1123); rebuild the typed form.
                address: aurora_core::git_url::parse(x.get("address")?.as_str()?).ok()?,
                ref_: x
                    .get("ref")
                    .and_then(Value::as_str)
                    .filter(|s: &&str| !s.is_empty())
                    .map(str::to_owned),
                path: x
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|s: &&str| !s.is_empty())
                    .map(str::to_owned),
                skill: x.get("skill")?.as_str()?.to_owned(),
                required: x.get("required").and_then(Value::as_bool).unwrap_or(true),
                why: x
                    .get("why")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }),
            "organization" => Some(SkillRef::Organization {
                name: x.get("name")?.as_str()?.to_owned(),
                required: x.get("required").and_then(Value::as_bool).unwrap_or(true),
                why: x
                    .get("why")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }),
            _ => None,
        }
    }
    fn automation(x: &Value) -> Option<Automation> {
        match x.get("kind").and_then(Value::as_str)? {
            "schedule" => Some(Automation::Schedule {
                name: x.get("name")?.as_str()?.to_owned(),
                prompt: x.get("prompt")?.as_str()?.to_owned(),
                cron: x.get("cron")?.as_str()?.to_owned(),
            }),
            "webhook" => Some(Automation::Webhook {
                name: x.get("name")?.as_str()?.to_owned(),
                instructions: x.get("instructions")?.as_str()?.to_owned(),
                verification: match x.get("verification").and_then(Value::as_str) {
                    Some("password") => Verification::Password,
                    Some("none") => Verification::None,
                    _ => Verification::Signed,
                },
            }),
            _ => None,
        }
    }
    fn skill(x: &Value) -> Option<TemplateSkill> {
        Some(TemplateSkill {
            name: x.get("name")?.as_str()?.to_owned(),
            use_it_when: x.get("use_it_when")?.as_str()?.to_owned(),
            body: x.get("body")?.as_str()?.to_owned(),
            files: x
                .get("files")?
                .as_array()?
                .iter()
                .map(file)
                .collect::<Option<Vec<_>>>()?,
        })
    }

    let meta = v.get("meta")?;
    let str_of = |k: &str| meta.get(k).and_then(Value::as_str);
    Some(TemplateContent {
        meta: TemplateMeta {
            format: u32::try_from(meta.get("format")?.as_u64()?).ok()?,
            id: str_of("id")?.to_owned(),
            version: str_of("version")?.to_owned(),
            name: str_of("name")?.to_owned(),
            summary: str_of("summary")?.to_owned(),
            min_core: str_of("min_core").map(str::to_owned),
            effort: match str_of("effort") {
                Some("low") => Some(Effort::Low),
                Some("high") => Some(Effort::High),
                Some("max") => Some(Effort::Max),
                Some("medium") => Some(Effort::Medium),
                _ => None,
            },
            inputs: meta
                .get("inputs")?
                .as_array()?
                .iter()
                .map(input)
                .collect::<Option<Vec<_>>>()?,
            apps: meta
                .get("apps")?
                .as_array()?
                .iter()
                .map(app)
                .collect::<Option<Vec<_>>>()?,
            skill_refs: meta
                .get("skill_refs")?
                .as_array()?
                .iter()
                .map(skill_ref)
                .collect::<Option<Vec<_>>>()?,
        },
        agent_name: v.get("agent_name")?.as_str()?.to_owned(),
        may_message_first: v
            .get("may_message_first")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        instructions: v.get("instructions")?.as_str()?.to_owned(),
        readme: v.get("readme")?.as_str()?.to_owned(),
        memories: v
            .get("memories")?
            .as_array()?
            .iter()
            .map(|m| {
                Some(MemoryFile {
                    key: m.get("key")?.as_str()?.to_owned(),
                    content: m.get("content")?.as_str()?.to_owned(),
                    locked: m.get("locked").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect::<Option<Vec<_>>>()?,
        skills: v
            .get("skills")?
            .as_array()?
            .iter()
            .map(skill)
            .collect::<Option<Vec<_>>>()?,
        helpers: v
            .get("helpers")?
            .as_array()?
            .iter()
            .map(file)
            .collect::<Option<Vec<_>>>()?,
        automations: v
            .get("automations")?
            .as_array()?
            .iter()
            .map(automation)
            .collect::<Option<Vec<_>>>()?,
        avatar: match v.get("avatar") {
            // `Some(null)` is the encoder's "no avatar"; an object must decode (a corrupted
            // one refuses the whole content, not silently drops the avatar).
            Some(a) if !a.is_null() => Some((
                a.get("name")?.as_str()?.to_owned(),
                B64.decode(a.get("bytes")?.as_str()?).ok()?,
            )),
            _ => None,
        },
    })
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

// ---- the admin templates list and the review (C-1108) ------------------------------------------

/// A text file of a stored version (C-1108's `FileView`): `text` for valid UTF-8 up to
/// 256 KiB (which includes every executable file, C-1141), else null.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct FileView {
    pub path: String,
    pub size: i64,
    pub executable: bool,
    pub sha256: String,
    pub text: Option<String>,
}

/// The byte cap under which a file is shown as text (C-1108, `FileView`).
pub(crate) const FILE_TEXT_MAX: usize = 256 * 1024;

impl FileView {
    /// From the content column's file shape (`{ path, size, executable, sha256, bytes }`,
    /// the bytes base64): decodes the bytes and cuts the text by the cap. A bad decode is
    /// `None`, not empty bytes — the view fails and the version cannot be reviewed or
    /// approved on a corrupted content column (SEC-146).
    fn from_json(f: &Value) -> Option<Self> {
        let bytes = B64.decode(f.get("bytes")?.as_str()?).ok()?;
        let size = f.get("size").and_then(Value::as_i64).unwrap_or(0);
        let executable = f
            .get("executable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sha256 = f
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let text = std::str::from_utf8(&bytes)
            .ok()
            .filter(|_| bytes.len() <= FILE_TEXT_MAX)
            .map(str::to_owned);
        Some(Self {
            path: f.get("path")?.as_str()?.to_owned(),
            size,
            executable,
            sha256,
            text,
        })
    }
}

/// A skill of a stored version (the review, C-1108).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ContentSkillView {
    pub name: String,
    pub use_it_when: String,
    pub body: String,
    pub files: Vec<FileView>,
}

/// A memory of a stored version (C-1151's `locked` shown in the review).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ContentMemoryView {
    pub key: String,
    pub content: String,
    pub locked: bool,
}

/// The agent line of a stored version (C-1108's `TemplateContentView.agent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ContentAgentView {
    pub name: String,
    pub may_message_first: bool,
}

/// Everything a version would tell or give an agent, in full, as data (C-1108's
/// `TemplateContentView`): decoded from the stored content, never re-read from the source.
/// `inputs`, `apps`, `skill_refs` and `automations` are C-1104's shapes, passed through as
/// stored (the review renders them; P6-TPL-3's creation reads them back the same way).
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TemplateContentView {
    pub name: String,
    pub summary: String,
    /// The template's `README.md`, in full.
    pub readme: String,
    pub effort: Option<String>,
    pub min_core: Option<String>,
    pub agent: ContentAgentView,
    pub instructions: String,
    pub inputs: Vec<Value>,
    pub apps: Vec<Value>,
    pub skills: Vec<ContentSkillView>,
    pub skill_refs: Vec<Value>,
    pub helpers: Vec<FileView>,
    pub memories: Vec<ContentMemoryView>,
    pub automations: Vec<Value>,
    pub has_avatar: bool,
}

impl TemplateContentView {
    /// Decode the content column's JSON (dto::content_json's shape) back into the view.
    /// `None` for a shape that does not decode (never written by this version's encoder).
    pub(crate) fn from_json(v: &Value) -> Option<Self> {
        let meta = v.get("meta")?;
        let str_of = |k: &str| meta.get(k).and_then(Value::as_str);
        Some(Self {
            name: str_of("name")?.to_owned(),
            summary: str_of("summary")?.to_owned(),
            readme: v.get("readme")?.as_str()?.to_owned(),
            effort: meta
                .get("effort")
                .and_then(Value::as_str)
                .map(str::to_owned),
            min_core: meta
                .get("min_core")
                .and_then(Value::as_str)
                .map(str::to_owned),
            agent: ContentAgentView {
                name: v.get("agent_name")?.as_str()?.to_owned(),
                may_message_first: v
                    .get("may_message_first")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            instructions: v.get("instructions")?.as_str()?.to_owned(),
            inputs: meta
                .get("inputs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            apps: meta
                .get("apps")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            skills: v
                .get("skills")?
                .as_array()?
                .iter()
                .map(|s| -> Option<ContentSkillView> {
                    Some(ContentSkillView {
                        name: s.get("name")?.as_str()?.to_owned(),
                        use_it_when: s.get("use_it_when")?.as_str()?.to_owned(),
                        body: s.get("body")?.as_str()?.to_owned(),
                        files: s
                            .get("files")?
                            .as_array()?
                            .iter()
                            .map(FileView::from_json)
                            .collect::<Option<Vec<_>>>()?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            skill_refs: meta
                .get("skill_refs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            helpers: v
                .get("helpers")?
                .as_array()?
                .iter()
                .map(FileView::from_json)
                .collect::<Option<Vec<_>>>()?,
            memories: v
                .get("memories")?
                .as_array()?
                .iter()
                .map(|m| -> Option<ContentMemoryView> {
                    Some(ContentMemoryView {
                        key: m.get("key")?.as_str()?.to_owned(),
                        content: m.get("content")?.as_str()?.to_owned(),
                        locked: m.get("locked").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            automations: v
                .get("automations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            has_avatar: v.get("avatar").map(|a| !a.is_null()).unwrap_or(false),
        })
    }

    /// The avatar's PNG bytes (the content column's `avatar.bytes`, base64).
    pub(crate) fn avatar_bytes(v: &Value) -> Option<Vec<u8>> {
        B64.decode(v.get("avatar")?.get("bytes")?.as_str()?).ok()
    }
}

/// Who decided a version (`decided_by`: id and display name, null when not decided yet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct DecidedByView {
    pub id: Uuid,
    pub display_name: String,
}

/// One row of the admin list's `versions` (C-1108, newest first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TemplateVersionListItem {
    pub version: String,
    /// `awaiting_package` | `pending_approval` | `approved` | `rejected` | `invalid`.
    pub status: String,
    pub status_reason: Option<String>,
    pub commit_sha: String,
    pub size_bytes: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<DecidedByView>,
}

/// An app the template's apps entries name, and whether the instance has it (C-1108).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NeedsAppView {
    pub app_id: String,
    pub installed: bool,
}

/// A skill library the template references, and whether one is added here (C-1108).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NeedsLibraryView {
    pub url: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: String,
    pub added_here: bool,
}

/// One template of the admin list (C-1108): the stored rows, the offered version and what
/// the current version needs.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct AdminTemplateView {
    pub template_id: String,
    pub name: String,
    pub summary: String,
    pub source: AdminTemplateSourceRef,
    pub offered: bool,
    pub removed_upstream: bool,
    pub head_version: Option<String>,
    /// `ok` | `needs_newer_aurora` | `invalid`.
    pub head_state: String,
    pub head_problems: Option<Value>,
    /// The offered version (newest `approved`, C-1108), null when none is approved.
    pub current_version: Option<String>,
    pub versions: Vec<TemplateVersionListItem>,
    pub needs_apps: Vec<NeedsAppView>,
    pub needs_libraries: Vec<NeedsLibraryView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AdminTemplateSourceRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct AdminTemplateList {
    pub items: Vec<AdminTemplateView>,
}

/// The source behind a version (C-1108's `TemplateVersionDetail.source`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct VersionSourceView {
    pub id: Uuid,
    pub name: String,
    /// The repository's display form (`host[:port]/path`).
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: String,
}

/// One version of a template, everything the review shows (C-1108).
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TemplateVersionDetail {
    pub template_id: String,
    pub version: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub commit_sha: String,
    pub source: VersionSourceView,
    pub content_sha256: Option<String>,
    pub archive_sha256: Option<String>,
    pub size_bytes: Option<i32>,
    pub content: Option<TemplateContentView>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = ApproveTemplateVersionRequest)]
#[serde(deny_unknown_fields)]
pub struct ApproveRequest {
    /// The hash the review showed; it must still be the stored one.
    pub content_sha256: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTemplateRequest {
    /// Withdraw (`false`) or offer again (`true`).
    pub offered: bool,
}

/// One template of an organization's list (C-1109): every offered template, on by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct OrgTemplateItem {
    pub template_id: String,
    pub name: String,
    pub summary: String,
    /// The offered version (C-1108's rule), null when none is approved.
    pub version: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct OrgTemplateList {
    pub items: Vec<OrgTemplateItem>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetOrgTemplateRequest {
    /// On (`true`, the default) or off (`false`) for this organization.
    pub enabled: bool,
}

// ---- the workspace catalogue (C-1110) ------------------------------------------------------------

/// One `apps:` entry of the card (C-1110, C-1142's app skills).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CardAppEntry {
    /// The entry's app ids (`app`, or every id of `any_of`).
    pub app_ids: Vec<String>,
    pub required: bool,
    pub available: bool,
    /// The app skills of the entry's available apps (C-1142), sorted, without repeats.
    pub skills: Vec<String>,
}

/// One skill reference of the card (C-1110).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CardSkillRef {
    pub index: usize,
    /// `library` | `organization`.
    pub kind: String,
    pub name: String,
    pub required: bool,
    pub available: bool,
    /// The reference's own library address in display form (`host/path`, C-1123's
    /// normalization); null for an `organization` reference.
    pub repo: Option<String>,
}

/// One automation of the card (C-1110).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AutomationBrief {
    /// `schedule` | `webhook`.
    pub kind: String,
    pub name: String,
}

/// A template the workspace may use (C-1110's `TemplateCard`), about the offered version.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TemplateCard {
    pub template_id: String,
    pub name: String,
    pub summary: String,
    /// The offered version's `README.md`, in full.
    pub readme: String,
    pub version: String,
    /// The source's display name, as the admin sources list shows it (display only, C-1110).
    pub source_name: String,
    pub has_avatar: bool,
    pub apps: Vec<CardAppEntry>,
    pub skill_refs: Vec<CardSkillRef>,
    pub has_helpers: bool,
    pub automations: Vec<AutomationBrief>,
    /// Every required app entry has an available app and every required skill reference is
    /// available.
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TemplateCardList {
    pub items: Vec<TemplateCard>,
}

/// The template's tools of one choice that the app's current catalogue has, and the ones it
/// does not (never written, C-1110).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CustomToolsView {
    pub allow_tools: Vec<String>,
    pub ask_tools: Vec<String>,
    pub dropped_tools: Vec<String>,
}

/// One app of one entry, with what creating would preselect (C-1110's choice).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PlanChoice {
    pub app_id: String,
    pub app_name: String,
    pub available: bool,
    /// `not_installed` | `not_available_here` | null.
    pub unavailable_reason: Option<String>,
    /// The template's, except `act` on a `custom_only` app, which becomes `ask`.
    pub level: String,
    /// `custom_only` when the level was cut, else null.
    pub level_note: Option<String>,
    /// The template's `tools:`, cut to the app's current catalogue; null without `tools:`.
    pub custom: Option<CustomToolsView>,
    /// The app's accounts the caller may give to an agent of this workspace (C-917 shape).
    pub accounts: Vec<Account>,
}

/// One entry of the plan (C-1110).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PlanEntry {
    pub index: usize,
    pub required: bool,
    pub why: String,
    pub choices: Vec<PlanChoice>,
}

/// The resolved library of a skill reference (C-1142's `SkillRefPlan.library`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ResolvedLibraryView {
    pub id: Uuid,
    pub label: String,
    /// `organization` | `instance`.
    pub scope: String,
    /// The display form `host/path`.
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: String,
    pub commit_sha: String,
}

/// The referenced skill's content, exactly what an add would store (C-1142).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SkillContentBrief {
    pub use_it_when: String,
    pub body: String,
    pub files: Vec<FileView>,
}

/// One skill reference, resolved for the workspace (C-1142's `SkillRefPlan`).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SkillRefPlan {
    pub index: usize,
    /// `library` | `organization`.
    pub kind: String,
    pub name: String,
    pub required: bool,
    pub why: String,
    pub available: bool,
    /// `library_not_added` | `skill_not_in_library` | `library_unreadable` |
    /// `libraries_disabled` | `not_in_organization` | null.
    pub unavailable_reason: Option<String>,
    /// The reference's own library address in display form (`host/path`, C-1123's
    /// normalization); null for an `organization` reference.
    pub repo: Option<String>,
    /// The matched library, with the commit the content comes from; null when unmatched.
    pub library: Option<ResolvedLibraryView>,
    /// The skill's content at that commit (the plan only); null while unavailable.
    pub content: Option<SkillContentBrief>,
}

/// The full plan of one template (C-1110's `TemplatePlan`): the card's fields, the reviewed
/// content, the app entries with their per-app choices and the resolved skill references.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TemplatePlan {
    pub template_id: String,
    pub name: String,
    pub summary: String,
    pub readme: String,
    pub version: String,
    /// The source's display name, as the admin sources list shows it (display only, C-1110).
    pub source_name: String,
    pub has_avatar: bool,
    pub apps: Vec<CardAppEntry>,
    pub has_helpers: bool,
    pub automations: Vec<AutomationBrief>,
    pub ready: bool,
    pub content_sha256: Option<String>,
    pub content: TemplateContentView,
    pub entries: Vec<PlanEntry>,
    pub skill_refs: Vec<SkillRefPlan>,
}

// ---- updates (C-1112, P6-TPL-3) ---------------------------------------------------------------

/// (C-1112) Answer of `GET /v1/agents/{agent}/template`: the provenance, whether the template
/// is still available, and the update offer when a newer version is offered.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AgentTemplateStatus {
    pub template: Option<AgentTemplateRefView>,
    pub available: bool,
    pub update: Option<TemplateUpdateOffer>,
    /// (C-1112) What the apply left alone: `skill_kept` when a `removed_upstream` library
    /// skill was not deleted (it is no longer sourced from the matching library, or it is an
    /// ordinary skill of the same name). Empty on the GET.
    pub notes: Vec<ApplyNote>,
}

/// (C-1112) One note of an apply.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApplyNote {
    /// `skill_kept`.
    pub code: String,
    pub detail: String,
}

/// (C-1112) The provenance shown on the agent's Template section: plain text — deleting the
/// source or the template changes no agent.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AgentTemplateRefView {
    pub template_id: String,
    pub name: String,
    pub version: String,
}

/// (C-1112) The update offer: what the offered (semver-newer) version would change. Until the
/// person applies it, the agent does not change.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TemplateUpdateOffer {
    pub version: String,
    pub content_sha256: String,
    /// The new version's `README.md`: shown, never applied, not a part.
    pub readme: String,
    /// Format 1 keeps `CHANGELOG.md` out of the content: always null.
    pub changelog: Option<String>,
    /// The inputs the stored inputs do not cover (the apply supplies them).
    pub inputs_needed: Vec<InputNeededView>,
    /// The offered version's app entries the agent has no access for yet.
    pub apps_added: Vec<UpdateAppEntry>,
    /// Only the parts that differ, each with its state.
    pub parts: Vec<OfferPart>,
}

/// (C-1112) An input the agent's stored inputs do not cover.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InputNeededView {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// `text` | `choice`.
    pub kind: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// (C-1112) One app entry of the offered version the agent has no access for yet: given on the
/// Access tab as usual, never by the apply.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UpdateAppEntry {
    /// The entry's app ids (`app`, or every id of `any_of`).
    pub app_ids: Vec<String>,
    pub required: bool,
    pub why: String,
    /// The template's level, capped to `ask` on a `custom_only` app (as the plan's).
    pub level: Option<String>,
}

/// (C-1112) One part of the offer. Only parts that differ are listed. `current` and
/// `proposed` are the full texts, rendered with the stored inputs (a needed input renders as
/// `{input.<id>}`). A library skill's content is never a part.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OfferPart {
    /// `instructions` | `skill` | `helpers` | `library_skill`.
    pub kind: String,
    /// Empty for `instructions` and `helpers`; the skill name; the C-1104 `skill_ref_key`.
    pub key: String,
    /// `unchanged_here` | `edited_here` | `removed_here` | `new` | `removed_upstream`.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed: Option<String>,
    /// For a `library_skill`: the reference, resolved as C-1142.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_ref: Option<SkillRefPlan>,
}

/// (C-1112) Body of `POST /v1/agents/{agent}/template/apply`: which offered version, which
/// parts of it. An empty `parts` is "skip this version" (the provenance still moves).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyTemplateRequest {
    pub version: String,
    /// The offered version's content hash the offer showed.
    pub content_sha256: String,
    /// Supplies `inputs_needed`; stored inputs that still pass are kept.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// Required for every `new` `library_skill` part taken.
    #[serde(default)]
    pub skill_refs: Vec<ApplySkillRef>,
    pub parts: Vec<ApplyPart>,
}

/// (C-1112) One library skill reference of an apply (the create's shape).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplySkillRef {
    /// The `skill_refs:` entry's index.
    pub index: usize,
    pub library_id: Uuid,
    /// The commit the person reviewed (the fetch pins to it).
    pub commit_sha: String,
}

/// (C-1112) One part to take.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyPart {
    pub kind: String,
    pub key: String,
}
