//! The parsed template (C-1141) and the repository manifest (C-1103).

use aurora_core::git_url::GitAddress;

/// A file of a template in the tree form: the path including the root directory (`<id>/…`),
/// the bytes and the git mode. `0o100755` is executable, `0o100644` is not; `0o120000`
/// (symlink), `0o160000` (submodule) and anything else are refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mode: u32,
}

/// One own skill of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateSkill {
    pub name: String,
    /// The optional `title` front-matter line (the skill's display name).
    pub title: Option<String>,
    /// The `use-it-when` front-matter line (the skill's `description`).
    pub use_it_when: String,
    pub body: String,
    /// The skill's files other than `SKILL.md`.
    pub files: Vec<TemplateFile>,
}

/// A file kept byte for byte (helpers and skill files; never placeholder-filled, never cleaned).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateFile {
    /// Path relative to the template root (`scripts/run.sh`).
    pub path: String,
    pub bytes: Vec<u8>,
    pub executable: bool,
}

/// A template memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFile {
    pub key: String,
    pub content: String,
    /// C-1151: the optional front-matter boolean, default false.
    pub locked: bool,
}

/// An automation of the export's shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Automation {
    Schedule {
        name: String,
        prompt: String,
        cron: String,
    },
    Webhook {
        name: String,
        instructions: String,
        verification: Verification,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    Signed,
    Password,
    None,
}

/// The access level an app entry preselects (a suggestion, never a grant; SEC-146).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    #[default]
    Read,
    Ask,
    Act,
}

/// The tool split of an app entry with a custom choice (`tools:`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppTools {
    pub allow: Vec<String>,
    pub ask: Vec<String>,
}

/// One `apps:` entry: either one app or `any_of` alternatives (D-435(e)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEntry {
    App {
        app: String,
        required: bool,
        level: Level,
        tools: Option<AppTools>,
        why: String,
    },
    AnyOf {
        any_of: Vec<String>,
        required: bool,
        level: Level,
        why: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Text,
    Choice,
}

/// One declared input of `template.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDecl {
    pub id: String,
    pub label: String,
    pub help: Option<String>,
    pub kind: InputKind,
    pub required: bool,
    pub pattern: Option<String>,
    pub max_length: Option<u32>,
    pub options: Vec<String>,
    pub default: Option<String>,
}

/// A skill reference (C-1142, format only; resolution is P6-TPL-2's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillRef {
    Library {
        /// The library's https address, normalized by C-1123.
        address: GitAddress,
        /// An explicit `ref`, or the page URL's ref hint.
        ref_: Option<String>,
        /// An explicit `path`, or the page URL's folder hint.
        path: Option<String>,
        /// The skill's name in the library.
        skill: String,
        required: bool,
        why: String,
    },
    Organization {
        name: String,
        required: bool,
        why: String,
    },
}

impl SkillRef {
    /// The referenced skill's name (own-skill clash checks use it).
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Library { skill, .. } => skill,
            Self::Organization { name, .. } => name,
        }
    }

    #[must_use]
    pub fn required(&self) -> bool {
        match self {
            Self::Library { required, .. } | Self::Organization { required, .. } => *required,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
    Max,
}

/// `template.yaml`, strictly parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateMeta {
    pub format: u32,
    pub id: String,
    pub version: String,
    pub name: String,
    pub summary: String,
    pub min_core: Option<String>,
    pub effort: Option<Effort>,
    pub inputs: Vec<InputDecl>,
    pub apps: Vec<AppEntry>,
    pub skill_refs: Vec<SkillRef>,
}

/// The whole parsed template: what an admin approves and what creation copies (C-1104).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateContent {
    pub meta: TemplateMeta,
    /// `agent.yaml`: the suggested agent name.
    pub agent_name: String,
    /// `agent.yaml`: `may_message_first`.
    pub may_message_first: bool,
    /// `instructions.md`, cleaned.
    pub instructions: String,
    /// The required `README.md` (D-458), cleaned, never placeholder-filled.
    pub readme: String,
    pub memories: Vec<MemoryFile>,
    pub skills: Vec<TemplateSkill>,
    /// `helpers/` files, byte for byte.
    pub helpers: Vec<TemplateFile>,
    pub automations: Vec<Automation>,
    /// `avatar.png`, `avatar.jpg` or `avatar.webp`, byte for byte.
    pub avatar: Option<(String, Vec<u8>)>,
}

/// The repository manifest `aurora-templates.yaml` (C-1103). No signing key and no publisher
/// (D-435(b)): the admin's review is the trust gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateManifest {
    pub api: u32,
    pub repo: RepoInfo,
    pub packages: Packages,
}

/// The repository, shown to the reviewing admin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub name: String,
    pub description: String,
    pub homepage: Option<String>,
    pub contact: Option<String>,
}

/// Where packages are published: an https URL template with exactly the placeholders `{id}` and
/// `{version}` (each at least once, nothing else in braces).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packages {
    pub url: String,
}
