//! Reading a repository through the gateway and turning it into skills (docs/10 §Git skill
//! library; phase 4, P4-GSL, D-148). Pure assembly and validation live here so browse, add and
//! refresh apply the same rules; everything that comes from the repository is untrusted.

use std::collections::BTreeMap;
use std::time::Duration;

use aurora_core::limits;
use aurora_core::sanitize::{is_stripped, one_line};
use aurora_registry::library::{self, Step, TreeEntry};
use aurora_registry::manifest::{Library, LibraryPaginate};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::gateway::{FetchParams, FetchRequest, GatewayError, LibraryGateway};
use crate::domains::skills::service as skills;

/// More files than this in one skill directory is refused (bounds the blob reads of an add).
pub const FILES_PER_SKILL_MAX: usize = 50;
const TREE_MAX_BYTES: usize = 4 * 1024 * 1024;
const BLOB_MAX_BYTES: usize = 2 * limits::LIBRARY_FILE_MAX + 4096;
const BUSY_RETRIES: u32 = 40;
const BUSY_PAUSE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The repository could not be read (a sanitized, fixed reason).
    Unreachable(String),
    /// The tree is truncated or longer than `LIBRARY_TREE_MAX`: narrow `path`.
    TreeTooLarge,
}

impl ReadError {
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Unreachable(r) => r.clone(),
            Self::TreeTooLarge => {
                "the repository tree is too large; set a narrower folder (path)".into()
            }
        }
    }
}

pub struct Reader<'a> {
    pub gw: &'a dyn LibraryGateway,
    pub lib: &'a Library,
    pub org: Uuid,
    pub conn: Uuid,
    pub repo: &'a str,
}

impl Reader<'_> {
    async fn call(
        &self,
        step: Step,
        params: FetchParams,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ReadError> {
        let (body, _) = self.call_full(step, params, max_bytes).await?;
        Ok(body)
    }

    async fn call_full(
        &self,
        step: Step,
        params: FetchParams,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, Option<String>), ReadError> {
        let req = FetchRequest {
            org_id: self.org,
            connection_id: self.conn,
            step,
            params,
            max_bytes,
        };
        let mut tries = 0;
        let ok = loop {
            match self.gw.fetch(&req).await {
                Ok(ok) => break ok,
                Err(GatewayError::Busy) if tries < BUSY_RETRIES => {
                    tries += 1;
                    tokio::time::sleep(BUSY_PAUSE).await;
                }
                Err(e) => return Err(ReadError::Unreachable(e.reason().into())),
            }
        };
        match ok.status {
            200..=299 => Ok((ok.body, ok.link)),
            401 => Err(ReadError::Unreachable(
                "the git host rejected the connection's credential".into(),
            )),
            403 | 404 => Err(ReadError::Unreachable(
                "the repository, ref or file was not found, or the connection cannot see it".into(),
            )),
            429 => Err(ReadError::Unreachable(
                "the git host is rate limiting requests; retry later".into(),
            )),
            300..=399 => Err(ReadError::Unreachable(
                "the git host answered with a redirect (the repository may have moved)".into(),
            )),
            s => Err(ReadError::Unreachable(format!(
                "the git host answered with status {s}"
            ))),
        }
    }

    fn params(&self) -> FetchParams {
        FetchParams {
            repo: self.repo.to_owned(),
            ..FetchParams::default()
        }
    }

    /// The commit a ref points at. No ref means the repository's default branch: `HEAD` resolves
    /// it in the one `resolve_ref` call (both built-in hosts accept it); a host that does not
    /// falls back to reading the default branch name first (the `default_path` template).
    pub async fn resolve(&self, reference: Option<&str>) -> Result<String, ReadError> {
        let bad = |_| ReadError::Unreachable("unexpected response from the git host".into());
        let branch;
        let reference = match reference {
            Some(r) => r,
            None => {
                if let Ok(body) = self
                    .call(
                        Step::ResolveRef,
                        FetchParams {
                            reference: Some("HEAD".to_owned()),
                            ..self.params()
                        },
                        TREE_MAX_BYTES,
                    )
                    .await
                    && let Ok(sha) = library::resolved_sha(self.lib, &body)
                {
                    return Ok(sha);
                }
                let body = self
                    .call(Step::ResolveRef, self.params(), TREE_MAX_BYTES)
                    .await?;
                branch = library::default_branch(self.lib, &body).map_err(bad)?;
                &branch
            }
        };
        let body = self
            .call(
                Step::ResolveRef,
                FetchParams {
                    reference: Some(reference.to_owned()),
                    ..self.params()
                },
                TREE_MAX_BYTES,
            )
            .await?;
        library::resolved_sha(self.lib, &body).map_err(bad)
    }

    /// Every entry of one commit's recursive tree (all pages).
    pub async fn tree(&self, sha: &str) -> Result<Vec<TreeEntry>, ReadError> {
        let mut out: Vec<TreeEntry> = Vec::new();
        let mut page: Option<u32> = None;
        let max_pages = self.lib.list_tree.max_pages.unwrap_or(1).max(1);
        for i in 0..max_pages {
            let (body, link) = self
                .call_full(
                    Step::ListTree,
                    FetchParams {
                        sha: Some(sha.to_owned()),
                        page,
                        ..self.params()
                    },
                    TREE_MAX_BYTES,
                )
                .await?;
            let tp = library::tree_page(self.lib, &body).map_err(|_| {
                ReadError::Unreachable("unexpected response from the git host".into())
            })?;
            if tp.truncated {
                return Err(ReadError::TreeTooLarge);
            }
            let empty = tp.entries.is_empty();
            out.extend(tp.entries);
            if out.len() > limits::LIBRARY_TREE_MAX {
                return Err(ReadError::TreeTooLarge);
            }
            let next = match self.lib.list_tree.paginate {
                LibraryPaginate::None => None,
                LibraryPaginate::LinkHeader => link.as_deref().and_then(library::next_page),
                LibraryPaginate::PageParam => (!empty).then_some(i + 2),
            };
            match next {
                Some(n) if (2..=library::PAGE_MAX).contains(&n) && i + 1 < max_pages => {
                    page = Some(n);
                }
                // More pages than the package's `max_pages` allows.
                Some(_) => return Err(ReadError::TreeTooLarge),
                None => return Ok(out),
            }
        }
        Err(ReadError::TreeTooLarge)
    }

    /// One file's bytes.
    pub async fn blob(&self, blob: &str) -> Result<Vec<u8>, ReadError> {
        let body = self
            .call(
                Step::GetBlob,
                FetchParams {
                    blob: Some(blob.to_owned()),
                    ..self.params()
                },
                BLOB_MAX_BYTES,
            )
            .await?;
        library::blob_bytes(self.lib, &body)
            .map_err(|_| ReadError::Unreachable("unexpected response from the git host".into()))
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
    pub problems: Vec<String>,
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

    #[must_use]
    pub fn problem(&self) -> Option<String> {
        (!self.problems.is_empty()).then(|| self.problems.join("; "))
    }
}

const REGULAR: [&str; 2] = ["100644", "100755"];

/// Skill directories of a tree: depth 1 under `path`, in name order. Symlinks (mode `120000`),
/// submodules (mode `160000`), other non-regular entries, oversize files, over-long listings and
/// unusable paths are problems of their directory, never stored.
#[must_use]
pub fn assemble(lib: &Library, path: &str, entries: &[TreeEntry]) -> Vec<Candidate> {
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
        let is_file = e.is_file_kind(lib);
        let c = dirs.entry(dir.to_owned()).or_insert_with(|| Candidate {
            dir: dir.to_owned(),
            skill_md: None,
            files: Vec::new(),
            problems: Vec::new(),
        });
        let Some(inner) = inner else {
            // The directory entry itself (or a top-level file, which is not a skill).
            if e.kind == "commit" || e.mode == "160000" {
                c.problems.push("a submodule is not allowed".into());
                marker.insert(dir.to_owned());
            } else if is_file && e.mode == "120000" && !dir.contains('.') {
                c.problems.push("a symlink is not allowed".into());
                marker.insert(dir.to_owned());
            }
            continue;
        };
        if e.kind == "tree" {
            continue;
        }
        if e.kind == "commit" || e.mode == "160000" {
            c.problems
                .push(format!("a submodule is not allowed ({inner})"));
            continue;
        }
        if !is_file {
            c.problems
                .push(format!("an unsupported entry type ({inner})"));
            continue;
        }
        if e.mode == "120000" {
            if inner.eq_ignore_ascii_case("skill.md") {
                marker.insert(dir.to_owned());
            }
            c.problems
                .push(format!("a symlink is not allowed ({inner})"));
            continue;
        }
        if !REGULAR.contains(&e.mode.as_str()) {
            c.problems
                .push(format!("an unsupported file mode ({inner})"));
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
            c.problems.push(format!(
                "the skill file must be named exactly SKILL.md ({inner})"
            ));
            continue;
        }
        if skills::validate_path(inner).is_err() {
            c.problems
                .push(format!("an unsupported file path ({inner})"));
            continue;
        }
        if f.size.is_some_and(|s| s > limits::LIBRARY_FILE_MAX as u64) {
            c.problems.push(format!(
                "a file is larger than {} bytes ({inner})",
                limits::LIBRARY_FILE_MAX
            ));
            continue;
        }
        c.files.push(f);
    }
    dirs.into_iter()
        .filter(|(d, c)| marker.contains(d) || c.skill_md.is_some())
        .map(|(d, mut c)| {
            if c.skill_md.is_none() && c.problems.is_empty() {
                c.problems.push("SKILL.md is missing".into());
            }
            if c.files.len() > FILES_PER_SKILL_MAX {
                c.problems
                    .push(format!("more than {FILES_PER_SKILL_MAX} files"));
            }
            if skills::validate_name(&d).is_err() {
                c.problems
                    .push("the directory name is not a valid skill name".into());
            }
            if let Some(s) = &c.skill_md
                && s.size.is_some_and(|n| n > limits::LIBRARY_FILE_MAX as u64)
            {
                c.problems.push(format!(
                    "SKILL.md is larger than {} bytes",
                    limits::LIBRARY_FILE_MAX
                ));
            }
            c
        })
        .collect()
}

// ---- SKILL.md and the stored form -----------------------------------------------------------

/// Control and format characters (bidi, zero-width, …) are removed from repository text before it
/// can become part of what an agent is told; CRLF becomes LF. Everything else, including
/// indentation, is kept exactly.
#[must_use]
pub fn clean_text(s: &str) -> String {
    s.replace("\r\n", "\n")
        .chars()
        .filter(|c| !is_stripped(*c))
        .collect()
}

/// `(description, body)` of a `SKILL.md`: YAML frontmatter with a single-line `description`
/// (and optionally a `name` equal to the directory name), then the body. Failures are fixed,
/// sanitized texts.
pub fn parse_skill_md(raw: &[u8], dir: &str) -> Result<(String, String), String> {
    let text = std::str::from_utf8(raw).map_err(|_| "SKILL.md is not valid UTF-8".to_owned())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else {
        return Err("SKILL.md needs YAML frontmatter with a description".into());
    };
    let mut offset = 0;
    let mut end = None;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches('\n') == "---" {
            end = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let Some((fm_end, body_start)) = end else {
        return Err("SKILL.md frontmatter is not closed with ---".into());
    };
    let value: serde_yaml::Value = serde_yaml::from_str(&rest[..fm_end])
        .map_err(|_| "SKILL.md frontmatter is not valid YAML".to_owned())?;
    let Some(map) = value.as_mapping() else {
        return Err("SKILL.md frontmatter must be a mapping".into());
    };
    let get = |k: &str| map.get(serde_yaml::Value::String(k.into()));
    if let Some(n) = get("name") {
        match n.as_str() {
            Some(n) if n == dir => {}
            _ => return Err("the frontmatter name must equal the directory name".into()),
        }
    }
    let desc = get("description")
        .and_then(serde_yaml::Value::as_str)
        .ok_or_else(|| "SKILL.md frontmatter needs a description".to_owned())?;
    let desc = clean_text(desc);
    if desc.trim().is_empty() {
        return Err("the description must not be empty".into());
    }
    if desc.chars().count() > limits::SKILL_DESCRIPTION_MAX {
        return Err(format!(
            "the description is longer than {} characters",
            limits::SKILL_DESCRIPTION_MAX
        ));
    }
    // A description is one line in the projected frontmatter.
    let description = one_line(&desc, limits::SKILL_DESCRIPTION_MAX);
    let body = clean_text(&rest[body_start..]);
    let body = body.trim_start_matches('\n').to_owned();
    if body.chars().count() > limits::SKILL_BODY_MAX {
        return Err(format!(
            "the body is longer than {} characters",
            limits::SKILL_BODY_MAX
        ));
    }
    Ok((description, body))
}

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

#[must_use]
pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

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
    Invalid(String),
    Read(ReadError),
}

impl From<ReadError> for LoadError {
    fn from(e: ReadError) -> Self {
        Self::Read(e)
    }
}

fn invalid(s: impl Into<String>) -> LoadError {
    LoadError::Invalid(s.into())
}

/// Reads and validates one candidate: the manual skills' rules plus the library rules.
pub async fn load(reader: &Reader<'_>, c: &Candidate) -> Result<LoadedSkill, LoadError> {
    if let Some(p) = c.problem() {
        return Err(invalid(p));
    }
    let Some(sm) = &c.skill_md else {
        return Err(invalid("SKILL.md is missing"));
    };
    let raw = reader.blob(&sm.blob).await?;
    if raw.len() > limits::LIBRARY_FILE_MAX {
        return Err(invalid(format!(
            "SKILL.md is larger than {} bytes",
            limits::LIBRARY_FILE_MAX
        )));
    }
    let (description, body) = parse_skill_md(&raw, &c.dir).map_err(invalid)?;
    let mut files = Vec::with_capacity(c.files.len());
    for f in &c.files {
        let content = reader.blob(&f.blob).await?;
        if content.len() > limits::LIBRARY_FILE_MAX {
            return Err(invalid(format!(
                "a file is larger than {} bytes ({})",
                limits::LIBRARY_FILE_MAX,
                f.rel
            )));
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
    skills::validate_name(&name)
        .map_err(|_| invalid("the directory name is not a valid skill name"))?;
    let size = body.chars().count()
        + files
            .iter()
            .map(|f| skills::content_size(&f.content))
            .sum::<usize>();
    if size > limits::SKILL_TOTAL_MAX {
        return Err(invalid(format!(
            "the body and files total more than {} characters",
            limits::SKILL_TOTAL_MAX
        )));
    }
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
