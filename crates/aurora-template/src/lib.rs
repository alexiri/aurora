//! aurora-template: the one implementation of the agent-template format (phase 6, P6-TPL-1,
//! C-1141, C-1103, C-1104, C-1142; docs/25-agent-templates.md is the public format).
//!
//! Pure: no I/O, no async, no database. It reads a template from a zip ([`read_archive`]) or
//! from a git tree ([`read_tree`]) — both end in the same validation — parses the repository
//! manifest ([`parse_manifest`]), fills inputs ([`check_inputs`], [`render`]), computes the
//! canonical hashes ([`content_sha256`], the part hashes) and packs the deterministic zip
//! ([`pack`]). The API (P6-TPL-2/3), the CLI and the tests use only this crate; nothing here is
//! re-implemented elsewhere.
//!
//! Security posture (SEC-146): everything read here is untrusted repository or package bytes.
//! Names are checked before anything is inflated; zip bombs (ratio and declared sizes that lie)
//! are refused without allocating the unpacked size; YAML anchors, aliases, tags and multiple
//! documents are refused; every problem comes back as data, never a panic.

#![forbid(unsafe_code)]

mod archive;
mod hash;
mod manifest;
mod meta;
mod model;
mod problem;
mod render;
mod skill_rules;
mod validate;
mod yaml;

pub use hash::{content_sha256, helpers_hash, instructions_hash, skill_hash, skill_ref_key};
pub use manifest::{ManifestProblem, parse_manifest, render_package_url};
pub use meta::parse_meta;
pub use model::{
    AppEntry, AppTools, Automation, Effort, InputDecl, InputKind, Level, MemoryFile, SkillRef,
    TemplateContent, TemplateFile, TemplateManifest, TemplateMeta, TemplateSkill, TreeFile,
    Verification,
};
pub use problem::{ProblemCode, TemplateProblem};
pub use render::{
    CheckedInputs, InputProblem, InputProblemCode, RenderedAutomation, RenderedMemory,
    RenderedSkill, RenderedTemplate, check_inputs, render,
};
pub use skill_rules::is_text;
pub use validate::{helper_path_ok, read_tree};

/// Reads a packed template (a zip whose only top-level directory is `<id>/`, stored or deflate).
/// Every C-1141 check runs; all problems (at most 20) come back when it is refused.
///
/// Zip rules are enforced before anything is inflated: entry count, names, the one root
/// directory, symlink and submodule modes, declared sizes and the 100× ratio. The unpacked caps
/// and the ratio are enforced again while inflating, so declared sizes that lie cannot allocate
/// the unpacked size.
pub fn read_archive(bytes: &[u8]) -> Result<TemplateContent, Vec<TemplateProblem>> {
    archive::read_archive(bytes)
}

/// Packs `files` (paths under `root_name/`, git modes) into the deterministic template zip:
/// sorted names, the fixed DOS timestamp 1980-01-01, stored entries and the executable bit in
/// the external attributes. CI compares a fresh pack with a published package byte for byte
/// (C-1144), so nothing here may depend on time, order or the environment. The names must have
/// passed [`read_tree`] (they are re-checked and packing refuses bad ones).
pub fn pack(root_name: &str, files: &[TreeFile]) -> Vec<u8> {
    archive::pack(root_name, files)
}
