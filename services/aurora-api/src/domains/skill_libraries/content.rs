//! Reading a repository and turning it into skills (docs/10 §Git skill library; phase 4,
//! P4-GSL, D-148; phase 5, P5-SKLIB-1, D-448). Pure assembly and validation live here so browse,
//! add and refresh apply the same rules; everything that comes from the repository is untrusted.
//! The repository itself is behind [`RepoSource`] (the API's git reader, `super::git`; an
//! in-memory one in unit tests).

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use async_trait::async_trait;
use aurora_core::limits;
use aurora_registry::skill_format;
use sha2::{Digest, Sha256};

use super::dto::SkillInvalidReason;
use super::dto::SkillInvalidReason as Code;
use crate::domains::skills::service as skills;

/// More files than this in one skill directory is refused (bounds the blob reads of an add).
pub use aurora_registry::skill_format::FILES_PER_SKILL_MAX;

/// One entry of a commit's tree under the library path (git's own ids and modes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// The full path from the repository root.
    pub path: String,
    /// `blob`, `tree` or `commit` (a submodule).
    pub kind: String,
    /// git's octal mode (`100644`, `100755`, `120000`, `40000`, `160000`).
    pub mode: String,
    /// The object id (40 hex).
    pub blob: String,
    /// Known only when the blob itself was read.
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Branch,
    Tag,
    Commit,
}

impl RefKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::Tag => "tag",
            Self::Commit => "commit",
        }
    }
}

/// What a ref resolved to (C-1124's `resolved`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub sha: String,
    pub kind: RefKind,
    pub name: String,
    /// The repository's default branch.
    pub default: bool,
}

/// A readable repository (C-1125's three reads). Each value is one read: implementations may
/// cache what they fetched for its duration, never longer.
#[async_trait]
pub trait RepoSource: Send + Sync {
    /// The commit a ref points at; `None` is the default branch.
    async fn resolve(&self, reference: Option<&str>) -> Result<Resolved, ReadError>;
    /// Every entry under `path` of commit `sha` (recursive, full paths).
    async fn tree(&self, sha: &str, path: &str) -> Result<Vec<TreeEntry>, ReadError>;
    /// The bytes of these blobs, in as few requests as possible.
    async fn blobs(&self, ids: &[String]) -> Result<HashMap<String, Vec<u8>>, ReadError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The repository could not be read (a sanitized, fixed reason).
    Unreachable(String),
    /// The tree is truncated or longer than `LIBRARY_TREE_MAX`: narrow `path`.
    TreeTooLarge,
    /// The git host answered 429/503: not a failure of the library, retry after this many
    /// seconds (the host's `Retry-After`, capped; FIX-096).
    Throttled(u64),
}

/// Pause after a throttled answer without a usable `Retry-After`, and the cap on one.
const THROTTLE_DEFAULT_SECS: u64 = 300;
const THROTTLE_MAX_SECS: u64 = 3600;

/// `Retry-After` as delta-seconds (an HTTP date is not honoured: the default applies), capped.
#[must_use]
pub fn throttle_secs(header: Option<&str>) -> u64 {
    header
        .and_then(|h| h.trim().parse::<u64>().ok())
        .unwrap_or(THROTTLE_DEFAULT_SECS)
        .clamp(1, THROTTLE_MAX_SECS)
}

impl ReadError {
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Unreachable(r) => r.clone(),
            Self::Throttled(_) => "the git host is rate limiting requests; retry later".into(),
            Self::TreeTooLarge => {
                "the repository tree is too large; set a narrower folder (path)".into()
            }
        }
    }
}

/// The tunable values of a repository read (P6-TPL-2 §3, C-1107): the skill library's values
/// are the default, so its behaviour is unchanged; template sources pass their own (the
/// `aurora-templates/<version>` user agent and `TEMPLATE_HELPER_FILE_MAX`). The token's purpose
/// and owner are not read-dependent: they are parameters of the token open ([`SealContext`]),
/// which each caller builds from its own row.
///
/// [`SealContext`]: aurora_secrets::SealContext
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadProfile {
    /// The `USER_AGENT` of every git request (the reader appends ` git/2.0`, as git clients do).
    pub user_agent: String,
    /// The largest blob one read accepts (`LIBRARY_FILE_MAX`, `TEMPLATE_HELPER_FILE_MAX`).
    pub file_max: usize,
}

impl Default for ReadProfile {
    fn default() -> Self {
        Self {
            user_agent: format!("aurora-skill-library/{}", env!("CARGO_PKG_VERSION")),
            file_max: limits::LIBRARY_FILE_MAX,
        }
    }
}

/// One read of a library: the repository, the library path and the blobs fetched so far.
pub struct Reader<'a> {
    src: &'a dyn RepoSource,
    path: &'a str,
    profile: ReadProfile,
    cache: Mutex<HashMap<String, Vec<u8>>>,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(src: &'a dyn RepoSource, path: &'a str) -> Self {
        Self::with_profile(src, path, ReadProfile::default())
    }

    /// A read with another profile (template sources, C-1107).
    #[must_use]
    pub fn with_profile(src: &'a dyn RepoSource, path: &'a str, profile: ReadProfile) -> Self {
        Self {
            src,
            path,
            profile,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The commit a ref points at; `None` is the repository's default branch.
    pub async fn resolve(&self, reference: Option<&str>) -> Result<String, ReadError> {
        Ok(self.src.resolve(reference).await?.sha)
    }

    /// What a ref resolved to, with its kind and whether it is the default branch.
    pub async fn resolve_full(&self, reference: Option<&str>) -> Result<Resolved, ReadError> {
        self.src.resolve(reference).await
    }

    /// Every entry under the library path of one commit.
    pub async fn tree(&self, sha: &str) -> Result<Vec<TreeEntry>, ReadError> {
        self.src.tree(sha, self.path).await
    }

    /// Fetches these blobs in one go (browse, add and refresh call this first, so one read is
    /// one blob request); later [`Reader::blob`] calls are served from memory.
    pub async fn prefetch(&self, ids: &[String]) -> Result<(), ReadError> {
        let missing: Vec<String> = {
            let c = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ids.iter()
                .filter(|i| !c.contains_key(*i))
                .cloned()
                .collect()
        };
        if missing.is_empty() {
            return Ok(());
        }
        let got = self.src.blobs(&missing).await?;
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(got);
        Ok(())
    }

    /// A file's bytes if this read has already fetched them.
    #[must_use]
    pub fn cached(&self, blob: &str) -> Option<Vec<u8>> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(blob)
            .cloned()
    }

    /// Whether this read has already fetched a file.
    #[must_use]
    pub fn has(&self, blob: &str) -> bool {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(blob)
    }

    /// Bytes of every file fetched so far.
    #[must_use]
    pub fn held_bytes(&self) -> usize {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(Vec::len)
            .sum()
    }

    /// Every entry of one commit's whole repository (the same tree read; no further request).
    pub async fn root_tree(&self, sha: &str) -> Result<Vec<TreeEntry>, ReadError> {
        self.src.tree(sha, "").await
    }

    /// One file's bytes.
    pub async fn blob(&self, blob: &str) -> Result<Vec<u8>, ReadError> {
        if let Some(b) = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(blob)
        {
            return Ok(b.clone());
        }
        self.prefetch(&[blob.to_owned()]).await?;
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(blob)
            .cloned()
            .ok_or_else(|| ReadError::Unreachable(super::git::reason::MISMATCH.into()))
    }
}

// ---- assembling skills from a tree --------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    /// Path relative to the skill directory.
    pub rel: String,
    pub blob: String,
    pub executable: bool,
    pub size: Option<u64>,
}

/// A directory directly under the library path that holds a `SKILL.md` (or tried to).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub dir: String,
    pub skill_md: Option<FileRef>,
    pub files: Vec<FileRef>,
    /// Why the directory cannot be stored (sanitized, fixed texts only).
    pub problems: Vec<Invalid>,
}

/// One reason a skill cannot be stored: the code clients branch on and the text beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub code: SkillInvalidReason,
    pub reason: String,
}

impl Invalid {
    pub fn new(code: SkillInvalidReason, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }
}

impl From<skill_format::SkillError> for Invalid {
    fn from(e: skill_format::SkillError) -> Self {
        Self::new(e.kind.into(), e.message)
    }
}

impl Candidate {
    /// `{ "<relative path>": "<blob id>" }` of `SKILL.md` and every file (what is pinned).
    #[must_use]
    pub fn blobs(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        if let Some(s) = &self.skill_md {
            m.insert("SKILL.md".to_owned(), s.blob.clone());
        }
        for f in &self.files {
            m.insert(f.rel.clone(), f.blob.clone());
        }
        m
    }

    /// The summed tree sizes (an estimate before the files are read).
    #[must_use]
    pub fn tree_size(&self) -> u64 {
        self.skill_md
            .iter()
            .chain(&self.files)
            .filter_map(|f| f.size)
            .sum()
    }

    /// Every blob id a load of this candidate reads.
    #[must_use]
    pub fn blob_ids(&self) -> Vec<String> {
        self.skill_md
            .iter()
            .chain(&self.files)
            .map(|f| f.blob.clone())
            .collect()
    }

    /// All the problems in one text; the code is the first one's.
    #[must_use]
    pub fn problem(&self) -> Option<Invalid> {
        let first = self.problems.first()?;
        let texts: Vec<&str> = self.problems.iter().map(|p| p.reason.as_str()).collect();
        Some(Invalid::new(first.code, texts.join("; ")))
    }

    fn refuse(&mut self, code: SkillInvalidReason, reason: impl Into<String>) {
        self.problems.push(Invalid::new(code, reason));
    }
}

const REGULAR: [&str; 2] = ["100644", "100755"];

/// Skill directories of a tree: depth 1 under `path`, in name order. Symlinks (mode `120000`),
/// submodules (mode `160000`), other non-regular entries, oversize files, over-long listings and
/// unusable paths are problems of their directory, never stored.
#[must_use]
pub fn assemble(path: &str, entries: &[TreeEntry]) -> Vec<Candidate> {
    assemble_with(path, entries, limits::LIBRARY_FILE_MAX)
}

/// [`assemble`] with another per-file cap (template sources read with their own, C-1107).
#[must_use]
pub fn assemble_with(path: &str, entries: &[TreeEntry], file_max: usize) -> Vec<Candidate> {
    let prefix = path.trim_matches('/');
    let mut dirs: BTreeMap<String, Candidate> = BTreeMap::new();
    let mut marker: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in entries {
        let rel = if prefix.is_empty() {
            e.path.as_str()
        } else {
            match e
                .path
                .strip_prefix(prefix)
                .and_then(|r| r.strip_prefix('/'))
            {
                Some(r) => r,
                None => continue,
            }
        };
        let (dir, inner) = match rel.split_once('/') {
            Some((d, i)) => (d, Some(i)),
            None => (rel, None),
        };
        if dir.is_empty() {
            continue;
        }
        // A `.github` folder is never content, at any depth: not a skill directory, not listed,
        // not counted (owner request, 2026-11-25; the git walk skips it too — this also covers
        // tree sources that arrive already listed).
        if aurora_core::meta_paths::has_github_segment(rel) {
            continue;
        }
        let is_file = e.kind == "blob";
        let c = dirs.entry(dir.to_owned()).or_insert_with(|| Candidate {
            dir: dir.to_owned(),
            skill_md: None,
            files: Vec::new(),
            problems: Vec::new(),
        });
        let Some(inner) = inner else {
            // The directory entry itself (or a top-level file, which is not a skill).
            if e.kind == "commit" || e.mode == "160000" {
                c.refuse(Code::UnsupportedEntry, "a submodule is not allowed");
                marker.insert(dir.to_owned());
            } else if is_file && e.mode == "120000" && !dir.contains('.') {
                c.refuse(Code::UnsupportedEntry, "a symlink is not allowed");
                marker.insert(dir.to_owned());
            }
            continue;
        };
        if e.kind == "tree" {
            continue;
        }
        if e.kind == "commit" || e.mode == "160000" {
            c.refuse(
                Code::UnsupportedEntry,
                format!("a submodule is not allowed ({inner})"),
            );
            continue;
        }
        if !is_file {
            c.refuse(
                Code::UnsupportedEntry,
                format!("an unsupported entry type ({inner})"),
            );
            continue;
        }
        if e.mode == "120000" {
            if inner.eq_ignore_ascii_case("skill.md") {
                marker.insert(dir.to_owned());
            }
            c.refuse(
                Code::UnsupportedEntry,
                format!("a symlink is not allowed ({inner})"),
            );
            continue;
        }
        if !REGULAR.contains(&e.mode.as_str()) {
            c.refuse(
                Code::UnsupportedEntry,
                format!("an unsupported file mode ({inner})"),
            );
            continue;
        }
        let f = FileRef {
            rel: inner.to_owned(),
            blob: e.blob.clone(),
            executable: e.mode == "100755",
            size: e.size,
        };
        if inner == "SKILL.md" {
            marker.insert(dir.to_owned());
            c.skill_md = Some(f);
            continue;
        }
        if inner.eq_ignore_ascii_case("skill.md") {
            // `skill.md` beside the other files (any case but the exact one) is a skill that
            // would not be read by the harness: refuse it with a reason.
            marker.insert(dir.to_owned());
            c.refuse(
                Code::SkillMd,
                format!("the skill file must be named exactly SKILL.md ({inner})"),
            );
            continue;
        }
        if skills::validate_path(inner).is_err() {
            c.refuse(
                Code::UnsupportedPath,
                format!("an unsupported file path ({inner})"),
            );
            continue;
        }
        if f.size.is_some_and(|s| s > file_max as u64) {
            c.refuse(
                Code::FileTooLarge,
                format!("a file is larger than {} bytes ({inner})", file_max),
            );
            continue;
        }
        c.files.push(f);
    }
    dirs.into_iter()
        .filter(|(d, c)| marker.contains(d) || c.skill_md.is_some())
        .map(|(d, mut c)| {
            if c.skill_md.is_none() && c.problems.is_empty() {
                c.refuse(Code::SkillMd, "SKILL.md is missing");
            }
            if c.files.len() > FILES_PER_SKILL_MAX {
                c.refuse(
                    Code::TooManyFiles,
                    format!("more than {FILES_PER_SKILL_MAX} files"),
                );
            }
            if skills::validate_name(&d).is_err() {
                c.refuse(
                    Code::BadName,
                    "the directory name is not a valid skill name",
                );
            }
            if let Some(s) = &c.skill_md
                && s.size.is_some_and(|n| n > file_max as u64)
            {
                c.refuse(
                    Code::FileTooLarge,
                    format!("SKILL.md is larger than {} bytes", file_max),
                );
            }
            c
        })
        .collect()
}

// ---- SKILL.md and the stored form -----------------------------------------------------------

pub use aurora_registry::skill_format::{clean_text, parse_skill_md, parse_skill_md_typed};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedFile {
    pub path: String,
    pub content: Vec<u8>,
    pub executable: bool,
}

/// A skill ready to store: validated, with its pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSkill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub files: Vec<LoadedFile>,
    pub blobs: BTreeMap<String, String>,
    pub content_sha256: [u8; 32],
    /// Body plus files in characters (binary counted as base64 length): the budget unit.
    pub size: usize,
}

fn put(h: &mut Sha256, b: &[u8]) {
    h.update((b.len() as u64).to_be_bytes());
    h.update(b);
}

/// SHA-256 of the canonical stored content: description, body, then the files sorted by path with
/// their executable bit. What an approval quotes.
#[must_use]
pub fn content_sha256(description: &str, body: &str, files: &[LoadedFile]) -> [u8; 32] {
    let mut sorted: Vec<&LoadedFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut h = Sha256::new();
    h.update(b"aurora-library-skill-v1\0");
    put(&mut h, description.as_bytes());
    put(&mut h, body.as_bytes());
    for f in sorted {
        put(&mut h, f.path.as_bytes());
        h.update([u8::from(f.executable)]);
        put(&mut h, &f.content);
    }
    h.finalize().into()
}

/// SHA-256 of a non-content proposal state (`removed`, `invalid`): the same state hashes the same.
#[must_use]
pub fn marker_sha256(kind: &str, reason: &str, blobs: &BTreeMap<String, String>) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"aurora-library-marker-v1\0");
    put(&mut h, kind.as_bytes());
    put(&mut h, reason.as_bytes());
    for (k, v) in blobs {
        put(&mut h, k.as_bytes());
        put(&mut h, v.as_bytes());
    }
    h.finalize().into()
}

pub use ::hex::encode as hex;

#[must_use]
pub fn parse_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The skill breaks a rule: nothing is stored (422 `library_skill_invalid`).
    Invalid(Invalid),
    Read(ReadError),
}

impl From<ReadError> for LoadError {
    fn from(e: ReadError) -> Self {
        Self::Read(e)
    }
}

fn invalid(code: Code, s: impl Into<String>) -> LoadError {
    LoadError::Invalid(Invalid::new(code, s))
}

/// Reads and validates one candidate: the manual skills' rules plus the library rules.
pub async fn load(reader: &Reader<'_>, c: &Candidate) -> Result<LoadedSkill, LoadError> {
    if let Some(p) = c.problem() {
        return Err(LoadError::Invalid(p));
    }
    let Some(sm) = &c.skill_md else {
        return Err(invalid(Code::SkillMd, "SKILL.md is missing"));
    };
    // One blob request for the whole skill (a no-op when the caller prefetched).
    reader.prefetch(&c.blob_ids()).await?;
    let raw = reader.blob(&sm.blob).await?;
    if raw.len() > reader.profile.file_max {
        return Err(invalid(
            Code::FileTooLarge,
            format!("SKILL.md is larger than {} bytes", reader.profile.file_max),
        ));
    }
    let (description, body) =
        parse_skill_md_typed(&raw, &c.dir).map_err(|e| LoadError::Invalid(e.into()))?;
    let mut files = Vec::with_capacity(c.files.len());
    for f in &c.files {
        let content = reader.blob(&f.blob).await?;
        if content.len() > reader.profile.file_max {
            return Err(invalid(
                Code::FileTooLarge,
                format!(
                    "a file is larger than {} bytes ({})",
                    reader.profile.file_max, f.rel
                ),
            ));
        }
        files.push(LoadedFile {
            path: f.rel.clone(),
            content,
            executable: f.executable,
        });
    }
    finish(c.dir.clone(), description, body, files, c.blobs())
}

/// Totals and hash of an already-read skill (also used to re-validate a stored proposal).
pub fn finish(
    name: String,
    description: String,
    body: String,
    files: Vec<LoadedFile>,
    blobs: BTreeMap<String, String>,
) -> Result<LoadedSkill, LoadError> {
    skills::validate_name(&name).map_err(|_| {
        invalid(
            Code::BadName,
            "the directory name is not a valid skill name",
        )
    })?;
    // (FIX-096, SEC-83) The same rules the refresh applied, again: an approval re-validates what
    // the stored proposal says, never trusting its JSON. The rules are the shared validator's
    // (P5-APPSKILL-1), the same ones a package skill passes.
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|f| (f.path.as_str(), f.content.as_slice()))
        .collect();
    let size =
        skill_format::check_content(&name, &description, &body, &refs, limits::LIBRARY_FILE_MAX)
            .map_err(|e| LoadError::Invalid(e.into()))?;
    let content_sha256 = content_sha256(&description, &body, &files);
    Ok(LoadedSkill {
        name,
        description,
        body,
        files,
        blobs,
        content_sha256,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn md(front: &str) -> Vec<u8> {
        format!("---\n{front}\n---\nbody\n").into_bytes()
    }

    /// An in-memory repository: one commit, files by path; counts blob requests.
    pub(crate) struct MemRepo {
        pub files: BTreeMap<String, (String, Vec<u8>)>,
        pub blob_requests: std::sync::atomic::AtomicUsize,
    }

    impl MemRepo {
        pub(crate) fn new(files: &[(&str, &str, &[u8])]) -> Self {
            Self {
                files: files
                    .iter()
                    .map(|(p, m, c)| ((*p).to_owned(), ((*m).to_owned(), c.to_vec())))
                    .collect(),
                blob_requests: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn id(c: &[u8]) -> String {
            super::super::git::pack::hex(&super::super::git::pack::object_id(
                super::super::git::pack::Kind::Blob,
                c,
            ))
        }
    }

    #[async_trait]
    impl RepoSource for MemRepo {
        async fn resolve(&self, _r: Option<&str>) -> Result<Resolved, ReadError> {
            Ok(Resolved {
                sha: "a".repeat(40),
                kind: RefKind::Branch,
                name: "main".into(),
                default: true,
            })
        }

        async fn tree(&self, _sha: &str, path: &str) -> Result<Vec<TreeEntry>, ReadError> {
            let mut out = Vec::new();
            let mut dirs = std::collections::BTreeSet::new();
            for (p, (mode, c)) in &self.files {
                if !p.starts_with(path) {
                    continue;
                }
                let mut d = p.as_str();
                while let Some((parent, _)) = d.rsplit_once('/') {
                    dirs.insert(parent.to_owned());
                    d = parent;
                }
                out.push(TreeEntry {
                    path: p.clone(),
                    kind: "blob".into(),
                    mode: mode.clone(),
                    blob: Self::id(c),
                    size: None,
                });
            }
            for d in dirs {
                out.push(TreeEntry {
                    path: d,
                    kind: "tree".into(),
                    mode: "40000".into(),
                    blob: "b".repeat(40),
                    size: None,
                });
            }
            Ok(out)
        }

        async fn blobs(&self, ids: &[String]) -> Result<HashMap<String, Vec<u8>>, ReadError> {
            self.blob_requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self
                .files
                .values()
                .filter(|(_, c)| ids.contains(&Self::id(c)))
                .map(|(_, c)| (Self::id(c), c.clone()))
                .collect())
        }
    }

    #[tokio::test]
    async fn a_skill_loads_through_the_seam_with_one_blob_request() {
        let repo = MemRepo::new(&[
            (
                "lib/alpha/SKILL.md",
                "100644",
                b"---\ndescription: Alpha\n---\nBody\n",
            ),
            ("lib/alpha/run.sh", "100755", b"echo hi\n"),
            ("lib/alpha/notes/a.txt", "100644", b"n"),
            ("lib/beta/SKILL.md", "120000", b"../alpha/SKILL.md"),
            ("lib/README", "100644", b"r"),
        ]);
        let reader = Reader::new(&repo, "lib");
        let sha = reader.resolve(None).await.unwrap();
        let cands = assemble("lib", &reader.tree(&sha).await.unwrap());
        assert_eq!(cands.len(), 2);
        let p = cands[1].problem().unwrap();
        assert!(p.reason.contains("symlink"));
        assert_eq!(p.code, Code::UnsupportedEntry);
        let s = load(&reader, &cands[0]).await.unwrap();
        assert_eq!(s.description, "Alpha");
        assert_eq!(s.files.len(), 2);
        assert!(s.files.iter().any(|f| f.path == "run.sh" && f.executable));
        assert_eq!(
            repo.blob_requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    /// A `.github` folder anywhere in the tree is not a skill directory: not listed, not a
    /// problem, not counted (owner request, 2026-11-25).
    #[test]
    fn a_github_folder_is_not_a_skill() {
        let md = |d: &str| format!("---\ndescription: {d}\n---\nBody\n").into_bytes();
        let entries = vec![
            TreeEntry {
                path: "lib/.github/workflows/ci.yml".into(),
                kind: "blob".into(),
                mode: "100644".into(),
                blob: "b1".into(),
                size: Some(10),
            },
            TreeEntry {
                path: "lib/.github/skills/x/SKILL.md".into(),
                kind: "blob".into(),
                mode: "100644".into(),
                blob: "b2".into(),
                size: Some(10),
            },
            TreeEntry {
                path: "lib/.github".into(),
                kind: "tree".into(),
                mode: "40000".into(),
                blob: "t1".into(),
                size: None,
            },
            TreeEntry {
                path: "lib/alpha/SKILL.md".into(),
                kind: "blob".into(),
                mode: "100644".into(),
                blob: "b3".into(),
                size: Some(md("Alpha").len() as u64),
            },
        ];
        let cands = assemble("lib", &entries);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].dir, "alpha");
        assert!(cands[0].problem().is_none());
    }

    /// FIX-096: nested aliases multiply (the parser bounds jumps, not the product); they are
    /// refused before parsing, so the cost is the file size, not the expansion. (This one is a
    /// scaled-down bomb — about 10^7 nodes — so a regression fails the time bound instead of
    /// taking the test host down.)
    #[test]
    fn a_nested_alias_document_is_refused_fast() {
        let mut y = String::from("description: x\na: &a [");
        y.push_str(&vec!["1"; 2000].join(","));
        y.push_str("]\nb: &b [");
        y.push_str(&vec!["*a"; 200].join(","));
        y.push_str("]\nc: [");
        y.push_str(&vec!["*b"; 25].join(","));
        y.push_str("]\n");
        assert!(y.len() < 64 * 1024);
        let t = std::time::Instant::now();
        let e = parse_skill_md(&md(&y), "s").unwrap_err();
        assert!(e.contains("anchors or aliases"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }

    #[test]
    fn anchors_and_aliases_are_refused_in_every_position_and_plain_text_is_not() {
        for bad in [
            "description: &a x",
            "description: x\nb: *a",
            "description: x\nl:\n  - *a",
            "description: x\nl: [*a]",
            "description: x\nl: {k: *a}",
            "description: x\nl: [1, *a]",
            "&a description: x",
            "description: x\n? *a\n: 1",
        ] {
            let e = parse_skill_md(&md(bad), "s").unwrap_err();
            assert!(e.contains("anchors or aliases"), "{bad}: {e}");
        }
        for ok in [
            "description: Use *emphasis* and a & b, or x",
            "description: \"quoted, *star* and &amp\"",
            "description: 'a: *b'",
            "name: s\ndescription: Fine; see a*b",
        ] {
            assert!(parse_skill_md(&md(ok), "s").is_ok(), "{ok}");
        }
    }
}
